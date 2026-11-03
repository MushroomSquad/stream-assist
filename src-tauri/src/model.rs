use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Source {
    Spotify { uri: String },
    YouTube { video_id: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub source: Source,
    pub title: String,
    pub artist: String,
    pub duration_ms: Option<u64>,
    pub explicit: Option<bool>,
    pub cover_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RedemptionRef {
    pub reward_id: String,
    pub redemption_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: i64,
    pub track: Track,
    pub requester: String,
    pub redemption: Option<RedemptionRef>,
    pub started: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Redemption {
    pub r: RedemptionRef,
    pub user_login: String,
    pub user_input: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaybackSnapshot {
    pub device_id: Option<String>,
    pub is_playing: bool,
    pub track_uri: Option<String>,
    pub progress_ms: u64,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix timestamp in seconds.
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlayError {
    NoDevice,
    Unauthorized,
    Api(u16),
    Net,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fragment {
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub user_login: String,
    pub broadcaster_id: String,
    pub source_broadcaster_id: Option<String>,
    pub fragments: Vec<Fragment>,
    pub badges: Vec<String>,
}
