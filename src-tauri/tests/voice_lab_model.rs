//! Manual CPU spike. Set SUPERTONIC_MODEL_DIR to the downloaded revision root.
use std::{path::PathBuf, time::Instant};

use ndarray::{Array3, Axis};
use supertonic3_tts::{
    Device,
    helper::{load_text_to_speech_on, load_voice_style},
};

fn model_dir() -> PathBuf {
    std::env::var_os("SUPERTONIC_MODEL_DIR")
        .map(PathBuf::from)
        .expect("set SUPERTONIC_MODEL_DIR to the downloaded model root")
}

#[test]
#[ignore = "requires the pinned Supertonic 3 model files"]
fn ru_synthesis_smoke() -> anyhow::Result<()> {
    let root = model_dir();
    let start = Instant::now();
    let mut tts = load_text_to_speech_on(root.join("onnx").to_str().unwrap(), Device::Cpu)?;
    let load = start.elapsed();
    println!("model_load_time_s={:.6}", load.as_secs_f64());
    assert_eq!(tts.sample_rate, 44100);
    let style = load_voice_style(
        &[root
            .join("voice_styles/M1.json")
            .to_string_lossy()
            .into_owned()],
        false,
    )?;
    let start = Instant::now();
    let (samples, _) = tts.call(
        "Привет! Это проверка русского синтеза.",
        "ru",
        &style,
        8,
        1.0,
        0.3,
    )?;
    let synth = start.elapsed().as_secs_f64();
    let duration = samples.len() as f64 / f64::from(tts.sample_rate);
    println!(
        "synth_time_s={synth:.6} audio_duration_s={duration:.6} RTF={:.6}",
        synth / duration
    );
    assert!((1.0..=10.0).contains(&duration));
    assert!(samples.iter().all(|x| x.is_finite()));
    assert!(samples.iter().map(|x| x.abs()).fold(0.0_f32, f32::max) > 0.01);
    Ok(())
}

fn row_norm_range(tensor: &Array3<f32>) -> (f64, f64) {
    tensor
        .axis_iter(Axis(0))
        .flat_map(|batch| {
            batch
                .axis_iter(Axis(0))
                .map(|row| {
                    row.iter()
                        .map(|&x| f64::from(x).powi(2))
                        .sum::<f64>()
                        .sqrt()
                })
                .collect::<Vec<_>>()
        })
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), norm| {
            (min.min(norm), max.max(norm))
        })
}

#[test]
#[ignore = "requires the pinned Supertonic 3 preset files"]
fn preset_row_norms() -> anyhow::Result<()> {
    let root = model_dir();
    for prefix in ["M", "F"] {
        for index in 1..=5 {
            let name = format!("{prefix}{index}");
            let style = load_voice_style(
                &[root
                    .join(format!("voice_styles/{name}.json"))
                    .to_string_lossy()
                    .into_owned()],
                false,
            )?;
            assert_eq!(style.ttl.shape(), &[1, 50, 256]);
            assert_eq!(style.dp.shape(), &[1, 8, 16]);
            assert!(
                style
                    .ttl
                    .iter()
                    .chain(style.dp.iter())
                    .all(|x| x.is_finite())
            );
            let (ttl_min, ttl_max) = row_norm_range(&style.ttl);
            let (dp_min, dp_max) = row_norm_range(&style.dp);
            println!(
                "{name}: ttl min={ttl_min:.9} max={ttl_max:.9}; dp min={dp_min:.9} max={dp_max:.9}"
            );
        }
    }
    Ok(())
}

fn real_worker(
    req: stream_assist_lib::voice_lab::worker::WorkerRequest,
) -> Vec<stream_assist_lib::voice_lab::worker::WorkerEvent> {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    use stream_assist_lib::voice_lab::worker::WorkerEvent;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("request.json");
    std::fs::write(&path, serde_json::to_vec(&req).unwrap()).unwrap();
    let t = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_stream-assist"))
        .arg("--vl-worker")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _stdin = child.stdin.take().unwrap();
    let events: Vec<WorkerEvent> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(Result::unwrap)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect();
    let status = child.wait().unwrap();
    println!("worker_wall_s={:.3}", t.elapsed().as_secs_f64());
    assert!(status.success(), "{status}: {events:?}");
    assert!(matches!(events.last(), Some(WorkerEvent::Done { .. })));
    events
}
#[test]
#[ignore = "requires real model"]
fn worker_synth_real() {
    use stream_assist_lib::voice_lab::{store::RenderParams, worker::*};
    let d = tempfile::tempdir().unwrap();
    let root = model_dir();
    let out = d.path().join("out.wav");
    let events = real_worker(WorkerRequest::Synth {
        model_dir: root.clone(),
        style: root.join("voice_styles/M1.json"),
        out_wav: out.clone(),
        params: RenderParams {
            text: "Hello! This is a worker synthesis test.".into(),
            lang: "en".into(),
            speed: 1.0,
            quality: 8,
        },
    });
    let wav = hound::WavReader::open(out).unwrap();
    assert_eq!(wav.spec().sample_rate, 44100);
    assert!(wav.duration() > 44100);
    println!("{events:?}");
}
#[test]
#[ignore = "requires real model and English reference WAV"]
fn worker_clone_fast_real() {
    use stream_assist_lib::voice_lab::{style::read_style_file, worker::*};
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("out.json");
    let events = real_worker(WorkerRequest::Clone {
        model_dir: model_dir(),
        reference_wav: std::env::var_os("SUPERTONIC_REF_WAV")
            .expect("SUPERTONIC_REF_WAV")
            .into(),
        out_style: out.clone(),
        language: "en".into(),
        transcript: std::env::var("SUPERTONIC_REF_TEXT").ok(),
        preset: ClonePreset::Fast,
        seed: 42,
        speaker_model: None,
    });
    read_style_file(&out).unwrap();
    println!("final_event={:?}", events.last());
}

/// End-to-end acceptance without Tauri/UI; references must be downloaded separately.
#[tokio::test]
#[ignore = "requires pinned model and luna/nora/watson_reference.wav; run in release"]
async fn acceptance_three_refs_mix_export_import() -> anyhow::Result<()> {
    use std::{
        fs,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use stream_assist_lib::voice_lab::{
        audio,
        mixer::{DpStrategy, RecipeInput, mix, new_recipe},
        models,
        runner::{Emit, Runner},
        store::{RenderParams, Store},
        style::{canonical_bytes, read_style_file},
        worker::ClonePreset,
    };

    let model = model_dir().canonicalize()?;
    let names = ["luna", "nora", "watson"];
    for name in names {
        assert!(
            model.join(format!("{name}_reference.wav")).is_file(),
            "missing {name}_reference.wav"
        );
    }
    let data = tempfile::tempdir()?;
    let root = data.path().join("original");
    let store = Arc::new(Mutex::new(Store::open(&root)?));
    // A fresh model tree for each Store, with no shared profiles/database.
    let attach_model = |root: &std::path::Path| -> anyhow::Result<()> {
        let dest = models::model_dir(root);
        fs::create_dir_all(dest.parent().unwrap())?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&model, &dest)?;
        #[cfg(not(unix))]
        for file in models::MODEL_FILES {
            let target = dest.join(file.path);
            fs::create_dir_all(target.parent().unwrap())?;
            fs::copy(model.join(file.path), target)?;
        }
        assert!(models::installed(root));
        Ok(())
    };
    attach_model(&root)?;
    let runner = Arc::new(Runner::new(
        env!("CARGO_BIN_EXE_stream-assist").into(),
        vec![],
    ));
    let emit: Emit = Arc::new(|_| {});
    let mut profiles = Vec::new();
    for name in names {
        let source = audio::import(
            &store,
            &model.join(format!("{name}_reference.wav")),
            true,
            false,
        )?;
        let end = (source.duration_ms as u64).min(10_000);
        assert!((1_000..=15_000).contains(&end));
        let started = Instant::now();
        let id = runner.start_clone(
            store.clone(),
            root.clone(),
            models::model_dir(&root),
            None,
            source.id,
            0,
            end,
            String::new(),
            "en".into(),
            name.into(),
            ClonePreset::Fast,
            42,
            emit.clone(),
        )?;
        let job = tokio::time::timeout(Duration::from_secs(1800), async {
            loop {
                let job = runner.jobs().into_iter().find(|j| j.id == id).unwrap();
                if job.status != "running" {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await?;
        println!(
            "clone_{name}_wall_s={:.3} region_ms={end}",
            started.elapsed().as_secs_f64()
        );
        assert_eq!(job.status, "completed", "{job:?}");
        let s = store.lock().unwrap();
        let profile = s.profile(job.profile_id.unwrap())?;
        assert_eq!(profile.origin, "clone");
        s.load_style(profile.id)?;
        profiles.push(profile);
        assert_eq!(fs::read_dir(root.join("tmp"))?.count(), 0);
    }
    assert_eq!(store.lock().unwrap().profiles()?.len(), 3);
    let styles = profiles
        .iter()
        .map(|p| store.lock().unwrap().load_style(p.id))
        .collect::<Result<Vec<_>, _>>()?;
    let weights = [55.0, 30.0, 15.0];
    let inputs = styles.iter().zip(weights).collect::<Vec<_>>();
    let mixed = mix(&inputs, DpStrategy::Dominant, None)?;
    let recipe = new_recipe(
        profiles
            .iter()
            .zip(weights)
            .map(|(p, weight)| RecipeInput {
                profile_sha256: p.style_sha256.clone(),
                name: p.name.clone(),
                weight,
            })
            .collect(),
        DpStrategy::Dominant,
        None,
        "2026-11-02T00:00:00Z".into(),
    );
    let snapshot =
        store
            .lock()
            .unwrap()
            .put_profile(&mixed, "Final voice", "mix", Some(&recipe), None)?;
    assert_eq!(snapshot.origin, "mix");
    assert_eq!(snapshot.recipe.as_ref(), Some(&recipe));
    let params = RenderParams {
        text: "Привет! Это проверка смешанного голоса.".into(),
        lang: "ru".into(),
        speed: 1.0,
        quality: 8,
    };
    let (mut s, mut r, mut p) = (store.clone(), root.clone(), snapshot.clone());
    for label in ["mixed", "imported"] {
        let path = s.lock().unwrap().profile_path(&p);
        let started = Instant::now();
        let render = runner
            .synth(
                &s,
                &r,
                path,
                Some(p.id),
                p.recipe.clone(),
                params.clone(),
                emit.clone(),
            )
            .await?;
        let samples = audio::load_mono(&s.lock().unwrap().render_path(render.id))?;
        assert!(render.duration_ms > 0 && render.wall_ms > 0);
        assert!(samples.iter().all(|v| v.is_finite()));
        assert!(samples.iter().any(|v| v.abs() > 0.01));
        println!(
            "{label}_wall_s={:.3} synth_ms={} duration_ms={} RTF={:.3}",
            started.elapsed().as_secs_f64(),
            render.wall_ms,
            render.duration_ms,
            render.wall_ms as f64 / render.duration_ms as f64
        );
        assert_eq!(s.lock().unwrap().renders(200)?.len(), 1);
        assert_eq!(fs::read_dir(r.join("tmp"))?.count(), 0);
        if label == "mixed" {
            let exported = data.path().join("final_voice.json");
            let bytes = fs::read(store.lock().unwrap().profile_path(&snapshot))?;
            let json: serde_json::Value = serde_json::from_slice(&bytes)?;
            assert_eq!(json.as_object().unwrap().len(), 2);
            assert!(json.get("metadata").is_none());
            assert_eq!(bytes, canonical_bytes(&mixed));
            fs::write(&exported, &bytes)?;
            let fresh_root = data.path().join("fresh");
            let fresh = Arc::new(Mutex::new(Store::open(&fresh_root)?));
            attach_model(&fresh_root)?;
            assert!(fresh.lock().unwrap().profiles()?.is_empty());
            assert!(fresh.lock().unwrap().sources()?.is_empty());
            let imported = fresh.lock().unwrap().put_profile(
                &read_style_file(&exported)?,
                "Imported",
                "import",
                None,
                None,
            )?;
            assert_eq!(imported.style_sha256, snapshot.style_sha256);
            assert!(imported.recipe.is_none());
            (s, r, p) = (fresh, fresh_root, imported);
        }
    }
    Ok(())
}
