use anyhow::Result;
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
    let client = Client::connect(&cfg.host, cfg.port, cfg.password.as_deref()).await?;
    Ok(client.general().version().await?.obs_version.to_string())
}

pub async fn add_widget_to_current_scene(cfg: &ObsCfg, url: &str, w: u32, h: u32) -> Result<()> {
    const NAME: &str = "Stream Assist — Music";
    let client = Client::connect(&cfg.host, cfg.port, cfg.password.as_deref()).await?;
    let scene = client.scenes().current_program_scene().await?;
    let settings = serde_json::json!({"url": url, "width": w, "height": h, "reroute_audio": true});
    let exists = client
        .inputs()
        .list(None)
        .await?
        .iter()
        .any(|input| input.id.name == NAME);
    if exists {
        client
            .inputs()
            .set_settings(SetSettings {
                input: NAME.into(),
                settings: &settings,
                overlay: Some(true),
            })
            .await?;
        let in_scene = client
            .scene_items()
            .list(scene.id.uuid.into())
            .await?
            .iter()
            .any(|item| item.source_name == NAME);
        if !in_scene {
            client
                .scene_items()
                .create(CreateSceneItem {
                    scene: scene.id.uuid.into(),
                    source: NAME.into(),
                    enabled: Some(true),
                })
                .await?;
        }
    } else {
        client
            .inputs()
            .create(Create {
                scene: scene.id.uuid.into(),
                input: NAME,
                kind: "browser_source",
                settings: Some(&settings),
                enabled: Some(true),
            })
            .await?;
    }
    client
        .inputs()
        .set_audio_monitor_type(NAME.into(), MonitorType::MonitorAndOutput)
        .await?;
    Ok(())
}
