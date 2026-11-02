pub mod themes;
mod ws;
use crate::player::{self, WidgetState};
use anyhow::Result;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};
pub use themes::{ThemeMeta, import_theme_zip, list_themes};
use tokio::sync::{broadcast, mpsc, watch};
#[derive(Clone, Debug, Serialize)]
pub enum WsOut {
    State(WidgetState),
    YtPlay {
        item_id: i64,
        video_id: String,
        start_ms: u64,
    },
    YtUnmute,
    YtStop,
    YtPause,
    YtResume,
    Reload,
}
pub struct ServerChannels {
    pub running_port: u16,
    pub task: Option<tokio::task::JoinHandle<()>>,
    pub out_tx: broadcast::Sender<WsOut>,
    pub yt_rx: mpsc::Receiver<player::Input>,
    pub oauth_rx: mpsc::Receiver<(Result<String, String>, String)>,
}
#[derive(Clone)]
struct App {
    port: u16,
    themes: PathBuf,
    active: Arc<RwLock<String>>,
    last: watch::Receiver<WidgetState>,
    out: broadcast::Sender<WsOut>,
    yt: mpsc::Sender<player::Input>,
    oauth: mpsc::Sender<(Result<String, String>, String)>,
    player: Arc<tokio::sync::Mutex<Option<watch::Sender<bool>>>>,
}
fn build(
    port: u16,
    themes: PathBuf,
    active: Arc<RwLock<String>>,
    last: watch::Receiver<WidgetState>,
) -> (Router, ServerChannels) {
    let (out, out_rx) = broadcast::channel(64);
    drop(out_rx);
    let (yt, yt_rx) = mpsc::channel(64);
    let (oauth, oauth_rx) = mpsc::channel(16);
    let app = App {
        port,
        themes,
        active,
        last,
        out: out.clone(),
        yt,
        oauth,
        player: Arc::default(),
    };
    (
        Router::new()
            .route("/widget/music", get(music))
            .route(
                "/widget/sdk.js",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                        include_str!("../../assets/widget/sdk.js"),
                    )
                }),
            )
            .route("/widget/theme/{id}/{*file}", get(asset))
            .route("/ws", get(ws::upgrade))
            .route("/callback/spotify", get(callback))
            .with_state(app),
        ServerChannels {
            running_port: port,
            task: None,
            out_tx: out,
            yt_rx,
            oauth_rx,
        },
    )
}
/// Keep the driver and settings usable even when the widget listener cannot bind.
pub async fn start_or_disabled(
    port: u16,
    themes: PathBuf,
    active: Arc<RwLock<String>>,
    last: watch::Receiver<WidgetState>,
) -> (ServerChannels, Option<&'static str>) {
    match serve(port, themes.clone(), active.clone(), last.clone()).await {
        Ok(channels) => (channels, None),
        Err(error) => {
            let code = if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse)
            {
                "server_port_busy"
            } else {
                "server_start_failed"
            };
            let (_, channels) = build(port, themes, active, last);
            (channels, Some(code))
        }
    }
}
pub async fn serve(
    port: u16,
    themes_dir: PathBuf,
    active_theme: Arc<RwLock<String>>,
    last_state: watch::Receiver<WidgetState>,
) -> Result<ServerChannels> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let (app, mut ch) = build(
        listener.local_addr()?.port(),
        themes_dir,
        active_theme,
        last_state,
    );
    ch.task = Some(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("Widget server stopped: {e}");
        }
    }));
    Ok(ch)
}
async fn music(State(a): State<App>) -> Response {
    let id = a.active.read().unwrap_or_else(|e| e.into_inner()).clone();
    file(&a, &id, "index.html")
}
async fn asset(State(a): State<App>, Path((id, path)): Path<(String, String)>) -> Response {
    file(&a, &id, &path)
}
fn file(a: &App, id: &str, path: &str) -> Response {
    match themes::asset(&a.themes, id, path) {
        Some(bytes) => {
            let mime = match path.rsplit('.').next().unwrap_or("") {
                "html" => "text/html; charset=utf-8",
                "css" => "text/css; charset=utf-8",
                "js" => "text/javascript; charset=utf-8",
                "json" => "application/json",
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "svg" => "image/svg+xml",
                "webp" => "image/webp",
                "woff2" => "font/woff2",
                "ttf" => "font/ttf",
                _ => "application/octet-stream",
            };
            ([(header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    error: Option<String>,
    state: String,
}
async fn callback(State(a): State<App>, Query(q): Query<Callback>) -> Response {
    let (result, message) = match (q.code, q.error) {
        (_, Some(error)) => (Err(error), "Вход отменён, вернитесь в программу"),
        (Some(code), None) => (Ok(code), "Готово, вернитесь в программу"),
        (None, None) => return (StatusCode::BAD_REQUEST, "Missing code or error").into_response(),
    };
    if a.oauth.try_send((result, q.state)).is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "OAuth callback unavailable",
        )
            .into_response();
    }
    axum::response::Html(format!(
        "<!doctype html><meta charset='utf-8'><p>{message}</p>"
    ))
    .into_response()
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[tokio::test]
    async fn startup_busy_port_returns_warning_and_disabled_server() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = WidgetState {
            visible: false,
            now: None,
            queue: vec![],
            progress_ms: 0,
            mode: "Idle".into(),
            warning: None,
        };
        let (_tx, rx) = watch::channel(state);
        let (server, warning) = start_or_disabled(
            port,
            PathBuf::new(),
            Arc::new(RwLock::new("underground".into())),
            rx,
        )
        .await;
        assert_eq!(warning, Some("server_port_busy"));
        assert!(server.task.is_none());
        assert_eq!(server.running_port, port);
        assert_eq!(listener.local_addr().unwrap().port(), port);
    }
}
