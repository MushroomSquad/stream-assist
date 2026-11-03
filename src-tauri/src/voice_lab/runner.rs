use super::{
    VlError, audio, err,
    mixer::Recipe,
    models::model_dir,
    store::{Render, RenderParams, Store},
    style::{read_style_file, sha256_hex},
    worker::{BatchItem, ClonePreset, WorkerEvent, WorkerRequest, transcript_required},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader},
    sync::oneshot,
};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobState {
    pub id: u64,
    pub kind: String,
    pub status: String,
    pub progress: Option<WorkerEvent>,
    pub error: Option<String>,
    pub profile_id: Option<i64>,
    pub render_id: Option<i64>,
}
pub type Emit = Arc<dyn Fn(&JobState) + Send + Sync>;
pub struct Runner {
    exe: PathBuf,
    extra_env: Vec<(String, String)>,
    next: AtomicU64,
    clone_slot: Mutex<Option<(u64, oneshot::Sender<()>)>>,
    synth_cancel: Mutex<Option<(u64, oneshot::Sender<()>)>>,
    synth_busy: AtomicBool,
    jobs: Mutex<HashMap<u64, JobState>>,
}
enum Slot {
    Clone,
    Synth,
    Batch,
}

struct JobGuard<R: std::ops::Deref<Target = Runner>> {
    runner: R,
    id: u64,
    slot: Slot,
    finished: bool,
    emit: Emit,
}
impl<R: std::ops::Deref<Target = Runner>> Drop for JobGuard<R> {
    fn drop(&mut self) {
        let job = self
            .runner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&self.id)
            .filter(|j| j.status == "running")
            .cloned();
        if let Some(mut job) = job.filter(|_| !self.finished) {
            job.status = "failed".into();
            job.error = Some("WORKER_CRASHED".into());
            // A callback must not prevent releasing the slot, including during unwinding.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.runner.publish(job, &self.emit);
            }));
        }
        if matches!(self.slot, Slot::Clone) {
            let mut slot = self
                .runner
                .clone_slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|(id, _)| *id == self.id) {
                *slot = None;
            }
        } else {
            if matches!(self.slot, Slot::Batch) {
                let mut slot = self
                    .runner
                    .synth_cancel
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if slot.as_ref().is_some_and(|(id, _)| *id == self.id) {
                    *slot = None;
                }
            }
            self.runner.synth_busy.store(false, Ordering::SeqCst);
        }
    }
}
pub(super) struct TempFiles(pub(super) Vec<PathBuf>);
impl Drop for TempFiles {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}
const WORKER_CODES: &[&str] = &[
    "STYLE_BAD_SCHEMA",
    "STYLE_WRONG_SHAPE",
    "STYLE_NOT_FINITE",
    "STYLE_TOO_LARGE",
    "WORKER_FAILED",
    "IO_FAILED",
];
fn known(code: &str) -> &'static str {
    WORKER_CODES
        .iter()
        .copied()
        .find(|s| *s == code)
        .unwrap_or("WORKER_FAILED")
}
impl Runner {
    pub fn new(exe: PathBuf, extra_env: Vec<(String, String)>) -> Self {
        Self {
            exe,
            extra_env,
            next: AtomicU64::new(1),
            clone_slot: Mutex::new(None),
            synth_cancel: Mutex::new(None),
            synth_busy: AtomicBool::new(false),
            jobs: Mutex::new(HashMap::new()),
        }
    }
    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
    fn publish(&self, j: JobState, emit: &Emit) {
        let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        if j.status != "running" {
            jobs.retain(|_, v| v.kind != j.kind || v.status == "running");
        }
        jobs.insert(j.id, j.clone());
        drop(jobs);
        emit(&j);
    }
    pub fn jobs(&self) -> Vec<JobState> {
        self.jobs
            .lock()
            .expect("jobs lock")
            .values()
            .cloned()
            .collect()
    }
    pub async fn run_worker(
        &self,
        req: &WorkerRequest,
        tmp: &Path,
        mut cancel: Option<oneshot::Receiver<()>>,
        mut on_event: impl FnMut(WorkerEvent),
    ) -> Result<Option<WorkerEvent>, VlError> {
        let path = tmp.join(format!("job-{}.json", self.id()));
        let _file = TempFiles(vec![path.clone()]);
        std::fs::write(&path, serde_json::to_vec(req)?)?;
        let mut command = tokio::process::Command::new(&self.exe);
        command
            .arg("--vl-worker")
            .arg(&path)
            .envs(self.extra_env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000 | 0x00004000);
        let mut child = command.spawn()?;
        let _stdin = child.stdin.take();
        let mut stdout = BufReader::new(child.stdout.take().ok_or_else(|| err("WORKER_CRASHED"))?);
        let mut buffer = Vec::new();
        let mut done = None;
        let mut error = None;
        let deadline = match req {
            WorkerRequest::SynthBatch { items, .. } => Some(
                tokio::time::Instant::now() + Duration::from_secs(300 + 60 * items.len() as u64),
            ),
            _ => None,
        };
        let result = async {
            loop {
                let line = tokio::select! {
                    biased;
                    _ = async { match cancel.as_mut() { Some(rx) => { let _ = rx.await; }, None => std::future::pending::<()>().await } } => return Err(err("JOB_CANCELLED")),
                    _ = async { match deadline { Some(end) => tokio::time::sleep_until(end).await, None => std::future::pending::<()>().await } } => return Err(err("WORKER_CRASHED")),
                    line = next_line(&mut stdout, &mut buffer) => line?,
                };
                let Some(line) = line else { break; };
                if let Ok(e) = serde_json::from_slice::<WorkerEvent>(&line) {
                    match &e {
                        WorkerEvent::Done { .. } => done = Some(e.clone()),
                        WorkerEvent::Error { code } => error = Some(known(code)),
                        _ => {}
                    }
                    on_event(e);
                }
            }
            let status = tokio::select! {
                biased;
                _ = async {
                    match cancel.as_mut() {
                        Some(rx) => { let _ = rx.await; }
                        None => std::future::pending::<()>().await,
                    }
                } => return Err(err("JOB_CANCELLED")),
                _ = async { match deadline { Some(end) => tokio::time::sleep_until(end).await, None => std::future::pending::<()>().await } } => return Err(err("WORKER_CRASHED")),
                s = child.wait() => s.map_err(|_| err("WORKER_CRASHED"))?,
            };
            if let Some(e) = error {
                return Err(err(e));
            }
            if !status.success() {
                return Err(err("WORKER_CRASHED"));
            }
            done.map(Some).ok_or_else(|| err("WORKER_CRASHED"))
        }
        .await;
        if result.is_err() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn synth(
        &self,
        store: &Arc<Mutex<Store>>,
        root: &Path,
        style_path: PathBuf,
        profile_id: Option<i64>,
        recipe: Option<Recipe>,
        params: RenderParams,
        emit: Emit,
    ) -> Result<Render, VlError> {
        if self
            .synth_busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(err("JOB_BUSY"));
        }
        let id = self.id();
        let _guard = JobGuard {
            runner: self,
            id,
            slot: Slot::Synth,
            finished: false,
            emit: emit.clone(),
        };
        let out = root.join(format!("tmp/render-{id}.wav"));
        let files = TempFiles(vec![out.clone()]);
        let mut job = JobState {
            id,
            kind: "synth".into(),
            status: "running".into(),
            progress: None,
            error: None,
            profile_id: None,
            render_id: None,
        };
        self.publish(job.clone(), &emit);
        let t = Instant::now();
        let req = WorkerRequest::Synth {
            model_dir: model_dir(root),
            style: style_path,
            out_wav: out.clone(),
            params: params.clone(),
        };
        let result = async {
            let event = tokio::time::timeout(
                Duration::from_secs(300),
                self.run_worker(&req, &root.join("tmp"), None, |e| {
                    job.progress = Some(e);
                    self.publish(job.clone(), &emit);
                }),
            )
            .await
            .map_err(|_| err("WORKER_CRASHED"))??;
            let Some(WorkerEvent::Done {
                duration_ms,
                synth_ms,
                ..
            }) = event
            else {
                return Err(err("WORKER_CRASHED"));
            };
            let s = store.lock().map_err(|_| err("IO_FAILED"))?;
            let r = s.add_render(
                profile_id,
                recipe.as_ref(),
                &params,
                duration_ms.unwrap_or(0),
                synth_ms.unwrap_or(t.elapsed().as_millis() as i64),
            )?;
            if let Err(e) = std::fs::rename(&out, s.render_path(r.id)) {
                s.rollback_render(r.id)?;
                return Err(e.into());
            }
            s.prune_renders(200)?;
            Ok(r)
        }
        .await;
        drop(files);
        match &result {
            Ok(r) => {
                job.status = "completed".into();
                job.render_id = Some(r.id);
            }
            Err(e) => {
                job.status = "failed".into();
                job.error = Some(e.code());
            }
        }
        self.publish(job, &emit);
        result
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn batch(
        &self,
        store: &Arc<Mutex<Store>>,
        root: &Path,
        items: Vec<(PathBuf, Recipe)>,
        params: RenderParams,
        reference: Option<PathBuf>,
        encoder: Option<PathBuf>,
        emit: Emit,
    ) -> Result<BatchResult, VlError> {
        let mut files = TempFiles(items.iter().map(|(path, _)| path.clone()).collect());
        let id = self.id();
        let batch: Vec<_> = items
            .iter()
            .enumerate()
            .map(|(i, (style, _))| {
                let out_wav = root.join(format!("tmp/batch-{id}-{i}.wav"));
                files.0.push(out_wav.clone());
                BatchItem {
                    style: style.clone(),
                    out_wav,
                }
            })
            .collect();
        if self
            .synth_busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(err("JOB_BUSY"));
        }
        let _guard = JobGuard {
            runner: self,
            id,
            slot: Slot::Batch,
            finished: false,
            emit: emit.clone(),
        };
        let (tx, rx) = oneshot::channel();
        *self.synth_cancel.lock().map_err(|_| err("IO_FAILED"))? = Some((id, tx));
        let mut job = JobState {
            id,
            kind: "batch".into(),
            status: "running".into(),
            progress: None,
            error: None,
            profile_id: None,
            render_id: None,
        };
        self.publish(job.clone(), &emit);
        let mut completed = vec![None; items.len()];
        let mut invalid = false;
        let req = WorkerRequest::SynthBatch {
            model_dir: model_dir(root),
            items: batch.clone(),
            params: params.clone(),
            encoder,
            reference_wav: reference,
        };
        let result = self
            .run_worker(&req, &root.join("tmp"), Some(rx), |event| {
                if invalid {
                    return;
                }
                if let WorkerEvent::Item {
                    index,
                    duration_ms,
                    synth_ms,
                    similarity,
                } = event
                {
                    if index >= completed.len() || completed[index].is_some() {
                        invalid = true;
                        let _ = self.cancel(id);
                        return;
                    }
                    completed[index] = Some((duration_ms, synth_ms, similarity));
                    job.progress = Some(event);
                    self.publish(job.clone(), &emit);
                }
            })
            .await;
        // A clean exit must have reported every item; anything else is a protocol break.
        let short = result.is_ok() && completed.iter().any(Option::is_none);
        let result = if invalid || short {
            Err(err("WORKER_CRASHED"))
        } else {
            result
        };
        let cancelled = result.as_ref().is_err_and(|e| e.code() == "JOB_CANCELLED");
        let registered = (|| {
            let s = store.lock().map_err(|_| err("IO_FAILED"))?;
            let mut renders = Vec::new();
            for (index, item) in completed.into_iter().enumerate() {
                if let Some((duration, synth, similarity)) = item {
                    let render =
                        s.add_render(None, Some(&items[index].1), &params, duration, synth)?;
                    if let Err(error) =
                        std::fs::rename(&batch[index].out_wav, s.render_path(render.id))
                    {
                        s.rollback_render(render.id)?;
                        return Err(error.into());
                    }
                    renders.push((render, similarity));
                }
            }
            s.prune_renders(200)?;
            Ok(renders)
        })();
        let outcome = registered.and_then(|items| {
            if cancelled {
                Ok(BatchResult {
                    items,
                    cancelled: true,
                })
            } else {
                result.map(|_| BatchResult {
                    items,
                    cancelled: false,
                })
            }
        });
        match &outcome {
            Ok(_) => job.status = if cancelled { "cancelled" } else { "completed" }.into(),
            Err(error) => {
                job.status = "failed".into();
                job.error = Some(error.code());
            }
        }
        self.publish(job, &emit);
        outcome
    }
    #[allow(clippy::too_many_arguments)]
    pub fn start_clone(
        self: &Arc<Self>,
        store: Arc<Mutex<Store>>,
        root: PathBuf,
        model_dir: PathBuf,
        speaker_model: Option<PathBuf>,
        source_id: i64,
        start_ms: u64,
        end_ms: u64,
        transcript: String,
        lang: String,
        name: String,
        preset: ClonePreset,
        seed: u64,
        emit: Emit,
    ) -> Result<u64, VlError> {
        if self
            .clone_slot
            .lock()
            .map_err(|_| err("IO_FAILED"))?
            .is_some()
            || self
                .jobs()
                .iter()
                .any(|j| j.kind == "clone" && j.status == "running")
        {
            return Err(err("JOB_BUSY"));
        }
        let source = {
            let s = store.lock().map_err(|_| err("IO_FAILED"))?;
            s.source(source_id)?
        };
        if !source.consent {
            return Err(err("NO_CONSENT"));
        }
        audio::validate_range(source.duration_ms as u64, start_ms, end_ms, 1000, 15000)?;
        if transcript_required(&lang, &transcript) {
            return Err(err("TRANSCRIPT_REQUIRED"));
        }
        let id = self.id();
        let reference = cut_reference(&store, &root, source_id, start_ms, end_ms, 1000, 15000)?;
        let out = root.join(format!("tmp/clone-{id}.json"));
        let files = TempFiles(vec![reference.clone(), out.clone()]);
        let req = WorkerRequest::Clone {
            model_dir,
            reference_wav: reference,
            out_style: out.clone(),
            language: lang.clone(),
            transcript: Some(transcript.clone()),
            preset,
            seed,
            speaker_model,
        };
        let (tx, rx) = oneshot::channel();
        let mut slot = self.clone_slot.lock().map_err(|_| err("IO_FAILED"))?;
        if slot.is_some()
            || self
                .jobs()
                .iter()
                .any(|j| j.kind == "clone" && j.status == "running")
        {
            return Err(err("JOB_BUSY"));
        }
        *slot = Some((id, tx));
        let mut job = JobState {
            id,
            kind: "clone".into(),
            status: "running".into(),
            progress: None,
            error: None,
            profile_id: None,
            render_id: None,
        };
        self.jobs.lock().expect("jobs lock").insert(id, job.clone());
        drop(slot);
        let guard = JobGuard {
            runner: Arc::clone(self),
            id,
            slot: Slot::Clone,
            finished: false,
            emit: emit.clone(),
        };
        emit(&job);
        let runner = Arc::clone(self);
        tokio::spawn(async move {
            let mut guard = guard;
            let result = runner
                .run_worker(&req, &root.join("tmp"), Some(rx), |e| {
                    job.progress = Some(e);
                    runner.publish(job.clone(), &emit);
                })
                .await;
            let result = (|| {
                let slot = runner.clone_slot.lock().map_err(|_| err("IO_FAILED"))?;
                if slot.is_none() {
                    return Err(err("JOB_CANCELLED"));
                }
                let event = result?;
                let Some(WorkerEvent::Done {
                    loss, similarity, ..
                }) = event
                else {
                    return Err(err("WORKER_CRASHED"));
                };
                let style = read_style_file(&out)?;
                let meta = serde_json::json!({"sourceId":source_id,"startMs":start_ms,"endMs":end_ms,"lang":lang,"transcriptSha256":sha256_hex(transcript.as_bytes()),"preset":preset,"seed":seed,"loss":loss,"similarity":similarity,"engine":"supertonic3-tts 1.3.0"});
                store.lock().map_err(|_| err("IO_FAILED"))?.put_profile(
                    &style,
                    &name,
                    "clone",
                    None,
                    Some(meta),
                )
            })();
            match result {
                Ok(p) => {
                    job.status = "completed".into();
                    job.profile_id = Some(p.id);
                }
                Err(e) => {
                    job.status = if e.code() == "JOB_CANCELLED" {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    job.error = Some(e.code());
                }
            }
            drop(files);
            guard.finished = true;
            drop(guard);
            runner.publish(job, &emit);
        });
        Ok(id)
    }
    pub fn cancel(&self, job_id: u64) -> Result<(), VlError> {
        for slot in [&self.clone_slot, &self.synth_cancel] {
            let mut slot = slot.lock().map_err(|_| err("IO_FAILED"))?;
            if slot.as_ref().is_some_and(|(id, _)| *id == job_id) {
                let (_, tx) = slot.take().unwrap();
                let _ = tx.send(());
                return Ok(());
            }
        }
        Err(err("NOT_FOUND"))
    }
}

#[derive(Debug)]
pub struct BatchResult {
    pub items: Vec<(Render, Option<f32>)>,
    pub cancelled: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cut_reference(
    store: &Arc<Mutex<Store>>,
    root: &Path,
    source_id: i64,
    start_ms: u64,
    end_ms: u64,
    min_ms: u64,
    max_ms: u64,
) -> Result<PathBuf, VlError> {
    let (source, path) = {
        let store = store.lock().map_err(|_| err("IO_FAILED"))?;
        (store.source(source_id)?, store.source_work_path(source_id))
    };
    if !source.consent {
        return Err(err("NO_CONSENT"));
    }
    audio::validate_range(source.duration_ms as u64, start_ms, end_ms, min_ms, max_ms)?;
    let samples = audio::read_segment(&path, start_ms, end_ms)?;
    let path = root.join(format!("tmp/ref-{}.wav", rand::random::<u64>()));
    let mut cleanup = TempFiles(vec![path.clone()]);
    std::fs::write(&path, audio::wav_bytes(&samples, audio::SR))?;
    cleanup.0.clear();
    Ok(path)
}

pub(super) async fn next_line<R: AsyncBufRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
) -> Result<Option<Vec<u8>>, VlError> {
    loop {
        let chunk = r.fill_buf().await.map_err(|_| err("WORKER_CRASHED"))?;
        if chunk.is_empty() {
            return Ok(if buf.is_empty() {
                None
            } else {
                Some(std::mem::take(buf))
            });
        }
        let end = chunk.iter().position(|b| *b == b'\n');
        let n = end.map_or(chunk.len(), |i| i + 1);
        if buf.len() + n > 65536 {
            return Err(err("WORKER_CRASHED"));
        }
        buf.extend_from_slice(&chunk[..n]);
        r.consume(n);
        if end.is_some() {
            return Ok(Some(std::mem::take(buf)));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn cut_reference_requires_consent() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::open(root.path()).unwrap()));
        let source = store
            .lock()
            .unwrap()
            .add_source("test", "wav", "ref", 70000, false, &[])
            .unwrap();
        assert_eq!(
            cut_reference(&store, root.path(), source.id, 0, 1000, 1000, 60000)
                .unwrap_err()
                .code(),
            "NO_CONSENT"
        );
        assert_eq!(
            std::fs::read_dir(root.path().join("tmp")).unwrap().count(),
            0
        );
    }
    #[test]
    fn cut_reference_bad_region_leaves_no_file() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::open(root.path()).unwrap()));
        let source = store
            .lock()
            .unwrap()
            .add_source("test", "wav", "ref", 70000, true, &[])
            .unwrap();
        for end in [999, 60001] {
            assert_eq!(
                cut_reference(&store, root.path(), source.id, 0, end, 1000, 60000)
                    .unwrap_err()
                    .code(),
                "REGION_INVALID"
            );
        }
        assert_eq!(
            std::fs::read_dir(root.path().join("tmp")).unwrap().count(),
            0
        );
    }
    #[test]
    fn clone_slot_not_held_during_segment() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let source = store
            .add_source("test", "wav", "reference", 2000, true, &[])
            .unwrap();
        let path = store.source_work_path(source.id);
        let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        let runner = Arc::new(Runner::new(PathBuf::new(), vec![]));
        let r = runner.clone();
        let dir = root.path().to_owned();
        let read = std::thread::spawn(move || {
            r.start_clone(
                Arc::new(Mutex::new(store)),
                dir.clone(),
                dir,
                None,
                source.id,
                0,
                1000,
                "words".into(),
                "en".into(),
                "Clone".into(),
                ClonePreset::Fast,
                42,
                Arc::new(|_| {}),
            )
        });
        // Opening the writer synchronizes with WavReader::open. Withhold the header
        // to stall segment input deterministically, without timing a large WAV read.
        let mut writer = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let slot_free = runner
            .clone_slot
            .try_lock()
            .is_ok_and(|slot| slot.is_none());
        writer.write_all(b"invalid wav").unwrap();
        drop(writer);
        assert_eq!(read.join().unwrap().unwrap_err().code(), "IO_FAILED");
        assert!(slot_free, "clone slot held while segment input was blocked");
        assert_eq!(
            std::fs::read_dir(root.path().join("tmp")).unwrap().count(),
            0
        );
    }
}

#[cfg(test)]
mod line_tests {
    use super::*;
    #[tokio::test]
    async fn next_line_returns_trailing_partial() {
        let mut r = BufReader::new(&b"first\nlast"[..]);
        let mut b = Vec::new();
        assert_eq!(
            next_line(&mut r, &mut b).await.unwrap(),
            Some(b"first\n".to_vec())
        );
        assert_eq!(
            next_line(&mut r, &mut b).await.unwrap(),
            Some(b"last".to_vec())
        );
        assert_eq!(next_line(&mut r, &mut b).await.unwrap(), None);
    }
}
