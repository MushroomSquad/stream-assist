use anyhow::Result;
async fn request<T, E: Into<anyhow::Error>>(
    future: impl std::future::Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await?
        .map_err(Into::into)
}
use obws::{
    Client,
    common::MonitorType,
    requests::{
        inputs::{Create, SetSettings},
        scene_items::CreateSceneItem,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ObsCfg {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
}

impl Default for ObsCfg {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 4455,
            password: None,
        }
    }
}

pub async fn check(cfg: &ObsCfg) -> Result<String> {
    let client = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Client::connect(&cfg.host, cfg.port, cfg.password.as_deref()),
    )
    .await??;
    Ok(request(client.general().version())
        .await?
        .obs_version
        .to_string())
}

pub async fn add_widget_to_current_scene(cfg: &ObsCfg, url: &str, w: u32, h: u32) -> Result<()> {
    const NAME: &str = "Stream Assist — Music";
    let client = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Client::connect(&cfg.host, cfg.port, cfg.password.as_deref()),
    )
    .await??;
    let scene = request(client.scenes().current_program_scene()).await?;
    let settings = serde_json::json!({"url": url, "width": w, "height": h, "reroute_audio": true});
    let exists = request(client.inputs().list(None))
        .await?
        .iter()
        .any(|input| input.id.name == NAME);
    if exists {
        request(client.inputs().set_settings(SetSettings {
            input: NAME.into(),
            settings: &settings,
            overlay: Some(true),
        }))
        .await?;
        let in_scene = request(client.scene_items().list(scene.id.uuid.into()))
            .await?
            .iter()
            .any(|item| item.source_name == NAME);
        if !in_scene {
            request(client.scene_items().create(CreateSceneItem {
                scene: scene.id.uuid.into(),
                source: NAME.into(),
                enabled: Some(true),
            }))
            .await?;
        }
    } else {
        request(client.inputs().create(Create {
            scene: scene.id.uuid.into(),
            input: NAME,
            kind: "browser_source",
            settings: Some(&settings),
            enabled: Some(true),
        }))
        .await?;
    }
    request(
        client
            .inputs()
            .set_audio_monitor_type(NAME.into(), MonitorType::MonitorAndOutput),
    )
    .await?;
    Ok(())
}
