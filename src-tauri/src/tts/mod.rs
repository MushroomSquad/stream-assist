pub mod engine;
pub mod filter;
pub mod queue;

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

impl TtsSettings {
    pub fn reward_body(&self, enabled: bool) -> serde_json::Value {
        serde_json::json!({"title":self.reward_title,"cost":self.reward_cost,"is_enabled":enabled,"is_paused":false,"global_cooldown_setting":{"is_enabled":self.reward_cooldown_s > 0,"global_cooldown_seconds":self.reward_cooldown_s}})
    }
}
pub fn tts_reward_patch(old: &TtsSettings, new: &TtsSettings) -> Option<serde_json::Value> {
    if (
        old.reward_title.as_str(),
        old.reward_cost,
        old.reward_cooldown_s,
        old.reward_enabled,
    ) == (
        new.reward_title.as_str(),
        new.reward_cost,
        new.reward_cooldown_s,
        new.reward_enabled,
    ) {
        None
    } else if old.reward_enabled && !new.reward_enabled {
        Some(serde_json::json!({"is_enabled":false}))
    } else {
        Some(new.reward_body(new.reward_enabled))
    }
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

#[cfg(test)]
mod reward_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tts_reward_body() {
        let s = TtsSettings::default();
        for enabled in [false, true] {
            assert_eq!(
                s.reward_body(enabled),
                json!({"title":"Озвучить текст","cost":300,"is_enabled":enabled,"is_paused":false,"global_cooldown_setting":{"is_enabled":false,"global_cooldown_seconds":0}})
            );
        }
    }
    #[test]
    fn tts_reward_patch_cases() {
        let old = TtsSettings::default();
        assert_eq!(tts_reward_patch(&old, &old), None);
        let mut new = old.clone();
        new.reward_enabled = true;
        assert_eq!(tts_reward_patch(&old, &new), Some(new.reward_body(true)));
        let enabled = new.clone();
        new.reward_enabled = false;
        assert_eq!(
            tts_reward_patch(&enabled, &new),
            Some(json!({"is_enabled":false}))
        );
        new = enabled.clone();
        new.reward_cost += 1;
        assert_eq!(
            tts_reward_patch(&enabled, &new),
            Some(new.reward_body(true))
        );
        new = enabled.clone();
        new.reward_title = "Другое".into();
        new.reward_cooldown_s = 60;
        assert_eq!(
            tts_reward_patch(&enabled, &new),
            Some(new.reward_body(true))
        );
    }
}
