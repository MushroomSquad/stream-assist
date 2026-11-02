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

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ClonePreset {
    Fast,
    Standard,
    Precise,
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
    Clone {
        model_dir: PathBuf,
        reference_wav: PathBuf,
        out_style: PathBuf,
        language: String,
        transcript: Option<String>,
        preset: ClonePreset,
        seed: u64,
        speaker_model: Option<PathBuf>,
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
    Done {
        duration_ms: Option<i64>,
        synth_ms: Option<i64>,
        loss: Option<f32>,
        similarity: Option<f32>,
    },
    Error {
        code: String,
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
        ..Default::default()
    };
    match preset {
        ClonePreset::Fast => {
            opts.mix_iterations = 20;
            opts.row_iterations = 30;
            opts.iterations = 50;
        }
        ClonePreset::Precise => {
            opts.mix_iterations = 80;
            opts.row_iterations = 120;
            opts.iterations = 200;
        }
        ClonePreset::Standard => {}
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
fn emit(e: WorkerEvent) {
    let mut out = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, &e);
    let _ = writeln!(out);
    let _ = out.flush();
}
fn execute(req: WorkerRequest) -> Result<WorkerEvent, VlError> {
    let dir = match &req {
        WorkerRequest::Synth { model_dir, .. } | WorkerRequest::Clone { model_dir, .. } => {
            model_dir
        }
    };
    let onnx = dir.join("onnx");
    let path = onnx.to_str().ok_or_else(|| err("IO_FAILED"))?;
    let mut tts = load_text_to_speech_on(path, Device::Cpu).map_err(|_| err("WORKER_FAILED"))?;
    match req {
        WorkerRequest::Synth {
            style,
            out_wav,
            params,
            ..
        } => {
            let style = read_style_file(&style)?;
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
                .map_err(|_| err("WORKER_FAILED"))?;
            let synth_ms = t.elapsed().as_millis() as i64;
            write_wav_file(&out_wav, &samples, tts.sample_rate)
                .map_err(|_| err("WORKER_FAILED"))?;
            Ok(WorkerEvent::Done {
                duration_ms: Some(samples.len() as i64 * 1000 / tts.sample_rate as i64),
                synth_ms: Some(synth_ms),
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
        } => {
            let audio = features::load_wav(reference_wav).map_err(|_| err("WORKER_FAILED"))?;
            let opts = clone_options(
                &language,
                transcript.as_deref(),
                preset,
                seed,
                speaker_model,
            );
            let cv = VoiceCloner::new(&mut tts, &audio, &model_dir.join("voice_styles"), opts)
                .map_err(|_| err("WORKER_FAILED"))?
                .run(&mut |p| {
                    emit(WorkerEvent::Progress {
                        stage: p.stage.to_string(),
                        iteration: p.iteration,
                        total: p.total,
                        loss: p.loss,
                        best_loss: p.best_loss,
                    })
                })
                .map_err(|_| err("WORKER_FAILED"))?;
            save_voice_style(out_style, &cv.style, None).map_err(|_| err("WORKER_FAILED"))?;
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
        let req = serde_json::from_slice(&std::fs::read(request_path)?)?;
        execute(req)
    })();
    match result {
        Ok(e) => {
            emit(e);
            0
        }
        Err(e) => {
            emit(WorkerEvent::Error { code: e.code() });
            1
        }
    }
}
