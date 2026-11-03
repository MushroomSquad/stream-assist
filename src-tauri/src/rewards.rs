use crate::rules::{ActionKind, reward_prompt};
use crate::twitch::helix::{Helix, RewardError};
use anyhow::{Result, anyhow};
use std::collections::HashSet;

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
