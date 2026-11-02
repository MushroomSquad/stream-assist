use super::*;
use crate::player::{Input, YtEvent};
use axum::{
    extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    http::HeaderMap,
};
#[derive(Deserialize)]
pub(super) struct Role {
    role: Option<String>,
}
pub(super) async fn upgrade(
    State(a): State<App>,
    Query(q): Query<Role>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let allowed = [
        format!("127.0.0.1:{}", a.port),
        format!("localhost:{}", a.port),
    ];
    let host = headers.get("host").and_then(|v| v.to_str().ok());
    let origin = headers.get("origin").and_then(|v| v.to_str().ok());
    if !allowed.iter().any(|s| Some(s.as_str()) == host)
        || !allowed
            .iter()
            .any(|s| Some(format!("http://{s}").as_str()) == origin)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| session(socket, a, q.role.as_deref() == Some("player")))
}
#[derive(Deserialize)]
struct YtMessage {
    r#type: String,
    item_id: i64,
    event: String,
    duration_ms: Option<u64>,
}
async fn session(mut socket: WebSocket, a: App, is_player: bool) {
    let mut out = a.out.subscribe();
    // Serializing acquisition and the connected signal keeps replay commands ordered.
    let mut evicted = if is_player {
        let mut guard = a.player.lock().await;
        let (tx, rx) = watch::channel(false);
        if let Some(old) = guard.replace(tx) {
            let _ = old.send(true);
        }
        if a.yt.send(Input::YtPlayerConnected).await.is_err() {
            return;
        }
        Some(rx)
    } else {
        None
    };
    let initial = WsOut::State(a.last.borrow().clone());
    if socket
        .send(Message::Text(
            serde_json::to_string(&initial).unwrap().into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            biased;
            _ = async {
                match evicted.as_mut() {
                    Some(rx) => { let _ = rx.changed().await; }
                    None => std::future::pending().await,
                }
            } => {
                let _ = socket.send(Message::Close(Some(CloseFrame {
                    code: 4001,
                    reason: "Another player connected".into(),
                }))).await;
                break;
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) if is_player => {
                        // Once replaced, the old connection cannot submit playback events.
                        if evicted.as_ref().is_some_and(|rx| *rx.borrow()) { continue; }
                        if let Some(input) = parse_event(&text) {
                            if a.yt.send(input).await.is_err() { break; }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            message = out.recv() => match message {
                Ok(m) => {
                    if is_player || matches!(m, WsOut::State(_) | WsOut::Reload) {
                        let text = serde_json::to_string(&m).unwrap();
                        if socket.send(Message::Text(text.into())).await.is_err() { break; }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }
}
fn parse_event(text: &str) -> Option<Input> {
    let m: YtMessage = serde_json::from_str(text).ok()?;
    if m.r#type != "yt" {
        return None;
    }
    let ev = match m.event.as_str() {
        "started" => YtEvent::Started {
            duration_ms: m.duration_ms?,
        },
        "ended" => YtEvent::Ended,
        "error" => YtEvent::Error,
        _ => return None,
    };
    Some(Input::Yt {
        item_id: m.item_id,
        ev,
    })
}
