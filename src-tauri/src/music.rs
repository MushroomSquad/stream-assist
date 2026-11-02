use crate::{
    i18n,
    limits::{self, Limits},
    model::{QueueItem, Source, Track},
    request::RequestInput,
    spotify::Spotify,
    youtube,
};
use anyhow::Result;
pub enum Resolution {
    Candidates(Vec<Track>),
    NotFound,
}
pub enum Decision {
    Enqueue(Track),
    Reject(String),
}
pub fn normalize_title(s: &str) -> String {
    let mut depth = 0usize;
    let clean: String = s
        .to_lowercase()
        .chars()
        .filter(|c| match c {
            '(' | '[' => {
                depth += 1;
                false
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect();
    clean
        .replace("official video", "")
        .split_whitespace()
        .filter(|w| !matches!(*w, "audio" | "lyrics" | "mv" | "hd"))
        .collect::<Vec<_>>()
        .join(" ")
}
pub async fn resolve(input: RequestInput, sp: &Spotify, yt_base: &str) -> Result<Resolution> {
    let tracks = match input {
        RequestInput::Empty => vec![],
        RequestInput::Query(q) => sp.search_track(&q).await?.into_iter().take(5).collect(),
        RequestInput::SpotifyTrack(id) => vec![sp.track(&id).await?],
        RequestInput::YouTube(id) => {
            let Some(t) = youtube::oembed(&id, yt_base).await? else {
                return Ok(Resolution::NotFound);
            };
            let matches = sp
                .search_track(&format!("{} {}", t.artist, normalize_title(&t.title)))
                .await?;
            let found = matches.into_iter().find(|m| {
                normalize_title(&m.title) == normalize_title(&t.title)
                    && normalize_title(&m.artist) == normalize_title(&t.artist)
            });
            vec![found.unwrap_or(t)]
        }
    };
    Ok(if tracks.is_empty() {
        Resolution::NotFound
    } else {
        Resolution::Candidates(tracks)
    })
}
pub fn decide(
    res: Resolution,
    limits: &Limits,
    requester: &str,
    queue: &[QueueItem],
    current: Option<&QueueItem>,
    recent: &[Source],
) -> Decision {
    let mut first = None;
    if let Resolution::Candidates(ts) = res {
        for t in ts {
            match limits::check(limits, &t, requester, queue, current, recent) {
                Ok(()) => return Decision::Enqueue(t),
                Err(v) => {
                    first.get_or_insert(v);
                }
            }
        }
    }
    Decision::Reject(
        first
            .as_ref()
            .map(i18n::violation_text)
            .unwrap_or(i18n::TEXT_NOT_FOUND)
            .into(),
    )
}
