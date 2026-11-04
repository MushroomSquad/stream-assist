use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};
use stream_assist_lib::{
    model::{QueueItem, Source, Track},
    player::WidgetState,
    server::{self, WsOut},
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let item = |id, artist: &str, title: &str, user: &str| QueueItem {
        id,
        track: Track {
            source: Source::YouTube {
                video_id: "demo".into(),
            },
            title: title.into(),
            artist: artist.into(),
            duration_ms: Some(180_000),
            explicit: None,
            cover_url: None,
        },
        requester: user.into(),
        redemption: None,
        started: id == 1,
    };
    // Select a built-in theme for browser previews with THEME=glass (etc.).
    let theme = std::env::var("THEME").unwrap_or_else(|_| "underground".into());
    let mut state = WidgetState {
        visible: true,
        now: Some(item(
            1,
            "Snoop Dogg feat. The Doors",
            "Riders on the Storm (Fredwreck Remix)",
            "shadow_drift",
        )),
        queue: vec![
            item(2, "Lil Jon", "Get Low", "shadow_drift"),
            item(3, "Chingy", "I Do", "nitro_kid"),
            item(4, "Xzibit", "Hey Now", "bayview99"),
            item(5, "The Doors", "Riders on the Storm", "driver"),
            item(6, "Snoop Dogg", "Who Am I", "racer"),
        ],
        progress_ms: 0,
        mode: std::env::var("MODE").unwrap_or_else(|_| "Playing".into()),
        warning: None,
    };
    let (tx, rx) = tokio::sync::watch::channel(state.clone());
    let port = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(47821);
    let mut channels = server::serve(
        port,
        PathBuf::from("themes"),
        Arc::new(RwLock::new(theme)),
        rx,
        PathBuf::from("voice-lab/tmp/tts"),
        server::noop_debug(),
    )
    .await?;
    println!(
        "http://127.0.0.1:{port}/widget/music?part={}&scale={}",
        std::env::var("PART").unwrap_or_else(|_| "all".into()),
        std::env::var("SCALE").unwrap_or_else(|_| "1".into())
    );
    // PROGRESS=<ms> freezes the bar for deterministic screenshots.
    let frozen: Option<u64> = std::env::var("PROGRESS").ok().and_then(|v| v.parse().ok());
    let cycle_demo = std::env::var("DEMO").is_ok_and(|value| value == "cycle");
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(if cycle_demo {
        2500
    } else {
        1000
    }));
    let mut tick = 0_u64;
    let mut next_id = 7;
    let titles = [
        ("Lil Jon", "Get Low"),
        ("Chingy", "I Do"),
        ("Xzibit", "Hey Now"),
        ("The Doors", "Riders on the Storm"),
        ("Snoop Dogg", "Who Am I"),
    ];
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = channels.yt_rx.recv() => {},
            _ = channels.oauth_rx.recv() => {},
            _ = interval.tick() => {
                if cycle_demo {
                    let cycle = tick / 3 + 1;
                    if tick.is_multiple_of(3) {
                        let mut now = state.queue.remove(0);
                        now.started = true;
                        state.now = Some(now);
                        state.progress_ms = 0;
                        let additions = (cycle % 3) as usize;
                        let additions = additions.max(2_usize.saturating_sub(state.queue.len()));
                        for _ in 0..additions {
                            if state.queue.len() == 7 {
                                break;
                            }
                            let (artist, title) = titles[(next_id as usize - 7) % titles.len()];
                            state.queue.push(item(next_id, artist, title, "demo_listener"));
                            next_id += 1;
                        }
                        state.mode = if cycle.is_multiple_of(4) { "Paused" } else { "Playing" }.into();
                    } else if state.mode == "Playing" {
                        state.progress_ms += 2500;
                    }
                    state.visible = !(cycle.is_multiple_of(5) && tick.is_multiple_of(3));
                    tick += 1;
                } else {
                    state.progress_ms = frozen.unwrap_or((state.progress_ms + 1000) % 180_000);
                }
                tx.send_replace(state.clone());
                let _ = channels.out_tx.send(WsOut::State(state.clone()));
            }
        }
    }
    Ok(())
}
