// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let (Some("--vl-worker"), Some(p)) = (args.get(1).map(String::as_str), args.get(2)) {
        std::process::exit(stream_assist_lib::voice_lab::worker::main(
            std::path::Path::new(p),
        ));
    }
    stream_assist_lib::run()
}
