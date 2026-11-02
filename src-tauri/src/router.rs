use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FeatureKind {
    Music,
}

pub fn route(reward_id: &str, music_reward_id: Option<&str>) -> Option<FeatureKind> {
    if music_reward_id == Some(reward_id) {
        Some(FeatureKind::Music)
    } else {
        None
    }
}
