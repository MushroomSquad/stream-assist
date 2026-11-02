pub mod app;
pub mod db;
pub mod driver;
pub mod i18n;
pub mod limits;
pub mod model;
pub mod music;
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
        .invoke_handler(tauri::generate_handler![
            app::get_state,
            app::skip,
            app::remove_item,
            app::move_item,
            app::toggle_pause,
            app::resume_queue,
            app::set_volume,
            app::get_settings,
            app::save_settings,
            app::twitch_login_start,
            app::twitch_login_poll,
            app::twitch_logout,
            app::spotify_login,
            app::spotify_status,
            app::obs_check,
            app::obs_add_widget,
            app::list_themes,
            app::import_theme,
            app::widget_test
        ])
        .setup(|app| crate::app::setup(app).map_err(Into::into))
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                api.prevent_exit();
                crate::app::exit(app);
            }
        });
}
