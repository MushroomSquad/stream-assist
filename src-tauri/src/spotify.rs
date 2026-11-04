use crate::model::{PlayError, PlaybackSnapshot, Source, Tokens, Track};
use anyhow::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use reqwest::{Client, Method, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
fn random_string(n: usize) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    (0..n)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect()
}
pub fn new_pkce() -> Pkce {
    let verifier = random_string(64);
    Pkce {
        challenge: challenge(&verifier),
        verifier,
        state: random_string(32),
    }
}
pub fn authorize_url(client_id: &str, redirect: &str, p: &Pkce) -> String {
    let mut url = url::Url::parse("https://accounts.spotify.com/authorize").unwrap();
    url.query_pairs_mut().extend_pairs([("client_id",client_id),("response_type","code"),("redirect_uri",redirect),("code_challenge_method","S256"),("code_challenge",&p.challenge),("state",&p.state),("scope","user-read-playback-state user-modify-playback-state user-read-currently-playing user-read-private")]);
    url.into()
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}
#[derive(Debug, Deserialize)]
pub struct Device {
    pub id: Option<String>,
    pub is_active: bool,
    #[serde(rename = "type")]
    pub kind: String,
}
pub struct Spotify {
    client: Client,
    api: String,
    accounts: String,
    client_id: String,
    tokens: Arc<Mutex<Tokens>>,
    on_rotate: Arc<dyn Fn(&Tokens) + Send + Sync>,
}
impl Spotify {
    pub fn new(
        client_id: String,
        tokens: Tokens,
        api: &str,
        accounts: &str,
        on_rotate: impl Fn(&Tokens) + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: crate::http::http_client(),
            api: api.trim_end_matches('/').into(),
            accounts: accounts.trim_end_matches('/').into(),
            client_id,
            tokens: Arc::new(Mutex::new(tokens)),
            on_rotate: Arc::new(on_rotate),
        }
    }
    pub async fn exchange_code(
        client_id: &str,
        code: &str,
        redirect: &str,
        verifier: &str,
        accounts: &str,
    ) -> Result<Tokens> {
        let t: TokenResponse = crate::http::http_client()
            .post(format!("{}/api/token", accounts.trim_end_matches('/')))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("code", code),
                ("redirect_uri", redirect),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(anyhow::Error::from)
            .inspect_err(|e| {
                crate::dlog!(
                    "spotify",
                    "exchange_code error={}",
                    crate::diag::http_err(e)
                );
            })?
            .error_for_status()
            .map_err(anyhow::Error::from)
            .inspect_err(|e| {
                crate::dlog!(
                    "spotify",
                    "exchange_code error={}",
                    crate::diag::http_err(e)
                );
            })?
            .json()
            .await
            .map_err(anyhow::Error::from)
            .inspect_err(|e| {
                crate::dlog!(
                    "spotify",
                    "exchange_code error={}",
                    crate::diag::http_err(e)
                );
            })?;
        Ok(Tokens {
            access_token: t.access_token,
            refresh_token: t
                .refresh_token
                .ok_or_else(|| anyhow::anyhow!("Missing refresh token"))?,
            expires_at: now() + t.expires_in,
        })
    }
    async fn auth(&self, rejected: Option<&str>) -> Result<String> {
        let tokens = self.tokens.clone();
        let on_rotate = self.on_rotate.clone();
        let client_id = self.client_id.clone();
        let rejected = rejected.map(str::to_owned);
        let client = self.client.clone();
        let accounts = self.accounts.clone();
        tokio::spawn(async move {
            let mut t = tokens.lock().await;
            if (rejected.is_none() && t.expires_at - now() < 300)
                || rejected.as_deref().is_some_and(|old| old == t.access_token)
            {
                let next: TokenResponse = client
                    .post(format!("{}/api/token", accounts))
                    .form(&[
                        ("grant_type", "refresh_token"),
                        ("client_id", &client_id),
                        ("refresh_token", &t.refresh_token),
                    ])
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                t.access_token = next.access_token;
                if let Some(refresh) = next.refresh_token {
                    t.refresh_token = refresh;
                }
                t.expires_at = now() + next.expires_in;
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
        body: Option<serde_json::Value>,
    ) -> Result<Response> {
        let mut token = self.auth(None).await.inspect_err(|e| {
            crate::dlog!("spotify", "authorize error={}", crate::diag::http_err(e));
        })?;
        for attempt in 0..2 {
            let mut req = self
                .client
                .request(method.clone(), format!("{}{}", self.api, path))
                .bearer_auth(&token)
                .query(query);
            if let Some(ref b) = body {
                req = req.json(b);
            }
            let response = req.send().await.map_err(|e| {
                let e = anyhow::Error::from(e);
                crate::dlog!(
                    "spotify",
                    "request method={method} operation={path} error={}",
                    crate::diag::http_err(&e)
                );
                e
            })?;
            if let Err(e) = response.error_for_status_ref() {
                crate::dlog!(
                    "spotify",
                    "request method={method} operation={path} error={}",
                    crate::diag::http_err(&e.into())
                );
            }
            if response.status() != 401 || attempt == 1 {
                return Ok(response);
            }
            token = self.auth(Some(&token)).await.inspect_err(|e| {
                crate::dlog!("spotify", "refresh error={}", crate::diag::http_err(e));
            })?;
        }
        unreachable!()
    }
    pub async fn me_product(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct Me {
            product: String,
        }
        Ok(self
            .request(Method::GET, "/v1/me", &[], None)
            .await?
            .error_for_status()?
            .json::<Me>()
            .await?
            .product)
    }
    pub async fn search_track(&self, q: &str) -> Result<Vec<Track>> {
        #[derive(Deserialize)]
        struct Items {
            items: Vec<ApiTrack>,
        }
        #[derive(Deserialize)]
        struct Search {
            tracks: Items,
        }
        Ok(self
            .request(
                Method::GET,
                "/v1/search",
                &[
                    ("q", q),
                    ("type", "track"),
                    ("limit", "5"),
                    ("market", "from_token"),
                ],
                None,
            )
            .await?
            .error_for_status()?
            .json::<Search>()
            .await?
            .tracks
            .items
            .into_iter()
            .map(Into::into)
            .collect())
    }
    pub async fn track(&self, id: &str) -> Result<Track> {
        Ok(self
            .request(
                Method::GET,
                &format!(
                    "/v1/tracks/{}",
                    url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
                ),
                &[],
                None,
            )
            .await?
            .error_for_status()?
            .json::<ApiTrack>()
            .await?
            .into())
    }
    pub async fn player(&self) -> Result<Option<PlaybackSnapshot>> {
        let r = self
            .request(Method::GET, "/v1/me/player", &[], None)
            .await?
            .error_for_status()?;
        if r.status() == 204 {
            return Ok(None);
        }
        #[derive(Deserialize)]
        struct DeviceId {
            id: Option<String>,
        }
        #[derive(Deserialize)]
        struct Item {
            uri: Option<String>,
            duration_ms: u64,
        }
        #[derive(Deserialize)]
        struct Player {
            device: Option<DeviceId>,
            is_playing: bool,
            progress_ms: Option<u64>,
            item: Option<Item>,
        }
        let p = r.json::<Player>().await?;
        Ok(Some(PlaybackSnapshot {
            device_id: p.device.and_then(|d| d.id),
            is_playing: p.is_playing,
            track_uri: p.item.as_ref().and_then(|i| i.uri.clone()),
            progress_ms: p.progress_ms.unwrap_or(0),
            duration_ms: p.item.map_or(0, |i| i.duration_ms),
        }))
    }
    pub async fn devices(&self) -> Result<Vec<Device>> {
        #[derive(Deserialize)]
        struct Devices {
            devices: Vec<Device>,
        }
        Ok(self
            .request(Method::GET, "/v1/me/player/devices", &[], None)
            .await?
            .error_for_status()?
            .json::<Devices>()
            .await?
            .devices)
    }
    pub async fn play(
        &self,
        uri: &str,
        device_id: Option<&str>,
        position_ms: u64,
    ) -> Result<(), PlayError> {
        let query = device_id
            .map(|d| vec![("device_id", d)])
            .unwrap_or_default();
        let r = self
            .request(
                Method::PUT,
                "/v1/me/player/play",
                &query,
                Some(serde_json::json!({"uris":[uri],"position_ms":position_ms})),
            )
            .await
            .map_err(|_| PlayError::Net)?;
        let status = r.status();
        if status.is_success() {
            return Ok(());
        }
        if status == 401 {
            return Err(PlayError::Unauthorized);
        }
        if status == 404 {
            let v = r.json::<serde_json::Value>().await.unwrap_or_default();
            if v["error"]["reason"] == "NO_ACTIVE_DEVICE" {
                return Err(PlayError::NoDevice);
            }
        }
        Err(PlayError::Api(status.as_u16()))
    }
    pub async fn pause(&self) -> Result<()> {
        self.request(Method::PUT, "/v1/me/player/pause", &[], None)
            .await?
            .error_for_status()?;
        Ok(())
    }
    pub async fn resume(&self) -> Result<()> {
        self.request(Method::PUT, "/v1/me/player/play", &[], None)
            .await?
            .error_for_status()?;
        Ok(())
    }
    pub async fn volume(&self) -> Result<Option<u8>> {
        let response = self
            .request(Method::GET, "/v1/me/player", &[], None)
            .await?
            .error_for_status()?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let state: serde_json::Value = response.json().await?;
        Ok(state["device"]["volume_percent"]
            .as_u64()
            .filter(|v| *v <= 100)
            .map(|v| v as u8))
    }
    pub async fn set_volume(&self, pct: u8) -> Result<()> {
        self.request(
            Method::PUT,
            "/v1/me/player/volume",
            &[("volume_percent", &pct.to_string())],
            None,
        )
        .await?
        .error_for_status()?;
        Ok(())
    }
}
#[derive(Deserialize)]
struct Artist {
    name: String,
}
#[derive(Deserialize)]
struct Image {
    url: String,
    width: Option<u64>,
    height: Option<u64>,
}
#[derive(Deserialize)]
struct Album {
    images: Vec<Image>,
}
#[derive(Deserialize)]
struct ApiTrack {
    uri: String,
    name: String,
    artists: Vec<Artist>,
    duration_ms: Option<u64>,
    explicit: Option<bool>,
    album: Album,
}
impl From<ApiTrack> for Track {
    fn from(t: ApiTrack) -> Self {
        Self {
            source: Source::Spotify { uri: t.uri },
            title: t.name,
            artist: t
                .artists
                .into_iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(", "),
            duration_ms: t.duration_ms,
            explicit: t.explicit,
            cover_url: t
                .album
                .images
                .into_iter()
                .max_by_key(|i| i.width.unwrap_or(0).saturating_mul(i.height.unwrap_or(0)))
                .map(|i| i.url),
        }
    }
}
pub fn pick_device(devs: &[Device]) -> Option<String> {
    devs.iter()
        .filter(|d| d.id.is_some())
        .find(|d| d.is_active)
        .or_else(|| devs.iter().find(|d| d.id.is_some() && d.kind == "Computer"))
        .or_else(|| devs.iter().find(|d| d.id.is_some()))
        .and_then(|d| d.id.clone())
}
#[cfg(test)]
mod tests;

pub fn take_pending_if_state_matches(pending: &mut Option<Pkce>, state: &str) -> Option<Pkce> {
    if pending.as_ref().is_some_and(|p| p.state == state) {
        pending.take()
    } else {
        None
    }
}
