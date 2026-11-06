//! Linux: cargo run --example spotify_local_probe -- spotify:track:<id>
//! With no argument, only reads the local player. A URI explicitly exercises playback controls.
#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use stream_assist_lib::{model::Tokens, spotify::Spotify, spotify_local::DEVICE};
    let client = Spotify::new(
        String::new(),
        Tokens {
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: i64::MAX,
        },
        "http://127.0.0.1:1",
        "http://127.0.0.1:1",
        |_| {},
    );
    client.set_local_playback(true);
    println!("Before: {:?}", client.player().await?);
    if let Some(uri) = std::env::args().nth(1) {
        client
            .play_confirmed(&uri, Some(DEVICE), 0)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        println!("Confirmed: {:?}", client.player().await?);
        client.pause().await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(!client.player().await?.unwrap().is_playing);
        client.resume().await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(client.player().await?.unwrap().is_playing);
        let volume = client.volume().await?.unwrap();
        client.set_volume(volume).await?;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(client.volume().await?, Some(volume));
        client
            .play_confirmed(&uri, Some(DEVICE), 7000)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let state = client.player().await?.unwrap();
        assert!(state.progress_ms >= 6500, "seek failed: {state:?}");
        println!("Playback, pause, resume, volume and seek verified: {state:?}");
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() {
    println!("Local Spotify control is supported on Linux.");
}
