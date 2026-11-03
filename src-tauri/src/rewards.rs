use crate::rules::{
    ActionKind, Retiring, RewardOp, Rules, TriggerKind, connect_ops, plan_save, reward_body,
    reward_prompt,
};
use crate::twitch::helix::{Helix, RewardError};
use anyhow::{Result, anyhow};
use std::collections::{BTreeMap, HashMap, HashSet};

pub async fn ensure_reward(
    h: &Helix,
    action: ActionKind,
    known_id: Option<&str>,
    title: &str,
    cost: u32,
    create: bool,
    exclude: &[String],
) -> Result<Option<String>> {
    if let Some(id) = h.find_manageable_reward(known_id, title, exclude).await? {
        return Ok(Some(id));
    }
    if !create {
        return Ok(None);
    }
    h.create_reward(title, cost, reward_prompt(action)).await.map(Some).map_err(|error| anyhow!(match error {
        RewardError::NotAffiliate => "нужен статус компаньона/партнёра",
        RewardError::DuplicateTitle => "награда с таким названием уже есть, но создана не этой программой — переименуйте её или удалите",
        RewardError::Other(_) => "Не удалось создать награду",
    }))
}

pub async fn retire_reward(h: &Helix, id: &str, keep: &HashSet<String>) -> Result<()> {
    if !h.disable_reward(id).await? {
        return Ok(());
    }
    for r in h.unfulfilled(id).await? {
        if !keep.contains(&r.r.redemption_id) {
            h.set_redemption_status(&r.r, false).await?;
        }
    }
    h.delete_reward(id).await
}

#[derive(Default)]
pub struct Lifecycle {
    ensuring: HashMap<String, bool>,
    // A completed ensure may outlive its trigger, so retain its action until ready.
    ensuring_actions: HashMap<String, ActionKind>,
    retiring_now: HashSet<String>,
    pub errors: BTreeMap<String, String>,
}

impl Lifecycle {
    pub fn on_connect(&mut self, rules: &Rules) -> Vec<RewardOp> {
        self.admit(connect_ops(rules))
    }

    pub fn on_save(&mut self, current: &Rules, incoming: Rules) -> (Rules, Vec<RewardOp>) {
        let (rules, ops) = plan_save(current, incoming);
        for (_, trigger) in current.triggers() {
            if rules.trigger(&trigger.id).is_none() {
                self.errors.remove(&trigger.id);
            }
        }
        (rules, self.admit(ops))
    }

    fn admit(&mut self, ops: Vec<RewardOp>) -> Vec<RewardOp> {
        ops.into_iter()
            .filter(|op| match op {
                RewardOp::Ensure {
                    action, trigger_id, ..
                } => {
                    if let Some(dirty) = self.ensuring.get_mut(trigger_id) {
                        *dirty = true;
                        false
                    } else {
                        self.ensuring.insert(trigger_id.clone(), false);
                        self.ensuring_actions.insert(trigger_id.clone(), *action);
                        true
                    }
                }
                RewardOp::Retire { reward_id, .. } => self.retiring_now.insert(reward_id.clone()),
                RewardOp::Patch { .. } => true,
            })
            .collect()
    }

    pub fn release_ensure(&mut self, trigger_id: &str) {
        self.ensuring.remove(trigger_id);
        self.ensuring_actions.remove(trigger_id);
    }

    pub fn release_retire(&mut self, reward_id: &str) {
        self.retiring_now.remove(reward_id);
    }

    pub fn exclude(&self, rules: &Rules) -> Vec<String> {
        rules.retiring.iter().map(|r| r.reward_id.clone()).collect()
    }

    pub fn on_ready(
        &mut self,
        rules: &mut Rules,
        trigger_id: &str,
        known_id: Option<String>,
        result: Result<Option<String>, String>,
    ) -> (bool, Vec<RewardOp>) {
        let dirty = self.ensuring.remove(trigger_id).unwrap_or(false);
        let action = self.ensuring_actions.remove(trigger_id);
        let mut changed = false;
        let mut refresh = dirty;
        let mut ops = Vec::new();
        let result = match result {
            Ok(Some(id)) if rules.retiring.iter().any(|r| r.reward_id == id) => Ok(None),
            other => other,
        };
        match result {
            Ok(Some(id)) => {
                let accepts = rules.trigger(trigger_id).is_some_and(|(_, t)| {
                    matches!(&t.kind, TriggerKind::Reward { reward_id, .. }
                        if reward_id.is_none() || reward_id == &known_id)
                });
                if accepts {
                    changed = rules.trigger(trigger_id).is_some_and(|(_, t)| {
                        matches!(&t.kind, TriggerKind::Reward { reward_id, .. }
                            if reward_id.as_ref() != Some(&id))
                    });
                    rules.set_reward_id(trigger_id, Some(id));
                    self.errors.remove(trigger_id);
                    refresh = true;
                } else if !rules.reward_ids().contains(&id) {
                    if let Some(action) = action {
                        rules.retiring.push(Retiring {
                            action,
                            reward_id: id.clone(),
                        });
                        ops.push(RewardOp::Retire {
                            action,
                            reward_id: id,
                        });
                        changed = true;
                    }
                }
            }
            Ok(None) => {
                let matches = rules.trigger(trigger_id).is_some_and(|(_, t)| {
                    matches!(&t.kind, TriggerKind::Reward { reward_id, .. } if reward_id == &known_id)
                });
                if matches {
                    changed = known_id.is_some();
                    rules.set_reward_id(trigger_id, None);
                    refresh = true;
                }
            }
            Err(error) => {
                if rules.trigger(trigger_id).is_some_and(|(_, t)| {
                    matches!(
                        &t.kind,
                        TriggerKind::Reward {
                            reward_id: None,
                            ..
                        }
                    )
                }) {
                    self.errors.insert(trigger_id.into(), error);
                }
            }
        }
        // Both completion and dirty state use one snapshot and emit at most one update.
        if refresh {
            if let Some((action, trigger)) = rules.trigger(trigger_id) {
                if let TriggerKind::Reward {
                    title,
                    cost,
                    cooldown_s,
                    reward_id,
                } = &trigger.kind
                {
                    let active = rules.is_active(action, trigger);
                    if let Some(id) = reward_id {
                        ops.push(RewardOp::Patch {
                            reward_id: id.clone(),
                            body: reward_body(title, *cost, *cooldown_s, active),
                        });
                    } else if active {
                        ops.push(RewardOp::Ensure {
                            action,
                            trigger_id: trigger_id.into(),
                            known_id: None,
                            title: title.clone(),
                            cost: *cost,
                            create: true,
                        });
                    }
                }
            }
        }
        (changed, self.admit(ops))
    }

    pub fn on_retired(
        &mut self,
        rules: &mut Rules,
        reward_id: &str,
        result: Result<(), String>,
    ) -> bool {
        self.retiring_now.remove(reward_id);
        let key = format!("retire:{reward_id}");
        match result {
            Ok(()) => {
                let before = rules.retiring.len();
                rules.retiring.retain(|r| r.reward_id != reward_id);
                self.errors.remove(&key);
                rules.retiring.len() != before
            }
            Err(error) => {
                self.errors.insert(key, error);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Retiring, RewardOp, Rules, TriggerKind, reward_body};

    fn title(r: &mut Rules, value: &str) {
        if let TriggerKind::Reward { title, .. } = &mut r.music.triggers[0].kind {
            *title = value.into();
        }
    }
    fn id(r: &Rules) -> Option<&str> {
        match &r.music.triggers[0].kind {
            TriggerKind::Reward { reward_id, .. } => reward_id.as_deref(),
            _ => panic!("expected reward"),
        }
    }
    fn ensures(ops: &[RewardOp]) -> usize {
        ops.iter()
            .filter(|op| matches!(op, RewardOp::Ensure { .. }))
            .count()
    }
    fn retiring(r: &mut Rules) {
        r.retiring.push(Retiring {
            action: ActionKind::Music,
            reward_id: "old".into(),
        });
    }

    #[test]
    fn save_during_ensure_single_ensure() {
        let mut l = Lifecycle::default();
        let r = Rules::default();
        assert_eq!(ensures(&l.on_connect(&r)), 1);
        let mut incoming = r.clone();
        title(&mut incoming, "Новое название");
        let (mut r, ops) = l.on_save(&r, incoming);
        assert!(ops.is_empty());
        let (changed, ops) = l.on_ready(&mut r, "music-reward", None, Ok(Some("r1".into())));
        assert!(changed);
        assert_eq!(id(&r), Some("r1"));
        assert_eq!(
            ops,
            vec![RewardOp::Patch {
                reward_id: "r1".into(),
                body: reward_body("Новое название", 500, 0, true)
            }]
        );
    }

    #[test]
    fn ready_for_deleted_trigger_retires() {
        for action in [ActionKind::Music, ActionKind::Tts] {
            let mut l = Lifecycle::default();
            let mut r = Rules::default();
            r.music.triggers.clear();
            r.tts.triggers.truncate(1);
            r.tts.triggers[0].enabled = true;
            r.tts.triggers[0].id = "arbitrary-id".into();
            if action == ActionKind::Music {
                r.music.triggers.push(r.tts.triggers.remove(0));
            }
            l.on_connect(&r);
            let mut incoming = r.clone();
            incoming.music.triggers.clear();
            incoming.tts.triggers.clear();
            let (mut r, _) = l.on_save(&r, incoming);
            l.errors.insert("arbitrary-id".into(), "stale".into());
            let (changed, ops) = l.on_ready(&mut r, "arbitrary-id", None, Ok(Some("r9".into())));
            assert!(changed);
            assert_eq!(
                r.retiring,
                vec![Retiring {
                    action,
                    reward_id: "r9".into()
                }]
            );
            assert_eq!(
                ops,
                vec![RewardOp::Retire {
                    action,
                    reward_id: "r9".into()
                }]
            );
        }
    }

    #[test]
    fn ready_none_keeps_foreign_id() {
        let mut l = Lifecycle::default();
        let mut r = Rules::default();
        r.set_reward_id("music-reward", Some("r2".into()));
        let (changed, ops) = l.on_ready(&mut r, "music-reward", Some("r1".into()), Ok(None));
        assert!(!changed);
        assert!(ops.is_empty());
        assert_eq!(id(&r), Some("r2"));
    }

    #[test]
    fn ready_none_then_active_recreates() {
        let mut l = Lifecycle::default();
        let mut r = Rules::default();
        r.music.enabled = false;
        r.set_reward_id("music-reward", Some("r1".into()));
        assert!(matches!(
            &l.on_connect(&r)[..],
            [RewardOp::Ensure { create: false, .. }]
        ));
        let mut incoming = r.clone();
        incoming.music.enabled = true;
        let (mut r, _) = l.on_save(&r, incoming);
        let (changed, ops) = l.on_ready(&mut r, "music-reward", Some("r1".into()), Ok(None));
        assert!(changed);
        assert_eq!(id(&r), None);
        assert!(matches!(
            &ops[..],
            [RewardOp::Ensure {
                create: true,
                known_id: None,
                ..
            }]
        ));
    }

    #[test]
    fn ready_err_sets_error_unless_id() {
        let mut l = Lifecycle::default();
        let mut r = Rules::default();
        assert!(
            !l.on_ready(&mut r, "music-reward", None, Err("failed".into()))
                .0
        );
        assert_eq!(l.errors["music-reward"], "failed");
        l.errors.clear();
        r.set_reward_id("music-reward", Some("r1".into()));
        assert!(
            !l.on_ready(&mut r, "music-reward", None, Err("ignored".into()))
                .0
        );
        assert!(l.errors.is_empty());
    }

    #[test]
    fn retiring_survives_failure() {
        let mut l = Lifecycle::default();
        let mut r = Rules::default();
        retiring(&mut r);
        l.on_connect(&r);
        assert!(!l.on_retired(&mut r, "old", Err("failed".into())));
        assert_eq!(r.retiring.len(), 1);
        assert_eq!(l.errors["retire:old"], "failed");
        assert!(l.on_connect(&r).contains(&RewardOp::Retire {
            action: ActionKind::Music,
            reward_id: "old".into()
        }));
        assert!(l.on_retired(&mut r, "old", Ok(())));
        assert!(r.retiring.is_empty());
        assert!(l.errors.is_empty());
        assert!(!l.on_retired(&mut r, "old", Ok(())));
    }

    #[test]
    fn retire_not_duplicated_in_flight() {
        let mut l = Lifecycle::default();
        let mut r = Rules::default();
        retiring(&mut r);
        assert_eq!(
            l.on_connect(&r)
                .iter()
                .filter(|op| matches!(op, RewardOp::Retire { .. }))
                .count(),
            1
        );
        assert!(l.on_connect(&r).is_empty());
    }

    #[test]
    fn offline_release_then_connect_reissues() {
        let mut l = Lifecycle::default();
        let mut r = Rules::default();
        r.set_reward_id("music-reward", Some("old".into()));
        let mut incoming = r.clone();
        incoming.music.triggers[0].id.clear();
        let (r, ops) = l.on_save(&r, incoming);
        assert_eq!(ops.len(), 2);
        let trigger = &r.music.triggers[0].id;
        l.release_ensure(trigger);
        l.release_retire("old");
        assert!(l.ensuring.is_empty());
        assert!(l.retiring_now.is_empty());
        let mut incoming = r.clone();
        title(&mut incoming, "Changed");
        let (r, ops) = l.on_save(&r, incoming);
        assert_eq!(ensures(&ops), 1);
        assert_eq!(l.ensuring.get(trigger), Some(&false));
        l.release_ensure(trigger);
        assert_eq!(l.on_connect(&r).len(), 2);
    }

    #[test]
    fn ready_with_retiring_id_not_assigned() {
        let mut l = Lifecycle::default();
        let mut r = Rules::default();
        retiring(&mut r);
        assert_eq!(l.exclude(&r), vec!["old".to_string()]);
        l.on_connect(&r);
        let (changed, ops) = l.on_ready(&mut r, "music-reward", None, Ok(Some("old".into())));
        assert!(!changed);
        assert_eq!(id(&r), None);
        assert!(matches!(&ops[..], [RewardOp::Ensure { create: true, .. }]));
    }

    #[test]
    fn save_removes_deleted_trigger_errors() {
        let mut l = Lifecycle::default();
        let r = Rules::default();
        l.errors.insert("music-reward".into(), "failed".into());
        l.errors.insert("tts-reward".into(), "failed".into());
        l.errors.insert("retire:old".into(), "failed".into());
        let mut incoming = r.clone();
        incoming.music.triggers.clear();
        l.on_save(&r, incoming);
        assert!(!l.errors.contains_key("music-reward"));
        assert!(l.errors.contains_key("tts-reward"));
        assert!(l.errors.contains_key("retire:old"));
    }
}
