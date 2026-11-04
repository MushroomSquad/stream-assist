pub mod engine;
pub mod filter;
pub mod queue;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsCfg {
    pub profile_id: Option<i64>,
    pub lang: String,
    pub speed: f32,
    pub roles: filter::Roles,
    pub global_cooldown_s: u32,
    pub user_cooldown_s: u32,
    pub strip_links: bool,
    pub stop_words: Vec<String>,
}
impl Default for TtsCfg {
    fn default() -> Self {
        Self {
            profile_id: None,
            lang: "ru".into(),
            speed: 1.0,
            roles: filter::Roles::default(),
            global_cooldown_s: 10,
            user_cooldown_s: 60,
            strip_links: true,
            stop_words: vec![],
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct TtsRuntime {
    pub cfg: TtsCfg,
    pub in_use: bool,
    pub chat_active: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct ItemView {
    pub id: u64,
    pub user: String,
    pub text: String,
    pub reward: bool,
    pub synthesizing: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct TtsView {
    pub current: Option<ItemView>,
    pub queue: Vec<ItemView>,
    pub widget: bool,
    pub warning: Option<String>,
}

pub fn describe(id: u64, user: &str, src: &str, text: &str) -> String {
    format!("id={id} src={src} user={user} len={}", text.chars().count())
}

#[cfg(test)]
mod tests {
    #[test]
    fn describe_has_no_text() {
        let description = super::describe(1, "bob", "chat", "секрет123");
        assert!(description.contains("user=bob"));
        assert!(description.contains("len=9"));
        assert!(!description.contains("секрет123"));
    }
}
