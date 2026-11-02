use super::auth;
use crate::model::{Redemption, RedemptionRef, Tokens};
use anyhow::{Result, anyhow};
use reqwest::{Client, Method, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

const REWARDS: &str = "/helix/channel_points/custom_rewards";
const REDEMPTIONS: &str = "/helix/channel_points/custom_rewards/redemptions";
#[derive(Debug, thiserror::Error)]
pub enum RewardError {
    #[error("Broadcaster is not an affiliate or partner")]
    NotAffiliate,
    #[error("Duplicate reward title")]
    DuplicateTitle,
    #[error("{0}")]
    Other(String),
}
pub struct Helix {
    client: Client,
    client_id: String,
    api: String,
    id_base: String,
    tokens: Arc<Mutex<Tokens>>,
    on_rotate: Arc<dyn Fn(&Tokens) + Send + Sync>,
}
#[derive(Deserialize)]
pub(super) struct ApiRedemption {
    id: String,
    reward: Reward,
    user_login: String,
    user_input: String,
}
#[derive(Deserialize)]
struct Reward {
    id: String,
}
impl From<ApiRedemption> for Redemption {
    fn from(v: ApiRedemption) -> Self {
        Self {
            r: RedemptionRef {
                reward_id: v.reward.id,
                redemption_id: v.id,
            },
            user_login: v.user_login,
            user_input: v.user_input,
        }
    }
}
impl Helix {
    pub fn new(
        client_id: String,
        tokens: Tokens,
        api: &str,
        id_base: &str,
        on_rotate: impl Fn(&Tokens) + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Client::new(),
            client_id,
            api: api.trim_end_matches('/').into(),
            id_base: id_base.trim_end_matches('/').into(),
            tokens: Arc::new(Mutex::new(tokens)),
            on_rotate: Arc::new(on_rotate),
        }
    }
    async fn auth(&self, rejected: Option<&str>) -> Result<String> {
        let tokens = self.tokens.clone();
        let on_rotate = self.on_rotate.clone();
        let client_id = self.client_id.clone();
        let rejected = rejected.map(str::to_owned);
        let id_base = self.id_base.clone();
        tokio::spawn(async move {
            let mut t = tokens.lock().await;
            if (rejected.is_none() && t.expires_at - auth::now() < 300)
                || rejected.as_deref().is_some_and(|old| old == t.access_token)
            {
                *t = auth::refresh(&client_id, &t.refresh_token, &id_base).await?;
                (on_rotate)(&t);
            }
            Ok(t.access_token.clone())
        })
        .await?
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Response> {
        let mut token = self.auth(None).await?;
        for attempt in 0..2 {
            let mut req = self
                .client
                .request(method.clone(), format!("{}{path}", self.api))
                .header("Client-Id", &self.client_id)
                .bearer_auth(&token)
                .query(query);
            if let Some(ref b) = body {
                req = req.json(b);
            }
            let response = req.send().await?;
            if response.status() != 401 || attempt == 1 {
                return Ok(response);
            }
            token = self.auth(Some(&token)).await?;
        }
        unreachable!()
    }
    pub async fn me(&self) -> Result<(String, String)> {
        let v: Value = self
            .request(Method::GET, "/helix/users", &[], None)
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok((
            string(&v["data"][0]["id"])?,
            string(&v["data"][0]["login"])?,
        ))
    }
    pub async fn find_manageable_reward(
        &self,
        id: Option<&str>,
        title: &str,
    ) -> Result<Option<String>> {
        let (b, _) = self.me().await?;
        let r = self
            .request(
                Method::GET,
                REWARDS,
                &[("broadcaster_id", &b), ("only_manageable_rewards", "true")],
                None,
            )
            .await?;
        if r.status() == 404 {
            return Ok(None);
        }
        let v: Value = r.error_for_status()?.json().await?;
        let rows = v["data"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing rewards"))?;
        Ok(rows
            .iter()
            .find(|r| id.is_some_and(|id| r["id"] == id))
            .or_else(|| rows.iter().find(|r| r["title"] == title))
            .and_then(|r| r["id"].as_str())
            .map(str::to_owned))
    }
    pub async fn create_reward(
        &self,
        title: &str,
        cost: u32,
        prompt: &str,
    ) -> Result<String, RewardError> {
        let (b, _) = self
            .me()
            .await
            .map_err(|_| RewardError::Other("Twitch identity request failed".into()))?;
        let r = self
            .request(
                Method::POST,
                REWARDS,
                &[("broadcaster_id", &b)],
                Some(json!({
                    "title": title,
                    "cost": cost,
                    "prompt": prompt,
                    "is_user_input_required": true
                })),
            )
            .await
            .map_err(|_| RewardError::Other("Twitch request failed".into()))?;
        let status = r.status();
        if status == 403 {
            return Err(RewardError::NotAffiliate);
        }
        if !status.is_success() {
            let v: Value = r.json().await.unwrap_or_default();
            let msg = v["message"].as_str().unwrap_or_default().to_lowercase();
            if status == 400
                && (msg.contains("duplicate") || msg.contains("already") || msg.contains("unique"))
            {
                return Err(RewardError::DuplicateTitle);
            }
            return Err(RewardError::Other(format!(
                "Twitch reward request failed ({status})"
            )));
        }
        let v: Value = r
            .json()
            .await
            .map_err(|_| RewardError::Other("Invalid reward response".into()))?;
        string(&v["data"][0]["id"]).map_err(|_| RewardError::Other("Missing reward id".into()))
    }
    pub async fn update_reward(&self, id: &str, patch: &Value) -> Result<()> {
        let (b, _) = self.me().await?;
        self.request(
            Method::PATCH,
            REWARDS,
            &[("broadcaster_id", &b), ("id", id)],
            Some(patch.clone()),
        )
        .await?
        .error_for_status()?;
        Ok(())
    }
    pub async fn set_redemption_status(&self, r: &RedemptionRef, fulfilled: bool) -> Result<()> {
        let (b, _) = self.me().await?;
        let response = self
            .request(
                Method::PATCH,
                REDEMPTIONS,
                &[
                    ("id", &r.redemption_id),
                    ("broadcaster_id", &b),
                    ("reward_id", &r.reward_id),
                ],
                Some(json!({"status":if fulfilled {"FULFILLED"}else{"CANCELED"}})),
            )
            .await?;
        let status = response.status();
        if status == 404 {
            return Ok(());
        }
        if status == 400 {
            let v: Value = response.json().await?;
            if v["message"]
                .as_str()
                .is_some_and(|s| s.to_uppercase().contains("UNFULFILLED"))
            {
                return Ok(());
            }
            return Err(anyhow!("Twitch redemption update failed (400)"));
        }
        response.error_for_status()?;
        Ok(())
    }
    pub async fn unfulfilled(&self, reward_id: &str) -> Result<Vec<Redemption>> {
        let (b, _) = self.me().await?;
        let mut after = String::new();
        let mut rows = Vec::new();
        loop {
            let mut q = vec![
                ("broadcaster_id", b.as_str()),
                ("reward_id", reward_id),
                ("status", "UNFULFILLED"),
                ("first", "50"),
                ("sort", "OLDEST"),
            ];
            if !after.is_empty() {
                q.push(("after", &after));
            }
            let v: Value = self
                .request(Method::GET, REDEMPTIONS, &q, None)
                .await?
                .error_for_status()?
                .json()
                .await?;
            rows.extend(
                serde_json::from_value::<Vec<ApiRedemption>>(v["data"].clone())?
                    .into_iter()
                    .map(Redemption::from),
            );
            let next = v["pagination"]["cursor"].as_str().unwrap_or_default();
            if next.is_empty() {
                break;
            }
            if next == after {
                return Err(anyhow!("Repeated Twitch pagination cursor"));
            }
            after = next.into();
        }
        Ok(rows)
    }
    pub async fn chat(&self, text: &str) -> Result<()> {
        let (b, _) = self.me().await?;
        let v: Value = self
            .request(
                Method::POST,
                "/helix/chat/messages",
                &[],
                Some(json!({"broadcaster_id":b,"sender_id":b,"message":text})),
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        if v["data"][0]["is_sent"] != true {
            return Err(anyhow!("Twitch chat message was not sent"));
        }
        Ok(())
    }
    pub async fn subscribe_redemptions(&self, session_id: &str) -> Result<()> {
        let (b, _) = self.me().await?;
        self.request(Method::POST,"/helix/eventsub/subscriptions",&[],Some(json!({"type":"channel.channel_points_custom_reward_redemption.add","version":"1","condition":{"broadcaster_user_id":b},"transport":{"method":"websocket","session_id":session_id}}))).await?.error_for_status()?;
        Ok(())
    }
}
fn string(v: &Value) -> Result<String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("Missing Twitch response field"))
}
