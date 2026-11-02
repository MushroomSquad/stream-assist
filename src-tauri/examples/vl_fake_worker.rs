use std::{io::Write, time::Duration};
use stream_assist_lib::voice_lab::{
    audio::{SR, wav_bytes},
    worker::*,
};
fn emit(e: WorkerEvent) {
    println!("{}", serde_json::to_string(&e).unwrap());
    std::io::stdout().flush().unwrap();
}
fn main() {
    spawn_stdin_watchdog();
    if let Ok(path) = std::env::var("VL_FAKE_PID_FILE") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    if let Ok(output) = std::env::var("VL_FAKE_STDOUT") {
        print!("{output}");
        std::io::stdout().flush().unwrap();
        return;
    }
    let mode = std::env::var("VL_FAKE_MODE_FILE")
        .ok()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|| std::env::var("VL_FAKE_MODE").unwrap_or("ok".into()));
    if mode == "eofwatch" {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let path = std::env::args_os().nth(2).unwrap();
    let req: WorkerRequest = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    if mode == "crash" {
        std::process::exit(3);
    }
    for i in 1..=2 {
        emit(WorkerEvent::Progress {
            stage: "mix-presets".into(),
            iteration: i,
            total: 2,
            loss: 1.0,
            best_loss: 0.5,
        });
        if mode == "sleep" {
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
    }
    match req {
        WorkerRequest::Synth { out_wav, .. } => {
            std::fs::write(out_wav, wav_bytes(&vec![0.0; SR as usize], SR)).unwrap();
            emit(WorkerEvent::Done {
                duration_ms: Some(1000),
                synth_ms: Some(5),
                loss: None,
                similarity: None,
            });
        }
        WorkerRequest::Clone { out_style, .. } => {
            let width = if mode == "badstyle" { 128 } else { 256 };
            let json = serde_json::json!({"style_ttl":{"dims":[1,50,width],"type":"float32","data":vec![vec![vec![0.1; width];50]]},"style_dp":{"dims":[1,8,16],"type":"float32","data":vec![vec![vec![0.1;16];8]]}});
            std::fs::write(out_style, serde_json::to_vec(&json).unwrap()).unwrap();
            emit(WorkerEvent::Done {
                duration_ms: None,
                synth_ms: None,
                loss: Some(0.5),
                similarity: None,
            });
        }
    }
}
