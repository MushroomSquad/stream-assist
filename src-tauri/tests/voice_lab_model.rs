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
