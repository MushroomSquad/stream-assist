use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};
fn tokens(expiring: bool) -> Tokens {
    Tokens {
        access_token: "old".into(),
        refresh_token: "refresh".into(),
        expires_at: now() + if expiring { 10 } else { 3600 },
    }
}
fn client(s: &MockServer, expiring: bool) -> Spotify {
    Spotify::new(
        "client".into(),
        tokens(expiring),
        &s.uri(),
        &s.uri(),
        |_| {},
    )
}
fn track_json() -> serde_json::Value {
    serde_json::json!({"uri":"spotify:track:1","name":"Song","artists":[{"name":"A"},{"name":"B"}],"duration_ms":1234,"explicit":true,"album":{"images":[{"url":"small","width":64,"height":64},{"url":"large","width":640,"height":640}]}})
}
async fn refresh(s: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"access_token":"new","refresh_token":"rotated","expires_in":3600}),
        ))
        .expect(1)
        .mount(s)
        .await;
}
#[test]
fn pkce_challenge_matches_rfc7636_vector() {
    assert_eq!(
        challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    // The brief supplied a different verifier; verify its actual SHA-256 too.
    assert_eq!(
        challenge("dBjftJeZ4CVP-mJ92K9q2W8ivJXR5zGPRgo0vJyj6Mw"),
        "PgsTSlfXb3vqZqlYLt3UBDc6IiE-Fe1GUsnXMgUesAU"
    );
    let p = new_pkce();
    assert_eq!(p.verifier.len(), 64);
    assert_eq!(p.state.len(), 32);
    assert!(
        p.verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    );
    assert_eq!(p.challenge, challenge(&p.verifier));
}
#[tokio::test]
async fn player_204_is_none() {
    let s = MockServer::start().await;
    Mock::given(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&s)
        .await;
    assert_eq!(client(&s, false).player().await.unwrap(), None);
}

#[tokio::test]
async fn bodyless_playback_commands_send_content_length_zero() {
    let s = MockServer::start().await;
    // A stricter gateway returns 411 when Content-Length is missing.
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(411))
        .with_priority(10)
        .mount(&s)
        .await;
    for endpoint in [
        "/v1/me/player/pause",
        "/v1/me/player/play",
        "/v1/me/player/volume",
    ] {
        Mock::given(method("PUT"))
            .and(path(endpoint))
            .and(header("content-length", "0"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&s)
            .await;
    }
    let sp = client(&s, false);
    sp.pause().await.unwrap();
    sp.resume().await.unwrap();
    sp.set_volume(50).await.unwrap();
}
#[tokio::test]
async fn player_parses_snapshot() {
    let s = MockServer::start().await;
    Mock::given(path("/v1/me/player")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"device":{"id":"dev"},"is_playing":true,"progress_ms":42,"item":track_json()}))).mount(&s).await;
    assert_eq!(
        client(&s, false).player().await.unwrap(),
        Some(PlaybackSnapshot {
            device_id: Some("dev".into()),
            is_playing: true,
            track_uri: Some("spotify:track:1".into()),
            progress_ms: 42,
            duration_ms: 1234
        })
    );
}

#[tokio::test]
async fn accepted_play_waits_for_actual_track_on_requested_device() {
    let s = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&s)
        .await;
    let polls = Arc::new(AtomicUsize::new(0));
    let count = polls.clone();
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .respond_with(move |_: &wiremock::Request| {
            let attempt = count.fetch_add(1, Ordering::SeqCst);
            let uri = if attempt == 0 {
                "spotify:track:other"
            } else {
                "spotify:track:1"
            };
            let mut track = track_json();
            track["uri"] = uri.into();
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device":{"id":"dev"},"is_playing":true,"progress_ms":100,"item":track
            }))
        })
        .mount(&s)
        .await;
    client(&s, false)
        .play_confirmed("spotify:track:1", Some("dev"), 0)
        .await
        .unwrap();
    assert!(polls.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn other_track_paused_track_and_wrong_device_do_not_confirm_start() {
    for (uri, playing, device) in [
        ("spotify:track:other", true, "dev"),
        ("spotify:track:1", false, "dev"),
        ("spotify:track:1", true, "other-device"),
    ] {
        let s = MockServer::start().await;
        let mut track = track_json();
        track["uri"] = uri.into();
        Mock::given(path("/v1/me/player"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device":{"id":device},"is_playing":playing,"progress_ms":0,"item":track
            })))
            .mount(&s)
            .await;
        assert_eq!(
            client(&s, false)
                .wait_for_playback(
                    "spotify:track:1",
                    Some("dev"),
                    std::time::Duration::from_millis(30)
                )
                .await,
            Err(PlayError::NotStarted)
        );
    }
}
#[tokio::test]
async fn refresh_when_expiring_and_rotates() {
    let s = MockServer::start().await;
    refresh(&s).await;
    Mock::given(path("/v1/me"))
        .and(header("authorization", "Bearer new"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"product":"premium"})),
        )
        .mount(&s)
        .await;
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    let sp = Spotify::new(
        "client".into(),
        tokens(true),
        &s.uri(),
        &s.uri(),
        move |t| {
            assert_eq!(t.refresh_token, "rotated");
            c.fetch_add(1, Ordering::SeqCst);
        },
    );
    sp.me_product().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn concurrent_calls_refresh_once() {
    let s = MockServer::start().await;
    refresh(&s).await;
    Mock::given(path("/v1/me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"product":"premium"})),
        )
        .expect(2)
        .mount(&s)
        .await;
    let sp = client(&s, true);
    let (a, b) = tokio::join!(sp.me_product(), sp.me_product());
    a.unwrap();
    b.unwrap();
}
#[tokio::test]
async fn retry_once_on_401() {
    let s = MockServer::start().await;
    refresh(&s).await;
    for token in ["old", "new"] {
        Mock::given(path("/v1/me"))
            .and(header("authorization", format!("Bearer {token}")))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&s)
            .await;
    }
    assert!(client(&s, false).me_product().await.is_err());
}
#[tokio::test]
async fn play_404_is_no_device() {
    let s = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(serde_json::json!({"error":{"reason":"NO_ACTIVE_DEVICE"}})),
        )
        .mount(&s)
        .await;
    assert_eq!(
        client(&s, false).play("spotify:track:1", None, 0).await,
        Err(PlayError::NoDevice)
    );
}
#[tokio::test]
async fn me_product_reads_premium() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"product":"premium"})),
        )
        .mount(&s)
        .await;
    assert_eq!(client(&s, false).me_product().await.unwrap(), "premium");
}
#[tokio::test]
async fn search_maps_track_fields() {
    let s = MockServer::start().await;
    Mock::given(path("/v1/search"))
        .and(query_param("q", "a & b"))
        .and(query_param("type", "track"))
        .and(query_param("limit", "5"))
        .and(query_param("market", "from_token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"tracks":{"items":[track_json()]}})),
        )
        .mount(&s)
        .await;
    assert_eq!(
        client(&s, false).search_track("a & b").await.unwrap(),
        vec![Track {
            source: Source::Spotify {
                uri: "spotify:track:1".into()
            },
            title: "Song".into(),
            artist: "A, B".into(),
            duration_ms: Some(1234),
            explicit: Some(true),
            cover_url: Some("large".into())
        }]
    );
}
#[test]
fn pick_device_prefers_active_then_computer() {
    let mut ds = vec![
        Device {
            id: Some("phone".into()),
            is_active: false,
            kind: "Smartphone".into(),
        },
        Device {
            id: Some("pc".into()),
            is_active: false,
            kind: "Computer".into(),
        },
        Device {
            id: Some("active".into()),
            is_active: true,
            kind: "Speaker".into(),
        },
    ];
    assert_eq!(pick_device(&ds), Some("active".into()));
    ds.pop();
    assert_eq!(pick_device(&ds), Some("pc".into()));
    ds.pop();
    assert_eq!(pick_device(&ds), Some("phone".into()));
    assert_eq!(pick_device(&[]), None);
}

#[tokio::test]
async fn refresh_completes_even_if_caller_dropped() {
    let s = MockServer::start().await;
    Mock::given(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(150))
            .set_body_json(serde_json::json!({"access_token":"new","refresh_token":"rotated","expires_in":3600})))
        .expect(1).mount(&s).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sp = Spotify::new(
        "client".into(),
        tokens(true),
        &s.uri(),
        &s.uri(),
        move |t| {
            tx.send(t.refresh_token.clone()).unwrap();
        },
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), sp.auth(None))
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
    assert_eq!(sp.auth(None).await.unwrap(), "new");
}

#[test]
fn oauth_error_with_wrong_state_keeps_pending() {
    let mut pending = Some(new_pkce());
    let state = pending.as_ref().unwrap().state.clone();
    assert!(take_pending_if_state_matches(&mut pending, "wrong").is_none());
    assert_eq!(pending.as_ref().unwrap().state, state);
    assert!(take_pending_if_state_matches(&mut pending, &state).is_some());
    assert!(pending.is_none());
}

#[tokio::test]
async fn refresh_times_out_and_releases_lock() {
    use std::time::Duration;
    let s = MockServer::start().await;
    Mock::given(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .expect(1)
        .mount(&s)
        .await;
    let mut sp = client(&s, true);
    sp.client = crate::http::client_with_timeout(Duration::from_millis(200));
    let error = tokio::time::timeout(Duration::from_secs(1), sp.me_product())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.downcast_ref::<reqwest::Error>().unwrap().is_timeout());
    s.reset().await;
    refresh(&s).await;
    Mock::given(path("/v1/me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"product":"premium"})),
        )
        .expect(1)
        .mount(&s)
        .await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), sp.me_product())
            .await
            .unwrap()
            .unwrap(),
        "premium"
    );
}
