//! Real CPU acceptance through the same helpers used by the Tauri commands.
use super::commands::*;
use super::*;
use std::{fs, sync::Mutex, time::Instant};

fn fresh_lab(root: PathBuf, model: &std::path::Path) -> anyhow::Result<VoiceLab> {
    let store = Store::open(&root)?;
    let dest = models::model_dir(&root);
    for file in models::MODEL_FILES {
        let target = dest.join(file.path);
        fs::create_dir_all(target.parent().unwrap())?;
        fs::copy(model.join(file.path), target)?;
    }
    let encoder = models::encoder_path(&root);
    fs::create_dir_all(encoder.parent().unwrap())?;
    fs::copy(model.join("speaker_encoder.onnx"), encoder)?;
    assert!(models::status(&root).encoder_installed);
    let exe = std::env::current_exe()?
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("stream-assist{}", std::env::consts::EXE_SUFFIX));
    assert!(exe.is_file(), "build the real stream-assist binary first");
    Ok(VoiceLab {
        root,
        store: Arc::new(Mutex::new(store)),
        runner: Arc::new(Runner::new(exe, vec![])),
        gpu_install: tokio::sync::Mutex::new(None),
        gpu_gate: std::sync::Mutex::new(()),
        installing: tokio::sync::Mutex::new(()),
    })
}

#[tokio::test]
#[ignore = "requires SUPERTONIC_MODEL_DIR, encoder and English reference WAVs; run in release"]
async fn acceptance_p2() -> anyhow::Result<()> {
    use mixer::DpStrategy;
    use runner::Emit;
    use worker::{ClonePreset, WorkerEvent};
    let total = Instant::now();
    let model =
        PathBuf::from(std::env::var_os("SUPERTONIC_MODEL_DIR").expect("set SUPERTONIC_MODEL_DIR"))
            .canonicalize()?;
    let data = tempfile::tempdir()?;
    let lab = fresh_lab(data.path().join("original"), &model)?;
    let emit: Emit = Arc::new(|_| {});
    let mut profiles = Vec::new();
    let mut sources = Vec::new();
    for name in ["luna", "nora"] {
        let source = audio::import(
            &lab.store,
            &model.join(format!("{name}_reference.wav")),
            true,
            false,
        )?;
        let started = Instant::now();
        let id = clone(
            &lab,
            source.id,
            0,
            (source.duration_ms as u64).min(10_000),
            String::new(),
            "en".into(),
            name.into(),
            ClonePreset::Fast,
            42,
            None,
            emit.clone(),
        )?;
        let job = tokio::time::timeout(std::time::Duration::from_secs(1800), async {
            loop {
                let job = lab.runner.jobs().into_iter().find(|j| j.id == id).unwrap();
                if job.status != "running" {
                    break job;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await?;
        assert_eq!(job.status, "completed", "{job:?}");
        println!("p2_clone_{name}_s={:.3}", started.elapsed().as_secs_f64());
        profiles.push(lab.store.lock().unwrap().profile(job.profile_id.unwrap())?);
        sources.push(source);
    }
    let params = store::RenderParams {
        text: "Привет! Это проверка нового голоса.".into(),
        lang: "ru".into(),
        speed: 1.0,
        quality: 8,
    };
    let (a, b) = (profiles[0].id, profiles[1].id);
    let started = Instant::now();
    let interp = explore(
        &lab,
        Explore::Interp { a, b, points: 5 },
        params.clone(),
        None,
        emit.clone(),
    )
    .await?;
    assert!(!interp.cancelled);
    assert_eq!(interp.items.len(), 5);
    for (i, item) in interp.items.iter().enumerate() {
        let recipe = item.render.recipe.as_ref().unwrap();
        assert_eq!(recipe.dp_strategy, DpStrategy::Weighted);
        assert_eq!(
            recipe.inputs.iter().map(|v| v.weight).collect::<Vec<_>>(),
            vec![100.0 - i as f64 * 25.0, i as f64 * 25.0]
        );
        assert!(
            lab.store
                .lock()
                .unwrap()
                .render_path(item.render.id)
                .is_file()
        );
    }
    println!("p2_interp_s={:.3}", started.elapsed().as_secs_f64());
    let recipe = interp.items[1].render.recipe.as_ref().unwrap();
    let snapshot = save_snapshot(
        &lab,
        recipe
            .inputs
            .iter()
            .map(|input| {
                let p = profiles
                    .iter()
                    .find(|p| p.style_sha256 == input.profile_sha256)
                    .unwrap();
                (p.id, input.weight)
            })
            .collect(),
        DpStrategy::Weighted,
        None,
        "P2 snapshot".into(),
    )?;
    assert!(![a, b].contains(&snapshot.id));
    lab.store
        .lock()
        .unwrap()
        .rename_profile(snapshot.id, "P2 snapshot", &["acceptance".into()])?;
    let ids = vec![a, b, snapshot.id];
    let started = Instant::now();
    let mut weights = Vec::new();
    for _ in 0..2 {
        let result = explore(
            &lab,
            Explore::Random {
                ids: ids.clone(),
                k: 4,
                seed: 7,
                dp: DpStrategy::Weighted,
            },
            params.clone(),
            None,
            emit.clone(),
        )
        .await?;
        assert_eq!(result.items.len(), 4);
        assert!(!result.cancelled);
        weights.push(
            result
                .items
                .iter()
                .map(|item| {
                    item.render
                        .recipe
                        .as_ref()
                        .unwrap()
                        .inputs
                        .iter()
                        .map(|v| v.weight)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(weights[0], weights[1]);
    println!("p2_random_twice_s={:.3}", started.elapsed().as_secs_f64());
    let started = Instant::now();
    let mut similar = explore(
        &lab,
        Explore::Interp { a, b, points: 3 },
        params.clone(),
        Some(RefRegion {
            source_id: sources[0].id,
            start_ms: 0,
            end_ms: 3000,
        }),
        emit.clone(),
    )
    .await?;
    assert_eq!(similar.items.len(), 3);
    assert!(
        similar
            .items
            .iter()
            .all(|v| v.similarity.is_some_and(f32::is_finite))
    );
    similar
        .items
        .sort_by(|a, b| b.similarity.unwrap().total_cmp(&a.similarity.unwrap()));
    assert!(
        similar
            .items
            .windows(2)
            .all(|v| v[0].similarity >= v[1].similarity)
    );
    println!(
        "p2_similarity_s={:.3} scores={:?}",
        started.elapsed().as_secs_f64(),
        similar
            .items
            .iter()
            .map(|v| v.similarity.unwrap())
            .collect::<Vec<_>>()
    );
    let runner = lab.runner.clone();
    let cancel: Emit = Arc::new(move |job| {
        if job.status == "running"
            && matches!(job.progress, Some(WorkerEvent::Item { index: 1, .. }))
        {
            let _ = runner.cancel(job.id);
        }
    });
    let started = Instant::now();
    let cancelled = explore(
        &lab,
        Explore::Interp { a, b, points: 9 },
        params.clone(),
        None,
        cancel,
    )
    .await?;
    assert!(cancelled.cancelled);
    assert!((2..9).contains(&cancelled.items.len()));
    {
        let s = lab.store.lock().unwrap();
        for item in &cancelled.items {
            assert!(s.render_path(item.render.id).is_file());
        }
    }
    println!(
        "p2_cancel_s={:.3} finished={}",
        started.elapsed().as_secs_f64(),
        cancelled.items.len()
    );
    lab.store
        .lock()
        .unwrap()
        .set_favorite(interp.items[1].render.id, true)?;
    let archive = data.path().join("project.svlab");
    project::export_project(&lab.store, &archive)?;
    let fresh = fresh_lab(data.path().join("fresh"), &model)?;
    let report = project::import_project(&fresh.store, &archive)?;
    assert_eq!((report.profiles, report.renders, report.skipped), (3, 1, 0));
    assert!(!report.revision_mismatch);
    let imported = {
        let s = fresh.store.lock().unwrap();
        assert!(s.sources()?.is_empty());
        for profile in lab.store.lock().unwrap().profiles()? {
            let p = s
                .profiles()?
                .into_iter()
                .find(|p| p.style_sha256 == profile.style_sha256)
                .unwrap();
            assert_eq!(p.recipe, profile.recipe);
            assert_eq!(p.tags, profile.tags);
            assert_eq!(p.origin, "import");
        }
        s.profiles()?
            .into_iter()
            .find(|p| p.style_sha256 == snapshot.style_sha256)
            .unwrap()
    };
    fresh.store.lock().unwrap().prune_renders(0)?;
    let favorites = fresh.store.lock().unwrap().renders(200)?;
    assert_eq!(favorites.len(), 1);
    assert!(favorites[0].favorite);
    assert_eq!(favorites[0].recipe, interp.items[1].render.recipe);
    let started = Instant::now();
    let output = render(&fresh, imported.id, params, emit).await?;
    let samples = audio::load_mono(&fresh.store.lock().unwrap().render_path(output.id))?;
    assert!(samples.iter().all(|v| v.is_finite()));
    assert!(samples.iter().any(|v| v.abs() > 0.01));
    println!(
        "p2_imported_synth_s={:.3} synth_ms={} duration_ms={}",
        started.elapsed().as_secs_f64(),
        output.wall_ms,
        output.duration_ms
    );
    for root in [&lab.root, &fresh.root] {
        assert_eq!(fs::read_dir(root.join("tmp"))?.count(), 0);
    }
    println!("p2_total_s={:.3}", total.elapsed().as_secs_f64());
    Ok(())
}

#[tokio::test]
#[ignore = "needs NVIDIA GPU, CUDA 13, cuDNN 9 and GPU pack"]
async fn real_gpu_clone_fast() -> anyhow::Result<()> {
    use supertonic3_tts::{
        Device, features,
        speaker::{SpeakerEncoder, cosine},
    };
    use worker::{CloneDevice, ClonePreset};
    if std::env::var_os("VL_GPU_TEST_CHILD").is_some() {
        use supertonic3_tts::helper::{load_text_to_speech_on, write_wav_file};
        let model = PathBuf::from(
            std::env::var_os("SUPERTONIC_MODEL_DIR").expect("acceptance helper only"),
        );
        let style = style::read_style_file(&PathBuf::from(
            std::env::var_os("VL_GPU_TEST_STYLE").unwrap(),
        ))?;
        let out = PathBuf::from(std::env::var_os("VL_GPU_TEST_OUT").unwrap());
        for (device, name) in [
            (Device::Cpu, "cpu.wav"),
            (Device::Cpu, "cpu2.wav"),
            (Device::Gpu, "gpu.wav"),
        ] {
            let mut tts = load_text_to_speech_on(model.join("onnx").to_str().unwrap(), device)?;
            let (samples, _) = tts.call(
                "Hello, this is a voice compatibility test.",
                "en",
                &style,
                8,
                1.0,
                0.3,
            )?;
            write_wav_file(out.join(name), &samples, tts.sample_rate)?;
        }
        return Ok(());
    }
    let model =
        PathBuf::from(std::env::var_os("SUPERTONIC_MODEL_DIR").expect("set SUPERTONIC_MODEL_DIR"))
            .canonicalize()?;
    let pack = PathBuf::from(std::env::var_os("VL_GPU_PACK_DIR").expect("set VL_GPU_PACK_DIR"));
    let data = tempfile::tempdir()?;
    let lab = fresh_lab(data.path().join("gpu-clone"), &model)?;
    fs::create_dir_all(gpu::dir(&lab.root))?;
    for name in [
        "libonnxruntime_providers_cuda.so",
        "libonnxruntime_providers_shared.so",
        "LICENSE",
        "ThirdPartyNotices.txt",
    ] {
        fs::copy(pack.join(name), gpu::dir(&lab.root).join(name))?;
    }
    let source = audio::import(&lab.store, &model.join("luna_reference.wav"), true, false)?;
    let id = clone(
        &lab,
        source.id,
        0,
        (source.duration_ms as u64).min(10_000),
        String::new(),
        "en".into(),
        "GPU fast".into(),
        ClonePreset::Fast,
        42,
        Some(CloneDevice::Gpu),
        Arc::new(|_| {}),
    )?;
    let job = tokio::time::timeout(std::time::Duration::from_secs(1800), async {
        loop {
            let job = lab.runner.jobs().into_iter().find(|j| j.id == id).unwrap();
            if job.status != "running" {
                break job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await?;
    assert_eq!(job.status, "completed", "{job:?}");
    let style_path = {
        let s = lab.store.lock().unwrap();
        s.profile_path(&s.profile(job.profile_id.unwrap())?)
    };
    // A separate process must set argv[0] before ORT initializes.
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    #[cfg(unix)]
    command.arg0(gpu::dir(&lab.root).join("stream-assist"));
    let status = command
        .args([
            "--ignored",
            "--exact",
            "voice_lab::acceptance_p2::real_gpu_clone_fast",
        ])
        .env("VL_GPU_TEST_CHILD", "1")
        .env("VL_GPU_TEST_STYLE", style_path)
        .env("SUPERTONIC_MODEL_DIR", &model)
        .env("VL_GPU_TEST_OUT", data.path())
        .status()
        .await?;
    assert!(status.success());
    let mut encoder = SpeakerEncoder::on_device(model.join("speaker_encoder.onnx"), Device::Cpu)?;
    let mut embeddings = Vec::new();
    let mut lengths = Vec::new();
    for name in ["cpu.wav", "cpu2.wav", "gpu.wav"] {
        let wav = features::load_wav(data.path().join(name))?;
        lengths.push(wav.samples.len());
        embeddings.push(encoder.embed(&wav.samples, wav.sample_rate)?);
    }
    // Synthesis samples noise: two CPU runs land at ~0.88-0.90 and a single CPU/GPU pair at 0.83-0.92.
    // Broken GPU numerics would give unrelated audio (well below 0.75) or a different length.
    let cpu_cpu = cosine(&embeddings[0], &embeddings[1]);
    let cpu_gpu = cosine(&embeddings[0], &embeddings[2]);
    println!("similarity cpu-cpu {cpu_cpu:.4} cpu-gpu {cpu_gpu:.4}");
    assert!(
        cpu_gpu >= 0.75 && cpu_gpu >= cpu_cpu - 0.10,
        "cpu-cpu {cpu_cpu} cpu-gpu {cpu_gpu}"
    );
    assert!((lengths[0] as f64 / lengths[2] as f64 - 1.0).abs() <= 0.05);
    Ok(())
}
