pub mod filter;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsSettings {
    pub reward_enabled: bool,
    pub chat_enabled: bool,
    pub profile_id: Option<i64>,
    pub lang: String,
    pub speed: f32,
    pub reward_title: String,
    pub reward_cost: u32,
    pub reward_cooldown_s: u32,
    pub reward_id: Option<String>,
    pub marker: String,
    pub roles: filter::Roles,
    pub global_cooldown_s: u32,
    pub user_cooldown_s: u32,
    pub strip_links: bool,
    pub stop_words: Vec<String>,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self {
            reward_enabled: false,
            chat_enabled: false,
            profile_id: None,
            lang: "ru".into(),
            speed: 1.0,
            reward_title: "Озвучить текст".into(),
            reward_cost: 300,
            reward_cooldown_s: 0,
            reward_id: None,
            marker: "%".into(),
            roles: filter::Roles::default(),
            global_cooldown_s: 10,
            user_cooldown_s: 60,
            strip_links: true,
            stop_words: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_settings_from_empty_json() {
        let settings: TtsSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, TtsSettings::default());
        assert!(!settings.reward_enabled);
        assert!(!settings.chat_enabled);
        assert_eq!(settings.profile_id, None);
        assert_eq!(settings.lang, "ru");
        assert_eq!(settings.speed, 1.0);
        assert_eq!(settings.reward_title, "Озвучить текст");
        assert_eq!(settings.reward_cost, 300);
        assert_eq!(settings.reward_cooldown_s, 0);
        assert_eq!(settings.reward_id, None);
        assert_eq!(settings.marker, "%");
        assert_eq!(settings.roles, filter::Roles::default());
        assert_eq!(settings.global_cooldown_s, 10);
        assert_eq!(settings.user_cooldown_s, 60);
        assert!(settings.strip_links);
        assert!(settings.stop_words.is_empty());
    }
}
