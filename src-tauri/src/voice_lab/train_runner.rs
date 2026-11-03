use super::{
    VlError, audio, err, models,
    runner::next_line,
    store::{Store, TrainParams, TrainRun},
    style::read_style_file,
    trainer,
};
use serde::Deserialize;
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{io::BufReader, process::ChildStdin, sync::Notify};
#[derive(Deserialize, Debug)]
#[serde(
    tag = "event",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum TrainEvent {
    Started {
        resumed_from: u32,
    },
    Progress {
        step: u32,
        total: u32,
        loss: Option<f64>,
        best_loss: Option<f64>,
        lr: Option<f64>,
    },
    Checkpoint {
        step: u32,
        style_path: PathBuf,
        best_loss: Option<f64>,
    },
    Done {
        step: u32,
        style_path: PathBuf,
        best_loss: Option<f64>,
    },
    Stopped {
        step: u32,
        style_path: Option<PathBuf>,
    },
    Error {
        code: String,
    },
}
pub fn train_code(code: &str) -> &'static str {
    match code {
        "GPU_UNAVAILABLE" => "GPU_UNAVAILABLE",
        "GPU_OUT_OF_MEMORY" => "GPU_OUT_OF_MEMORY",
        "CUDA_DRIVER_MISMATCH" => "CUDA_DRIVER_MISMATCH",
        "TRAINING_STATE_CORRUPTED" => "TRAINING_STATE_CORRUPTED",
        "JOB_BUSY" => "JOB_BUSY",
        _ => "WORKER_FAILED",
    }
}
struct Active {
    run_id: i64,
    stdin: Option<ChildStdin>,
    kill: Arc<Notify>,
    stop_requested: Arc<AtomicBool>,
}
pub struct TrainRunner {
    python: PathBuf,
    script: PathBuf,
    extra_env: Vec<(String, String)>,
    stop_grace: Duration,
    active: Mutex<Option<Active>>,
}
type Emit = Arc<dyn Fn(&TrainRun) + Send + Sync>;
struct RunGuard {
    runner: Arc<TrainRunner>,
    store: Arc<Mutex<Store>>,
    id: i64,
    emit: Emit,
    lr: Option<f64>,
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        let run = {
            let s = self
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if s.run(self.id).is_ok_and(|j| j.status == "running") {
                let _ = s.set_run_status(self.id, "failed", Some("WORKER_CRASHED"));
            }
            s.run(self.id).ok()
        };
        *self
            .runner
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        if let Some(mut j) = run {
            j.lr = self.lr;
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.emit)(&j)));
        }
    }
}
impl TrainRunner {
    pub fn new(
        python: PathBuf,
        script: PathBuf,
        extra_env: Vec<(String, String)>,
        stop_grace: Duration,
    ) -> Self {
        Self {
            python,
            script,
            extra_env,
            stop_grace,
            active: Mutex::new(None),
        }
    }
    pub fn active(&self) -> Option<i64> {
        self.active.lock().unwrap().as_ref().map(|a| a.run_id)
    }
    pub fn delete_run(&self, store: &Arc<Mutex<Store>>, run_id: i64) -> Result<(), VlError> {
        let slot = self.active.lock().map_err(|_| err("IO_FAILED"))?;
        if slot.as_ref().is_some_and(|a| a.run_id == run_id) {
            return Err(err("JOB_BUSY"));
        }
        store
            .lock()
            .map_err(|_| err("IO_FAILED"))?
            .delete_run(run_id)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        self: &Arc<Self>,
        store: Arc<Mutex<Store>>,
        root: PathBuf,
        source_id: i64,
        start_ms: u64,
        end_ms: u64,
        lang: String,
        params: TrainParams,
        emit: Emit,
    ) -> Result<i64, VlError> {
        let mut slot = self.active.lock().map_err(|_| err("IO_FAILED"))?;
        if slot.is_some() {
            return Err(err("JOB_BUSY"));
        }
        let (source, path) = {
            let s = store.lock().map_err(|_| err("IO_FAILED"))?;
            (s.source(source_id)?, s.source_work_path(source_id))
        };
        if !source.consent {
            return Err(err("NO_CONSENT"));
        }
        audio::validate_range(source.duration_ms as u64, start_ms, end_ms, 3000, 30000)?;
        let run = store.lock().map_err(|_| err("IO_FAILED"))?.create_run(
            Some(source_id),
            start_ms as i64,
            end_ms as i64,
            &lang,
            &params,
        )?;
        let dir = store.lock().map_err(|_| err("IO_FAILED"))?.run_dir(run.id);
        let result = (|| -> Result<(), VlError> {
            std::fs::create_dir_all(&dir)?;
            let samples = audio::read_segment(&path, start_ms, end_ms)?;
            std::fs::write(
                dir.join("reference.wav"),
                audio::wav_bytes(&samples, audio::SR),
            )?;
            Ok(())
        })();
        if let Err(e) = result {
            let s = store.lock().unwrap();
            s.set_run_status(run.id, "failed", Some(&e.code()))?;
            let j = s.run(run.id)?;
            drop(s);
            drop(slot);
            emit(&j);
            return Err(e);
        }
        self.launch(&mut slot, store, root, run, "failed".into(), false, emit);
        Ok(slot.as_ref().unwrap().run_id)
    }
    pub fn resume(
        self: &Arc<Self>,
        store: Arc<Mutex<Store>>,
        root: PathBuf,
        run_id: i64,
        emit: Emit,
    ) -> Result<(), VlError> {
        let mut slot = self.active.lock().map_err(|_| err("IO_FAILED"))?;
        if slot.is_some() {
            return Err(err("JOB_BUSY"));
        }
        let (run, resume) = {
            let s = store.lock().map_err(|_| err("IO_FAILED"))?;
            let j = s.run(run_id)?;
            if !matches!(j.status.as_str(), "stopped" | "interrupted" | "failed") {
                return Err(err("JOB_BUSY"));
            }
            if let Some(source_id) = j.source_id {
                if !s.source(source_id)?.consent {
                    return Err(err("NO_CONSENT"));
                }
            }
            let resume = s.run_dir(run_id).join("state.pt").is_file();
            s.set_run_status(run_id, "running", None)?;
            (j, resume)
        };
        let previous = run.status.clone();
        self.launch(&mut slot, store, root, run, previous, resume, emit);
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn launch(
        self: &Arc<Self>,
        slot: &mut Option<Active>,
        store: Arc<Mutex<Store>>,
        root: PathBuf,
        run: TrainRun,
        previous: String,
        resume: bool,
        emit: Emit,
    ) {
        let kill = Arc::new(Notify::new());
        let stop = Arc::new(AtomicBool::new(false));
        *slot = Some(Active {
            run_id: run.id,
            stdin: None,
            kill: kill.clone(),
            stop_requested: stop.clone(),
        });
        let guard = RunGuard {
            runner: self.clone(),
            store: store.clone(),
            id: run.id,
            emit,
            lr: None,
        };
        tauri::async_runtime::spawn(async move {
            let mut guard = guard;
            let result = guard
                .runner
                .clone()
                .execute(&mut guard, root, run, previous, resume, kill, stop.clone())
                .await;
            if let Err(e) = result {
                let s = store.lock().unwrap();
                let code = e.code();
                let _ = s.set_run_status(
                    guard.id,
                    if stop.load(Ordering::SeqCst) {
                        "stopped"
                    } else {
                        "failed"
                    },
                    if stop.load(Ordering::SeqCst) {
                        None
                    } else {
                        Some(&code)
                    },
                );
            }
        });
    }
    pub fn stop(&self, run_id: i64) -> Result<(), VlError> {
        let mut slot = self.active.lock().map_err(|_| err("IO_FAILED"))?;
        let a = slot
            .as_mut()
            .filter(|a| a.run_id == run_id)
            .ok_or_else(|| err("NOT_FOUND"))?;
        a.stop_requested.store(true, Ordering::SeqCst);
        a.stdin.take();
        let kill = a.kill.clone();
        let grace = self.stop_grace;
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(grace).await;
            kill.notify_one();
        });
        Ok(())
    }
    pub fn kill(&self, run_id: i64) {
        if let Some(a) = self
            .active
            .lock()
            .unwrap()
            .as_ref()
            .filter(|a| a.run_id == run_id)
        {
            a.kill.notify_one();
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        guard: &mut RunGuard,
        root: PathBuf,
        run: TrainRun,
        previous: String,
        resume: bool,
        kill: Arc<Notify>,
        stop: Arc<AtomicBool>,
    ) -> Result<(), VlError> {
        trainer::write_adapter(&root)?;
        let dir = guard.store.lock().unwrap().run_dir(run.id);
        std::fs::create_dir_all(&dir)?;
        let pack = trainer::trainer_dir(&root);
        let p = &run.params;
        let request = dir.join("request.json");
        std::fs::write(
            &request,
            serde_json::to_vec(
                &serde_json::json!({"protocolVersion":1,"upstreamDir":pack.join("adapter/upstream"),"modelDir":models::model_dir(&root),"wavlmDir":pack.join("weights/wavlm-large"),"runDir":dir,"lang":run.lang,"totalSteps":p.total_steps,"lr":p.lr,"saveEvery":p.save_every,"seed":p.seed,"threshold":p.threshold,"vocoderSteps":6,"resume":resume}),
            )?,
        )?;
        let mut cmd = tokio::process::Command::new(&self.python);
        cmd.arg("-I")
            .arg(&self.script)
            .arg(request)
            .current_dir(&dir)
            .env_clear();
        for key in ["PATH", "HOME", "CUDA_VISIBLE_DEVICES"] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        cmd.env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .envs(self.extra_env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("worker.log"))?,
            );
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000 | 0x00004000);
        let mut child = cmd.spawn()?;
        let pid = child.id().ok_or_else(|| err("WORKER_CRASHED"))?;
        {
            let mut slot = self.active.lock().unwrap();
            let a = slot.as_mut().unwrap();
            if !stop.load(Ordering::SeqCst) {
                a.stdin = child.stdin.take();
            } else {
                child.stdin.take();
            }
        }
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut buf = Vec::new();
        let mut terminal = false;
        let mut killed = false;
        let mut failure = None;
        let mut last_emit = None;
        loop {
            let line = tokio::select! {biased;_=kill.notified()=>{killed=true;break;},line=next_line(&mut stdout,&mut buf)=>line};
            match line {
                Err(e) => {
                    failure = Some(e);
                    killed = true;
                    break;
                }
                Ok(None) => break,
                Ok(Some(line)) => {
                    if terminal {
                        continue;
                    }
                    let Ok(event) = serde_json::from_slice::<TrainEvent>(&line) else {
                        continue;
                    };
                    let done = matches!(event, TrainEvent::Done { .. });
                    let result = (|| -> Result<(), VlError> {
                        let s = guard.store.lock().map_err(|_| err("IO_FAILED"))?;
                        match event {
                            TrainEvent::Started { resumed_from } => {
                                s.set_run_step(run.id, resumed_from as i64)?;
                                s.truncate_run_history(run.id, resumed_from as i64)?;
                            }
                            TrainEvent::Progress {
                                step,
                                loss,
                                best_loss,
                                lr,
                                ..
                            } => {
                                s.set_run_step(run.id, step as i64)?;
                                if let (Some(loss), Some(best)) = (loss, best_loss) {
                                    s.add_run_point(run.id, step as i64, loss, best)?;
                                }
                                if let Some(best) = best_loss.filter(|v| v.is_finite()) {
                                    s.set_run_best(run.id, best)?;
                                }
                                guard.lr = lr.filter(|v| v.is_finite());
                            }
                            TrainEvent::Checkpoint {
                                step,
                                style_path,
                                best_loss,
                            }
                            | TrainEvent::Done {
                                step,
                                style_path,
                                best_loss,
                            } => {
                                if done {
                                    terminal = true;
                                }
                                let style = read_style_file(&style_path)?;
                                let name = run
                                    .source_id
                                    .and_then(|id| s.source(id).ok())
                                    .map(|s| s.name)
                                    .unwrap_or_else(|| "Обучение".into());
                                s.put_profile(&style,&format!("{name} · шаг {step}"),"train",None,Some(serde_json::json!({"runId":run.id,"step":step,"bestLoss":best_loss,"trainerRev":trainer::TRAINER_REV})))?;
                                s.set_run_step(run.id, step as i64)?;
                                if let Some(best) = best_loss.filter(|v| v.is_finite()) {
                                    s.set_run_best(run.id, best)?;
                                }
                                if done {
                                    s.set_run_status(run.id, "completed", None)?;
                                }
                            }
                            TrainEvent::Stopped { step, .. } => {
                                terminal = true;
                                if step > 0 {
                                    s.set_run_step(run.id, step as i64)?;
                                }
                                s.set_run_status(run.id, "stopped", None)?;
                            }
                            TrainEvent::Error { code } => {
                                terminal = true;
                                let code = train_code(&code);
                                s.set_run_status(
                                    run.id,
                                    if code == "JOB_BUSY" {
                                        &previous
                                    } else {
                                        "failed"
                                    },
                                    Some(code),
                                )?;
                            }
                        }
                        Ok(())
                    })();
                    if let Err(e) = result {
                        failure = Some(e);
                        killed = true;
                        break;
                    }
                    if !terminal
                        && last_emit.is_none_or(|t: Instant| t.elapsed() >= Duration::from_secs(1))
                    {
                        let snapshot = guard.store.lock().unwrap().run(run.id);
                        match snapshot {
                            Ok(mut j) => {
                                j.lr = guard.lr;
                                (guard.emit)(&j);
                                last_emit = Some(Instant::now());
                            }
                            Err(e) => {
                                failure = Some(e);
                                killed = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        if killed {
            #[cfg(unix)]
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
            #[cfg(not(unix))]
            let _ = child.start_kill();
        }
        // EOF can precede process exit: stop/kill must remain effective until wait completes.
        tokio::select! {biased;_=kill.notified()=>{
        #[cfg(unix)] unsafe{libc::killpg(pid as i32,libc::SIGKILL);}
        #[cfg(not(unix))] let _=child.start_kill();
        killed=true;let _=child.wait().await;
        },result=child.wait()=>{result?;}}
        if let Some(e) = failure {
            return Err(e);
        }
        if !terminal {
            let s = guard.store.lock().unwrap();
            if stop.load(Ordering::SeqCst) {
                s.set_run_status(run.id, "stopped", None)?;
            } else {
                s.set_run_status(
                    run.id,
                    "failed",
                    Some(if killed {
                        "JOB_CANCELLED"
                    } else {
                        "WORKER_CRASHED"
                    }),
                )?;
            }
        }
        Ok(())
    }
}
