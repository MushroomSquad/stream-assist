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

pub async fn add_widget_to_current_scene(
    cfg: &ObsCfg,
    url: &str,
    parts: &crate::server::themes::ThemeParts,
    scale: f64,
) -> Result<()> {
    let client = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Client::connect(&cfg.host, cfg.port, cfg.password.as_deref()),
    )
    .await??;
    let scene = request(client.scenes().current_program_scene()).await?;
    for (name, part, size, audio) in [
        ("Stream Assist — Сейчас играет", "now", &parts.now, true),
        ("Stream Assist — Очередь", "queue", &parts.queue, false),
    ] {
        let settings = serde_json::json!({"url": format!("{url}?part={part}&scale={scale}"), "width": (size.width as f64 * scale).ceil() as u32, "height": (size.height as f64 * scale).ceil() as u32, "reroute_audio": audio});
        let exists = request(client.inputs().list(None))
            .await?
            .iter()
            .any(|input| input.id.name == name);
        if exists {
            request(client.inputs().set_settings(SetSettings {
                input: name.into(),
                settings: &settings,
                overlay: Some(true),
            }))
            .await?;
            let in_scene = request(client.scene_items().list(scene.id.uuid.into()))
                .await?
                .iter()
                .any(|item| item.source_name == name);
            if !in_scene {
                request(client.scene_items().create(CreateSceneItem {
                    scene: scene.id.uuid.into(),
                    source: name.into(),
                    enabled: Some(true),
                }))
                .await?;
            }
        } else {
            request(client.inputs().create(Create {
                scene: scene.id.uuid.into(),
                input: name,
                kind: "browser_source",
                settings: Some(&settings),
                enabled: Some(true),
            }))
            .await?;
        }
        if audio {
            request(
                client
                    .inputs()
                    .set_audio_monitor_type(name.into(), MonitorType::MonitorAndOutput),
            )
            .await?;
        }
    }
    Ok(())
}
