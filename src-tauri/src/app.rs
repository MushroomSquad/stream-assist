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
pub fn save_tokens(key: &str, t: &Tokens) -> anyhow::Result<()> {
    crate::secrets::store(key, &serde_json::to_string(t)?)
}
fn rotate(key: &str, t: &Tokens) {
    if save_tokens(key, t).is_err() {
        eprintln!("Хранилище ключей недоступно");
    }
}
pub fn spotify_client(id: String, t: Tokens) -> Arc<Spotify> {
    Arc::new(Spotify::new(
        id,
        t,
        "https://api.spotify.com",
        "https://accounts.spotify.com",
        |t| rotate("spotify", t),
    ))
}
pub fn twitch_client(t: Tokens) -> Arc<Helix> {
    Arc::new(Helix::new(
        option_env!("TWITCH_CLIENT_ID").unwrap_or_default().into(),
        t,
        "https://api.twitch.tv",
        "https://id.twitch.tv",
        |t| rotate("twitch", t),
    ))
}
fn load(key: &str) -> anyhow::Result<Option<Tokens>> {
    crate::secrets::load(key)?
        .map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
pub fn setup(app: &mut tauri::App) -> anyhow::Result<()> {
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
            s.warning = Some("Не удалось открыть базу или запустить сервер виджета".into());
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
        .unwrap()
        .get_setting::<Settings>("settings")?
        .unwrap_or_default();
    let mut warning = None;
    let spotify = match load("spotify") {
        Ok(Some(t)) => settings
            .spotify_client_id
            .clone()
            .map(|id| spotify_client(id, t)),
        Ok(None) => None,
        Err(_) => {
            warning = Some("Хранилище ключей недоступно: проверьте Secret Service".into());
            None
        }
    };
    let helix = match load("twitch") {
        Ok(Some(t)) => Some(twitch_client(t)),
        Ok(None) => None,
        Err(_) => {
            warning = Some("Хранилище ключей недоступно: проверьте Secret Service".into());
            None
        }
    };
    if warning.is_some() {
        let mut s = state_tx.borrow().clone();
        s.warning = warning;
        state_tx.send_replace(s.clone());
        let _ = tauri::Emitter::emit(&handle, "state", s);
    }
    let ws = crate::server::serve(
        settings.port,
        dir.join("themes"),
        Arc::new(RwLock::new(settings.active_theme.clone())),
        state,
    )
    .await?;
    crate::driver::run(
        DriverCtx {
            db,
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
            if let (Some(t), Some(id)) = (load("twitch")?, settings.music_reward_id) {
                twitch_client(t)
                    .update_reward(&id, &serde_json::json!({"is_paused":true}))
                    .await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), operation).await;
        app.exit(0);
    });
}
