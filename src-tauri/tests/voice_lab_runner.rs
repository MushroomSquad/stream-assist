use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use stream_assist_lib::voice_lab::{
    audio::{SR, wav_bytes},
    runner::{JobState, Runner},
    store::{RenderParams, Store},
    worker::*,
};
fn fake() -> PathBuf {
    let p = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("vl_fake_worker{}", std::env::consts::EXE_SUFFIX));
    assert!(
        p.is_file(),
        "cargo build --example vl_fake_worker: {}",
        p.display()
    );
    p
}
fn setup(mode: &str, consent: bool) -> (tempfile::TempDir, Arc<Mutex<Store>>, Arc<Runner>, i64) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let a = s
        .add_source("test", "wav", "reference", 20000, consent, &[])
        .unwrap();
    std::fs::write(
        s.source_work_path(a.id),
        wav_bytes(&vec![0.1; SR as usize * 20], SR),
    )
    .unwrap();
    let pid_path = d.path().join("worker.pid").to_string_lossy().into_owned();
    (
        d,
        Arc::new(Mutex::new(s)),
        Arc::new(Runner::new(
            fake(),
            vec![
                ("VL_FAKE_MODE".into(), mode.into()),
                ("VL_FAKE_PID_FILE".into(), pid_path),
            ],
        )),
        a.id,
    )
}
fn start(
    r: &Arc<Runner>,
    s: &Arc<Mutex<Store>>,
    p: &Path,
    id: i64,
    end: u64,
) -> Result<u64, stream_assist_lib::voice_lab::VlError> {
    r.start_clone(
        s.clone(),
        p.into(),
        p.join("models"),
        None,
        id,
        0,
        end,
        "words".into(),
        "en".into(),
        "Clone".into(),
        ClonePreset::Fast,
        42,
        Arc::new(|_| {}),
    )
}
async fn finished(r: &Runner, id: u64) -> JobState {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(j) = r
                .jobs()
                .into_iter()
                .find(|j| j.id == id && j.status != "running")
            {
                break j;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn clean(p: &Path) {
    assert_eq!(std::fs::read_dir(p.join("tmp")).unwrap().count(), 0);
}
#[tokio::test]
async fn clone_success_creates_profile() {
    let (d, s, r, id) = setup("ok", true);
    let j = finished(&r, start(&r, &s, d.path(), id, 1000).unwrap()).await;
    assert_eq!(j.status, "completed");
    assert_eq!(
        s.lock()
            .unwrap()
            .profile(j.profile_id.unwrap())
            .unwrap()
            .origin,
        "clone"
    );
    clean(d.path());
}
#[tokio::test]
async fn cancel_kills_worker() {
    let (d, s, r, id) = setup("sleep", true);
    let j = start(&r, &s, d.path(), id, 1000).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !d.path().join("worker.pid").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let t = Instant::now();
    r.cancel(j).unwrap();
    assert_eq!(finished(&r, j).await.status, "cancelled");
    assert!(t.elapsed() < Duration::from_secs(2));
    #[cfg(unix)]
    {
        let pid: i32 = std::fs::read_to_string(d.path().join("worker.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "worker still exists");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
    assert!(s.lock().unwrap().profiles().unwrap().is_empty());
    clean(d.path());
}
#[tokio::test]
async fn crashed_worker_fails_job() {
    let (d, s, r, id) = setup("crash", true);
    let j = finished(&r, start(&r, &s, d.path(), id, 1000).unwrap()).await;
    assert_eq!(j.status, "failed");
    assert_eq!(j.error.as_deref(), Some("WORKER_CRASHED"));
    clean(d.path());
}
#[tokio::test]
async fn invalid_style_fails_job() {
    let (d, s, r, id) = setup("badstyle", true);
    let j = finished(&r, start(&r, &s, d.path(), id, 1000).unwrap()).await;
    assert_eq!(j.error.as_deref(), Some("STYLE_WRONG_SHAPE"));
    assert!(s.lock().unwrap().profiles().unwrap().is_empty());
    clean(d.path());
}
#[tokio::test]
async fn second_clone_is_busy() {
    let (d, s, r, id) = setup("sleep", true);
    let j = start(&r, &s, d.path(), id, 1000).unwrap();
    assert_eq!(
        start(&r, &s, d.path(), id, 1000).unwrap_err().code(),
        "JOB_BUSY"
    );
    r.cancel(j).unwrap();
    finished(&r, j).await;
}
#[test]
fn no_consent_rejected() {
    let (d, s, r, id) = setup("ok", false);
    assert_eq!(
        start(&r, &s, d.path(), id, 1000).unwrap_err().code(),
        "NO_CONSENT"
    );
    clean(d.path());
}
#[test]
fn region_over_15s_rejected() {
    let (d, s, r, id) = setup("ok", true);
    assert_eq!(
        start(&r, &s, d.path(), id, 15001).unwrap_err().code(),
        "REGION_INVALID"
    );
    clean(d.path());
}
#[test]
fn transcript_rules() {
    for l in ["ru", "uk", "na"] {
        assert!(transcript_required(l, "  "));
    }
    assert!(!transcript_required("en", ""));
    for l in ["ru", "uk", "na", "en"] {
        assert!(!transcript_required(l, "words"));
    }
}
#[test]
fn clone_options_presets() {
    for (p, e) in [
        (ClonePreset::Fast, (10, 15, 25)),
        (ClonePreset::Standard, (20, 30, 50)),
        (ClonePreset::Precise, (40, 60, 100)),
    ] {
        let o = clone_options("ru", Some(" words "), p, 7, None);
        assert_eq!((o.mix_iterations, o.row_iterations, o.iterations), e);
        assert_eq!(o.voice_quality, if p == ClonePreset::Fast { 5 } else { 8 });
        assert!(matches!(o.device, supertonic3_tts::Device::Cpu));
        assert_eq!(o.probe_bank, RU_PROBES);
        assert_eq!(o.reference_text.as_deref(), Some("words"));
        assert_eq!(o.reference_window, None);
        assert_eq!(o.seed, 7);
    }
    assert_eq!(
        clone_options("en", Some("  "), ClonePreset::Fast, 42, None).reference_text,
        None
    );
}
#[test]
fn worker_protocol_json() {
    let e = WorkerEvent::Progress {
        stage: "mix".into(),
        iteration: 1,
        total: 2,
        loss: 1.0,
        best_loss: 0.5,
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains("\"event\":\"progress\""));
    assert!(j.contains("\"bestLoss\""));
    assert_eq!(serde_json::from_str::<WorkerEvent>(&j).unwrap(), e);
}
fn params() -> RenderParams {
    RenderParams {
        text: "Hello world.".into(),
        lang: "en".into(),
        speed: 1.0,
        quality: 8,
    }
}
#[tokio::test]
async fn synth_success_adds_render() {
    let (d, s, r, _) = setup("ok", true);
    let out = r
        .synth(
            &s,
            d.path(),
            d.path().join("style"),
            None,
            None,
            params(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert!(s.lock().unwrap().render_path(out.id).is_file());
    assert_eq!(out.duration_ms, 1000);
    clean(d.path());
}
#[tokio::test]
async fn synth_after_crash_not_busy() {
    let (d, s, _, _) = setup("crash", true);
    let mode = d.path().join("mode");
    std::fs::write(&mode, "crash").unwrap();
    let r = Runner::new(
        fake(),
        vec![(
            "VL_FAKE_MODE_FILE".into(),
            mode.to_string_lossy().into_owned(),
        )],
    );
    assert_eq!(
        r.synth(
            &s,
            d.path(),
            d.path().join("style"),
            None,
            None,
            params(),
            Arc::new(|_| {})
        )
        .await
        .unwrap_err()
        .code(),
        "WORKER_CRASHED"
    );
    std::fs::write(mode, "ok").unwrap();
    r.synth(
        &s,
        d.path(),
        d.path().join("style"),
        None,
        None,
        params(),
        Arc::new(|_| {}),
    )
    .await
    .unwrap();
    clean(d.path());
}
#[tokio::test]
async fn worker_exits_on_stdin_eof() {
    let mut c = tokio::process::Command::new(fake())
        .args(["--vl-worker", "unused"])
        .env("VL_FAKE_MODE", "eofwatch")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdin = c.stdin.take().unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(stdin);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), c.wait())
            .await
            .unwrap()
            .unwrap()
            .code(),
        Some(2)
    );
}

#[tokio::test]
async fn cancel_on_done_does_not_create_profile() {
    let (d, s, r, id) = setup("ok", true);
    let weak = Arc::downgrade(&r);
    let emit = Arc::new(move |job: &JobState| {
        if job.status == "running" && matches!(job.progress, Some(WorkerEvent::Done { .. })) {
            weak.upgrade().unwrap().cancel(job.id).unwrap();
        }
    });
    let j = r
        .start_clone(
            s.clone(),
            d.path().into(),
            d.path().join("models"),
            None,
            id,
            0,
            1000,
            "words".into(),
            "en".into(),
            "Clone".into(),
            ClonePreset::Fast,
            42,
            emit,
        )
        .unwrap();
    assert_eq!(finished(&r, j).await.status, "cancelled");
    assert!(s.lock().unwrap().profiles().unwrap().is_empty());
    clean(d.path());
}
#[tokio::test]
async fn synth_rename_failure_rolls_back() {
    let (d, s, r, _) = setup("ok", true);
    std::fs::create_dir(s.lock().unwrap().render_path(1)).unwrap();
    assert_eq!(
        r.synth(
            &s,
            d.path(),
            d.path().join("style"),
            None,
            None,
            params(),
            Arc::new(|_| {})
        )
        .await
        .unwrap_err()
        .code(),
        "IO_FAILED"
    );
    assert!(s.lock().unwrap().renders(200).unwrap().is_empty());
    clean(d.path());
}
#[tokio::test]
async fn jobs_keep_last_completed_per_kind() {
    let (d, s, r, id) = setup("ok", true);
    for _ in 0..3 {
        let j = start(&r, &s, d.path(), id, 1000).unwrap();
        finished(&r, j).await;
    }
    assert_eq!(r.jobs().len(), 1);
}

#[tokio::test]
async fn worker_protocol_limits_and_error_codes() {
    for (output,expected) in [
        ("x".repeat(65537),Some("WORKER_CRASHED")),
        ("{\"event\":\"error\",\"code\":\"STYLE_NOT_FINITE\"}\n".into(),Some("STYLE_NOT_FINITE")),
        ("{\"event\":\"error\",\"code\":\"unknown details\"}\n".into(),Some("WORKER_FAILED")),
        ("log line\n{\"event\":\"done\",\"durationMs\":1000,\"synthMs\":1,\"loss\":null,\"similarity\":null}".into(),None),
    ] {
        let d=tempfile::tempdir().unwrap();
        let stdout = d.path().join("stdout");
        std::fs::write(&stdout, output).unwrap();
        let r=Runner::new(fake(),vec![("VL_FAKE_STDOUT_FILE".into(),stdout.to_string_lossy().into_owned())]);
        let req=WorkerRequest::Synth{model_dir:d.path().into(),style:d.path().join("style"),out_wav:d.path().join("out"),params:params()};
        let result=r.run_worker(&req,d.path(),None,|_|{}).await;
        match expected {Some(code)=>assert_eq!(result.unwrap_err().code(),code),None=>assert!(matches!(result.unwrap(),Some(WorkerEvent::Done{..})))}
        std::fs::remove_file(stdout).unwrap();
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(),0);
    }
}

#[tokio::test]
async fn dropped_synth_publishes_failure_and_releases_slot() {
    let (d, s, r, _) = setup("sleep", true);
    let task = {
        let r = r.clone();
        let s = s.clone();
        let root = d.path().to_owned();
        tokio::spawn(async move {
            r.synth(
                &s,
                &root,
                root.join("style"),
                None,
                None,
                params(),
                Arc::new(|_| {}),
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while !d.path().join("worker.pid").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let job = r.jobs().pop().unwrap();
    assert_eq!(job.status, "failed");
    assert_eq!(job.error.as_deref(), Some("WORKER_CRASHED"));
    // A new future must pass the busy check; dropping it also publishes failure.
    let mut next = Box::pin(r.synth(
        &s,
        d.path(),
        d.path().join("style"),
        None,
        None,
        params(),
        Arc::new(|_| {}),
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut next)
            .await
            .is_err()
    );
    drop(next);
    assert!(r.jobs().iter().all(|j| j.status == "failed"));
    clean(d.path());
}
