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
        && music.trim().to_lowercase() == tts.trim().to_lowercase()
    {
        let suffix = " (озвучка)";
        *tts = tts
            .chars()
            .take(45 - suffix.chars().count())
            .collect::<String>()
            + suffix;
        if music.trim().to_lowercase() == tts.trim().to_lowercase() {
            *tts = tts
                .chars()
                .take(45 - suffix.chars().count() - 1)
                .collect::<String>()
                + suffix;
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

#[derive(Clone, Debug, PartialEq)]
pub struct RewardRoute {
    pub action: ActionKind,
    pub trigger_id: Option<String>,
    pub active: bool,
}
pub fn route_reward(r: &Rules, reward_id: &str) -> Option<RewardRoute> {
    for (action, t) in r.triggers() {
        if matches!(&t.kind, TriggerKind::Reward { reward_id: Some(id), .. } if id == reward_id) {
            return Some(RewardRoute {
                action,
                trigger_id: Some(t.id.clone()),
                active: r.is_active(action, t),
            });
        }
    }
    r.retiring
        .iter()
        .find(|t| t.reward_id == reward_id)
        .map(|t| RewardRoute {
            action: t.action,
            trigger_id: None,
            active: false,
        })
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MusicDecision {
    Ignore,
    Refund,
    Process,
}
pub fn music_decision(r: &Rules, reward_id: &str) -> MusicDecision {
    match route_reward(r, reward_id) {
        Some(RewardRoute {
            action: ActionKind::Music,
            active: true,
            ..
        }) => MusicDecision::Process,
        Some(RewardRoute {
            action: ActionKind::Music,
            active: false,
            ..
        }) => MusicDecision::Refund,
        _ => MusicDecision::Ignore,
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ChatRoute {
    pub action: ActionKind,
    pub trigger_id: String,
    pub text: String,
}
pub fn route_chat(r: &Rules, m: &crate::model::ChatMessage) -> Option<ChatRoute> {
    if m.source_broadcaster_id
        .as_ref()
        .is_some_and(|id| id != &m.broadcaster_id)
    {
        return None;
    }
    let text = crate::tts::filter::strip_emotes(&m.fragments);
    r.triggers().into_iter().find_map(|(action, t)| {
        if !r.is_active(action, t) {
            return None;
        }
        if let TriggerKind::ChatMarker { marker } = &t.kind {
            crate::tts::filter::extract_marked(&text, marker).map(|text| ChatRoute {
                action,
                trigger_id: t.id.clone(),
                text,
            })
        } else {
            None
        }
    })
}
pub fn reconcile_targets(r: &Rules) -> Vec<(ActionKind, String)> {
    r.triggers()
        .into_iter()
        .filter_map(|(action, t)| {
            if let TriggerKind::Reward {
                reward_id: Some(id),
                ..
            } = &t.kind
            {
                Some((action, id.clone()))
            } else {
                None
            }
        })
        .collect()
}
pub fn reward_body(title: &str, cost: u32, cooldown_s: u32, enabled: bool) -> serde_json::Value {
    serde_json::json!({"title":title,"cost":cost,"is_enabled":enabled,"is_paused":false,"global_cooldown_setting":{"is_enabled":cooldown_s>0,"global_cooldown_seconds":cooldown_s}})
}
pub fn reward_prompt(a: ActionKind) -> &'static str {
    match a {
        ActionKind::Music => "Название или ссылка на трек",
        ActionKind::Tts => "Текст для озвучки",
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum RewardOp {
    Ensure {
        action: ActionKind,
        trigger_id: String,
        known_id: Option<String>,
        title: String,
        cost: u32,
        create: bool,
    },
    Patch {
        reward_id: String,
        body: serde_json::Value,
    },
    Retire {
        action: ActionKind,
        reward_id: String,
    },
}
pub fn connect_ops(r: &Rules) -> Vec<RewardOp> {
    let mut ops = Vec::new();
    for (action, t) in r.triggers() {
        if let TriggerKind::Reward {
            title,
            cost,
            reward_id,
            ..
        } = &t.kind
        {
            let active = r.is_active(action, t);
            if reward_id.is_some() || active {
                ops.push(RewardOp::Ensure {
                    action,
                    trigger_id: t.id.clone(),
                    known_id: reward_id.clone(),
                    title: title.clone(),
                    cost: *cost,
                    create: active,
                });
            }
        }
    }
    ops.extend(r.retiring.iter().map(|t| RewardOp::Retire {
        action: t.action,
        reward_id: t.reward_id.clone(),
    }));
    ops
}
pub fn plan_save(current: &Rules, mut incoming: Rules) -> (Rules, Vec<RewardOp>) {
    incoming.retiring = current.retiring.clone();
    incoming.next_id = current.next_id;
    let mut seen = HashSet::new();
    for (action, t) in incoming
        .music
        .triggers
        .iter_mut()
        .map(|t| (ActionKind::Music, t))
        .chain(
            incoming
                .tts
                .triggers
                .iter_mut()
                .map(|t| (ActionKind::Tts, t)),
        )
    {
        let old = current.trigger(&t.id).filter(|(a, old)| {
            *a == action && old.kind.tag() == t.kind.tag() && !seen.contains(&t.id)
        });
        if old.is_none() {
            t.id = format!("t{}", incoming.next_id);
            incoming.next_id += 1;
        }
        if let TriggerKind::Reward { reward_id, .. } = &mut t.kind {
            *reward_id = old.and_then(|(_, old)| match &old.kind {
                TriggerKind::Reward { reward_id, .. } => reward_id.clone(),
                _ => None,
            });
        }
        seen.insert(t.id.clone());
    }
    let mut ops = Vec::new();
    for (action, t) in current.triggers() {
        if incoming.trigger(&t.id).is_none()
            && let TriggerKind::Reward {
                reward_id: Some(id),
                ..
            } = &t.kind
        {
            incoming.retiring.push(Retiring {
                action,
                reward_id: id.clone(),
            });
            ops.push(RewardOp::Retire {
                action,
                reward_id: id.clone(),
            });
        }
    }
    for (action, t) in incoming.triggers() {
        if let TriggerKind::Reward {
            title,
            cost,
            cooldown_s,
            reward_id,
        } = &t.kind
        {
            let active = incoming.is_active(action, t);
            let old = current.trigger(&t.id);
            let was_active = old.is_some_and(|(a, t)| current.is_active(a, t));
            let changed = old.is_none_or(|(_, old)| match &old.kind {
                TriggerKind::Reward {
                    title: ot,
                    cost: oc,
                    cooldown_s: os,
                    ..
                } => (ot, oc, os) != (title, cost, cooldown_s),
                _ => true,
            });
            if let Some(id) = reward_id {
                if changed || was_active != active {
                    ops.push(RewardOp::Patch {
                        reward_id: id.clone(),
                        body: if was_active && !active {
                            serde_json::json!({"is_enabled":false})
                        } else {
                            reward_body(title, *cost, *cooldown_s, active)
                        },
                    });
                }
            } else if active && (!was_active || changed) {
                ops.push(RewardOp::Ensure {
                    action,
                    trigger_id: t.id.clone(),
                    known_id: None,
                    title: title.clone(),
                    cost: *cost,
                    create: true,
                });
            }
        }
    }
    (incoming, ops)
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
        assert_eq!(
            r.tts.triggers[0].kind,
            TriggerKind::Reward {
                title: "Скажи".into(),
                cost: 100,
                cooldown_s: 0,
                reward_id: Some("t1".into())
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
        for prefix in ["я".repeat(35), "я ".to_string() + &"я".repeat(33)] {
            let title = prefix + " (озвучка)";
            let r = from_legacy(&json!({"reward_title":title,"tts":{"reward_title":title}}));
            let TriggerKind::Reward {
                title: migrated, ..
            } = &r.tts.triggers[0].kind
            else {
                panic!();
            };
            assert!(!migrated.starts_with(char::is_whitespace));
            assert_ne!(migrated.trim().to_lowercase(), title.trim().to_lowercase());
            assert_eq!(migrated.chars().count(), 44);
            assert_eq!(validate(&r), Ok(()));
        }
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
        r.music.triggers[0].kind = TriggerKind::ChatMarker { marker: "!".into() };
        cases.push(r);
        let mut r = Rules::default();
        r.tts.triggers.push(r.tts.triggers[1].clone());
        cases.push(r);
        let mut r = Rules::default();
        r.music.triggers = (0..11)
            .map(|i| Trigger {
                id: format!("music-{i}"),
                enabled: true,
                kind: TriggerKind::Reward {
                    title: format!("Трек {i}"),
                    cost: 500,
                    cooldown_s: 0,
                    reward_id: None,
                },
            })
            .collect();
        cases.push(r.clone());
        r.music.triggers.pop();
        assert_eq!(validate(&r), Ok(()));
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

#[cfg(test)]
mod routing_tests {
    use super::*;
    use serde_json::json;

    fn known() -> Rules {
        let mut r = Rules::default();
        r.set_reward_id("music-reward", Some("m1".into()));
        r.set_reward_id("tts-reward", Some("t1".into()));
        r.retiring.push(Retiring {
            action: ActionKind::Music,
            reward_id: "old".into(),
        });
        r
    }
    fn reward(id: &str) -> Trigger {
        Trigger {
            id: id.into(),
            enabled: true,
            kind: TriggerKind::Reward {
                title: id.into(),
                cost: 10,
                cooldown_s: 0,
                reward_id: Some("forged".into()),
            },
        }
    }
    #[test]
    fn route_reward_cases() {
        let r = known();
        assert_eq!(
            route_reward(&r, "m1"),
            Some(RewardRoute {
                action: ActionKind::Music,
                trigger_id: Some("music-reward".into()),
                active: true
            })
        );
        assert!(!route_reward(&r, "t1").unwrap().active);
        assert_eq!(
            route_reward(&r, "old"),
            Some(RewardRoute {
                action: ActionKind::Music,
                trigger_id: None,
                active: false
            })
        );
        assert_eq!(route_reward(&r, "unknown"), None);
    }
    #[test]
    fn music_decision_cases() {
        let mut r = known();
        assert_eq!(music_decision(&r, "m1"), MusicDecision::Process);
        r.music.enabled = false;
        assert_eq!(music_decision(&r, "m1"), MusicDecision::Refund);
        r.music.enabled = true;
        r.music.triggers[0].enabled = false;
        for id in ["m1", "old"] {
            assert_eq!(music_decision(&r, id), MusicDecision::Refund);
        }
        for id in ["t1", "unknown"] {
            assert_eq!(music_decision(&r, id), MusicDecision::Ignore);
        }
    }
    #[test]
    fn route_chat_cases() {
        use crate::model::{ChatMessage, Fragment};
        let mut r = Rules::default();
        r.tts.triggers[1].enabled = true;
        let mut m = ChatMessage {
            user_login: "u".into(),
            broadcaster_id: "b".into(),
            source_broadcaster_id: None,
            fragments: vec![],
            badges: vec![],
        };
        let fragment = |kind: &str, text: &str| Fragment {
            kind: kind.into(),
            text: text.into(),
        };
        m.fragments = vec![fragment("text", "привет %")];
        assert_eq!(
            route_chat(&r, &m),
            Some(ChatRoute {
                action: ActionKind::Tts,
                trigger_id: "tts-chat".into(),
                text: "привет".into()
            })
        );
        m.source_broadcaster_id = Some("other".into());
        assert_eq!(route_chat(&r, &m), None);
        m.source_broadcaster_id = Some("b".into());
        assert!(route_chat(&r, &m).is_some());
        r.tts.triggers[1].enabled = false;
        assert_eq!(route_chat(&r, &m), None);
        r.tts.triggers[1].enabled = true;
        r.tts.enabled = false;
        assert_eq!(route_chat(&r, &m), None);
        r.tts.enabled = true;
        m.fragments = vec![fragment("text", "на 100% согласен")];
        assert_eq!(route_chat(&r, &m), None);
        m.fragments = vec![fragment("text", "привет "), fragment("emote", "%")];
        assert_eq!(route_chat(&r, &m), None);
        m.fragments.push(fragment("text", "!!"));
        r.tts.triggers.push(Trigger {
            id: "second".into(),
            enabled: true,
            kind: TriggerKind::ChatMarker {
                marker: "!!".into(),
            },
        });
        assert_eq!(route_chat(&r, &m).unwrap().trigger_id, "second");
        m.fragments = vec![fragment("text", "привет !! потом %")];
        assert_eq!(route_chat(&r, &m).unwrap().trigger_id, "tts-chat");
    }
    #[test]
    fn reconcile_targets_cases() {
        let mut r = known();
        r.music.triggers.push(reward("new"));
        r.set_reward_id("new", None);
        assert_eq!(
            reconcile_targets(&r),
            vec![
                (ActionKind::Music, "m1".into()),
                (ActionKind::Tts, "t1".into())
            ]
        );
    }
    #[test]
    fn reward_body_shape() {
        for cooldown in [0, 30] {
            assert_eq!(
                reward_body("Название", 500, cooldown, true),
                json!({"title":"Название","cost":500,"is_enabled":true,"is_paused":false,"global_cooldown_setting":{"is_enabled":cooldown>0,"global_cooldown_seconds":cooldown}})
            );
        }
        assert_eq!(
            reward_prompt(ActionKind::Music),
            "Название или ссылка на трек"
        );
        let legacy = crate::tts::TtsSettings::default();
        for enabled in [false, true] {
            assert_eq!(
                reward_body(
                    &legacy.reward_title,
                    legacy.reward_cost,
                    legacy.reward_cooldown_s,
                    enabled
                )
                .to_string(),
                legacy.reward_body(enabled).to_string()
            );
        }
        assert_eq!(reward_prompt(ActionKind::Tts), "Текст для озвучки");
    }
    #[test]
    fn connect_ops_lazy() {
        assert_eq!(
            connect_ops(&Rules::default()),
            vec![RewardOp::Ensure {
                action: ActionKind::Music,
                trigger_id: "music-reward".into(),
                known_id: None,
                title: "Заказать трек".into(),
                cost: 500,
                create: true
            }]
        );
        assert_eq!(
            connect_ops(&known()),
            vec![
                RewardOp::Ensure {
                    action: ActionKind::Music,
                    trigger_id: "music-reward".into(),
                    known_id: Some("m1".into()),
                    title: "Заказать трек".into(),
                    cost: 500,
                    create: true
                },
                RewardOp::Ensure {
                    action: ActionKind::Tts,
                    trigger_id: "tts-reward".into(),
                    known_id: Some("t1".into()),
                    title: "Озвучить текст".into(),
                    cost: 300,
                    create: false
                },
                RewardOp::Retire {
                    action: ActionKind::Music,
                    reward_id: "old".into()
                }
            ]
        );
    }
    #[test]
    fn connect_ops_after_migration_no_create() {
        let r = from_legacy(
            &json!({"music_reward_id":"m1","tts":{"reward_id":"t1","reward_enabled":true}}),
        );
        let ops = connect_ops(&r);
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().all(|op| matches!(
            op,
            RewardOp::Ensure {
                known_id: Some(_),
                ..
            }
        )));
    }
    #[test]
    fn plan_save_ops() {
        let r = known();
        assert!(plan_save(&r, r.clone()).1.is_empty());
        let mut incoming = r.clone();
        if let TriggerKind::Reward { cost, .. } = &mut incoming.music.triggers[0].kind {
            *cost = 600;
        }
        assert_eq!(
            plan_save(&r, incoming).1,
            vec![RewardOp::Patch {
                reward_id: "m1".into(),
                body: reward_body("Заказать трек", 600, 0, true)
            }]
        );
        for action_off in [false, true] {
            let mut incoming = r.clone();
            if action_off {
                incoming.music.enabled = false;
            } else {
                incoming.music.triggers[0].enabled = false;
            }
            assert_eq!(
                plan_save(&r, incoming).1,
                vec![RewardOp::Patch {
                    reward_id: "m1".into(),
                    body: json!({"is_enabled":false})
                }]
            );
        }
        let mut incoming = r.clone();
        incoming.tts.triggers[0].enabled = true;
        assert_eq!(
            plan_save(&r, incoming).1,
            vec![RewardOp::Patch {
                reward_id: "t1".into(),
                body: reward_body("Озвучить текст", 300, 0, true)
            }]
        );
        let mut incoming = r.clone();
        if let TriggerKind::Reward { title, .. } = &mut incoming.tts.triggers[0].kind {
            *title = "Новое".into();
        }
        assert_eq!(
            plan_save(&r, incoming).1,
            vec![RewardOp::Patch {
                reward_id: "t1".into(),
                body: reward_body("Новое", 300, 0, false)
            }]
        );
        let mut multiple = r.clone();
        multiple.music.triggers.push(reward("second"));
        let mut incoming = multiple.clone();
        incoming.music.enabled = false;
        assert_eq!(
            plan_save(&multiple, incoming).1,
            vec![
                RewardOp::Patch {
                    reward_id: "m1".into(),
                    body: json!({"is_enabled":false})
                },
                RewardOp::Patch {
                    reward_id: "forged".into(),
                    body: json!({"is_enabled":false})
                },
            ]
        );
        let mut incoming = r.clone();
        incoming.music.triggers.clear();
        let (saved, ops) = plan_save(&r, incoming);
        assert_eq!(
            ops,
            vec![RewardOp::Retire {
                action: ActionKind::Music,
                reward_id: "m1".into()
            }]
        );
        assert_eq!(
            saved.retiring,
            vec![
                Retiring {
                    action: ActionKind::Music,
                    reward_id: "old".into()
                },
                Retiring {
                    action: ActionKind::Music,
                    reward_id: "m1".into()
                }
            ]
        );
        let r = Rules::default();
        assert!(plan_save(&r, r.clone()).1.is_empty());
        let mut incoming = r.clone();
        incoming.music.triggers.clear();
        assert!(plan_save(&r, incoming).1.is_empty());
        let mut incoming = r.clone();
        incoming.tts.triggers[0].enabled = true;
        assert!(matches!(
            &plan_save(&r, incoming).1[..],
            [RewardOp::Ensure {
                known_id: None,
                create: true,
                ..
            }]
        ));
        let mut incoming = r.clone();
        if let TriggerKind::Reward { title, .. } = &mut incoming.music.triggers[0].kind {
            *title = "Новое".into();
        }
        assert!(matches!(
            &plan_save(&r, incoming).1[..],
            [RewardOp::Ensure { .. }]
        ));
    }
    #[test]
    fn assign_ids_keeps_owned_reward_id() {
        let r = known();
        let mut incoming = r.clone();
        incoming.set_reward_id("music-reward", Some("forged".into()));
        incoming.retiring.clear();
        incoming.next_id = 999;
        let (saved, ops) = plan_save(&r, incoming);
        assert_eq!(saved, r);
        assert!(ops.is_empty());
    }
    #[test]
    fn assign_ids_two_new_triggers_distinct() {
        let r = Rules::default();
        let mut incoming = r.clone();
        incoming.tts.triggers.extend([reward(""), reward("")]);
        let (saved, _) = plan_save(&r, incoming);
        assert_eq!(saved.tts.triggers[2].id, "t1");
        assert_eq!(saved.tts.triggers[3].id, "t2");
        assert_eq!(saved.next_id, 3);
        for trigger in &saved.tts.triggers[2..4] {
            assert!(matches!(
                trigger.kind,
                TriggerKind::Reward {
                    reward_id: None,
                    ..
                }
            ));
        }
    }
    #[test]
    fn assign_ids_unknown_id_is_new() {
        let mut r = Rules::default();
        r.music.triggers[0].id = "t4".into();
        r.next_id = 5;
        r.set_reward_id("t4", Some("m1".into()));
        let mut incoming = r.clone();
        incoming.music.triggers[0].id = "t99".into();
        let (saved, ops) = plan_save(&r, incoming);
        assert_eq!(saved.music.triggers[0].id, "t5");
        assert_eq!(saved.next_id, 6);
        assert!(matches!(
            saved.music.triggers[0].kind,
            TriggerKind::Reward {
                reward_id: None,
                ..
            }
        ));
        assert!(ops.contains(&RewardOp::Retire {
            action: ActionKind::Music,
            reward_id: "m1".into()
        }));
        let mut incoming = saved.clone();
        incoming.music.triggers[0].id = "t4".into();
        assert_eq!(plan_save(&saved, incoming).0.music.triggers[0].id, "t6");
    }
    #[test]
    fn assign_ids_kind_change_is_new() {
        let r = known();
        let mut incoming = r.clone();
        incoming.tts.triggers[1] = reward("tts-chat");
        let (saved, ops) = plan_save(&r, incoming);
        assert_eq!(saved.tts.triggers[1].id, "t1");
        assert!(!ops.iter().any(|op| matches!(op, RewardOp::Patch { .. })));
    }
    #[test]
    fn assign_ids_duplicate_incoming() {
        let r = known();
        let mut incoming = r.clone();
        incoming
            .music
            .triggers
            .push(incoming.music.triggers[0].clone());
        let (saved, _) = plan_save(&r, incoming);
        assert_eq!(saved.music.triggers[1].id, "t1");
        assert!(matches!(
            saved.music.triggers[1].kind,
            TriggerKind::Reward {
                reward_id: None,
                ..
            }
        ));
    }
}
