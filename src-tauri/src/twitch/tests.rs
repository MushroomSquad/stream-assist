use super::{
    auth,
    eventsub::{EsMsg, parse_message},
    helix::{Helix, RewardError},
};
use crate::model::{RedemptionRef, Tokens};
use serde_json::json;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::*};
fn tokens() -> Tokens {
    Tokens {
        access_token: "old".into(),
        refresh_token: "refresh".into(),
        expires_at: i64::MAX,
    }
}
async fn setup() -> (MockServer, Helix) {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/helix/users"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":[{"id":"b","login":"streamer"}]})),
        )
        .mount(&s)
        .await;
    let h = Helix::new("client".into(), tokens(), &s.uri(), &s.uri(), |_| {});
    (s, h)
}
#[tokio::test]
async fn device_poll_pending_is_none() {
    let s = MockServer::start().await;
    Mock::given(path("/oauth2/token"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"message":"authorization_pending"})),
        )
        .mount(&s)
        .await;
    assert!(
        auth::device_poll("c", "d", "s", &s.uri())
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn device_poll_success_tokens() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("device_code=d"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token":"a","refresh_token":"r","expires_in":3600})),
        )
        .mount(&s)
        .await;
    let t = auth::device_poll("c", "d", "s", &s.uri())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.access_token, "a");
    assert_eq!(t.refresh_token, "r");
}
#[test]
fn parse_welcome() {
    assert!(
        matches!(parse_message(r#"{"metadata":{"message_type":"session_welcome"},"payload":{"session":{"id":"s","keepalive_timeout_seconds":10}}}"#).unwrap(),EsMsg::Welcome{session_id} if session_id=="s")
    );
}
#[test]
fn parse_reconnect() {
    assert!(
        matches!(parse_message(r#"{"metadata":{"message_type":"session_reconnect"},"payload":{"session":{"reconnect_url":"wss://example.com"}}}"#).unwrap(),EsMsg::Reconnect{url} if url=="wss://example.com")
    );
}
#[test]
fn parse_redemption_notification() {
    let msg = parse_message(include_str!("fixtures/redemption.json")).unwrap();
    let EsMsg::Notification(r) = msg else {
        panic!()
    };
    assert_eq!(r.user_login, "cooler_user");
    assert_eq!(r.user_input, "pogchamp");
    assert_eq!(r.r.reward_id, "92af127c-7326-4483-a52b-b0da0be61c01");
}
fn redemption() -> RedemptionRef {
    RedemptionRef {
        reward_id: "reward".into(),
        redemption_id: "redemption".into(),
    }
}
#[tokio::test]
async fn set_status_sends_patch() {
    let (s, h) = setup().await;
    Mock::given(method("PATCH"))
        .and(path("/helix/channel_points/custom_rewards/redemptions"))
        .and(query_param("id", "redemption"))
        .and(query_param("reward_id", "reward"))
        .and(query_param("broadcaster_id", "b"))
        .and(body_json(json!({"status":"FULFILLED"})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&s)
        .await;
    h.set_redemption_status(&redemption(), true).await.unwrap();
}
#[tokio::test]
async fn set_status_already_done_is_ok() {
    let (s, h) = setup().await;
    Mock::given(method("PATCH"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"message":"The redemption status must be UNFULFILLED."})),
        )
        .mount(&s)
        .await;
    h.set_redemption_status(&redemption(), false).await.unwrap();
}
#[tokio::test]
async fn find_reward_404_is_none() {
    let (s, h) = setup().await;
    Mock::given(path("/helix/channel_points/custom_rewards"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&s)
        .await;
    assert!(
        h.find_manageable_reward(Some("r"), "title")
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn create_reward_403_not_affiliate() {
    let (s, h) = setup().await;
    Mock::given(method("POST"))
        .and(path("/helix/channel_points/custom_rewards"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&s)
        .await;
    assert!(matches!(
        h.create_reward("title", 1, "prompt").await,
        Err(RewardError::NotAffiliate)
    ));
}
#[tokio::test]
async fn unfulfilled_follows_pagination() {
    let (s, h) = setup().await;
    let row = json!({"id":"r","reward":{"id":"reward"},"user_login":"u","user_input":"song"});
    Mock::given(path("/helix/channel_points/custom_rewards/redemptions"))
        .and(query_param("first", "50"))
        .and(query_param("sort", "OLDEST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":[row],"pagination":{"cursor":"next"}})),
        )
        .mount(&s)
        .await;
    Mock::given(query_param("after", "next"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":[row],"pagination":{}})),
        )
        .with_priority(1)
        .mount(&s)
        .await;
    assert_eq!(h.unfulfilled("reward").await.unwrap().len(), 2);
}
#[tokio::test]
async fn helix_refresh_rotates_token() {
    let s = MockServer::start().await;
    Mock::given(path("/oauth2/token"))
        .and(body_string_contains("refresh_token=refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"access_token":"new","refresh_token":"rotated","expires_in":3600}),
        ))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(path("/helix/users"))
        .and(header("authorization", "Bearer old"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    Mock::given(path("/helix/users"))
        .and(header("authorization", "Bearer new"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"b","login":"u"}]})),
        )
        .mount(&s)
        .await;
    let rotated = std::sync::Arc::new(std::sync::Mutex::new(None));
    let copy = rotated.clone();
    let h = Helix::new("c".into(), tokens(), &s.uri(), &s.uri(), move |t| {
        *copy.lock().unwrap() = Some(t.clone())
    });
    let (a, b) = tokio::join!(h.me(), h.me());
    a.unwrap();
    b.unwrap();
    assert_eq!(
        rotated.lock().unwrap().as_ref().unwrap().refresh_token,
        "rotated"
    );
}

// Notification payload copied from Twitch's subscription-type documentation:
// https://dev.twitch.tv/docs/eventsub/eventsub-subscription-types/#channelchannel_points_custom_reward_redemptionadd
// Its optional-condition comment is removed to make valid JSON; the payload is
// wrapped in the WebSocket metadata/payload envelope.
#[tokio::test]
async fn eventsub_welcome_notification_and_migration() {
    use super::eventsub::{EsSignal, run};
    use futures_util::{SinkExt, StreamExt};
    use std::{sync::Arc, time::Duration};
    use tokio_tungstenite::{accept_async, tungstenite::Message};
    let (s, h) = setup().await;
    Mock::given(method("POST")).and(path("/helix/eventsub/subscriptions"))
        .and(body_json(json!({"type":"channel.channel_points_custom_reward_redemption.add","version":"1","condition":{"broadcaster_user_id":"b"},"transport":{"method":"websocket","session_id":"initial"}})))
        .respond_with(ResponseTemplate::new(202)).expect(1).mount(&s).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let replacement = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let replacement_url = format!("ws://{}", replacement.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(tcp).await.unwrap();
        ws.send(Message::Text(json!({"metadata":{"message_type":"session_welcome"},"payload":{"session":{"id":"initial","keepalive_timeout_seconds":10}}}).to_string().into())).await.unwrap();
        ws.send(Message::Text(
            include_str!("fixtures/redemption.json").into(),
        ))
        .await
        .unwrap();
        ws.send(Message::Text(json!({"metadata":{"message_type":"session_reconnect"},"payload":{"session":{"reconnect_url":replacement_url}}}).to_string().into())).await.unwrap();
        let (tcp, _) = replacement.accept().await.unwrap();
        let mut next = accept_async(tcp).await.unwrap();
        next.send(Message::Text(json!({"metadata":{"message_type":"session_welcome"},"payload":{"session":{"id":"migrated","keepalive_timeout_seconds":10}}}).to_string().into())).await.unwrap();
        // The old connection is closed only after replacement welcome.
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
        next.send(Message::Text(
            include_str!("fixtures/redemption.json").into(),
        ))
        .await
        .unwrap();
        // Keep the migrated session alive until the test finishes.
        let _ = next.next().await;
    });
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let driver = tokio::spawn(run(Arc::new(h), url, tx));
    for _ in 0..2 {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap(),
            Some(EsSignal::Redemption(_))
        ));
    }
    s.verify().await;
    drop(rx);
    tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .unwrap()
        .unwrap();
    server.abort();
}
#[tokio::test]
async fn set_status_bad_request_is_error() {
    let (s, h) = setup().await;
    Mock::given(method("PATCH"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"message":"Invalid broadcaster ID"})),
        )
        .mount(&s)
        .await;
    assert!(h.set_redemption_status(&redemption(), true).await.is_err());
}
#[tokio::test]
async fn find_reward_prefers_id_then_title() {
    let (s, h) = setup().await;
    Mock::given(method("GET"))
        .and(path("/helix/channel_points/custom_rewards"))
        .and(query_param("only_manageable_rewards", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[{"id":"one","title":"song"},{"id":"two","title":"old"}]}),
        ))
        .mount(&s)
        .await;
    assert_eq!(
        h.find_manageable_reward(Some("two"), "song").await.unwrap(),
        Some("two".into())
    );
    assert_eq!(
        h.find_manageable_reward(Some("missing"), "song")
            .await
            .unwrap(),
        Some("one".into())
    );
}
#[test]
fn russian_messages() {
    use crate::{i18n::*, limits::LimitViolation, player::CancelReason};
    assert_eq!(
        chat_added("user", "artist", "title", 3),
        "@user трек «artist — title» в очереди, позиция 3"
    );
    assert_eq!(
        chat_rejected("user", TEXT_NOT_FOUND),
        "@user заказ отклонён: ничего не найдено. Баллы возвращены."
    );
    assert_eq!(
        violation_text(&LimitViolation::TooLong),
        "слишком длинный трек"
    );
    assert_eq!(
        cancel_text(&CancelReason::RemovedByStreamer),
        "стример удалил заказ"
    );
}

#[tokio::test]
async fn eventsub_disconnect_subscribes_and_resyncs() {
    use super::eventsub::{EsSignal, run};
    use futures_util::{SinkExt, StreamExt};
    use std::{sync::Arc, time::Duration};
    use tokio_tungstenite::{accept_async, tungstenite::Message};
    let (s, h) = setup().await;
    Mock::given(method("POST"))
        .and(path("/helix/eventsub/subscriptions"))
        .respond_with(ResponseTemplate::new(202))
        .expect(2)
        .mount(&s)
        .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for id in ["initial", "recovered"] {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(tcp).await.unwrap();
            ws.send(Message::Text(json!({"metadata":{"message_type":"session_welcome"},"payload":{"session":{"id":id,"keepalive_timeout_seconds":10}}}).to_string().into())).await.unwrap();
            ws.send(Message::Text(
                include_str!("fixtures/redemption.json").into(),
            ))
            .await
            .unwrap();
            if id == "initial" {
                tokio::time::sleep(Duration::from_millis(50)).await;
                ws.close(None).await.unwrap();
            } else {
                let _ = ws.next().await;
            }
        }
    });
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let driver = tokio::spawn(run(Arc::new(h), url, tx));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap(),
        Some(EsSignal::Redemption(_))
    ));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap(),
        Some(EsSignal::Resync)
    ));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap(),
        Some(EsSignal::Redemption(_))
    ));
    s.verify().await;
    drop(rx);
    tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .unwrap()
        .unwrap();
    server.abort();
}

#[tokio::test]
async fn refresh_completes_even_if_caller_dropped() {
    let s = MockServer::start().await;
    Mock::given(path("/oauth2/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(150))
                .set_body_json(
                    json!({"access_token":"new","refresh_token":"rotated","expires_in":3600}),
                ),
        )
        .expect(1)
        .mount(&s)
        .await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut t = tokens();
    t.expires_at = 0;
    let h = Helix::new("client".into(), t, &s.uri(), &s.uri(), move |t| {
        tx.send(t.refresh_token.clone()).unwrap();
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), h.me())
            .await
            .is_err()
    );
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        "rotated"
    );
}
