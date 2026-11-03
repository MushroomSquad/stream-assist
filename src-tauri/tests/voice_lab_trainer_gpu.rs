//! Optional Linux/NVIDIA acceptance. See docs/voice-lab.md for setup and measurements.
#![cfg(target_os = "linux")]

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use stream_assist_lib::voice_lab::{
    audio, models,
    store::{Store, TrainParams},
    train_runner::TrainRunner,
    trainer,
};

fn gpu_processes() -> String {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,process_name,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .expect("nvidia-smi required");
    assert!(output.status.success(), "nvidia-smi failed");
    String::from_utf8(output.stdout).unwrap()
}

// Sample the worker's GPU allocation and RSS, excluding unrelated desktop processes.
fn sample(root: &std::path::Path, peak_vram: &mut u64, peak_rss: &mut u64) {
    for line in gpu_processes().lines() {
        let fields: Vec<_> = line.split(',').map(str::trim).collect();
        if fields.len() != 3 || fields[0].parse::<u32>().is_err() {
            continue;
        }
        let cmdline = fs::read(format!("/proc/{}/cmdline", fields[0])).unwrap_or_default();
        if !String::from_utf8_lossy(&cmdline).contains(root.to_str().unwrap()) {
            continue;
        }
        *peak_vram = (*peak_vram).max(fields[2].parse().expect("numeric VRAM in MiB"));
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", fields[0]])
            .output()
            .unwrap();
        if let Ok(rss) = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
        {
            *peak_rss = (*peak_rss).max(rss);
        }
    }
}

#[tokio::test]
#[ignore = "installs ~7 GB pack; requires model, reference and NVIDIA GPU"]
async fn gpu_train_stop_resume() -> anyhow::Result<()> {
    let root = PathBuf::from(
        std::env::var_os("VOICE_LAB_TRAINER_ROOT")
            .expect("set VOICE_LAB_TRAINER_ROOT to a persistent acceptance root"),
    );
    fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    let model =
        PathBuf::from(std::env::var_os("SUPERTONIC_MODEL_DIR").expect("set SUPERTONIC_MODEL_DIR"))
            .canonicalize()?;
    let reference = std::env::var_os("SUPERTONIC_REF_WAV")
        .map(PathBuf::from)
        .unwrap_or_else(|| model.join("luna_reference.wav"));
    let install_start = Instant::now();
    if trainer::pack_state(&root) != "ok" {
        let (_sender, cancel) = tokio::sync::oneshot::channel();
        trainer::install(
            &root,
            &|stage, done, total| {
                if done == 0 || done == total {
                    eprintln!("install {stage}: {done}/{total}");
                }
            },
            cancel,
        )
        .await?;
    }
    assert_eq!(trainer::pack_state(&root), "ok");
    let diagnostics = trainer::status(&root, &Mutex::new(None), true).await;
    assert_eq!(diagnostics.torch_cuda.as_deref(), Some("12.8"));
    eprintln!("pack ready: {:.2}s", install_start.elapsed().as_secs_f64());

    // Keep the pack persistent while isolating each run's database and profiles.
    let data = tempfile::Builder::new()
        .prefix("acceptance-")
        .tempdir_in(&root)?;
    let data_root = data.path().to_path_buf();
    std::os::unix::fs::symlink(
        trainer::trainer_dir(&root),
        trainer::trainer_dir(&data_root),
    )?;
    let dest = models::model_dir(&data_root);
    fs::create_dir_all(dest.parent().unwrap())?;
    std::os::unix::fs::symlink(model, dest)?;
    assert!(models::installed(&data_root));
    let store = Arc::new(Mutex::new(Store::open(&data_root)?));
    let source = audio::import(&store, &reference.canonicalize()?, true, false)?;
    assert!(
        source.duration_ms >= 10_000,
        "reference must contain at least 10 seconds"
    );
    let new_runner = || {
        Arc::new(TrainRunner::new(
            trainer::venv_python(&data_root),
            trainer::trainer_dir(&data_root).join("adapter/train_job.py"),
            vec![],
            Duration::from_secs(30),
        ))
    };
    let runner = new_runner();
    let start = Instant::now();
    let id = runner.start(
        store.clone(),
        data_root.clone(),
        source.id,
        0,
        10_000,
        "en".into(),
        TrainParams {
            preset: "fast".into(),
            total_steps: 1000,
            lr: 2e-4,
            save_every: 100,
            seed: 42,
            threshold: 0.,
        },
        Arc::new(|_| {}),
    )?;
    let mut peak_vram = 0;
    let mut peak_rss = 0;
    let mut stopped = false;
    while runner.active().is_some() {
        sample(&data_root, &mut peak_vram, &mut peak_rss);
        let run = store.lock().unwrap().run(id)?;
        if run.step >= 300 && !stopped {
            runner.stop(id)?;
            stopped = true;
        }
        if start.elapsed() > Duration::from_secs(1800) {
            runner.kill(id);
            anyhow::bail!("stop phase timed out; worker kill requested");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let run = store.lock().unwrap().run(id)?;
    assert!(stopped);
    assert_eq!(run.status, "stopped", "{run:?}");
    assert!(
        (300..400).contains(&run.step),
        "stop should be near step 300"
    );
    let stop_step = run.step;
    assert!(store.lock().unwrap().run_dir(id).join("state.pt").is_file());
    drop(runner);
    drop(store);
    // Reopen Store as well as TrainRunner, as after an application restart.
    let store = Arc::new(Mutex::new(Store::open(&data_root)?));
    assert_eq!(store.lock().unwrap().run(id)?.status, "stopped");
    let runner = new_runner();
    let resumed_from = Arc::new(Mutex::new(None));
    let observed = resumed_from.clone();
    runner.resume(
        store.clone(),
        data_root.clone(),
        id,
        Arc::new(move |run| {
            if run.status == "running" && run.error.is_none() {
                observed.lock().unwrap().get_or_insert(run.step);
            }
        }),
    )?;
    while runner.active().is_some() {
        sample(&data_root, &mut peak_vram, &mut peak_rss);
        if start.elapsed() > Duration::from_secs(1800) {
            runner.kill(id);
            anyhow::bail!("resume phase timed out; worker kill requested");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let run = store.lock().unwrap().run(id)?;
    assert_eq!(*resumed_from.lock().unwrap(), Some(stop_step));
    assert_eq!(run.status, "completed", "{run:?}");
    assert_eq!(run.step, 1000);
    let profiles = store
        .lock()
        .unwrap()
        .profiles()?
        .into_iter()
        .filter(|p| p.origin == "train")
        .count();
    assert!(
        profiles >= 2,
        "expected at least two train profiles, got {profiles}"
    );
    assert!(
        peak_vram > 0 && peak_rss > 0,
        "worker memory was not sampled"
    );
    let after = gpu_processes();
    eprintln!(
        "GPU acceptance: total={:.2}s, peak VRAM={peak_vram} MiB, peak Python RSS={peak_rss} KiB, stop step={stop_step}, train profiles={profiles}",
        start.elapsed().as_secs_f64()
    );
    eprintln!("nvidia-smi after:\n{after}");
    assert!(
        !after.to_ascii_lowercase().contains("python"),
        "Python remains on GPU"
    );
    Ok(())
}
