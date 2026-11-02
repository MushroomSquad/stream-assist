pub mod db;
pub mod i18n;
pub mod limits;
pub mod model;
pub mod obs;
pub mod player;
pub mod request;
pub mod router;
pub mod secrets;
pub mod server;
pub mod spotify;
pub mod twitch;
pub mod youtube;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
