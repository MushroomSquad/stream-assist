use crate::{
    driver::{DriverCtx, DriverMsg, Settings},
    model::Tokens,
    player::WidgetState,
    spotify::Spotify,
    twitch::helix::Helix,
};
use std::sync::{Arc, Mutex, RwLock};
use tauri::Manager;
use tokio::sync::{mpsc, watch};
pub struct AppState {
    pub driver: mpsc::Sender<DriverMsg>,
    pub state: watch::Receiver<WidgetState>,
}
pub fn save_tokens(key: &'static str, t: &Tokens, app: tauri::AppHandle) {
    save_tokens_for_generation(key, t, app, crate::secrets::generation(key));
}
fn save_tokens_for_generation(key: &'static str, t: &Tokens, app: tauri::AppHandle, epoch: u64) {
    let value = match serde_json::to_string(t) {
        Ok(value) => value,
        Err(_) => return,
    };
    let Some(pending) = crate::secrets::write_if_current(key, epoch, value) else {
        return;
    };
    tokio::spawn(async move {
        let result = pending.await;
        if !matches!(result, Ok(Ok(()))) {
            if let Some(state) = app.try_state::<AppState>() {
                let _ = send(&state, DriverMsg::Warning("keyring_unavailable".into())).await;
            }
        }
    });
}
pub fn spotify_client(id: String, t: Tokens, app: tauri::AppHandle) -> Arc<Spotify> {
    let epoch = crate::secrets::generation("spotify");
    Arc::new(Spotify::new(
        id,
        t,
        "https://api.spotify.com",
        "https://accounts.spotify.com",
        move |t| save_tokens_for_generation("spotify", t, app.clone(), epoch),
    ))
}
pub fn twitch_client(t: Tokens, app: tauri::AppHandle) -> Arc<Helix> {
    let epoch = crate::secrets::generation("twitch");
    Arc::new(Helix::new(
        option_env!("TWITCH_CLIENT_ID").unwrap_or_default().into(),
        t,
        "https://api.twitch.tv",
        "https://id.twitch.tv",
        move |t| save_tokens_for_generation("twitch", t, app.clone(), epoch),
    ))
}
async fn load(key: &'static str) -> anyhow::Result<Option<Tokens>> {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || load_blocking(key)),
    )
    .await??
}
fn load_blocking(key: &str) -> anyhow::Result<Option<Tokens>> {
    crate::secrets::load(key)?
        .map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
pub fn setup(app: &mut tauri::App) -> anyhow::Result<()> {
    app.manage(DeviceFlow::default());
    let dir = app.path().app_data_dir()?;
    let (state_tx, state) = watch::channel(WidgetState {
        visible: false,
        now: None,
        queue: vec![],
        progress_ms: 0,
        mode: "Idle".into(),
        warning: None,
    });
    let (driver, rx) = mpsc::channel(128);
    app.manage(AppState {
        driver,
        state: state.clone(),
    });
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        if start(handle.clone(), dir, state_tx.clone(), state, rx)
            .await
            .is_err()
        {
            let mut s = state_tx.borrow().clone();
            s.warning = Some("startup_failed".into());
            state_tx.send_replace(s.clone());
            let _ = tauri::Emitter::emit(&handle, "state", s);
        }
    });
    Ok(())
}
async fn start(
    handle: tauri::AppHandle,
    dir: std::path::PathBuf,
    state_tx: watch::Sender<WidgetState>,
    state: watch::Receiver<WidgetState>,
    rx: mpsc::Receiver<DriverMsg>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&dir)?;
    let db = Arc::new(Mutex::new(crate::db::Db::open(
        &dir.join("stream-assist.db"),
    )?));
    let settings = db
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_setting::<Settings>("settings")?
        .unwrap_or_default();
    handle.manage(db.clone());
    let mut warning = None;
    let spotify = match load("spotify").await {
        Ok(Some(t)) => settings
            .spotify_client_id
            .clone()
            .map(|id| spotify_client(id, t, handle.clone())),
        Ok(None) => None,
        Err(_) => {
            warning = Some("keyring_unavailable".into());
            None
        }
    };
    let helix = match load("twitch").await {
        Ok(Some(t)) => Some(twitch_client(t, handle.clone())),
        Ok(None) => None,
        Err(_) => {
            warning = Some("keyring_unavailable".into());
            None
        }
    };
    if warning.is_some() {
        let mut s = state_tx.borrow().clone();
        s.warning = warning;
        state_tx.send_replace(s.clone());
        let _ = tauri::Emitter::emit(&handle, "state", s);
    }
    let active_theme = Arc::new(RwLock::new(settings.active_theme.clone()));
    let tts_dir = dir.join("voice-lab/tmp/tts");
    let (mut ws, server_warning) = crate::server::start_or_disabled(
        settings.port,
        dir.join("themes"),
        active_theme.clone(),
        state,
        tts_dir,
    )
    .await;
    handle.manage(RunningPort(ws.task.as_ref().map(|_| ws.running_port)));
    if let Some(code) = server_warning {
        let mut s = state_tx.borrow().clone();
        s.warning = Some(code.into());
        state_tx.send_replace(s.clone());
        let _ = tauri::Emitter::emit(&handle, "state", s);
    }
    // TODO(Task 7): forward these events to the TTS engine.
    let _tts_rx = ws.tts_rx.take();
    crate::driver::run(
        DriverCtx {
            db,
            active_theme,
            state_tx,
            ws,
            app: handle,
            settings,
            spotify,
            helix,
        },
        rx,
    )
    .await;
    Ok(())
}
pub fn exit(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let operation = async {
            let dir = app.path().app_data_dir()?;
            let db = crate::db::Db::open(&dir.join("stream-assist.db"))?;
            let settings = db.get_setting::<Settings>("settings")?.unwrap_or_default();
            drop(db);
            if let (Some(t), Some(id)) = (load("twitch").await?, settings.music_reward_id) {
                twitch_client(t, app.clone())
                    .update_reward(&id, &serde_json::json!({"is_paused":true}))
                    .await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), operation).await;
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_rejects_invalid_settings() {
        let mut s = Settings::default();
        assert!(validate_settings(&s).is_ok());
        s.port = 0;
        assert!(validate_settings(&s).is_err());
        s.port = 47821;
        s.reward_cost = 0;
        assert!(validate_settings(&s).is_err());
    }
    #[test]
    fn panel_volume_range_is_checked() {
        for pct in [0, 50, 100] {
            assert!(validate_volume(pct).is_ok());
        }
        assert!(validate_volume(101).is_err());
    }
    #[tokio::test]
    async fn panel_commands_forward_without_player_lock() {
        let (driver, mut rx) = mpsc::channel(8);
        let (_, state) = watch::channel(WidgetState {
            visible: false,
            now: None,
            queue: vec![],
            progress_ms: 0,
            mode: "Idle".into(),
            warning: None,
        });
        let app = AppState { driver, state };
        send(&app, DriverMsg::Cmd(crate::player::Input::Move(42, 2)))
            .await
            .unwrap();
        assert!(matches!(
            rx.recv().await,
            Some(DriverMsg::Cmd(crate::player::Input::Move(42, 2)))
        ));
        drop(rx);
        assert!(send(&app, DriverMsg::WidgetTest).await.is_err());
    }
}

async fn send(state: &AppState, msg: DriverMsg) -> Result<(), String> {
    state
        .driver
        .send(msg)
        .await
        .map_err(|_| "driver_closed".into())
}
#[tauri::command]
pub fn get_state(state: tauri::State<'_, AppState>) -> WidgetState {
    state.state.borrow().clone()
}
#[tauri::command]
pub async fn skip(state: tauri::State<'_, AppState>) -> Result<(), String> {
    send(&state, DriverMsg::Cmd(crate::player::Input::Skip)).await
}
#[tauri::command]
pub async fn remove_item(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    send(&state, DriverMsg::Cmd(crate::player::Input::Remove(id))).await
}
#[tauri::command]
pub async fn move_item(
    state: tauri::State<'_, AppState>,
    id: i64,
    to: usize,
) -> Result<(), String> {
    send(&state, DriverMsg::Cmd(crate::player::Input::Move(id, to))).await
}
#[tauri::command]
pub async fn toggle_pause(state: tauri::State<'_, AppState>) -> Result<(), String> {
    send(&state, DriverMsg::Cmd(crate::player::Input::TogglePause)).await
}
#[tauri::command]
pub async fn resume_queue(state: tauri::State<'_, AppState>) -> Result<(), String> {
    send(&state, DriverMsg::Cmd(crate::player::Input::ResumeQueue)).await
}
#[tauri::command]
pub async fn set_volume(state: tauri::State<'_, AppState>, pct: u8) -> Result<(), String> {
    validate_volume(pct)?;
    send(&state, DriverMsg::SetVolume(pct)).await
}
#[tauri::command]
pub fn get_settings(app: tauri::AppHandle) -> Result<Settings, String> {
    let db = app
        .try_state::<Arc<Mutex<crate::db::Db>>>()
        .ok_or("starting")?;
    db.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_setting("settings")
        .map(|s| s.unwrap_or_default())
        .map_err(|_| "settings_read_failed".into())
}
#[tauri::command]
pub async fn save_settings(state: tauri::State<'_, AppState>, s: Settings) -> Result<(), String> {
    validate_settings(&s)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    send(&state, DriverMsg::SaveSettings(s, tx)).await?;
    rx.await.map_err(|_| "driver_closed".to_string())?
}
#[derive(Default)]
pub struct DeviceFlow(
    tokio::sync::Mutex<
        Option<(
            crate::twitch::auth::DeviceStart,
            std::time::Instant,
            std::time::Instant,
        )>,
    >,
);
const SCOPES: &str = "channel:read:redemptions channel:manage:redemptions user:write:chat";
fn twitch_id() -> Result<&'static str, String> {
    option_env!("TWITCH_CLIENT_ID")
        .filter(|s| !s.is_empty())
        .ok_or("twitch_client_missing".into())
}
#[tauri::command]
pub async fn twitch_login_start(
    flow: tauri::State<'_, DeviceFlow>,
) -> Result<crate::twitch::auth::DeviceStart, String> {
    let start = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        crate::twitch::auth::device_start(twitch_id()?, SCOPES, "https://id.twitch.tv"),
    )
    .await
    .map_err(|_| "twitch_timeout")?
    .map_err(|_| "twitch_login_start_failed")?;
    let response = crate::twitch::auth::DeviceStart {
        device_code: String::new(),
        user_code: start.user_code.clone(),
        verification_uri: start.verification_uri.clone(),
        interval: start.interval,
        expires_in: start.expires_in,
    };
    let now = std::time::Instant::now();
    *flow.0.lock().await = Some((start, now, now));
    Ok(response)
}
#[tauri::command]
pub async fn twitch_login_poll(
    flow: tauri::State<'_, DeviceFlow>,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    let mut pending = flow.0.lock().await;
    let (start, created, last) = pending.as_mut().ok_or("twitch_login_required")?;
    if created.elapsed().as_secs() >= start.expires_in {
        *pending = None;
        return Err("twitch_expired".into());
    }
    if last.elapsed().as_secs() < start.interval {
        return Ok(false);
    }
    *last = std::time::Instant::now();
    let tokens = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        crate::twitch::auth::device_poll(
            twitch_id()?,
            &start.device_code,
            SCOPES,
            "https://id.twitch.tv",
        ),
    )
    .await
    .map_err(|_| "twitch_timeout")?
    .map_err(|_| "twitch_login_failed")?;
    if let Some(tokens) = tokens {
        persist_login("twitch", tokens.clone()).await?;
        send(&state, DriverMsg::TwitchLogin(tokens)).await?;
        *pending = None;
        Ok(true)
    } else {
        Ok(false)
    }
}
async fn persist_login(key: &'static str, t: Tokens) -> Result<(), String> {
    crate::secrets::write(
        key,
        Some(serde_json::to_string(&t).map_err(|_| "keyring_unavailable")?),
    )
    .await
    .map_err(|_| "keyring_unavailable")?
    .map_err(|_| "keyring_unavailable".into())
}
#[tauri::command]
pub async fn twitch_logout(
    flow: tauri::State<'_, DeviceFlow>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    *flow.0.lock().await = None;
    send(&state, DriverMsg::TwitchLogout).await
}
#[tauri::command]
pub async fn spotify_login(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    client_id: String,
) -> Result<String, String> {
    if client_id.trim().is_empty() {
        return Err("client_id_required".into());
    }
    let mut s = get_settings(app.clone())?;
    s.spotify_client_id = Some(client_id.clone());
    let pkce = crate::spotify::new_pkce();
    let url = crate::spotify::authorize_url(
        &client_id,
        &format!(
            "http://127.0.0.1:{}/callback/spotify",
            running_port(app.clone()).ok_or("server_start_failed")?
        ),
        &pkce,
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    send(&state, DriverMsg::SaveSettings(s, tx)).await?;
    rx.await.map_err(|_| "driver_closed")??;
    send(&state, DriverMsg::SpotifyLoginStart(pkce)).await?;
    Ok(url)
}
#[derive(serde::Serialize)]
pub struct SpotifyStatus {
    pub connected: bool,
    pub premium: Option<bool>,
}
#[tauri::command]
pub async fn spotify_status(state: tauri::State<'_, AppState>) -> Result<SpotifyStatus, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    send(&state, DriverMsg::SpotifyStatus(tx)).await?;
    rx.await.map_err(|_| "driver_closed".into())
}
#[tauri::command]
pub async fn obs_check(app: tauri::AppHandle) -> Result<String, String> {
    crate::obs::check(&get_settings(app)?.obs)
        .await
        .map_err(|_| "obs_connect_failed".into())
}
fn themes_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|p| p.join("themes"))
        .map_err(|_| "themes_unavailable".into())
}
#[tauri::command]
pub fn list_themes(app: tauri::AppHandle) -> Result<Vec<crate::server::ThemeMeta>, String> {
    Ok(crate::server::list_themes(&themes_dir(&app)?))
}
#[tauri::command]
pub async fn import_theme(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let dir = themes_dir(&app)?;
    tokio::task::spawn_blocking(move || {
        crate::server::import_theme_zip(std::path::Path::new(&path), &dir)
    })
    .await
    .map_err(|_| "theme_import_failed")?
    .map_err(|_| "theme_import_failed".into())
}
#[tauri::command]
pub async fn obs_add_widget(app: tauri::AppHandle, scale: Option<f64>) -> Result<(), String> {
    let s = get_settings(app.clone())?;
    let port = running_port(app.clone()).ok_or("server_start_failed")?;
    let theme = list_themes(app)?
        .into_iter()
        .find(|t| t.id == s.active_theme)
        .ok_or("theme_missing")?;
    crate::obs::add_widget_to_current_scene(
        &s.obs,
        &format!("http://127.0.0.1:{}/widget/music", port),
        &theme.parts.expect("list_themes supplies fallback parts"),
        scale
            .filter(|s| s.is_finite())
            .unwrap_or(1.0)
            .clamp(0.5, 4.0),
    )
    .await
    .map_err(|_| "obs_add_failed".into())
}
#[tauri::command]
pub async fn widget_test(state: tauri::State<'_, AppState>) -> Result<(), String> {
    send(&state, DriverMsg::WidgetTest).await
}

fn validate_volume(pct: u8) -> Result<(), String> {
    if pct > 100 {
        Err("volume_invalid".into())
    } else {
        Ok(())
    }
}
fn validate_settings(s: &Settings) -> Result<(), String> {
    if s.port == 0 || s.obs.port == 0 || s.reward_cost == 0 || s.reward_title.trim().is_empty() {
        Err("settings_invalid".into())
    } else {
        Ok(())
    }
}

pub struct RunningPort(pub Option<u16>);
#[tauri::command]
pub fn running_port(app: tauri::AppHandle) -> Option<u16> {
    app.try_state::<RunningPort>().and_then(|p| p.0)
}
#[tauri::command]
pub async fn twitch_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    send(&state, DriverMsg::TwitchStatus(tx)).await?;
    rx.await.map_err(|_| "driver_closed".into())
}

#[tauri::command]
pub async fn spotify_volume(state: tauri::State<'_, AppState>) -> Result<Option<u8>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    send(&state, DriverMsg::SpotifyVolume(tx)).await?;
    rx.await.map_err(|_| "driver_closed".into())
}
