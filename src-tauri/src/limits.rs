use crate::model::{QueueItem, Source, Track};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Limits {
    pub max_duration_ms: Option<u64>,
    pub no_repeat_last_n: usize,
    pub max_per_user_in_queue: Option<usize>,
    pub block_explicit: bool,
    pub blocklist: Vec<String>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_duration_ms: Some(420_000),
            no_repeat_last_n: 10,
            max_per_user_in_queue: Some(2),
            block_explicit: false,
            blocklist: vec![],
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum LimitViolation {
    TooLong,
    Repeat,
    UserQuota,
    Explicit,
    Blocked,
}

/// Checks limits with `recent` ordered oldest to newest; only its last
/// `no_repeat_last_n` entries are considered for recent repeats.
pub fn check(
    l: &Limits,
    t: &Track,
    requester: &str,
    queue: &[QueueItem],
    current: Option<&QueueItem>,
    recent: &[Source],
) -> Result<(), LimitViolation> {
    if let (Some(duration), Some(max)) = (t.duration_ms, l.max_duration_ms) {
        if duration > max {
            return Err(LimitViolation::TooLong);
        }
    }
    if queue.iter().any(|item| item.track.source == t.source)
        || current.is_some_and(|item| item.track.source == t.source)
        || recent
            .iter()
            .rev()
            .take(l.no_repeat_last_n)
            .any(|source| source == &t.source)
    {
        return Err(LimitViolation::Repeat);
    }
    if l.max_per_user_in_queue.is_some_and(|max| {
        queue
            .iter()
            .filter(|item| item.requester == requester)
            .count()
            >= max
    }) {
        return Err(LimitViolation::UserQuota);
    }
    if l.block_explicit && t.explicit == Some(true) {
        return Err(LimitViolation::Explicit);
    }
    let description = format!("{} — {}", t.artist, t.title).to_lowercase();
    if l.blocklist
        .iter()
        .any(|entry| !entry.trim().is_empty() && description.contains(&entry.to_lowercase()))
    {
        return Err(LimitViolation::Blocked);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            source: Source::YouTube {
                video_id: "id".into(),
            },
            title: "Title".into(),
            artist: "Artist".into(),
            duration_ms: Some(200_000),
            explicit: Some(false),
            cover_url: None,
        }
    }
    fn item(track: Track) -> QueueItem {
        QueueItem {
            id: 1,
            track,
            requester: "viewer".into(),
            redemption: None,
            started: false,
        }
    }
    #[test]
    fn too_long() {
        let l = Limits::default();
        assert_eq!(l.max_duration_ms, Some(420_000));
        assert_eq!(l.no_repeat_last_n, 10);
        assert_eq!(l.max_per_user_in_queue, Some(2));
        assert!(!l.block_explicit);
        assert!(l.blocklist.is_empty());
        let mut t = track();
        t.duration_ms = Some(420_001);
        assert_eq!(
            check(&l, &t, "viewer", &[], None, &[]),
            Err(LimitViolation::TooLong)
        );
        t.duration_ms = Some(420_000);
        assert_eq!(check(&l, &t, "viewer", &[], None, &[]), Ok(()));
        t.duration_ms = Some(u64::MAX);
        assert_eq!(
            check(
                &Limits {
                    max_duration_ms: None,
                    ..l
                },
                &t,
                "viewer",
                &[],
                None,
                &[]
            ),
            Ok(())
        );
    }
    #[test]
    fn repeat_in_queue() {
        let t = track();
        assert_eq!(
            check(
                &Limits::default(),
                &t,
                "other",
                &[item(t.clone())],
                None,
                &[]
            ),
            Err(LimitViolation::Repeat)
        );
    }
    #[test]
    fn repeat_in_recent() {
        let t = track();
        let l = Limits {
            no_repeat_last_n: 1,
            ..Limits::default()
        };
        let other = Source::YouTube {
            video_id: "other".into(),
        };
        assert_eq!(
            check(
                &l,
                &t,
                "viewer",
                &[],
                None,
                &[other.clone(), t.source.clone()]
            ),
            Err(LimitViolation::Repeat)
        );
        assert_eq!(
            check(&l, &t, "viewer", &[], None, &[t.source.clone(), other]),
            Ok(())
        );
        assert_eq!(
            check(
                &Limits {
                    no_repeat_last_n: 0,
                    ..l
                },
                &t,
                "viewer",
                &[],
                None,
                &[t.source.clone()]
            ),
            Ok(())
        );
    }
    #[test]
    fn repeat_is_current() {
        let t = track();
        assert_eq!(
            check(
                &Limits::default(),
                &t,
                "other",
                &[],
                Some(&item(t.clone())),
                &[]
            ),
            Err(LimitViolation::Repeat)
        );
    }
    #[test]
    fn user_quota() {
        let mut t = track();
        t.source = Source::YouTube {
            video_id: "new".into(),
        };
        let q = [item(track()), item(track())];
        assert_eq!(
            check(&Limits::default(), &t, "viewer", &q, None, &[]),
            Err(LimitViolation::UserQuota)
        );
        assert_eq!(
            check(&Limits::default(), &t, "other", &q, None, &[]),
            Ok(())
        );
        assert_eq!(
            check(
                &Limits {
                    max_per_user_in_queue: None,
                    ..Limits::default()
                },
                &t,
                "viewer",
                &q,
                None,
                &[]
            ),
            Ok(())
        );
        assert_eq!(
            check(&Limits::default(), &t, "viewer", &q[..1], Some(&q[1]), &[]),
            Ok(())
        );
    }
    #[test]
    fn explicit_blocked_only_when_flag_true() {
        let mut t = track();
        t.explicit = Some(true);
        assert_eq!(
            check(&Limits::default(), &t, "viewer", &[], None, &[]),
            Ok(())
        );
        let l = Limits {
            block_explicit: true,
            ..Limits::default()
        };
        assert_eq!(
            check(&l, &t, "viewer", &[], None, &[]),
            Err(LimitViolation::Explicit)
        );
        for explicit in [None, Some(false)] {
            t.explicit = explicit;
            assert_eq!(check(&l, &t, "viewer", &[], None, &[]), Ok(()));
        }
    }
    #[test]
    fn unknown_duration_passes() {
        let mut t = track();
        t.duration_ms = None;
        assert_eq!(
            check(&Limits::default(), &t, "viewer", &[], None, &[]),
            Ok(())
        );
    }
    #[test]
    fn blocklist_ignores_blank_entries() {
        let l = Limits {
            blocklist: vec!["".into(), "  ".into()],
            ..Limits::default()
        };
        assert_eq!(check(&l, &track(), "viewer", &[], None, &[]), Ok(()));
    }
    #[test]
    fn blocklist_case_insensitive() {
        let mut t = track();
        t.artist = "Артист".into();
        let l = Limits {
            blocklist: vec!["ТИСТ — ti".into()],
            ..Limits::default()
        };
        assert_eq!(
            check(&l, &t, "viewer", &[], None, &[]),
            Err(LimitViolation::Blocked)
        );
        assert_eq!(check(&l, &track(), "viewer", &[], None, &[]), Ok(()));
    }
}
