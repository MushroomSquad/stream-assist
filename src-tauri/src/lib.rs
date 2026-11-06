use tauri::Manager;

pub mod app;
pub mod db;
pub mod diag;
pub mod driver;
pub mod http;
pub mod i18n;
pub mod limits;
pub mod model;
pub mod music;
pub mod obs;
pub mod player;
pub mod request;
pub mod rewards;
pub mod rules;
pub mod secrets;
pub mod server;
pub mod spotify;
#[cfg(target_os = "linux")]
pub mod spotify_local;
pub mod tts;
pub mod twitch;
pub mod voice_lab;
pub mod youtube;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            app::update_install_supported,
            app::prepare_update,
            app::update_install_failed,
            app::restart_after_update,
            app::open_logs,
            app::log_dir,
            app::tts_state,
            app::tts_skip,
            app::tts_clear,
            app::tts_test,
            app::obs_add_tts,
            app::get_state,
            app::skip,
            app::remove_item,
            app::move_item,
            app::toggle_pause,
            app::resume_queue,
            app::set_volume,
            app::get_settings,
            app::running_port,
            app::twitch_status,
            app::save_settings,
            app::rule_errors,
            app::twitch_login_start,
            app::twitch_login_poll,
            app::twitch_logout,
            app::spotify_login,
            app::spotify_status,
            app::spotify_volume,
            app::obs_check,
            app::obs_add_widget,
            app::list_themes,
            app::import_theme,
            app::widget_test,
            voice_lab::commands::vl_gpu_status,
            voice_lab::commands::vl_gpu_install,
            voice_lab::commands::vl_gpu_install_cancel,
            voice_lab::commands::vl_gpu_uninstall,
            voice_lab::commands::vl_status,
            voice_lab::commands::vl_install,
            voice_lab::commands::vl_import_audio,
            voice_lab::commands::vl_sources,
            voice_lab::commands::vl_set_consent,
            voice_lab::commands::vl_peaks,
            voice_lab::commands::vl_source_audio,
            voice_lab::commands::vl_cancel_job,
            voice_lab::commands::vl_jobs,
            voice_lab::commands::vl_import_profile,
            voice_lab::commands::vl_profiles,
            voice_lab::commands::vl_rename_profile,
            voice_lab::commands::vl_delete_profile,
            voice_lab::commands::vl_renders,
            voice_lab::commands::vl_render_audio,
            voice_lab::commands::vl_set_favorite,
            voice_lab::commands::vl_export_project,
            voice_lab::commands::vl_import_project,
            voice_lab::commands::vl_export_profile,
            voice_lab::commands::vl_export_recipe,
            voice_lab::commands::vl_export_render,
            voice_lab::commands::vl_save_snapshot,
            voice_lab::commands::vl_render,
            voice_lab::commands::vl_mix_preview,
            voice_lab::commands::vl_explore,
            voice_lab::commands::vl_clone
        ])
        .setup(|app| {
            crate::diag::init(&app.path().app_data_dir()?.join("logs"));
            crate::voice_lab::init(app);
            crate::app::setup(app).map_err(Into::into)
        })
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
