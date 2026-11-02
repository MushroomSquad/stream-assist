use super::{
    VlError, audio, err,
    mixer::Recipe,
    models::model_dir,
    store::{Render, RenderParams, Store},
    style::{read_style_file, sha256_hex},
    worker::{ClonePreset, WorkerEvent, WorkerRequest, transcript_required},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
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
    synth_busy: AtomicBool,
    jobs: Mutex<HashMap<u64, JobState>>,
}
struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
struct TempFiles(Vec<PathBuf>);
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
            synth_busy: AtomicBool::new(false),
            jobs: Mutex::new(HashMap::new()),
        }
    }
    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
    fn publish(&self, j: JobState, emit: &Emit) {
        let mut jobs = self.jobs.lock().expect("jobs lock");
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
        let result = async {
            loop {
                // Bound memory even if the worker never sends a newline.
                let chunk = tokio::select! {
                    biased;
                    _ = async {
                        match cancel.as_mut() {
                            Some(rx) => { let _ = rx.await; }
                            None => std::future::pending::<()>().await,
                        }
                    } => return Err(err("JOB_CANCELLED")),
                    data = stdout.fill_buf() => data.map_err(|_| err("WORKER_CRASHED"))?,
                };
                if chunk.is_empty() {
                    break;
                }
                let end = chunk.iter().position(|b| *b == b'\n');
                let n = end.map_or(chunk.len(), |i| i + 1);
                if buffer.len() + n > 65536 {
                    return Err(err("WORKER_CRASHED"));
                }
                buffer.extend_from_slice(&chunk[..n]);
                stdout.consume(n);
                if end.is_some() {
                    if let Ok(e) = serde_json::from_slice::<WorkerEvent>(&buffer) {
                        match &e {
                            WorkerEvent::Done { .. } => done = Some(e.clone()),
                            WorkerEvent::Error { code } => error = Some(known(code)),
                            _ => {}
                        }
                        on_event(e);
                    }
                    buffer.clear();
                }
            }
            if !buffer.is_empty()
                && let Ok(e) = serde_json::from_slice::<WorkerEvent>(&buffer)
            {
                match &e {
                    WorkerEvent::Done { .. } => done = Some(e.clone()),
                    WorkerEvent::Error { code } => error = Some(known(code)),
                    _ => {}
                }
                on_event(e);
            }
            let status = tokio::select! {
                biased;
                _ = async {
                    match cancel.as_mut() {
                        Some(rx) => { let _ = rx.await; }
                        None => std::future::pending::<()>().await,
                    }
                } => return Err(err("JOB_CANCELLED")),
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
        store: &Arc<StdMutex<Store>>,
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
        let _busy = BusyGuard(&self.synth_busy);
        let id = self.id();
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
            let event = self
                .run_worker(&req, &root.join("tmp"), None, |e| {
                    job.progress = Some(e);
                    self.publish(job.clone(), &emit);
                })
                .await?;
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
    pub fn start_clone(
        self: &Arc<Self>,
        store: Arc<StdMutex<Store>>,
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
        let mut slot = self.clone_slot.lock().map_err(|_| err("IO_FAILED"))?;
        if slot.is_some()
            || self
                .jobs()
                .iter()
                .any(|j| j.kind == "clone" && j.status == "running")
        {
            return Err(err("JOB_BUSY"));
        }
        let (source, path) = {
            let s = store.lock().map_err(|_| err("IO_FAILED"))?;
            (s.source(source_id)?, s.source_work_path(source_id))
        };
        if !source.consent {
            return Err(err("NO_CONSENT"));
        }
        audio::validate_range(source.duration_ms as u64, start_ms, end_ms, 1000, 15000)?;
        if transcript_required(&lang, &transcript) {
            return Err(err("TRANSCRIPT_REQUIRED"));
        }
        let id = self.id();
        let reference = root.join(format!("tmp/clone-{id}.wav"));
        let out = root.join(format!("tmp/clone-{id}.json"));
        let files = TempFiles(vec![reference.clone(), out.clone()]);
        let samples = audio::load_mono(&path)?;
        std::fs::write(
            &reference,
            audio::wav_bytes(audio::segment(&samples, start_ms, end_ms), audio::SR),
        )?;
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
        emit(&job);
        let runner = Arc::clone(self);
        tokio::spawn(async move {
            let result = runner
                .run_worker(&req, &root.join("tmp"), Some(rx), |e| {
                    job.progress = Some(e);
                    runner.publish(job.clone(), &emit);
                })
                .await;
            let mut slot = runner.clone_slot.lock().expect("clone slot lock");
            let result = (|| {
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
            {
                let mut jobs = runner.jobs.lock().expect("jobs lock");
                jobs.retain(|_, v| v.kind != job.kind || v.status == "running");
                jobs.insert(job.id, job.clone());
            }
            *slot = None;
            drop(slot);
            emit(&job);
        });
        Ok(id)
    }
    pub fn cancel(&self, job_id: u64) -> Result<(), VlError> {
        let mut slot = self.clone_slot.lock().map_err(|_| err("IO_FAILED"))?;
        if slot.as_ref().is_none_or(|(id, _)| *id != job_id) {
            return Err(err("NOT_FOUND"));
        }
        let (_, tx) = slot.take().unwrap();
        let _ = tx.send(());
        Ok(())
    }
}
