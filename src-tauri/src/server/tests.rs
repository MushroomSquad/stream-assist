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
    assert_eq!(
        ids,
        [
            "block",
            "crt",
            "glass",
            "moonlight",
            "phantom",
            "underground"
        ]
    );
}

#[test]
fn builtin_theme_parts_and_legacy_fallback() {
    let themes = list_themes(&PathBuf::from("/nonexistent"));
    assert_eq!(themes.len(), 6);
    for theme in themes {
        let parts = theme.parts.expect("built-in parts");
        assert!(parts.now.width > 0 && parts.now.height > 0);
        assert!(parts.queue.width > 0 && parts.queue.height > 0);
    }
    let dir = std::env::temp_dir().join(format!("sa-parts-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir(dir.as_path().join("legacy")).unwrap();
    std::fs::write(
        dir.as_path().join("legacy/theme.json"),
        r#"{"id":"legacy","name":"Legacy","author":"Test","width":640,"height":360}"#,
    )
    .unwrap();
    let theme = list_themes(dir.as_path())
        .into_iter()
        .find(|t| t.id == "legacy")
        .unwrap();
    let parts = theme.parts.unwrap();
    assert_eq!((parts.now.width, parts.now.height), (640, 360));
    assert_eq!((parts.queue.width, parts.queue.height), (640, 360));
    std::fs::remove_dir_all(dir).unwrap();
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
        PathBuf::new(),
        noop_debug(),
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
    let client = crate::http::http_client();
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
    let (tts, _) = mpsc::channel(1);
    let response = music(State(App {
        debug: noop_debug(),
        port: 0,
        themes: PathBuf::new(),
        active,
        last,
        out,
        yt,
        oauth,
        player: Arc::default(),
        tts_dir: PathBuf::new(),
        tts,
        tts_player: Arc::default(),
    }))
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn tts_wav_served_and_name_checked() {
    let dir = temp();
    for name in [
        "1.wav",
        "12345678901234567890.wav",
        "a.wav",
        "123456789012345678901.wav",
    ] {
        std::fs::write(dir.join(name), b"wav").unwrap();
    }
    let (_, rx) = watch::channel(state());
    let mut c = serve(
        0,
        PathBuf::new(),
        Arc::new(RwLock::new("underground".into())),
        rx,
        dir.clone(),
        noop_debug(),
    )
    .await
    .unwrap();
    let base = format!("http://127.0.0.1:{}", c.running_port);
    for name in ["1.wav", "12345678901234567890.wav"] {
        let r = reqwest::get(format!("{base}/tts/{name}")).await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()[header::CONTENT_TYPE], "audio/wav");
        assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(r.bytes().await.unwrap().as_ref(), b"wav");
    }
    for name in [
        "a.wav",
        "..%2Fx.wav",
        "123456789012345678901.wav",
        ".wav",
        "2.wav",
        "1.WAV",
    ] {
        assert_eq!(
            reqwest::get(format!("{base}/tts/{name}"))
                .await
                .unwrap()
                .status(),
            404
        );
    }
    let r = reqwest::get(format!("{base}/widget/tts")).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("role=tts"));
    c.task.take().unwrap().abort();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn tts_role_receives_only_tts() {
    let (p, mut c) = setup().await;
    let mut t = ws(p, "tts").await;
    let mut events = c.tts_rx.take().unwrap();
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Connected)
    ));
    let mut player = ws(p, "player").await;
    player.next().await.unwrap().unwrap();
    let mut view = ws(p, "view").await;
    view.next().await.unwrap().unwrap();
    assert!(timeout(Duration::from_millis(80), t.next()).await.is_err());
    c.out_tx.send(WsOut::State(state())).unwrap();
    c.out_tx
        .send(WsOut::YtPlay {
            item_id: 1,
            video_id: "x".into(),
            start_ms: 0,
        })
        .unwrap();
    c.out_tx.send(WsOut::TtsPlay { id: 7 }).unwrap();
    c.out_tx.send(WsOut::TtsStop).unwrap();
    c.out_tx.send(WsOut::Reload).unwrap();
    for expected in [r#"{"TtsPlay":{"id":7}}"#, r#""TtsStop""#, r#""Reload""#] {
        assert_eq!(
            t.next().await.unwrap().unwrap().to_text().unwrap(),
            expected
        );
    }
    for (socket, count) in [(&mut player, 3), (&mut view, 2)] {
        for _ in 0..count {
            let m = socket.next().await.unwrap().unwrap();
            assert!(!m.to_text().unwrap().contains("Tts"));
        }
        assert!(
            timeout(Duration::from_millis(80), socket.next())
                .await
                .is_err()
        );
    }
    assert!(timeout(Duration::from_millis(80), t.next()).await.is_err());
}

#[tokio::test]
async fn tts_events_forwarded() {
    let (p, mut c) = setup().await;
    let mut events = c.tts_rx.take().unwrap();
    let mut t = ws(p, "tts").await;
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Connected)
    ));
    for text in [
        r#"{"type":"tts","id":7,"event":"ended"}"#,
        r#"{"type":"tts","id":8,"event":"error"}"#,
    ] {
        t.send(Message::Text(text.into())).await.unwrap();
    }
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Ended(7))
    ));
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Error(8))
    ));
    let mut v = ws(p, "view").await;
    v.next().await.unwrap().unwrap();
    v.send(Message::Text(
        r#"{"type":"tts","id":9,"event":"ended"}"#.into(),
    ))
    .await
    .unwrap();
    t.send(Message::Text(
        r#"{"type":"yt","id":9,"event":"ended"}"#.into(),
    ))
    .await
    .unwrap();
    t.send(Message::Text(
        r#"{"type":"tts","id":9,"event":"started"}"#.into(),
    ))
    .await
    .unwrap();
    assert!(
        timeout(Duration::from_millis(80), events.recv())
            .await
            .is_err()
    );
    t.close(None).await.unwrap();
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Disconnected)
    ));
}

#[tokio::test]
async fn second_tts_widget_evicts_first() {
    let (p, mut c) = setup().await;
    let mut events = c.tts_rx.take().unwrap();
    let mut a = ws(p, "tts").await;
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Connected)
    ));
    let mut player = ws(p, "player").await;
    player.next().await.unwrap().unwrap();
    let mut b = ws(p, "tts").await;
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Connected)
    ));
    let _ = a
        .send(Message::Text(
            r#"{"type":"tts","id":1,"event":"ended"}"#.into(),
        ))
        .await;
    assert!(
        matches!(a.next().await.unwrap().unwrap(), Message::Close(Some(f)) if u16::from(f.code) == 4001)
    );
    assert!(
        timeout(Duration::from_millis(80), events.recv())
            .await
            .is_err()
    );
    b.send(Message::Text(
        r#"{"type":"tts","id":2,"event":"ended"}"#.into(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Ended(2))
    ));
    b.close(None).await.unwrap();
    assert!(matches!(
        events.recv().await,
        Some(TtsWidgetEvent::Disconnected)
    ));
    c.out_tx.send(WsOut::YtStop).unwrap();
    assert_eq!(
        player.next().await.unwrap().unwrap().to_text().unwrap(),
        r#""YtStop""#
    );
}

async fn debug_server() -> ServerChannels {
    let (_, rx) = watch::channel(state());
    serve(
        0,
        PathBuf::new(),
        Arc::new(RwLock::new("underground".into())),
        rx,
        PathBuf::new(),
        Arc::new(|| Box::pin(async { serde_json::json!({"ok": 1}) })),
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn debug_returns_snapshot_json() {
    let c = debug_server().await;
    let r = reqwest::get(format!("http://127.0.0.1:{}/debug", c.running_port))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({"ok": 1})
    );
    c.task.unwrap().abort();
}
#[tokio::test]
async fn debug_rejects_foreign_host() {
    let c = debug_server().await;
    let r = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/debug", c.running_port))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    c.task.unwrap().abort();
}
#[tokio::test]
async fn debug_rejects_cross_site_fetch() {
    let c = debug_server().await;
    for (site, status) in [
        ("same-origin", 403),
        ("same-site", 403),
        ("cross-site", 403),
        ("none", 200),
    ] {
        let r = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{}/debug", c.running_port))
            .header("Sec-Fetch-Site", site)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), status);
    }
    c.task.unwrap().abort();
}
