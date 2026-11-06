use stream_assist_lib::{
    db::RedemptionState, driver::*, limits::Limits, model::*, music::*, spotify::*,
};
fn track() -> Track {
    Track {
        source: Source::Spotify {
            uri: "spotify:track:1".into(),
        },
        title: "Around the World".into(),
        artist: "Daft Punk".into(),
        duration_ms: Some(1000),
        explicit: Some(false),
        cover_url: None,
    }
}
#[test]
fn decide_rejects_on_violation() {
    let mut t = track();
    t.duration_ms = Some(999999);
    assert!(matches!(
        decide(
            Resolution::Candidates(vec![t]),
            &Limits::default(),
            "u",
            &[],
            None,
            &[]
        ),
        Decision::Reject(_)
    ));
}
#[test]
fn decide_not_found_rejects() {
    assert!(matches!(
        decide(
            Resolution::NotFound,
            &Limits::default(),
            "u",
            &[],
            None,
            &[]
        ),
        Decision::Reject(_)
    ));
}
#[test]
fn decide_rejects_requested_track_instead_of_substituting_next_candidate() {
    let mut t = track();
    t.duration_ms = Some(999999);
    assert!(matches!(
        decide(
            Resolution::Candidates(vec![t, track()]),
            &Limits::default(),
            "u",
            &[],
            None,
            &[]
        ),
        Decision::Reject(_)
    ));
}
#[test]
fn normalize_title_strips_noise() {
    assert_eq!(
        normalize_title("Around The World (Official Video) [HD] lyrics MV"),
        "around the world"
    );
    for suffix in [
        "(Official Audio)",
        "- Official Audio",
        "[Official Music Video]",
        "- Lyric Video",
        "lyrics",
        "mv",
        "hd",
    ] {
        assert_eq!(
            normalize_title(&format!("Around The World {suffix}")),
            "around the world"
        );
    }
    assert_eq!(normalize_title("HD Lover"), "hd lover");
    assert_eq!(
        normalize_title("Official Audio Lover"),
        "official audio lover"
    );
    assert_eq!(normalize_title("Love (Live)"), "love (live)");
    assert_eq!(normalize_title("MyHD"), "myhd");
}
#[test]
fn oauth_state_mismatch_rejected() {
    let mut p = Some(new_pkce());
    assert!(take_pending_if_state_matches(&mut p, "bad").is_none());
    assert!(p.is_some());
}
#[test]
fn oauth_state_single_use() {
    let p = new_pkce();
    let s = p.state.clone();
    let mut p = Some(p);
    assert!(take_pending_if_state_matches(&mut p, &s).is_some());
    assert!(take_pending_if_state_matches(&mut p, &s).is_none());
}
#[test]
fn oauth_no_pending_rejected() {
    assert!(take_pending_if_state_matches(&mut None, "").is_none());
}
fn red() -> Redemption {
    Redemption {
        r: RedemptionRef {
            reward_id: "r".into(),
            redemption_id: "id".into(),
        },
        user_login: "u".into(),
        user_input: "q".into(),
    }
}
#[test]
fn reconcile_queued_skipped() {
    assert!(matches!(
        plan_reconcile(&[(red(), RedemptionState::Queued)])[0],
        ReconcileStep::Skip
    ));
}
#[test]
fn reconcile_started_fulfilled() {
    assert!(matches!(
        plan_reconcile(&[(red(), RedemptionState::Started)])[0],
        ReconcileStep::Fulfill(_)
    ));
}
#[test]
fn reconcile_cancelled_recancelled() {
    assert!(matches!(
        plan_reconcile(&[(red(), RedemptionState::Cancelled)])[0],
        ReconcileStep::Recancel(_)
    ));
}
#[test]
fn reconcile_unknown_processed() {
    assert!(matches!(
        plan_reconcile(&[(red(), RedemptionState::Unknown)])[0],
        ReconcileStep::Process(_)
    ));
}
#[test]
fn pick_device_skips_null_id() {
    let ds=serde_json::from_str::<Vec<Device>>(r#"[{"id":null,"is_active":true,"type":"Computer"},{"id":"ok","is_active":false,"type":"Phone"}]"#).unwrap();
    assert_eq!(pick_device(&ds), Some("ok".into()));
}
async fn youtube_case(matched: bool) {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let s = MockServer::start().await;
    Mock::given(path("/oembed")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"title":"Daft Punk - Around The World (Official Video)","author_name":"Daft Punk","thumbnail_url":"cover"}))).mount(&s).await;
    let items = if matched {
        serde_json::json!([{"uri":"spotify:track:1","name":"Around the World","artists":[{"name":"Daft Punk"}],"duration_ms":1000,"explicit":false,"album":{"images":[]}}])
    } else {
        serde_json::json!([])
    };
    Mock::given(path("/v1/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"tracks":{"items":items}})),
        )
        .expect(0)
        .mount(&s)
        .await;
    let sp = Spotify::new(
        "c".into(),
        Tokens {
            access_token: "test".into(),
            refresh_token: "test".into(),
            expires_at: i64::MAX,
        },
        &s.uri(),
        &s.uri(),
        |_| {},
    );
    let Resolution::Candidates(ts) = resolve(
        stream_assist_lib::request::RequestInput::YouTube("vid".into()),
        Some(&sp),
        &s.uri(),
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert!(matches!(&ts[0].source, Source::YouTube { video_id } if video_id == "vid"));
}
#[tokio::test]
async fn resolve_youtube_preserves_video_even_when_spotify_has_a_match() {
    youtube_case(true).await;
}
#[tokio::test]
async fn resolve_youtube_no_match_stays_youtube() {
    youtube_case(false).await;
}

async fn youtube_outage_case(connected: bool) {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let s = MockServer::start().await;
    Mock::given(path("/oembed")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"title":"Artist - Song","author_name":"Artist","thumbnail_url":"cover"}))).mount(&s).await;
    Mock::given(path("/v1/search"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&s)
        .await;
    let sp = Spotify::new(
        "c".into(),
        Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: i64::MAX,
        },
        &s.uri(),
        &s.uri(),
        |_| {},
    );
    let Resolution::Candidates(ts) = resolve(
        stream_assist_lib::request::RequestInput::YouTube("vid".into()),
        if connected { Some(&sp) } else { None },
        &s.uri(),
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert!(matches!(&ts[0].source, Source::YouTube {video_id} if video_id == "vid"));
}
#[tokio::test]
async fn resolve_youtube_without_spotify_stays_youtube() {
    youtube_outage_case(false).await;
}
#[tokio::test]
async fn resolve_youtube_spotify_500_stays_youtube() {
    youtube_outage_case(true).await;
}
