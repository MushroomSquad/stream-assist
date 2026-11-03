use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
fn emit(v: Value) {
    println!("{v}");
    std::io::stdout().flush().unwrap();
}
fn style(dir: &std::path::Path, step: u32) -> std::path::PathBuf {
    let p = dir.join(format!("ckpt_{step}.json"));
    std::fs::write(&p,serde_json::to_vec(&json!({"style_ttl":{"dims":[1,50,256],"type":"float32","data":vec![vec![vec![step as f32;256];50]]},"style_dp":{"dims":[1,8,16],"type":"float32","data":vec![vec![vec![0.1;16];8]]}})).unwrap()).unwrap();
    p
}
fn main() {
    let req: Value =
        serde_json::from_slice(&std::fs::read(std::env::args_os().last().unwrap()).unwrap())
            .unwrap();
    let dir = std::path::PathBuf::from(req["runDir"].as_str().unwrap());
    let mode = std::env::var("VL_FAKE_MODE").unwrap_or("ok".into());
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read_to_end(&mut Vec::new());
        flag.store(true, Ordering::SeqCst);
    });
    match mode.as_str() {
        "busy" | "oom" | "weird" | "nonfinite" | "error_done" => {
            emit(
                json!({"event":"error","code":match mode.as_str(){"busy"=>"JOB_BUSY","oom"|"error_done"=>"GPU_OUT_OF_MEMORY","nonfinite"=>"WORKER_FAILED",_=>"FOO"}}),
            );
            if mode == "error_done" {
                emit(json!({"event":"done","step":3,"stylePath":style(&dir,3),"bestLoss":0.5}));
            }
            return;
        }
        "crash" => std::process::exit(3),
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        "slowstart" => {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if stop.load(Ordering::SeqCst) {
                    emit(json!({"event":"stopped","step":0,"stylePath":null}));
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        _ => {}
    }
    let n = if req["resume"] == true {
        std::fs::read_to_string(dir.join("state.pt"))
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    } else {
        0
    };
    emit(json!({"event":"started","resumedFrom":n}));
    if mode == "resume" {
        emit(json!({"event":"done","step":n,"stylePath":style(&dir,n),"bestLoss":0.5}));
        return;
    }
    if mode == "stopped_done" {
        emit(json!({"event":"stopped","step":0,"stylePath":null}));
    }
    if mode == "stoppable" {
        let mut step = 0;
        loop {
            step += 1;
            emit(
                json!({"event":"progress","step":step,"total":1000,"loss":1.0,"bestLoss":0.5,"lr":0.0002}),
            );
            std::thread::sleep(Duration::from_millis(100));
            if stop.load(Ordering::SeqCst) {
                std::fs::write(dir.join("state.pt"), step.to_string()).unwrap();
                emit(json!({"event":"stopped","step":step,"stylePath":null}));
                return;
            }
        }
    }
    if mode == "ok" {
        for step in 1..=3 {
            emit(
                json!({"event":"progress","step":step,"total":1000,"loss":1.0,"bestLoss":0.5,"lr":0.0002}),
            );
        }
        emit(json!({"event":"checkpoint","step":2,"stylePath":style(&dir,2),"bestLoss":0.5}));
    }
    emit(json!({"event":"done","step":3,"stylePath":style(&dir,3),"bestLoss":0.5}));
    if mode == "done_error" {
        emit(json!({"event":"error","code":"GPU_OUT_OF_MEMORY"}));
    }
}
