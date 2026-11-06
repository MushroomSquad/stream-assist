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

fn named_query(query: &str) -> Option<(&str, &str)> {
    [" — ", " – ", " - "].into_iter().find_map(|separator| {
        let (artist, title) = query.split_once(separator)?;
        let (artist, title) = (artist.trim(), title.trim());
        (!artist.is_empty() && !title.is_empty()).then_some((artist, title))
    })
}

fn same_name(a: &str, b: &str) -> bool {
    normalize_title(a).replace('ё', "е") == normalize_title(b).replace('ё', "е")
}

fn select_requested_track(query: &str, candidates: Vec<Track>) -> Vec<Track> {
    match named_query(query) {
        Some((artist, title)) => candidates
            .into_iter()
            .filter(|track| {
                same_name(&track.title, title)
                    && (same_name(&track.artist, artist)
                        || track.artist.split(", ").any(|name| same_name(name, artist)))
            })
            .take(1)
            .collect(),
        None => candidates.into_iter().take(1).collect(),
    }
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
        RequestInput::Query(q) => {
            let search = match named_query(&q) {
                Some((artist, title)) => {
                    let literal = |value: &str| value.replace(['"', '\\'], " ");
                    format!(
                        "artist:\"{}\" track:\"{}\"",
                        literal(artist),
                        literal(title)
                    )
                }
                None => q.clone(),
            };
            let candidates = sp
                .ok_or_else(|| anyhow::anyhow!("Spotify disconnected"))?
                .search_track(&search)
                .await?;
            select_requested_track(&q, candidates)
        }
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
    if let Resolution::Candidates(ts) = res {
        if let Some(t) = ts.into_iter().next() {
            match limits::check(limits, &t, requester, queue, current, recent) {
                Ok(()) => return Decision::Enqueue(t),
                Err(v) => return Decision::Reject(i18n::violation_text(&v).into()),
            }
        }
    }
    Decision::Reject(i18n::TEXT_NOT_FOUND.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track(title: &str, artist: &str) -> Track {
        Track {
            source: Source::Spotify { uri: title.into() },
            title: title.into(),
            artist: artist.into(),
            duration_ms: Some(1000),
            explicit: None,
            cover_url: None,
        }
    }
    #[test]
    fn explicit_request_selects_requested_song_instead_of_first_artist_hit() {
        let selected = select_requested_track(
            "ПАНЦУШОТ — Семенем",
            vec![
                track("Девочка из меда", "ПАНЦУШОТ, KSB muzic"),
                track("Семенем", "ПАНЦУШОТ"),
            ],
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].title, "Семенем");
    }
    #[test]
    fn explicit_request_rejects_different_song_or_artist() {
        for candidate in [
            track("Девочка из меда", "ПАНЦУШОТ"),
            track("Семенем", "Другой исполнитель"),
        ] {
            assert!(select_requested_track("ПАНЦУШОТ — Семенем", vec![candidate]).is_empty());
        }
    }
    #[test]
    fn repeat_does_not_fall_through_to_another_song() {
        let wanted = track("Семенем", "ПАНЦУШОТ");
        assert!(
            matches!(decide(Resolution::Candidates(vec![wanted.clone(), track("Девочка из меда", "ПАНЦУШОТ")]),
            &Limits::default(), "viewer", &[], None, &[wanted.source]), Decision::Reject(reason)
            if reason == i18n::violation_text(&limits::LimitViolation::Repeat))
        );
    }
    #[test]
    fn query_separators_keep_hyphens_inside_artist_names() {
        for query in [
            "AC-DC — Thunderstruck",
            "AC-DC – Thunderstruck",
            "AC-DC - Thunderstruck",
        ] {
            assert_eq!(named_query(query), Some(("AC-DC", "Thunderstruck")));
        }
        assert_eq!(named_query("Thunderstruck"), None);
    }
}
