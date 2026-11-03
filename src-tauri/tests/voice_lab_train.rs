use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use stream_assist_lib::voice_lab::{
    audio::{SR, wav_bytes},
    store::{Store, TrainParams, TrainRun},
    train_runner::TrainRunner,
};
fn fake(name: &str) -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}
fn params() -> TrainParams {
    TrainParams {
        preset: "fast".into(),
        total_steps: 1000,
        lr: 2e-4,
        save_every: 500,
        seed: 42,
        threshold: 0.24,
    }
}
struct Test {
    d: tempfile::TempDir,
    s: Arc<Mutex<Store>>,
    r: Arc<TrainRunner>,
    source: i64,
}
impl Test {
    fn new(mode: &str) -> Self {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path()).unwrap();
        let source = s
            .add_source("a", "wav", "reference", 31000, true, &[])
            .unwrap()
            .id;
        std::fs::write(
            s.source_work_path(source),
            wav_bytes(&vec![0.1; SR as usize * 31], SR),
        )
        .unwrap();
        let r = Arc::new(TrainRunner::new(
            fake("vl_fake_trainer"),
            "unused".into(),
            vec![("VL_FAKE_MODE".into(), mode.into())],
            Duration::from_secs(1),
        ));
        Self {
            d,
            s: Arc::new(Mutex::new(s)),
            r,
            source,
        }
    }
    fn start(&self) -> i64 {
        self.r
            .start(
                self.s.clone(),
                self.d.path().into(),
                self.source,
                0,
                3000,
                "ru".into(),
                params(),
                Arc::new(|_| {}),
            )
            .unwrap()
    }
    async fn finish(&self, id: i64) -> TrainRun {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.r.active().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            self.s.lock().unwrap().run(id).unwrap()
        })
        .await
        .unwrap()
    }
    fn resume_with(&self, mode: &str, id: i64) -> Arc<TrainRunner> {
        let r = Arc::new(TrainRunner::new(
            fake("vl_fake_trainer"),
            "unused".into(),
            vec![("VL_FAKE_MODE".into(), mode.into())],
            Duration::from_secs(1),
        ));
        r.resume(self.s.clone(), self.d.path().into(), id, Arc::new(|_| {}))
            .unwrap();
        r
    }
}
#[tokio::test]
async fn run_ok_creates_checkpoint_profiles() {
    let t = Test::new("ok");
    let j = t.finish(t.start()).await;
    assert_eq!(j.status, "completed");
    let s = t.s.lock().unwrap();
    let p = s.profiles().unwrap();
    assert_eq!(p.len(), 2);
    assert!(p.iter().all(|p| p.origin == "train"));
    assert_eq!(j.history.len(), 3);
}
#[tokio::test]
async fn stop_saves_and_marks_stopped() {
    let t = Test::new("stoppable");
    let id = t.start();
    tokio::time::sleep(Duration::from_millis(250)).await;
    t.r.stop(id).unwrap();
    let j = t.finish(id).await;
    assert_eq!(j.status, "stopped");
    assert!(j.step > 0);
    assert!(t.s.lock().unwrap().run_dir(id).join("state.pt").exists());
    assert!(t.s.lock().unwrap().profiles().unwrap().is_empty());
}
#[tokio::test]
async fn stop_timeout_kills() {
    let t = Test::new("hang");
    let id = t.start();
    t.r.stop(id).unwrap();
    assert_eq!(t.finish(id).await.status, "stopped");
}
#[tokio::test]
async fn oom_event_fails_run() {
    failure("oom", "GPU_OUT_OF_MEMORY").await;
}
#[tokio::test]
async fn unknown_code_is_worker_failed() {
    failure("weird", "WORKER_FAILED").await;
    failure("nonfinite", "WORKER_FAILED").await;
}
#[tokio::test]
async fn crash_fails_run() {
    failure("crash", "WORKER_CRASHED").await;
}
async fn failure(mode: &str, code: &str) {
    let t = Test::new(mode);
    let j = t.finish(t.start()).await;
    assert_eq!(j.status, "failed");
    assert_eq!(j.error.as_deref(), Some(code));
}
#[tokio::test]
async fn second_run_busy() {
    let t = Test::new("hang");
    let id = t.start();
    assert_eq!(
        t.r.start(
            t.s.clone(),
            t.d.path().into(),
            t.source,
            0,
            3000,
            "ru".into(),
            params(),
            Arc::new(|_| {})
        )
        .unwrap_err()
        .code(),
        "JOB_BUSY"
    );
    t.r.kill(id);
    t.finish(id).await;
}
#[tokio::test]
async fn delete_active_run_busy() {
    let t = Test::new("hang");
    let id = t.start();
    assert_eq!(t.r.delete_run(&t.s, id).unwrap_err().code(), "JOB_BUSY");
    assert!(t.s.lock().unwrap().run(id).is_ok());
    t.r.kill(id);
    t.finish(id).await;
    t.r.delete_run(&t.s, id).unwrap();
}
#[tokio::test]
async fn resume_continues_step() {
    resume_case(true, "stopped").await;
}
#[tokio::test]
async fn resume_without_state_restarts() {
    resume_case(false, "interrupted").await;
}
#[tokio::test]
async fn resume_after_oom() {
    resume_case(false, "failed").await;
}
async fn resume_case(state: bool, status: &str) {
    let t = Test::new("resume");
    let id = {
        let s = t.s.lock().unwrap();
        let j = s
            .create_run(Some(t.source), 0, 3000, "ru", &params())
            .unwrap();
        s.set_run_status(j.id, status, Some("GPU_OUT_OF_MEMORY"))
            .unwrap();
        s.add_run_point(j.id, 900, 1., 1.).unwrap();
        std::fs::create_dir_all(s.run_dir(j.id)).unwrap();
        if state {
            std::fs::write(s.run_dir(j.id).join("state.pt"), "24").unwrap();
        }
        j.id
    };
    let r = t.resume_with("resume", id);
    while r.active().is_some() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let j = t.s.lock().unwrap().run(id).unwrap();
    assert_eq!(j.status, "completed");
    assert_eq!(j.step, if state { 24 } else { 0 });
    assert!(j.history.is_empty());
    assert!(j.error.is_none());
}
#[tokio::test]
async fn busy_lock_keeps_resumable() {
    for status in ["stopped", "interrupted", "failed"] {
        let t = Test::new("busy");
        let id =
            t.s.lock()
                .unwrap()
                .create_run(Some(t.source), 0, 3000, "ru", &params())
                .unwrap()
                .id;
        t.s.lock()
            .unwrap()
            .set_run_status(id, status, None)
            .unwrap();
        let r = t.resume_with("busy", id);
        while r.active().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let j = t.s.lock().unwrap().run(id).unwrap();
        assert_eq!(j.status, status);
        assert_eq!(j.error.as_deref(), Some("JOB_BUSY"));
    }
}
#[tokio::test]
async fn stop_before_loop_is_stopped() {
    let t = Test::new("slowstart");
    let id = t.start();
    t.r.stop(id).unwrap();
    assert_eq!(t.finish(id).await.status, "stopped");
}
#[tokio::test]
async fn kill_without_stop_is_failed() {
    let t = Test::new("hang");
    let id = t.start();
    t.r.kill(id);
    assert_eq!(t.finish(id).await.error.as_deref(), Some("JOB_CANCELLED"));
}
#[tokio::test]
async fn no_consent_rejected() {
    let t = Test::new("ok");
    t.s.lock().unwrap().set_consent(t.source, false).unwrap();
    assert_eq!(
        t.r.start(
            t.s.clone(),
            t.d.path().into(),
            t.source,
            0,
            3000,
            "ru".into(),
            params(),
            Arc::new(|_| {})
        )
        .unwrap_err()
        .code(),
        "NO_CONSENT"
    );
}
#[tokio::test]
async fn region_bounds_3_30s() {
    let t = Test::new("ok");
    for end in [2999, 30001] {
        assert_eq!(
            t.r.start(
                t.s.clone(),
                t.d.path().into(),
                t.source,
                0,
                end,
                "ru".into(),
                params(),
                Arc::new(|_| {})
            )
            .unwrap_err()
            .code(),
            "REGION_INVALID"
        );
    }
    for end in [3000, 30000] {
        let id =
            t.r.start(
                t.s.clone(),
                t.d.path().into(),
                t.source,
                0,
                end,
                "ru".into(),
                params(),
                Arc::new(|_| {}),
            )
            .unwrap();
        assert_eq!(t.finish(id).await.status, "completed");
    }
}
#[tokio::test]
async fn first_terminal_event_wins() {
    for mode in ["done_error", "error_done", "stopped_done"] {
        let t = Test::new(mode);
        let j = t.finish(t.start()).await;
        assert_eq!(
            j.status,
            match mode {
                "done_error" => "completed",
                "error_done" => "failed",
                _ => "stopped",
            }
        );
        assert_eq!(
            t.s.lock().unwrap().profiles().unwrap().len(),
            if mode == "done_error" { 1 } else { 0 }
        );
    }
}
#[tokio::test]
async fn clone_and_train_run_together() {
    use stream_assist_lib::voice_lab::{runner::Runner, worker::ClonePreset};
    let t = Test::new("hang");
    let id = t.start();
    let r = Arc::new(Runner::new(fake("vl_fake_worker"), vec![]));
    let job = r
        .start_clone(
            t.s.clone(),
            t.d.path().into(),
            t.d.path().join("models"),
            None,
            t.source,
            0,
            3000,
            "words".into(),
            "en".into(),
            "clone".into(),
            ClonePreset::Fast,
            42,
            Arc::new(|_| {}),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while r
            .jobs()
            .iter()
            .any(|j| j.id == job && j.status == "running")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(r.jobs()[0].status, "completed");
    assert_eq!(t.r.active(), Some(id));
    t.r.kill(id);
    t.finish(id).await;
}
