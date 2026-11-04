use super::{VlError, err, store::RenderParams, style::read_style_file};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};
use supertonic3_tts::{
    CloneOptions, Device, VoiceCloner, features,
    helper::{load_text_to_speech_on, save_voice_style, write_wav_file},
    probe_bank_for,
};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CloneDevice {
    #[default]
    Cpu,
    Gpu,
}
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ClonePreset {
    Fast,
    Standard,
    Precise,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BatchItem {
    pub style: PathBuf,
    pub out_wav: PathBuf,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum WorkerRequest {
    Synth {
        model_dir: PathBuf,
        style: PathBuf,
        out_wav: PathBuf,
        params: RenderParams,
    },
    #[serde(rename = "synthBatch")]
    SynthBatch {
        model_dir: PathBuf,
        items: Vec<BatchItem>,
        params: RenderParams,
        encoder: Option<PathBuf>,
        reference_wav: Option<PathBuf>,
    },
    Clone {
        model_dir: PathBuf,
        reference_wav: PathBuf,
        out_style: PathBuf,
        language: String,
        transcript: Option<String>,
        preset: ClonePreset,
        seed: u64,
        speaker_model: Option<PathBuf>,
        #[serde(default)]
        device: CloneDevice,
    },
}
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "event",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum WorkerEvent {
    Progress {
        stage: String,
        iteration: usize,
        total: usize,
        loss: f32,
        best_loss: f32,
    },
    Item {
        index: usize,
        duration_ms: i64,
        synth_ms: i64,
        similarity: Option<f32>,
    },
    Done {
        duration_ms: Option<i64>,
        synth_ms: Option<i64>,
        loss: Option<f32>,
        similarity: Option<f32>,
    },
    Error {
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}
pub const RU_PROBES: &[&str] = &[
    "Сегодня утром солнце осветило тихую городскую улицу.",
    "На столе лежали книги и свежие газеты.",
    "После обеда мы отправились гулять по парку.",
    "Вечером над рекой медленно поднимался лёгкий туман.",
    "За окном слышался негромкий шум осеннего дождя.",
    "Поезд прибыл на станцию точно по расписанию.",
    "В небольшом саду росли яблони и груши.",
    "Мы решили встретиться завтра возле старого театра.",
    "На берегу озера стоял небольшой деревянный дом.",
    "Тёплый ветер тихо шевелил листья высоких деревьев.",
    "Дорога через лес привела нас к деревне.",
    "Утром на площади открылась новая книжная лавка.",
];
pub fn clone_options(
    language: &str,
    transcript: Option<&str>,
    preset: ClonePreset,
    seed: u64,
    speaker_model: Option<PathBuf>,
) -> CloneOptions {
    let mut opts = CloneOptions {
        language: language.into(),
        reference_text: transcript
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        reference_window: None,
        seed,
        speaker_model,
        device: Device::Cpu,
        voice_quality: 8,
        ..Default::default()
    };
    match preset {
        ClonePreset::Fast => {
            opts.mix_iterations = 10;
            opts.row_iterations = 15;
            opts.iterations = 25;
            opts.voice_quality = 5;
        }
        ClonePreset::Standard => {
            opts.mix_iterations = 20;
            opts.row_iterations = 30;
            opts.iterations = 50;
        }
        ClonePreset::Precise => {}
    }
    if language == "ru" {
        opts.probe_bank = RU_PROBES.iter().map(|s| s.to_string()).collect();
    }
    opts
}
pub fn transcript_required(lang: &str, transcript: &str) -> bool {
    transcript.trim().is_empty() && (lang == "ru" || probe_bank_for(lang).is_empty())
}
pub fn spawn_stdin_watchdog() {
    std::thread::spawn(|| {
        let mut buf = [0; 1024];
        let mut input = std::io::stdin().lock();
        loop {
            match input.read(&mut buf) {
                Ok(0) | Err(_) => std::process::exit(2),
                Ok(_) => {}
            }
        }
    });
}
fn sanitize(mut e: WorkerEvent) -> WorkerEvent {
    fn finite(v: &mut f32) {
        if !v.is_finite() {
            *v = f32::MAX;
        }
    }
    match &mut e {
        WorkerEvent::Progress {
            loss, best_loss, ..
        } => {
            finite(loss);
            finite(best_loss);
        }
        WorkerEvent::Done {
            loss, similarity, ..
        } => {
            if let Some(loss) = loss {
                finite(loss);
            }
            *similarity = similarity.filter(|v| v.is_finite());
        }
        WorkerEvent::Item { similarity, .. } => {
            *similarity = similarity.filter(|v| v.is_finite());
        }
        WorkerEvent::Error { detail, .. } => {
            *detail = detail.take().map(|d| d.chars().take(2000).collect());
        }
    }
    e
}
fn emit(e: WorkerEvent) {
    let e = sanitize(e);
    let mut out = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, &e);
    let _ = writeln!(out);
    let _ = out.flush();
}
fn execute(req: WorkerRequest) -> Result<WorkerEvent, (VlError, Option<String>)> {
    let gpu = matches!(
        &req,
        WorkerRequest::Clone {
            device: CloneDevice::Gpu,
            ..
        }
    );
    let device = if gpu { Device::Gpu } else { Device::Cpu };
    let model_error = |e: anyhow::Error| {
        if gpu {
            (
                err("GPU_FAILED"),
                Some(format!("{e:#}").chars().take(2000).collect()),
            )
        } else {
            (err("WORKER_FAILED"), None)
        }
    };
    let dir = match &req {
        WorkerRequest::Synth { model_dir, .. }
        | WorkerRequest::SynthBatch { model_dir, .. }
        | WorkerRequest::Clone { model_dir, .. } => model_dir,
    };
    let onnx = dir.join("onnx");
    let path = onnx.to_str().ok_or_else(|| (err("IO_FAILED"), None))?;
    let mut tts = load_text_to_speech_on(path, device).map_err(model_error)?;
    match req {
        WorkerRequest::Synth {
            style,
            out_wav,
            params,
            ..
        } => {
            let style = read_style_file(&style).map_err(|e| (e, None))?;
            let t = Instant::now();
            let (samples, _) = tts
                .call(
                    &params.text,
                    &params.lang,
                    &style,
                    params.quality,
                    params.speed,
                    0.3,
                )
                .map_err(|_| (err("WORKER_FAILED"), None))?;
            let synth_ms = t.elapsed().as_millis() as i64;
            write_wav_file(&out_wav, &samples, tts.sample_rate)
                .map_err(|_| (err("WORKER_FAILED"), None))?;
            Ok(WorkerEvent::Done {
                duration_ms: Some(samples.len() as i64 * 1000 / tts.sample_rate as i64),
                synth_ms: Some(synth_ms),
                loss: None,
                similarity: None,
            })
        }
        WorkerRequest::SynthBatch {
            items,
            params,
            encoder,
            reference_wav,
            ..
        } => {
            let mut reference = match (encoder, reference_wav) {
                (None, None) => None,
                (Some(path), Some(wav)) => {
                    let mut encoder =
                        supertonic3_tts::speaker::SpeakerEncoder::on_device(path, Device::Cpu)
                            .map_err(|_| (err("WORKER_FAILED"), None))?;
                    let audio =
                        features::load_wav(wav).map_err(|_| (err("WORKER_FAILED"), None))?;
                    let embedding = encoder
                        .embed(&audio.samples, audio.sample_rate)
                        .map_err(|_| (err("WORKER_FAILED"), None))?;
                    Some((encoder, embedding))
                }
                _ => return Err((err("WORKER_FAILED"), None)),
            };
            for (index, item) in items.into_iter().enumerate() {
                let style = read_style_file(&item.style).map_err(|e| (e, None))?;
                let start = Instant::now();
                let (samples, _) = tts
                    .call(
                        &params.text,
                        &params.lang,
                        &style,
                        params.quality,
                        params.speed,
                        0.3,
                    )
                    .map_err(|_| (err("WORKER_FAILED"), None))?;
                let synth_ms = start.elapsed().as_millis() as i64;
                write_wav_file(&item.out_wav, &samples, tts.sample_rate)
                    .map_err(|_| (err("WORKER_FAILED"), None))?;
                let similarity = reference.as_mut().and_then(|(encoder, embedding)| {
                    encoder
                        .embed(&samples, tts.sample_rate as u32)
                        .ok()
                        .map(|value| supertonic3_tts::speaker::cosine(embedding, &value))
                        .filter(|value| value.is_finite())
                });
                emit(WorkerEvent::Item {
                    index,
                    duration_ms: samples.len() as i64 * 1000 / tts.sample_rate as i64,
                    synth_ms,
                    similarity,
                });
            }
            Ok(WorkerEvent::Done {
                duration_ms: None,
                synth_ms: None,
                loss: None,
                similarity: None,
            })
        }
        WorkerRequest::Clone {
            model_dir,
            reference_wav,
            out_style,
            language,
            transcript,
            preset,
            seed,
            speaker_model,
            device: _,
        } => {
            let audio =
                features::load_wav(reference_wav).map_err(|_| (err("WORKER_FAILED"), None))?;
            let mut opts = clone_options(
                &language,
                transcript.as_deref(),
                preset,
                seed,
                speaker_model,
            );
            opts.device = device;
            let cv = VoiceCloner::new(&mut tts, &audio, &model_dir.join("voice_styles"), opts)
                .map_err(model_error)?
                .run(&mut |p| {
                    emit(WorkerEvent::Progress {
                        stage: p.stage.to_string(),
                        iteration: p.iteration,
                        total: p.total,
                        loss: p.loss,
                        best_loss: p.best_loss,
                    })
                })
                .map_err(model_error)?;
            save_voice_style(out_style, &cv.style, None)
                .map_err(|_| (err("WORKER_FAILED"), None))?;
            Ok(WorkerEvent::Done {
                duration_ms: None,
                synth_ms: None,
                loss: Some(cv.loss),
                similarity: cv.speaker_similarity,
            })
        }
    }
}
pub fn main(request_path: &Path) -> i32 {
    spawn_stdin_watchdog();
    #[cfg(unix)]
    unsafe {
        libc::nice(10);
    }
    let result = (|| {
        let bytes = std::fs::read(request_path).map_err(|e| (VlError::from(e), None))?;
        let req = serde_json::from_slice(&bytes).map_err(|e| (VlError::from(e), None))?;
        execute(req)
    })();
    match result {
        Ok(e) => {
            emit(e);
            0
        }
        Err((e, detail)) => {
            emit(WorkerEvent::Error {
                code: e.code(),
                detail,
            });
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clone_request_defaults_to_cpu() {
        let req: WorkerRequest = serde_json::from_value(serde_json::json!({
            "op":"clone","modelDir":"m","referenceWav":"r","outStyle":"o",
            "language":"en","transcript":null,"preset":"fast","seed":42,"speakerModel":null
        }))
        .unwrap();
        assert!(matches!(
            req,
            WorkerRequest::Clone {
                device: CloneDevice::Cpu,
                ..
            }
        ));
    }
    #[test]
    fn error_event_detail_is_optional() {
        let event: WorkerEvent = serde_json::from_str(r#"{"event":"error","code":"X"}"#).unwrap();
        assert!(matches!(&event, WorkerEvent::Error { detail: None, .. }));
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({"event":"error","code":"X"})
        );
    }
    #[test]
    fn nonfinite_progress_is_parsable() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for event in [
                WorkerEvent::Progress {
                    stage: "mix".into(),
                    iteration: 1,
                    total: 2,
                    loss: value,
                    best_loss: value,
                },
                WorkerEvent::Item {
                    index: 0,
                    duration_ms: 1000,
                    synth_ms: 1,
                    similarity: Some(value),
                },
                WorkerEvent::Done {
                    duration_ms: None,
                    synth_ms: None,
                    loss: Some(value),
                    similarity: Some(value),
                },
            ] {
                let clean = sanitize(event);
                let json = serde_json::to_string(&clean).unwrap();
                assert_eq!(serde_json::from_str::<WorkerEvent>(&json).unwrap(), clean);
                match clean {
                    WorkerEvent::Progress {
                        loss, best_loss, ..
                    } => assert_eq!((loss, best_loss), (f32::MAX, f32::MAX)),
                    WorkerEvent::Done {
                        loss, similarity, ..
                    } => assert_eq!((loss, similarity), (Some(f32::MAX), None)),
                    WorkerEvent::Item { similarity, .. } => assert_eq!(similarity, None),
                    _ => unreachable!(),
                }
            }
        }
    }
}
