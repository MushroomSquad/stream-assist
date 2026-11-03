use stream_assist_lib::{
    model::Source,
    router::{FeatureKind, route},
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
    assert_eq!(route("music", Some("music"), None), Some(FeatureKind::Music));
    assert_eq!(route("other", Some("music"), None), None);
    assert_eq!(route("music", None, None), None);
}
