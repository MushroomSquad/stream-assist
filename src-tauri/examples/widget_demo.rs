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
        ],
        progress_ms: 0,
        mode: "Playing".into(),
        warning: None,
    };
    let (tx, rx) = tokio::sync::watch::channel(state.clone());
    let mut channels = server::serve(
        47821,
        PathBuf::from("themes"),
        Arc::new(RwLock::new("underground".into())),
        rx,
    )
    .await?;
    println!("http://127.0.0.1:47821/widget/music");
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {_=tokio::signal::ctrl_c()=>break,_=channels.yt_rx.recv()=>{},_=channels.oauth_rx.recv()=>{},_=interval.tick()=>{state.progress_ms=(state.progress_ms+1000)%180_000;tx.send_replace(state.clone());let _=channels.out_tx.send(WsOut::State(state.clone()));}}
    }
    Ok(())
}
