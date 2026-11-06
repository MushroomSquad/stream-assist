use crate::model::PlaybackSnapshot;
use anyhow::{Result, anyhow};
use dbus::{
    arg::{PropMap, RefArg},
    blocking::{Connection, stdintf::org_freedesktop_dbus::Properties},
};
use std::time::Duration;

pub const DEVICE: &str = "local-spotify-mpris";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

pub enum Command {
    State,
    Play(String, u64),
    Pause,
    Resume,
    Volume,
    SetVolume(u8),
}
pub enum Reply {
    State(PlaybackSnapshot),
    Volume(u8),
    Done,
}

fn snapshot(metadata: &PropMap, status: &str, position: i64) -> PlaybackSnapshot {
    let uri = metadata
        .get("xesam:url")
        .and_then(|v| v.0.as_str())
        .and_then(|url| {
            if url.starts_with("spotify:track:") {
                Some(url.to_string())
            } else {
                url::Url::parse(url).ok().and_then(|url| {
                    if url.host_str() != Some("open.spotify.com") {
                        return None;
                    }
                    url.path_segments()?
                        .collect::<Vec<_>>()
                        .windows(2)
                        .find(|s| s[0] == "track")
                        .map(|s| format!("spotify:track:{}", s[1]))
                })
            }
        });
    PlaybackSnapshot {
        device_id: Some(DEVICE.into()),
        is_playing: status == "Playing",
        track_uri: uri,
        progress_ms: position.max(0) as u64 / 1000,
        duration_ms: metadata
            .get("mpris:length")
            .and_then(|v| {
                v.0.as_i64()
                    .or_else(|| v.0.as_u64().and_then(|v| i64::try_from(v).ok()))
            })
            .unwrap_or(0)
            .max(0) as u64
            / 1000,
    }
}

pub async fn call(command: Command) -> Result<Reply> {
    tokio::task::spawn_blocking(move || {
        let connection = Connection::new_session()?;
        let proxy = connection.with_proxy(
            "org.mpris.MediaPlayer2.spotify",
            "/org/mpris/MediaPlayer2",
            Duration::from_secs(2),
        );
        match command {
            Command::State => {
                let metadata: PropMap = proxy.get(PLAYER, "Metadata")?;
                let status: String = proxy.get(PLAYER, "PlaybackStatus")?;
                let position: i64 = proxy.get(PLAYER, "Position")?;
                Ok(Reply::State(snapshot(&metadata, &status, position)))
            }
            Command::Play(uri, position_ms) => {
                if !uri.starts_with("spotify:track:") {
                    return Err(anyhow!("Invalid Spotify URI"));
                }
                let metadata: PropMap = proxy.get(PLAYER, "Metadata")?;
                let same = snapshot(&metadata, "", 0).track_uri.as_deref() == Some(&uri);
                if same {
                    let _: () = proxy.method_call(PLAYER, "Play", ())?;
                } else {
                    let _: () = proxy.method_call(PLAYER, "OpenUri", (uri.clone(),))?;
                }
                // OpenUri completes before the client has necessarily loaded metadata.
                // Verify the requested URI before seeking so another track is never sought.
                if position_ms > 0 || same {
                    for _ in 0..10 {
                        let metadata: PropMap = proxy.get(PLAYER, "Metadata")?;
                        if snapshot(&metadata, "", 0).track_uri.as_deref() == Some(&uri) {
                            if let Some(path) =
                                metadata.get("mpris:trackid").and_then(|v| v.0.as_str())
                            {
                                let path =
                                    dbus::Path::new(path.to_owned()).map_err(|e| anyhow!(e))?;
                                let position =
                                    position_ms.min(i64::MAX as u64 / 1000) as i64 * 1000;
                                let _: () =
                                    proxy.method_call(PLAYER, "SetPosition", (path, position))?;
                            }
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                Ok(Reply::Done)
            }
            Command::Pause => {
                let _: () = proxy.method_call(PLAYER, "Pause", ())?;
                Ok(Reply::Done)
            }
            Command::Resume => {
                let _: () = proxy.method_call(PLAYER, "Play", ())?;
                Ok(Reply::Done)
            }
            Command::Volume => {
                let volume: f64 = proxy.get(PLAYER, "Volume")?;
                Ok(Reply::Volume((volume.clamp(0.0, 1.0) * 100.0).round() as u8))
            }
            Command::SetVolume(volume) => {
                proxy.set(PLAYER, "Volume", f64::from(volume.min(100)) / 100.0)?;
                Ok(Reply::Done)
            }
        }
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_units_uri_and_paused_state() {
        let mut metadata = PropMap::new();
        metadata.insert(
            "xesam:url".into(),
            dbus::arg::Variant(Box::new("https://open.spotify.com/track/abc".to_owned())),
        );
        metadata.insert(
            "mpris:length".into(),
            dbus::arg::Variant(Box::new(158_919_000i64)),
        );
        let state = snapshot(&metadata, "Playing", 7_000_000);
        assert_eq!(state.track_uri.as_deref(), Some("spotify:track:abc"));
        assert_eq!(state.duration_ms, 158_919);
        assert_eq!(state.progress_ms, 7000);
        assert!(state.is_playing);
        assert!(!snapshot(&metadata, "Paused", -1).is_playing);
        assert_eq!(snapshot(&PropMap::new(), "Stopped", 0).track_uri, None);
    }
}
