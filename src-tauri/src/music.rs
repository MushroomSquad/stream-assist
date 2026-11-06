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
    const NOISE: &[&str] = &[
        "official music video",
        "official video",
        "official audio",
        "lyric video",
        "lyrics",
        "mv",
        "hd",
    ];
    let lower = s.to_lowercase();
    let mut clean = String::new();
    let mut chars = lower.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '(' || c == '[' {
            let close = if c == '(' { ')' } else { ']' };
            let mut phrase = String::new();
            for ch in chars.by_ref() {
                if ch == close {
                    break;
                }
                phrase.push(ch);
            }
            if !NOISE.contains(&phrase.trim()) {
                clean.push(c);
                clean.push_str(&phrase);
                clean.push(close);
            }
        } else {
            clean.push(c);
        }
    }
    let mut clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let mut stripped = false;
        for phrase in NOISE {
            if let Some(prefix) = clean.strip_suffix(phrase) {
                if prefix.is_empty()
                    || prefix.ends_with(|c: char| {
                        c.is_whitespace() || matches!(c, '-' | '–' | '—' | '|')
                    })
                {
                    clean = prefix
                        .trim_end_matches(|c: char| {
                            c.is_whitespace() || matches!(c, '-' | '–' | '—' | '|')
                        })
                        .to_string();
                    stripped = true;
                    break;
                }
            }
        }
        if !stripped {
            break;
        }
    }
    clean
}

pub async fn resolve(
    input: RequestInput,
    sp: Option<&Spotify>,
    yt_base: &str,
) -> Result<Resolution> {
    let tracks = match input {
        RequestInput::Empty => vec![],
        RequestInput::Query(q) => sp
            .ok_or_else(|| anyhow::anyhow!("Spotify disconnected"))?
            .search_track(&q)
            .await?
            .into_iter()
            .take(5)
            .collect(),
        RequestInput::SpotifyTrack(id) => vec![
            sp.ok_or_else(|| anyhow::anyhow!("Spotify disconnected"))?
                .track(&id)
                .await?,
        ],
        RequestInput::YouTube(id) => {
            let Some(t) = youtube::oembed(&id, yt_base).await? else {
                return Ok(Resolution::NotFound);
            };
            // A direct link selects the source, including the particular video
            // version. Do not replace it with a Spotify search result.
            vec![t]
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
