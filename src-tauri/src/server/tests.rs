use super::*;
use crate::player::{Input, WidgetState};
use futures_util::{SinkExt, StreamExt};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use tokio::time::{Duration, timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
#[test]
fn builtin_themes_listed() {
    let ids: Vec<_> = list_themes(&PathBuf::from("/nonexistent"))
        .into_iter()
        .map(|theme| theme.id)
        .collect();
    assert_eq!(ids, ["block", "crt", "glass", "underground"]);
}

fn state() -> WidgetState {
    WidgetState {
        visible: false,
        now: None,
        queue: vec![],
        progress_ms: 0,
        mode: "Idle".into(),
        warning: None,
    }
}
async fn setup() -> (u16, ServerChannels) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_, rx) = tokio::sync::watch::channel(state());
    let (app, ch) = build(
        listener.local_addr().unwrap().port(),
        PathBuf::from("/nonexistent"),
        Arc::new(RwLock::new("underground".into())),
        rx,
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, ch)
}
async fn ws(
    port: u16,
    role: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut r = format!("ws://127.0.0.1:{port}/ws?role={role}")
        .into_client_request()
        .unwrap();
    r.headers_mut().insert(
        "origin",
        format!("http://127.0.0.1:{port}").parse().unwrap(),
    );
    connect_async(r).await.unwrap().0
}
#[tokio::test]
async fn widget_serves_active_theme() {
    let (p, _) = setup().await;
    let r = reqwest::get(format!("http://127.0.0.1:{p}/widget/music"))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("MUSIC"));
}
#[tokio::test]
async fn sdk_served() {
    let (p, _) = setup().await;
    assert!(
        reqwest::get(format!("http://127.0.0.1:{p}/widget/sdk.js"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
            .contains("StreamAssist")
    );
}
#[tokio::test]
async fn ws_rejects_foreign_origin() {
    let (p, _) = setup().await;
    let mut r = format!("ws://127.0.0.1:{p}/ws")
        .into_client_request()
        .unwrap();
    r.headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    let e = connect_async(r).await.unwrap_err();
    assert!(matches!(e,tokio_tungstenite::tungstenite::Error::Http(r) if r.status()==403));
    // A valid Origin cannot bypass the Host check; missing Origin is also denied.
    for (host, origin) in [
        (
            "evil.example".to_string(),
            Some(format!("http://127.0.0.1:{p}")),
        ),
        (format!("127.0.0.1:{p}"), None),
    ] {
        let mut r = format!("ws://127.0.0.1:{p}/ws")
            .into_client_request()
            .unwrap();
        r.headers_mut().insert("host", host.parse().unwrap());
        if let Some(origin) = origin {
            r.headers_mut().insert("origin", origin.parse().unwrap());
        }
        let e = connect_async(r).await.unwrap_err();
        assert!(matches!(e,tokio_tungstenite::tungstenite::Error::Http(r) if r.status()==403));
    }
}
#[tokio::test]
async fn ws_sends_state_on_connect() {
    let (p, _) = setup().await;
    let mut s = ws(p, "view").await;
    let m = s.next().await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(m.to_text().unwrap()).unwrap()["State"]["mode"],
        "Idle"
    );
}
#[tokio::test]
async fn yt_commands_only_to_player() {
    let (p, c) = setup().await;
    let mut v = ws(p, "view").await;
    let mut s = ws(p, "player").await;
    v.next().await;
    s.next().await;
    c.out_tx
        .send(WsOut::YtPlay {
            item_id: 1,
            video_id: "x".into(),
            start_ms: 0,
        })
        .unwrap();
    assert!(
        s.next()
            .await
            .unwrap()
            .unwrap()
            .to_text()
            .unwrap()
            .contains("YtPlay")
    );
    assert!(timeout(Duration::from_millis(80), v.next()).await.is_err());
}
#[tokio::test]
async fn new_player_evicts_old_with_4001_and_signals_connected() {
    let (p, mut c) = setup().await;
    let mut a = ws(p, "player").await;
    a.next().await;
    assert!(matches!(
        c.yt_rx.recv().await,
        Some(Input::YtPlayerConnected)
    ));
    let mut b = ws(p, "player").await;
    b.next().await;
    assert!(
        matches!(a.next().await.unwrap().unwrap(),Message::Close(Some(f)) if u16::from(f.code)==4001)
    );
    assert!(matches!(
        c.yt_rx.recv().await,
        Some(Input::YtPlayerConnected)
    ));
    b.send(Message::Text(
        r#"{"type":"yt","item_id":7,"event":"ended"}"#.into(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        c.yt_rx.recv().await,
        Some(Input::Yt { item_id: 7, .. })
    ));
}
#[tokio::test]
async fn callback_forwards_code_and_state() {
    let (p, mut c) = setup().await;
    let r = reqwest::get(format!(
        "http://127.0.0.1:{p}/callback/spotify?code=abc&state=xyz"
    ))
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        c.oauth_rx.recv().await.unwrap(),
        (Ok("abc".into()), "xyz".into())
    );
}
#[tokio::test]
async fn callback_forwards_error() {
    let (p, mut c) = setup().await;
    let r = reqwest::get(format!(
        "http://127.0.0.1:{p}/callback/spotify?error=access_denied&state=xyz"
    ))
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.text()
            .await
            .unwrap()
            .contains("Вход отменён, вернитесь в программу")
    );
    assert_eq!(
        c.oauth_rx.recv().await.unwrap(),
        (Err("access_denied".into()), "xyz".into())
    );
}
#[tokio::test]
async fn callback_full_channel_returns_503() {
    let (p, c) = setup().await;
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{p}/callback/spotify?code=abc&state=xyz");
    for _ in 0..c.oauth_rx.max_capacity() {
        assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    }
    let r = timeout(Duration::from_secs(1), client.get(&url).send())
        .await
        .expect("full OAuth channel must respond promptly")
        .unwrap();
    assert_eq!(r.status(), 503);
    assert!(
        r.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    assert!(!r.text().await.unwrap().is_empty());
}
#[tokio::test]
async fn callback_closed_channel_returns_503() {
    let (p, c) = setup().await;
    drop(c.oauth_rx);
    let r = timeout(
        Duration::from_secs(1),
        reqwest::get(format!(
            "http://127.0.0.1:{p}/callback/spotify?code=abc&state=xyz"
        )),
    )
    .await
    .expect("closed OAuth channel must respond promptly")
    .unwrap();
    assert_eq!(r.status(), 503);
    assert!(!r.text().await.unwrap().is_empty());
}
#[tokio::test]
async fn theme_route_traversal_404() {
    let (p, _) = setup().await;
    assert_eq!(
        reqwest::get(format!(
            "http://127.0.0.1:{p}/widget/theme/underground/..%2F..%2Fsecret"
        ))
        .await
        .unwrap()
        .status(),
        404
    );
}
fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!("sa-theme-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn zip_file(dir: &std::path::Path, name: &str, data: &str) -> PathBuf {
    let p = dir.join("test.zip");
    let mut z = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
    z.start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    z.write_all(data.as_bytes()).unwrap();
    z.finish().unwrap();
    p
}
#[test]
fn zip_slip_rejected() {
    let p = temp();
    assert!(import_theme_zip(&zip_file(&p, "../secret", "bad"), &p).is_err());
    std::fs::remove_dir_all(p).unwrap();
}
#[test]
fn zip_without_theme_json_rejected() {
    let p = temp();
    assert!(import_theme_zip(&zip_file(&p, "index.html", "hi"), &p).is_err());
    std::fs::remove_dir_all(p).unwrap();
}
#[test]
fn custom_theme_overrides_builtin() {
    let p = temp();
    std::fs::create_dir(p.join("underground")).unwrap();
    std::fs::write(
        p.join("underground/theme.json"),
        r#"{"id":"underground","name":"Custom","author":"Me","width":800,"height":400}"#,
    )
    .unwrap();
    std::fs::write(p.join("underground/index.html"), "custom").unwrap();
    assert_eq!(
        list_themes(&p)
            .iter()
            .find(|t| t.id == "underground")
            .unwrap()
            .name,
        "Custom"
    );
    assert_eq!(
        themes::asset(&p, "underground", "index.html").unwrap(),
        b"custom"
    );
    std::fs::remove_dir_all(p).unwrap();
}

#[tokio::test]
async fn widget_survives_poisoned_theme_lock() {
    let active = Arc::new(RwLock::new("underground".into()));
    let poisoned = active.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poisoned.write().unwrap();
        panic!("simulate interrupted theme update");
    })
    .join();
    let (_, last) = watch::channel(state());
    let (out, _) = broadcast::channel(1);
    let (yt, _) = mpsc::channel(1);
    let (oauth, _) = mpsc::channel(1);
    let response = music(State(App {
        port: 0,
        themes: PathBuf::new(),
        active,
        last,
        out,
        yt,
        oauth,
        player: Arc::default(),
    }))
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}
