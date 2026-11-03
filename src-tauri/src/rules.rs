use crate::tts::{TtsCfg, TtsRuntime};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Music,
    Tts,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerTag {
    Reward,
    ChatMarker,
}
pub fn allowed(a: ActionKind) -> &'static [TriggerTag] {
    match a {
        ActionKind::Music => &[TriggerTag::Reward],
        ActionKind::Tts => &[TriggerTag::Reward, TriggerTag::ChatMarker],
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerKind {
    Reward {
        title: String,
        cost: u32,
        cooldown_s: u32,
        #[serde(default)]
        reward_id: Option<String>,
    },
    ChatMarker {
        marker: String,
    },
}
impl TriggerKind {
    pub fn tag(&self) -> TriggerTag {
        match self {
            Self::Reward { .. } => TriggerTag::Reward,
            Self::ChatMarker { .. } => TriggerTag::ChatMarker,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    #[serde(default)]
    pub id: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: TriggerKind,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action<C> {
    pub enabled: bool,
    pub cfg: C,
    pub triggers: Vec<Trigger>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MusicCfg {
    pub limits: crate::limits::Limits,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Retiring {
    pub action: ActionKind,
    pub reward_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    pub music: Action<MusicCfg>,
    pub tts: Action<TtsCfg>,
    #[serde(default)]
    pub retiring: Vec<Retiring>,
    #[serde(default = "one")]
    pub next_id: u32,
}
fn one() -> u32 {
    1
}
impl Default for Rules {
    fn default() -> Self {
        Self {
            music: Action {
                enabled: true,
                cfg: MusicCfg::default(),
                triggers: vec![Trigger {
                    id: "music-reward".into(),
                    enabled: true,
                    kind: TriggerKind::Reward {
                        title: "Заказать трек".into(),
                        cost: 500,
                        cooldown_s: 0,
                        reward_id: None,
                    },
                }],
            },
            tts: Action {
                enabled: true,
                cfg: TtsCfg::default(),
                triggers: vec![
                    Trigger {
                        id: "tts-reward".into(),
                        enabled: false,
                        kind: TriggerKind::Reward {
                            title: "Озвучить текст".into(),
                            cost: 300,
                            cooldown_s: 0,
                            reward_id: None,
                        },
                    },
                    Trigger {
                        id: "tts-chat".into(),
                        enabled: false,
                        kind: TriggerKind::ChatMarker { marker: "%".into() },
                    },
                ],
            },
            retiring: vec![],
            next_id: 1,
        }
    }
}
impl Rules {
    pub fn triggers(&self) -> Vec<(ActionKind, &Trigger)> {
        self.music
            .triggers
            .iter()
            .map(|t| (ActionKind::Music, t))
            .chain(self.tts.triggers.iter().map(|t| (ActionKind::Tts, t)))
            .collect()
    }
    pub fn trigger(&self, id: &str) -> Option<(ActionKind, &Trigger)> {
        self.triggers().into_iter().find(|(_, t)| t.id == id)
    }
    pub fn is_active(&self, a: ActionKind, t: &Trigger) -> bool {
        t.enabled
            && match a {
                ActionKind::Music => self.music.enabled,
                ActionKind::Tts => self.tts.enabled,
            }
    }
    pub fn reward_ids(&self) -> Vec<String> {
        self.triggers()
            .into_iter()
            .filter_map(|(_, t)| match &t.kind {
                TriggerKind::Reward { reward_id, .. } => reward_id.clone(),
                _ => None,
            })
            .chain(self.retiring.iter().map(|r| r.reward_id.clone()))
            .collect()
    }
    pub fn set_reward_id(&mut self, trigger_id: &str, id: Option<String>) -> bool {
        for t in self
            .music
            .triggers
            .iter_mut()
            .chain(self.tts.triggers.iter_mut())
        {
            if t.id == trigger_id {
                if let TriggerKind::Reward { reward_id, .. } = &mut t.kind {
                    *reward_id = id;
                    return true;
                }
                return false;
            }
        }
        false
    }
    pub fn tts_runtime(&self) -> TtsRuntime {
        TtsRuntime {
            cfg: self.tts.cfg.clone(),
            in_use: self
                .tts
                .triggers
                .iter()
                .any(|t| self.is_active(ActionKind::Tts, t)),
            chat_active: self.tts.triggers.iter().any(|t| {
                self.is_active(ActionKind::Tts, t) && t.kind.tag() == TriggerTag::ChatMarker
            }),
        }
    }
}
fn valid_marker(s: &str) -> bool {
    (1..=3).contains(&s.chars().count()) && !s.chars().any(char::is_whitespace)
}
pub fn validate(r: &Rules) -> Result<(), String> {
    let invalid = || Err("settings_invalid".into());
    if r.music.triggers.len() > 10 || r.tts.triggers.len() > 10 {
        return invalid();
    }
    let mut titles = HashSet::new();
    let mut markers = HashSet::new();
    for (a, t) in r.triggers() {
        if !allowed(a).contains(&t.kind.tag()) {
            return invalid();
        }
        match &t.kind {
            TriggerKind::Reward {
                title,
                cost,
                cooldown_s,
                ..
            } => {
                if !(1..=45).contains(&title.trim().chars().count())
                    || !(1..=2147483647).contains(cost)
                    || *cooldown_s > 604800
                    || !titles.insert(title.trim().to_lowercase())
                {
                    return invalid();
                }
            }
            TriggerKind::ChatMarker { marker } => {
                if !valid_marker(marker) || !markers.insert(marker) {
                    return invalid();
                }
            }
        }
    }
    let c = &r.tts.cfg;
    if !(0.7..=1.5).contains(&c.speed)
        || c.global_cooldown_s > 3600
        || c.user_cooldown_s > 3600
        || c.stop_words.len() > 200
        || c.stop_words.iter().any(|s| s.chars().count() > 50)
        || !supertonic3_tts::AVAILABLE_LANGS.contains(&c.lang.as_str())
    {
        return invalid();
    }
    Ok(())
}
pub fn from_legacy(v: &serde_json::Value) -> Rules {
    let mut r = Rules::default();
    let t = &v["tts"];
    // Overlay partial limits before deserializing; malformed limits fall back as a whole.
    let mut limits = serde_json::to_value(&r.music.cfg.limits).unwrap();
    if let Some(fields) = v["limits"].as_object() {
        limits.as_object_mut().unwrap().extend(fields.clone());
        r.music.cfg.limits = serde_json::from_value(limits).unwrap_or_default();
    }
    for (trigger, source, id_key) in [
        (&mut r.music.triggers[0], v, "music_reward_id"),
        (&mut r.tts.triggers[0], t, "reward_id"),
    ] {
        if let TriggerKind::Reward {
            title,
            cost,
            cooldown_s,
            reward_id,
        } = &mut trigger.kind
        {
            if let Some(s) = source["reward_title"].as_str() {
                let s: String = s.trim().chars().take(45).collect();
                if !s.is_empty() {
                    *title = s;
                }
            }
            if let Some(n) = source["reward_cost"].as_u64() {
                *cost = n.clamp(1, 2147483647) as u32;
            }
            if let Some(n) = source["reward_cooldown_s"].as_u64() {
                *cooldown_s = n.min(604800) as u32;
            }
            *reward_id = source[id_key].as_str().map(str::to_owned);
        }
    }
    if let (TriggerKind::Reward { title: music, .. }, TriggerKind::Reward { title: tts, .. }) =
        (&r.music.triggers[0].kind, &mut r.tts.triggers[0].kind)
    {
        if music.trim().to_lowercase() == tts.trim().to_lowercase() {
            let suffix = " (озвучка)";
            *tts = tts
                .chars()
                .take(45 - suffix.chars().count())
                .collect::<String>()
                + suffix;
            if music.trim().to_lowercase() == tts.trim().to_lowercase() {
                *tts = tts.chars().skip(1).collect();
            }
        }
    }
    r.tts.triggers[0].enabled = t["reward_enabled"].as_bool().unwrap_or(false);
    r.tts.triggers[1].enabled = t["chat_enabled"].as_bool().unwrap_or(false);
    if let Some(marker) = t["marker"].as_str().filter(|s| valid_marker(s)) {
        r.tts.triggers[1].kind = TriggerKind::ChatMarker {
            marker: marker.into(),
        };
    }
    let c = &mut r.tts.cfg;
    c.profile_id = t["profile_id"].as_i64();
    if let Some(lang) = t["lang"]
        .as_str()
        .filter(|s| supertonic3_tts::AVAILABLE_LANGS.contains(s))
    {
        c.lang = lang.into();
    }
    if let Some(speed) = t["speed"].as_f64() {
        c.speed = speed.clamp(0.7, 1.5) as f32;
    }
    for (dest, key) in [
        (&mut c.global_cooldown_s, "global_cooldown_s"),
        (&mut c.user_cooldown_s, "user_cooldown_s"),
    ] {
        if let Some(n) = t[key].as_u64() {
            *dest = n.min(3600) as u32;
        }
    }
    if let Some(b) = t["strip_links"].as_bool() {
        c.strip_links = b;
    }
    for (dest, key) in [
        (&mut c.roles.everyone, "everyone"),
        (&mut c.roles.subscribers, "subscribers"),
        (&mut c.roles.vips, "vips"),
        (&mut c.roles.moderators, "moderators"),
    ] {
        if let Some(b) = t["roles"][key].as_bool() {
            *dest = b;
        }
    }
    c.stop_words = serde_json::from_value::<Vec<String>>(t["stop_words"].clone())
        .unwrap_or_default()
        .into_iter()
        .take(200)
        .map(|s| s.chars().take(50).collect())
        .collect();
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_shape() {
        let r = Rules::default();
        assert!(r.music.enabled && r.tts.enabled);
        assert_eq!(
            serde_json::to_value(&r.music.triggers[0]).unwrap(),
            json!({"id":"music-reward","enabled":true,"kind":"reward","title":"Заказать трек","cost":500,"cooldown_s":0,"reward_id":null})
        );
        assert_eq!(
            serde_json::to_value(&r.tts.triggers[1]).unwrap(),
            json!({"id":"tts-chat","enabled":false,"kind":"chat_marker","marker":"%"})
        );
        assert_eq!(
            serde_json::from_value::<Rules>(serde_json::to_value(&r).unwrap()).unwrap(),
            r
        );
        let mut v = serde_json::to_value(&r).unwrap();
        v.as_object_mut().unwrap().remove("retiring");
        v.as_object_mut().unwrap().remove("next_id");
        assert_eq!(serde_json::from_value::<Rules>(v).unwrap(), r);
        assert_eq!(
            r.tts.cfg,
            serde_json::from_value::<TtsCfg>(json!({})).unwrap()
        );
    }
    #[test]
    fn migrate_legacy_full() {
        let limits = json!({"no_repeat_last_n":5});
        let r = from_legacy(
            &json!({"music_reward_id":"m1","reward_title":"Трек","reward_cost":700,"reward_cooldown_s":30,"limits":limits,"tts":{"reward_enabled":true,"chat_enabled":false,"reward_id":"t1","reward_title":"Скажи","reward_cost":100,"marker":"!!","speed":1.2,"stop_words":["x"]}}),
        );
        assert_eq!(
            r.music.triggers[0].kind,
            TriggerKind::Reward {
                title: "Трек".into(),
                cost: 700,
                cooldown_s: 30,
                reward_id: Some("m1".into())
            }
        );
        assert_eq!(r.music.cfg.limits.no_repeat_last_n, 5);
        assert!(r.music.enabled && r.tts.enabled && r.tts.triggers[0].enabled);
        assert!(!r.tts.triggers[1].enabled);
        assert_eq!(r.reward_ids(), vec!["m1", "t1"]);
        assert_eq!(
            r.tts.triggers[1].kind,
            TriggerKind::ChatMarker {
                marker: "!!".into()
            }
        );
        assert_eq!(r.tts.cfg.speed, 1.2);
        assert_eq!(r.tts.cfg.stop_words, vec!["x"]);
        assert_eq!(validate(&r), Ok(()));
    }
    #[test]
    fn migrate_legacy_empty() {
        assert_eq!(from_legacy(&json!({})), Rules::default());
    }
    #[test]
    fn migrate_legacy_lenient() {
        let title = "я".repeat(60);
        let r = from_legacy(
            &json!({"reward_title":title,"reward_cooldown_s":999999,"tts":{"speed":"fast","reward_id":"t1","reward_title":title}}),
        );
        assert_eq!(r.tts.cfg.speed, 1.0);
        assert_eq!(r.reward_ids(), vec!["t1"]);
        if let TriggerKind::Reward {
            title, cooldown_s, ..
        } = &r.music.triggers[0].kind
        {
            assert_eq!(title.chars().count(), 45);
            assert_eq!(*cooldown_s, 604800);
        } else {
            panic!();
        }
        assert_eq!(validate(&r), Ok(()));
    }
    #[test]
    fn migrate_legacy_titles_with_suffix() {
        let title = "я".repeat(35) + " (озвучка)";
        let r = from_legacy(&json!({"reward_title":title,"tts":{"reward_title":title}}));
        assert_eq!(validate(&r), Ok(()));
    }
    #[test]
    fn migrate_legacy_invalid_stop_words() {
        let r = from_legacy(&json!({"tts":{"reward_id":"t1","stop_words":["x",42]}}));
        assert!(r.tts.cfg.stop_words.is_empty());
        assert_eq!(r.reward_ids(), vec!["t1"]);
    }
    #[test]
    fn validate_ok_default() {
        assert_eq!(validate(&Rules::default()), Ok(()));
    }
    #[test]
    fn validate_rejects() {
        let mut cases = Vec::new();
        for title in [
            "".to_string(),
            "   ".into(),
            "я".repeat(46),
            " заказать ТРЕК ".into(),
        ] {
            let mut r = Rules::default();
            if let TriggerKind::Reward { title: t, .. } = &mut r.tts.triggers[0].kind {
                *t = title;
            }
            cases.push(r);
        }
        for marker in ["", "abcd", "a b", "\t"] {
            let mut r = Rules::default();
            r.tts.triggers[1].kind = TriggerKind::ChatMarker {
                marker: marker.into(),
            };
            cases.push(r);
        }
        let mut r = Rules::default();
        r.music.triggers[0].kind = r.tts.triggers[1].kind.clone();
        cases.push(r);
        let mut r = Rules::default();
        r.tts.triggers.push(r.tts.triggers[1].clone());
        cases.push(r);
        let mut r = Rules::default();
        r.music.triggers = vec![r.music.triggers[0].clone(); 11];
        cases.push(r);
        for (cost, cooldown_s) in [(0, 0), (2147483648, 0), (1, 604801)] {
            let mut r = Rules::default();
            r.music.triggers[0].kind = TriggerKind::Reward {
                title: "x".into(),
                cost,
                cooldown_s,
                reward_id: None,
            };
            cases.push(r);
        }
        for speed in [0.69, 1.51, f32::NAN] {
            let mut r = Rules::default();
            r.tts.cfg.speed = speed;
            cases.push(r);
        }
        let mut r = Rules::default();
        r.tts.cfg.global_cooldown_s = 3601;
        cases.push(r);
        let mut r = Rules::default();
        r.tts.cfg.user_cooldown_s = 3601;
        cases.push(r);
        let mut r = Rules::default();
        r.tts.cfg.stop_words = vec!["x".into(); 201];
        cases.push(r);
        let mut r = Rules::default();
        r.tts.cfg.stop_words = vec!["я".repeat(51)];
        cases.push(r);
        let mut r = Rules::default();
        r.tts.cfg.lang = "xx".into();
        cases.push(r);
        for r in cases {
            assert_eq!(validate(&r), Err("settings_invalid".into()), "{r:?}");
        }
        let mut r = Rules::default();
        if let TriggerKind::Reward { title, .. } = &mut r.music.triggers[0].kind {
            *title = "я".repeat(45);
        }
        assert_eq!(validate(&r), Ok(()));
    }
    #[test]
    fn is_active_matrix() {
        for action in [false, true] {
            for trigger in [false, true] {
                let mut r = Rules::default();
                r.music.enabled = action;
                r.music.triggers[0].enabled = trigger;
                assert_eq!(
                    r.is_active(ActionKind::Music, &r.music.triggers[0]),
                    action && trigger
                );
            }
        }
    }
    #[test]
    fn tts_runtime_flags() {
        for (reward, chat, action) in [
            (false, false, true),
            (true, false, true),
            (false, true, true),
            (true, true, false),
        ] {
            let mut r = Rules::default();
            r.tts.enabled = action;
            r.tts.triggers[0].enabled = reward;
            r.tts.triggers[1].enabled = chat;
            assert_eq!(
                r.tts_runtime(),
                TtsRuntime {
                    cfg: r.tts.cfg.clone(),
                    in_use: action && (reward || chat),
                    chat_active: action && chat
                }
            );
        }
    }
    #[test]
    fn reward_ids_include_retiring() {
        let mut r = Rules::default();
        assert!(r.set_reward_id("music-reward", Some("m".into())));
        assert!(r.set_reward_id("tts-reward", Some("t".into())));
        assert!(!r.set_reward_id("tts-chat", Some("x".into())));
        assert!(!r.set_reward_id("missing", None));
        r.retiring.push(Retiring {
            action: ActionKind::Tts,
            reward_id: "old".into(),
        });
        assert_eq!(r.reward_ids(), vec!["m", "t", "old"]);
        assert_eq!(
            r.triggers()
                .iter()
                .map(|(_, t)| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["music-reward", "tts-reward", "tts-chat"]
        );
        assert_eq!(r.trigger("tts-chat").unwrap().0, ActionKind::Tts);
        assert!(r.trigger("missing").is_none());
        assert!(r.set_reward_id("music-reward", None));
        assert_eq!(r.reward_ids(), vec!["t", "old"]);
    }
}
