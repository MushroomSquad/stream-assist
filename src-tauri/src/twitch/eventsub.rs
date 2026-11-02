use super::helix::{ApiRedemption, Helix};
use crate::model::Redemption;
use anyhow::{Result, anyhow, bail};
use futures_util::StreamExt;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::mpsc::Sender,
    time::{Instant, timeout},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Debug)]
pub enum EsMsg {
    Welcome { session_id: String },
    Keepalive,
    Notification(Redemption),
    Reconnect { url: String },
    Revocation,
}
pub fn parse_message(text: &str) -> Result<EsMsg> {
    let v: Value = serde_json::from_str(text)?;
    let session = &v["payload"]["session"];
    let field = |key: &str| {
        session[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Missing EventSub session field"))
    };
    Ok(match v["metadata"]["message_type"].as_str() {
        Some("session_welcome") => EsMsg::Welcome {
            session_id: field("id")?,
        },
        Some("session_keepalive") => EsMsg::Keepalive,
        Some("session_reconnect") => EsMsg::Reconnect {
            url: field("reconnect_url")?,
        },
        Some("revocation") => EsMsg::Revocation,
        Some("notification") => {
            let kind = v["metadata"]["subscription_type"]
                .as_str()
                .or_else(|| v["payload"]["subscription"]["type"].as_str());
            if kind != Some("channel.channel_points_custom_reward_redemption.add") {
                bail!("Unsupported EventSub notification");
            }
            EsMsg::Notification(
                serde_json::from_value::<ApiRedemption>(v["payload"]["event"].clone())?.into(),
            )
        }
        _ => bail!("Unsupported EventSub message"),
    })
}
#[derive(Debug)]
pub enum EsSignal {
    Redemption(Redemption),
    Resync,
}
fn keepalive(text: &str) -> Duration {
    let v: Value = serde_json::from_str(text).unwrap_or_default();
    Duration::from_secs(
        v["payload"]["session"]["keepalive_timeout_seconds"]
            .as_u64()
            .unwrap_or(10)
            .saturating_add(5),
    )
}
pub async fn run(helix: Arc<Helix>, ws_url: String, out: Sender<EsSignal>) {
    let mut backoff = 1;
    let mut resync = false;
    loop {
        if out.is_closed() {
            return;
        }
        let connected = tokio::select! {
            _=out.closed()=>return,
            result=timeout(Duration::from_secs(10),connect_async(&ws_url))=>result,
        };
        if let Ok(Ok((mut socket, _))) = connected {
            let mut deadline = Instant::now() + Duration::from_secs(15);
            let mut interval = Duration::from_secs(15);
            let mut welcomed = false;
            loop {
                let frame = tokio::select! {
                    _=out.closed()=>return,
                    frame=tokio::time::timeout_at(deadline,socket.next())=>frame,
                };
                let Ok(Some(Ok(frame))) = frame else {
                    break;
                };
                let text = match frame {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(msg) = parse_message(&text) else {
                    break;
                };
                match msg {
                    EsMsg::Welcome { session_id } => {
                        if welcomed {
                            break;
                        }
                        interval = keepalive(&text);
                        if !matches!(
                            timeout(
                                Duration::from_secs(10),
                                helix.subscribe_redemptions(&session_id)
                            )
                            .await,
                            Ok(Ok(()))
                        ) {
                            break;
                        }
                        welcomed = true;
                        if resync && out.send(EsSignal::Resync).await.is_err() {
                            return;
                        }
                        resync = false;
                        backoff = 1;
                    }
                    EsMsg::Keepalive => {}
                    EsMsg::Notification(r) => {
                        if !welcomed {
                            break;
                        }
                        if out.send(EsSignal::Redemption(r)).await.is_err() {
                            return;
                        }
                    }
                    EsMsg::Revocation => break,
                    EsMsg::Reconnect { url } => {
                        // Retain the old socket until the replacement has welcomed us.
                        let replacement = timeout(Duration::from_secs(10), async {
                            let (mut next, _) = connect_async(&url).await?;
                            loop {
                                match next.next().await {
                                    Some(Ok(Message::Text(t))) => {
                                        if !matches!(parse_message(&t)?, EsMsg::Welcome { .. }) {
                                            bail!("Expected reconnect welcome");
                                        }
                                        return Ok::<_, anyhow::Error>((next, keepalive(&t)));
                                    }
                                    Some(Ok(Message::Ping(_))) => continue,
                                    _ => bail!("Reconnect closed before welcome"),
                                }
                            }
                        })
                        .await;
                        let Ok(Ok((next, next_interval))) = replacement else {
                            break;
                        };
                        let _ = timeout(Duration::from_secs(1), socket.close(None)).await;
                        socket = next;
                        interval = next_interval;
                    }
                }
                deadline = Instant::now() + interval;
            }
        }
        resync = true;
        tokio::select! {
            _=out.closed()=>return,
            _=tokio::time::sleep(Duration::from_secs(backoff))=>{},
        }
        backoff = (backoff * 2).min(30);
    }
}
