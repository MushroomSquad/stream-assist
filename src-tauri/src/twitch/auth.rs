use crate::model::Tokens;
use anyhow::{Result, bail};
use serde::Deserialize;

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct DeviceStart {
    #[serde(skip_serializing)]
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}
async fn token(form: &[(&str, &str)], base: &str, pending: bool) -> Result<Option<Tokens>> {
    let response = crate::http::http_client()
        .post(format!("{}/oauth2/token", base.trim_end_matches('/')))
        .form(form)
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        if pending
            && status == 400
            && (body["message"] == "authorization_pending"
                || body["error"] == "authorization_pending")
        {
            return Ok(None);
        }
        bail!("Twitch token request failed ({status})");
    }
    let t: TokenResponse = response.json().await?;
    Ok(Some(Tokens {
        access_token: t.access_token,
        refresh_token: t.refresh_token,
        expires_at: now() + t.expires_in,
    }))
}
pub async fn device_start(client_id: &str, scopes: &str, base: &str) -> Result<DeviceStart> {
    Ok(crate::http::http_client()
        .post(format!("{}/oauth2/device", base.trim_end_matches('/')))
        .form(&[("client_id", client_id), ("scopes", scopes)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
pub async fn device_poll(
    client_id: &str,
    device_code: &str,
    scopes: &str,
    base: &str,
) -> Result<Option<Tokens>> {
    token(
        &[
            ("client_id", client_id),
            ("device_code", device_code),
            ("scopes", scopes),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ],
        base,
        true,
    )
    .await
}
pub async fn refresh(client_id: &str, refresh_token: &str, base: &str) -> Result<Tokens> {
    token(
        &[
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ],
        base,
        false,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("Missing Twitch tokens"))
}
