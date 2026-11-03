use stream_assist_lib::{
    model::Source,
    rules::{ActionKind, Rules, route_reward},
};

#[test]
fn source_roundtrip_json() {
    for source in [
        Source::Spotify {
            uri: "spotify:track:123".into(),
        },
        Source::YouTube {
            video_id: "abc123".into(),
        },
    ] {
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), source);
    }
}

#[test]
fn route_music() {
    let mut rules = Rules::default();
    rules.set_reward_id("music-reward", Some("music".into()));
    assert_eq!(
        route_reward(&rules, "music").unwrap().action,
        ActionKind::Music
    );
    assert_eq!(route_reward(&rules, "other"), None);
    assert_eq!(route_reward(&Rules::default(), "music"), None);
}
