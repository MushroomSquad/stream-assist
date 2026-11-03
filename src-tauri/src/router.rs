use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FeatureKind {
    Music,
    Tts,
}

pub fn route(reward_id: &str, music: Option<&str>, tts: Option<&str>) -> Option<FeatureKind> {
    if music == Some(reward_id) {
        Some(FeatureKind::Music)
    } else if tts == Some(reward_id) {
        Some(FeatureKind::Tts)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn route_tts() {
        assert_eq!(route("t", Some("m"), Some("t")), Some(FeatureKind::Tts));
        assert_eq!(route("m", Some("m"), Some("t")), Some(FeatureKind::Music));
        assert_eq!(route("x", Some("m"), Some("t")), None);
        assert_eq!(route("t", None, None), None);
    }
}
