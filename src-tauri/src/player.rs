use crate::model::{PlayError, PlaybackSnapshot, QueueItem, Source};
use serde::Serialize;

pub const PLAY_GRACE_MS: i64 = 4000;
pub const END_TOLERANCE_MS: u64 = 3000;
pub const POLL_MS: u64 = 2500;
pub const OUTAGE_MS: i64 = 30_000;
pub const YT_START_TIMEOUT_MS: i64 = 20_000;
pub const YT_END_SLACK_MS: i64 = 10_000;
pub const RETRY_BACKOFF_MS: [i64; 5] = [2000, 4000, 8000, 16000, 30000];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Idle,
    Starting,
    Playing,
    Paused,
    Suspended,
}
pub enum Input {
    Enqueue(QueueItem),
    Skip,
    Remove(i64),
    Move(i64, usize),
    ResumeQueue,
    TogglePause,
    SetMaxDuration(Option<u64>),
    PlayResult {
        item_id: i64,
        result: Result<(), PlayError>,
    },
    Spotify(Result<Option<PlaybackSnapshot>, ()>),
    Yt {
        item_id: i64,
        ev: YtEvent,
    },
    YtPlayerConnected,
    FallbackResolved {
        item_id: i64,
        video_id: Option<String>,
    },
    Restore {
        current: Option<QueueItem>,
        queue: Vec<QueueItem>,
    },
    Tick,
}
pub enum YtEvent {
    Started { duration_ms: u64 },
    Ended,
    Error,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Persist,
    SpotifyPlay {
        item_id: i64,
        uri: String,
        device_id: Option<String>,
        position_ms: u64,
    },
    SpotifyPause,
    SpotifyResume,
    YtPlay {
        item_id: i64,
        video_id: String,
        start_ms: u64,
    },
    YtUnmute,
    YtStop,
    YtPause,
    YtResume,
    ResolveFallback(QueueItem),
    Started(QueueItem),
    Cancel(QueueItem, CancelReason),
    Widget(WidgetState),
    Warn(Option<String>),
}
#[derive(Clone, Debug, PartialEq)]
pub enum CancelReason {
    RemovedByStreamer,
    TooLong,
    YtUnavailable,
}
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WidgetState {
    pub visible: bool,
    pub now: Option<QueueItem>,
    pub queue: Vec<QueueItem>,
    pub progress_ms: u64,
    pub mode: String,
    pub warning: Option<String>,
}
pub struct Player {
    current: Option<QueueItem>,
    queue: Vec<QueueItem>,
    mode: Mode,
    max_duration_ms: Option<u64>,
    last_device: Option<String>,
    last_progress: u64,
    anchor: i64,
    confirmed_at: i64,
    start_sent: i64,
    last_ok: Option<i64>,
    fallback: bool,
    resolving: bool,
    needs_unmute: bool,
    retry_at: Option<i64>,
    retry_index: usize,
    warning: Option<String>,
}
impl Player {
    pub fn new(max_duration_ms: Option<u64>) -> Self {
        Self {
            current: None,
            queue: vec![],
            mode: Mode::Idle,
            max_duration_ms,
            last_device: None,
            last_progress: 0,
            anchor: 0,
            confirmed_at: 0,
            start_sent: 0,
            last_ok: None,
            fallback: false,
            resolving: false,
            needs_unmute: false,
            retry_at: None,
            retry_index: 0,
            warning: None,
        }
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn snapshot(&self) -> (Option<QueueItem>, Vec<QueueItem>) {
        (self.current.clone(), self.queue.clone())
    }
    fn yt(&self) -> bool {
        matches!(
            self.current.as_ref().map(|i| &i.track.source),
            Some(Source::YouTube { .. })
        )
    }
    fn progress(&self, now: i64) -> u64 {
        self.last_progress
            .saturating_add(if self.mode == Mode::Playing {
                (now.saturating_sub(self.anchor)).max(0) as u64
            } else {
                0
            })
    }
    fn duration(&self) -> u64 {
        self.current
            .as_ref()
            .and_then(|i| i.track.duration_ms)
            .unwrap_or(0)
    }
    fn warn(&mut self, w: Option<String>, a: &mut Vec<Action>) {
        self.warning = w.clone();
        a.push(Action::Warn(w));
    }
    fn cancel(item: QueueItem, reason: CancelReason, a: &mut Vec<Action>) {
        if !item.started {
            a.push(Action::Cancel(item, reason));
        }
    }
    fn schedule(&mut self, now: i64) {
        self.retry_at = Some(now.saturating_add(RETRY_BACKOFF_MS[self.retry_index.min(4)]));
        self.retry_index = (self.retry_index + 1).min(4);
    }
    fn play_spotify(&self, a: &mut Vec<Action>) {
        if let Some(QueueItem {
            id,
            track:
                crate::model::Track {
                    source: Source::Spotify { uri },
                    ..
                },
            ..
        }) = &self.current
        {
            a.push(Action::SpotifyPlay {
                item_id: *id,
                uri: uri.clone(),
                device_id: self.last_device.clone(),
                position_ms: self.last_progress,
            });
        }
    }
    fn start(&mut self, now: i64, a: &mut Vec<Action>) {
        self.last_progress = 0;
        self.anchor = now;
        self.start_sent = now;
        self.retry_at = None;
        self.retry_index = 0;
        self.resolving = false;
        self.needs_unmute = false;
        let Some(i) = self.current.as_ref() else {
            self.mode = Mode::Idle;
            return;
        };
        self.mode = Mode::Starting;
        match &i.track.source {
            Source::Spotify { .. } if self.fallback => {
                self.resolving = true;
                a.push(Action::ResolveFallback(i.clone()));
            }
            Source::Spotify { .. } => self.play_spotify(a),
            Source::YouTube { video_id } => {
                a.push(Action::SpotifyPause);
                a.push(Action::YtPlay {
                    item_id: i.id,
                    video_id: video_id.clone(),
                    start_ms: 0,
                });
            }
        }
    }
    fn next(&mut self, now: i64, a: &mut Vec<Action>) {
        self.current = if self.queue.is_empty() {
            None
        } else {
            Some(self.queue.remove(0))
        };
        self.start(now, a);
    }
    fn confirmed(&mut self, now: i64, a: &mut Vec<Action>) {
        if let Some(i) = self.current.as_mut() {
            if !i.started {
                i.started = true;
                a.push(Action::Started(i.clone()));
            }
        }
        self.mode = Mode::Playing;
        self.anchor = now;
        self.confirmed_at = now;
        self.retry_at = None;
        self.retry_index = 0;
    }
    pub fn step(&mut self, input: Input, now_ms: i64) -> Vec<Action> {
        let now = now_ms;
        self.last_ok.get_or_insert(now);
        let before = self.snapshot();
        let old_mode = self.mode;
        let old_progress = self.last_progress;
        let old_warning = self.warning.clone();
        let mut a = vec![];
        match input {
            Input::Enqueue(i) => {
                if self.current.is_none() {
                    self.current = Some(i);
                    self.start(now, &mut a);
                } else {
                    self.queue.push(i);
                }
            }
            Input::SetMaxDuration(max) => self.max_duration_ms = max,
            Input::Move(id, pos) => {
                if let Some(n) = self.queue.iter().position(|i| i.id == id) {
                    let i = self.queue.remove(n);
                    self.queue.insert(pos.min(self.queue.len()), i);
                }
            }
            Input::Remove(id) => {
                if let Some(n) = self.queue.iter().position(|i| i.id == id) {
                    Self::cancel(
                        self.queue.remove(n),
                        CancelReason::RemovedByStreamer,
                        &mut a,
                    );
                }
            }
            Input::Skip => {
                if let Some(i) = self.current.take() {
                    a.push(if matches!(i.track.source, Source::YouTube { .. }) {
                        Action::YtStop
                    } else {
                        Action::SpotifyPause
                    });
                    Self::cancel(i, CancelReason::RemovedByStreamer, &mut a);
                    self.next(now, &mut a);
                }
            }
            Input::ResumeQueue => {
                if self.mode == Mode::Suspended {
                    self.next(now, &mut a);
                }
            }
            Input::TogglePause => {
                if matches!(self.mode, Mode::Playing | Mode::Paused) {
                    let pause = self.mode == Mode::Playing;
                    if pause {
                        self.last_progress = self.progress(now);
                        self.mode = Mode::Paused;
                    } else {
                        self.anchor = now;
                        self.mode = Mode::Playing;
                    }
                    if self.yt() && !pause && self.needs_unmute {
                        a.push(Action::YtUnmute);
                        self.needs_unmute = false;
                    }
                    a.push(match (self.yt(), pause) {
                        (true, true) => Action::YtPause,
                        (true, false) => Action::YtResume,
                        (false, true) => Action::SpotifyPause,
                        (false, false) => Action::SpotifyResume,
                    });
                }
            }
            Input::PlayResult { item_id, result } => {
                if self.current.as_ref().is_some_and(|i| i.id == item_id)
                    && !self.yt()
                    && !self.resolving
                {
                    match result {
                        Ok(()) if self.mode == Mode::Playing => {}
                        Ok(()) => self.confirmed(now, &mut a),
                        Err(e) => {
                            if e == PlayError::NoDevice {
                                self.last_device = None;
                            }
                            self.warn(
                                Some("Spotify: нет активного устройства / ошибка".into()),
                                &mut a,
                            );
                            self.schedule(now);
                        }
                    }
                }
            }
            Input::Yt { item_id, ev } => {
                if self.yt() && self.current.as_ref().is_some_and(|i| i.id == item_id) {
                    match ev {
                        YtEvent::Started { duration_ms: _ } if self.mode == Mode::Paused => {
                            self.needs_unmute = true;
                            a.push(Action::YtPause)
                        }
                        YtEvent::Started { duration_ms } => {
                            if self.max_duration_ms.is_some_and(|max| duration_ms > max) {
                                a.push(Action::YtStop);
                                Self::cancel(
                                    self.current.take().unwrap(),
                                    CancelReason::TooLong,
                                    &mut a,
                                );
                                self.next(now, &mut a);
                            } else {
                                self.current.as_mut().unwrap().track.duration_ms =
                                    Some(duration_ms);
                                // Reconnect acknowledgements preserve the existing clock.
                                self.needs_unmute = false;
                                a.push(Action::YtUnmute);
                                if self.mode == Mode::Starting {
                                    self.confirmed(now, &mut a);
                                }
                            }
                        }
                        YtEvent::Ended | YtEvent::Error => {
                            Self::cancel(
                                self.current.take().unwrap(),
                                CancelReason::YtUnavailable,
                                &mut a,
                            );
                            self.next(now, &mut a);
                        }
                    }
                }
            }
            Input::YtPlayerConnected => {
                if self.yt() {
                    let i = self.current.as_ref().unwrap();
                    if let Source::YouTube { video_id } = &i.track.source {
                        a.push(Action::YtPlay {
                            item_id: i.id,
                            video_id: video_id.clone(),
                            start_ms: self.progress(now),
                        });
                        if self.mode == Mode::Paused {
                            a.push(Action::YtPause);
                        }
                    }
                }
            }
            Input::FallbackResolved { item_id, video_id } => {
                if self.resolving && self.current.as_ref().is_some_and(|i| i.id == item_id) {
                    match video_id {
                        Some(video_id) => {
                            self.current.as_mut().unwrap().track.source =
                                Source::YouTube { video_id };
                            self.start(now, &mut a);
                        }
                        None => {
                            Self::cancel(
                                self.current.take().unwrap(),
                                CancelReason::YtUnavailable,
                                &mut a,
                            );
                            self.next(now, &mut a);
                        }
                    }
                }
            }
            Input::Restore { current, queue } => {
                self.current = current;
                self.queue = queue;
                if self.current.as_ref().is_some_and(|i| i.started) && !self.yt() {
                    self.mode = Mode::Playing;
                    self.last_progress = 0;
                    self.anchor = now;
                    self.confirmed_at = now;
                    self.retry_at = None;
                    self.resolving = false;
                } else if self.current.is_none() {
                    self.next(now, &mut a);
                } else {
                    self.start(now, &mut a);
                }
            }
            Input::Spotify(result) => {
                if result.is_ok() {
                    self.last_ok = Some(now);
                    if self.fallback {
                        self.fallback = false;
                        self.warn(None, &mut a);
                    }
                }
                if self.current.is_some()
                    && !self.yt()
                    && matches!(
                        self.mode,
                        Mode::Starting | Mode::Playing | Mode::Paused | Mode::Suspended
                    )
                {
                    match result {
                        Err(()) => {
                            if !self.resolving
                                && now.saturating_sub(self.last_ok.unwrap_or(now)) >= OUTAGE_MS
                                && (self.mode == Mode::Starting
                                    || (self.mode == Mode::Playing
                                        && self.progress(now) >= self.duration()))
                            {
                                self.fallback = true;
                                self.warn(
                                    Some("Spotify: недоступен, переход на YouTube".into()),
                                    &mut a,
                                );
                                if self.mode == Mode::Starting {
                                    self.start(now, &mut a);
                                } else {
                                    self.next(now, &mut a);
                                }
                            }
                        }
                        Ok(snapshot) if matches!(self.mode, Mode::Playing | Mode::Paused) => {
                            let own=snapshot.as_ref().is_some_and(|s|matches!(&self.current.as_ref().unwrap().track.source,Source::Spotify{uri} if s.track_uri.as_ref()==Some(uri)));
                            let near = self.duration() > END_TOLERANCE_MS + POLL_MS
                                && self.progress(now)
                                    >= self.duration() - (END_TOLERANCE_MS + POLL_MS);
                            if let Some(s) = &snapshot {
                                if s.device_id.is_some() {
                                    self.last_device = s.device_id.clone();
                                }
                            }
                            if own {
                                let s = snapshot.as_ref().unwrap();
                                self.last_progress = s.progress_ms;
                                self.anchor = now;
                                self.current.as_mut().unwrap().track.duration_ms =
                                    Some(s.duration_ms);
                                if !s.is_playing
                                    && (s.progress_ms
                                        >= s.duration_ms.saturating_sub(END_TOLERANCE_MS)
                                        || (s.progress_ms < END_TOLERANCE_MS && near))
                                {
                                    self.next(now, &mut a);
                                } else if s.device_id.is_none()
                                    && self.mode == Mode::Playing
                                    && now - self.confirmed_at >= PLAY_GRACE_MS
                                {
                                    if self.retry_at.is_none() {
                                        self.play_spotify(&mut a);
                                        self.schedule(now);
                                    }
                                } else {
                                    self.mode = if s.is_playing {
                                        Mode::Playing
                                    } else {
                                        Mode::Paused
                                    };
                                    self.retry_at = None;
                                    self.retry_index = 0;
                                }
                            } else if near {
                                self.next(now, &mut a);
                            } else if now - self.confirmed_at >= PLAY_GRACE_MS {
                                if snapshot.as_ref().is_none_or(|s| s.device_id.is_none()) {
                                    if self.mode == Mode::Playing && self.retry_at.is_none() {
                                        self.play_spotify(&mut a);
                                        self.schedule(now);
                                    }
                                } else {
                                    self.mode = Mode::Suspended;
                                    self.retry_at = None;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Input::Tick => {
                if self.yt() {
                    if self.mode == Mode::Starting && now - self.start_sent >= YT_START_TIMEOUT_MS {
                        a.push(Action::YtStop);
                        Self::cancel(
                            self.current.take().unwrap(),
                            CancelReason::YtUnavailable,
                            &mut a,
                        );
                        self.next(now, &mut a);
                    } else if self.mode == Mode::Playing
                        && self.progress(now)
                            >= self.duration().saturating_add(YT_END_SLACK_MS as u64)
                    {
                        self.next(now, &mut a);
                    }
                } else if self.retry_at.is_some_and(|t| now >= t)
                    && matches!(self.mode, Mode::Starting | Mode::Playing)
                {
                    self.play_spotify(&mut a);
                    self.schedule(now);
                }
            }
        }
        let changed = before != self.snapshot();
        if changed {
            a.insert(0, Action::Persist);
        }
        if changed
            || old_mode != self.mode
            || old_progress != self.last_progress
            || old_warning != self.warning
        {
            a.push(Action::Widget(WidgetState {
                visible: self.current.is_some(),
                now: self.current.clone(),
                queue: self.queue.clone(),
                progress_ms: self.progress(now),
                mode: format!("{:?}", self.mode),
                warning: self.warning.clone(),
            }));
        }
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Source, Track};
    fn item(id: i64, yt: bool) -> QueueItem {
        QueueItem {
            id,
            track: Track {
                source: if yt {
                    Source::YouTube {
                        video_id: format!("v{id}"),
                    }
                } else {
                    Source::Spotify {
                        uri: format!("s{id}"),
                    }
                },
                title: String::new(),
                artist: String::new(),
                duration_ms: Some(60_000),
                explicit: None,
                cover_url: None,
            },
            requester: String::new(),
            redemption: None,
            started: false,
        }
    }
    fn setup(yt: bool) -> Player {
        let mut p = Player::new(None);
        p.step(Input::Enqueue(item(1, yt)), 0);
        p
    }
    fn playing(yt: bool) -> Player {
        let mut p = setup(yt);
        if yt {
            p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Started {
                        duration_ms: 60_000,
                    },
                },
                0,
            );
        } else {
            p.step(
                Input::PlayResult {
                    item_id: 1,
                    result: Ok(()),
                },
                0,
            );
        }
        p
    }
    fn snap(uri: &str, playing: bool, progress: u64) -> Input {
        Input::Spotify(Ok(Some(PlaybackSnapshot {
            device_id: Some("dev".into()),
            track_uri: Some(uri.into()),
            is_playing: playing,
            progress_ms: progress,
            duration_ms: 60_000,
        })))
    }
    fn has(a: &[Action], f: impl Fn(&Action) -> bool) -> bool {
        a.iter().any(f)
    }
    fn no_cancel(a: &[Action]) {
        assert!(!has(a, |a| matches!(a, Action::Cancel(..))));
    }
    #[test]
    fn enqueue_idle_starts_without_fulfill() {
        let mut p = Player::new(None);
        let a = p.step(Input::Enqueue(item(1, false)), 0);
        assert!(has(&a, |a| matches!(
            a,
            Action::SpotifyPlay { item_id: 1, .. }
        )));
        assert!(!has(&a, |a| matches!(a, Action::Started(_))));
        assert_eq!(p.mode(), Mode::Starting);
    }
    #[test]
    fn play_ok_emits_started() {
        let mut p = setup(false);
        let a = p.step(
            Input::PlayResult {
                item_id: 1,
                result: Ok(()),
            },
            0,
        );
        assert!(has(&a, |a| matches!(a,Action::Started(i) if i.started)));
        assert_eq!(p.mode(), Mode::Playing);
    }
    #[test]
    fn play_failure_does_not_fulfill_and_retries() {
        let mut p = setup(false);
        let a = p.step(
            Input::PlayResult {
                item_id: 1,
                result: Err(PlayError::Net),
            },
            0,
        );
        assert!(has(&a, |a| matches!(a, Action::Warn(Some(_)))));
        no_cancel(&a);
        assert!(!has(&a, |a| matches!(a, Action::Started(_))));
        assert!(p.step(Input::Tick, 1999).is_empty());
        assert!(has(&p.step(Input::Tick, 2000), |a| matches!(
            a,
            Action::SpotifyPlay { .. }
        )));
    }
    #[test]
    fn enqueue_busy_appends() {
        let mut p = playing(false);
        let a = p.step(Input::Enqueue(item(2, false)), 1);
        assert_eq!(p.snapshot().1, vec![item(2, false)]);
        assert!(matches!(a[0], Action::Persist));
    }
    #[test]
    fn track_end_plays_next() {
        let mut p = playing(false);
        p.step(Input::Enqueue(item(2, false)), 0);
        assert!(has(&p.step(snap("s1", false, 60000), 60000), |a| matches!(
            a,
            Action::SpotifyPlay { item_id: 2, .. }
        )));
    }
    #[test]
    fn track_end_near_duration_is_end_not_pause() {
        let mut p = playing(false);
        p.step(snap("s1", false, 57000), 57000);
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn autoplay_after_end_is_end_not_suspend() {
        let mut p = playing(false);
        p.step(snap("s1", true, 55000), 55000);
        p.step(snap("other", true, 0), 60000);
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn last_track_end_goes_idle_and_hides() {
        let mut p = playing(false);
        let a = p.step(snap("s1", false, 60000), 60000);
        assert!(has(&a, |a| matches!(a,Action::Widget(w) if !w.visible)));
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn manual_pause_does_nothing() {
        let mut p = playing(false);
        let a = p.step(snap("s1", false, 10000), 10000);
        assert_eq!(p.mode(), Mode::Paused);
        assert!(!has(&a, |a| matches!(a, Action::SpotifyPlay { .. })));
        assert!(p.step(Input::Tick, 100000).is_empty());
    }
    #[test]
    fn manual_resume_back_to_playing() {
        let mut p = playing(false);
        p.step(snap("s1", false, 10000), 10000);
        p.step(snap("s1", true, 10000), 12000);
        assert_eq!(p.mode(), Mode::Playing);
    }
    #[test]
    fn foreign_track_suspends() {
        let mut p = playing(false);
        p.step(snap("other", true, 0), 5000);
        assert_eq!(p.mode(), Mode::Suspended);
        assert!(p.step(snap("s1", true, 10000), 10000).is_empty());
    }
    #[test]
    fn foreign_track_within_grace_ignored() {
        let mut p = playing(false);
        assert!(p.step(snap("other", true, 0), 3999).is_empty());
        assert_eq!(p.mode(), Mode::Playing);
    }
    #[test]
    fn resume_queue_plays_next() {
        let mut p = playing(false);
        p.step(Input::Enqueue(item(2, false)), 0);
        p.step(snap("x", true, 0), 5000);
        assert!(has(&p.step(Input::ResumeQueue, 6000), |a| matches!(
            a,
            Action::SpotifyPlay { item_id: 2, .. }
        )));
    }
    #[test]
    fn device_lost_replays_with_backoff() {
        let mut p = playing(false);
        p.step(snap("s1", true, 10000), 10000);
        assert!(has(&p.step(Input::Spotify(Ok(None)), 11000), |a| matches!(
            a,
            Action::SpotifyPlay {
                position_ms: 10000,
                ..
            }
        )));
        assert!(p.step(Input::Tick, 12999).is_empty());
        assert!(has(&p.step(Input::Tick, 13000), |a| matches!(
            a,
            Action::SpotifyPlay {
                position_ms: 10000,
                ..
            }
        )));
    }
    #[test]
    fn short_outage_no_fallback() {
        let mut p = playing(false);
        assert!(p.step(Input::Spotify(Err(())), 29000).is_empty());
        assert_eq!(p.mode(), Mode::Playing);
    }
    #[test]
    fn outage_before_expected_end_no_fallback() {
        let mut p = playing(false);
        assert!(p.step(Input::Spotify(Err(())), 40000).is_empty());
        assert_eq!(p.mode(), Mode::Playing);
    }
    #[test]
    fn outage_after_end_triggers_fallback() {
        let mut p = playing(false);
        p.step(Input::Enqueue(item(2, false)), 0);
        let a = p.step(Input::Spotify(Err(())), 60000);
        assert!(has(
            &a,
            |a| matches!(a,Action::ResolveFallback(i) if i.id==2)
        ));
        assert!(has(&a, |a| matches!(a, Action::Warn(Some(_)))));
    }
    fn fallback() -> Player {
        let mut p = playing(false);
        p.step(Input::Enqueue(item(2, false)), 0);
        p.step(Input::Spotify(Err(())), 60000);
        p
    }
    #[test]
    fn fallback_unresolved_cancels() {
        let mut p = fallback();
        assert!(has(
            &p.step(
                Input::FallbackResolved {
                    item_id: 2,
                    video_id: None
                },
                60001
            ),
            |a| matches!(a,Action::Cancel(i,CancelReason::YtUnavailable) if i.id==2)
        ));
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn recovery_clears_warning() {
        let mut p = fallback();
        assert!(has(&p.step(Input::Spotify(Ok(None)), 60001), |a| matches!(
            a,
            Action::Warn(None)
        )));
    }
    #[test]
    fn youtube_playing_ignores_spotify_snapshot() {
        let mut p = playing(true);
        assert!(p.step(snap("x", true, 0), 5000).is_empty());
        assert_eq!(p.mode(), Mode::Playing);
    }
    #[test]
    fn yt_error_before_start_cancels() {
        let mut p = setup(true);
        assert!(has(
            &p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Error
                },
                1
            ),
            |a| matches!(a, Action::Cancel(_, CancelReason::YtUnavailable))
        ));
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn yt_too_long_cancels() {
        let mut p = setup(true);
        p.step(Input::SetMaxDuration(Some(100)), 0);
        assert!(has(
            &p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Started { duration_ms: 101 }
                },
                1
            ),
            |a| matches!(a, Action::Cancel(_, CancelReason::TooLong))
        ));
    }
    #[test]
    fn yt_start_timeout_cancels() {
        let mut p = setup(true);
        let a = p.step(Input::Tick, 20000);
        assert!(has(&a, |a| matches!(a, Action::YtStop)));
        assert!(has(&a, |a| matches!(
            a,
            Action::Cancel(_, CancelReason::YtUnavailable)
        )));
    }
    #[test]
    fn yt_stale_item_ignored() {
        let mut p = setup(true);
        assert!(
            p.step(
                Input::Yt {
                    item_id: 2,
                    ev: YtEvent::Error
                },
                1
            )
            .is_empty()
        );
        assert_eq!(p.mode(), Mode::Starting);
    }
    #[test]
    fn toggle_pause_youtube_uses_yt_actions() {
        let mut p = playing(true);
        assert!(has(&p.step(Input::TogglePause, 1), |a| matches!(
            a,
            Action::YtPause
        )));
        assert_eq!(p.mode(), Mode::Paused);
        assert!(has(&p.step(Input::TogglePause, 2), |a| matches!(
            a,
            Action::YtResume
        )));
        assert_eq!(p.mode(), Mode::Playing);
    }
    #[test]
    fn toggle_pause_spotify_uses_spotify_actions() {
        let mut p = playing(false);
        assert!(has(&p.step(Input::TogglePause, 1), |a| matches!(
            a,
            Action::SpotifyPause
        )));
        assert_eq!(p.mode(), Mode::Paused);
        assert!(has(&p.step(Input::TogglePause, 2), |a| matches!(
            a,
            Action::SpotifyResume
        )));
    }
    #[test]
    fn move_reorders() {
        let mut p = playing(false);
        p.step(Input::Enqueue(item(2, false)), 0);
        p.step(Input::Enqueue(item(3, false)), 0);
        let a = p.step(Input::Move(3, 0), 1);
        assert_eq!(p.snapshot().1, vec![item(3, false), item(2, false)]);
        assert!(matches!(a[0], Action::Persist));
    }
    #[test]
    fn yt_start_pauses_spotify() {
        let mut p = Player::new(None);
        let a = p.step(Input::Enqueue(item(1, true)), 0);
        assert!(matches!(a[1], Action::SpotifyPause));
        assert!(matches!(a[2], Action::YtPlay { start_ms: 0, .. }));
    }
    #[test]
    fn remove_queued_cancels() {
        let mut p = playing(false);
        p.step(Input::Enqueue(item(2, false)), 0);
        assert!(has(
            &p.step(Input::Remove(2), 1),
            |a| matches!(a,Action::Cancel(i,CancelReason::RemovedByStreamer) if i.id==2)
        ));
    }
    #[test]
    fn skip_started_no_refund() {
        let mut p = playing(false);
        let a = p.step(Input::Skip, 1);
        no_cancel(&a);
        assert!(has(&a, |a| matches!(a, Action::SpotifyPause)));
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn skip_starting_cancels() {
        let mut p = setup(false);
        assert!(has(&p.step(Input::Skip, 1), |a| matches!(
            a,
            Action::Cancel(_, CancelReason::RemovedByStreamer)
        )));
    }
    fn restored(yt: bool) -> (Player, Vec<Action>) {
        let mut p = Player::new(None);
        let mut i = item(1, yt);
        i.started = true;
        let a = p.step(
            Input::Restore {
                current: Some(i),
                queue: vec![],
            },
            0,
        );
        (p, a)
    }
    #[test]
    fn restore_spotify_no_refulfill() {
        let (p, a) = restored(false);
        assert_eq!(p.mode(), Mode::Playing);
        assert!(!has(&a, |a| matches!(
            a,
            Action::Started(_) | Action::SpotifyPlay { .. }
        )));
    }
    #[test]
    fn restore_youtube_replays() {
        let (mut p, a) = restored(true);
        assert!(has(&a, |a| matches!(a, Action::YtPlay { start_ms: 0, .. })));
        assert!(!has(
            &p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Started { duration_ms: 60000 }
                },
                0
            ),
            |a| matches!(a, Action::Started(_))
        ));
    }
    #[test]
    fn persist_first_in_batch() {
        let mut p = setup(true);
        p.step(Input::Enqueue(item(3, false)), 0);
        for input in [
            Input::Enqueue(item(2, false)),
            Input::Move(2, 0),
            Input::Remove(2),
            Input::Skip,
        ] {
            let a = p.step(input, 1);
            assert!(matches!(a.first(), Some(Action::Persist)));
            assert!(has(&a, |a| matches!(a, Action::Widget(_))));
        }
    }
    #[test]
    fn play_result_stale_ignored() {
        let mut p = setup(false);
        assert!(
            p.step(
                Input::PlayResult {
                    item_id: 2,
                    result: Ok(())
                },
                1
            )
            .is_empty()
        );
        assert_eq!(p.mode(), Mode::Starting);
    }
    #[test]
    fn no_device_clears_last_device() {
        let mut p = playing(false);
        p.step(snap("s1", true, 1000), 1000);
        p.step(
            Input::PlayResult {
                item_id: 1,
                result: Err(PlayError::NoDevice),
            },
            2000,
        );
        assert!(has(&p.step(Input::Tick, 4000), |a| matches!(
            a,
            Action::SpotifyPlay {
                device_id: None,
                ..
            }
        )));
    }
    #[test]
    fn yt_unmute_only_after_length_ok() {
        let mut p = setup(true);
        assert!(has(
            &p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Started { duration_ms: 60000 }
                },
                1
            ),
            |a| matches!(a, Action::YtUnmute)
        ));
        let mut p = setup(true);
        p.step(Input::SetMaxDuration(Some(10)), 0);
        assert!(!has(
            &p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Started { duration_ms: 60000 }
                },
                1
            ),
            |a| matches!(a, Action::YtUnmute)
        ));
    }
    #[test]
    fn restored_started_yt_error_no_cancel() {
        let (mut p, _) = restored(true);
        let a = p.step(
            Input::Yt {
                item_id: 1,
                ev: YtEvent::Error,
            },
            1,
        );
        no_cancel(&a);
        assert_eq!(p.mode(), Mode::Idle);
        let (mut p, _) = restored(true);
        no_cancel(&p.step(Input::Tick, 20000));
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn skip_restored_started_no_cancel() {
        for yt in [false, true] {
            let (mut p, _) = restored(yt);
            no_cancel(&p.step(Input::Skip, 1));
            assert_eq!(p.mode(), Mode::Idle);
        }
    }
    #[test]
    fn yt_missing_end_times_out() {
        let mut p = playing(true);
        assert!(p.step(Input::Tick, 69999).is_empty());
        p.step(Input::Tick, 70000);
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn yt_missing_end_ignores_paused_time() {
        let mut p = playing(true);
        p.step(Input::TogglePause, 10000);
        assert!(p.step(Input::Tick, 100000).is_empty());
        p.step(Input::TogglePause, 100000);
        assert!(p.step(Input::Tick, 159999).is_empty());
        p.step(Input::Tick, 160000);
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn player_reconnect_replays_current_yt() {
        let mut p = playing(true);
        assert!(has(&p.step(Input::YtPlayerConnected, 10000), |a| matches!(
            a,
            Action::YtPlay {
                item_id: 1,
                start_ms: 10000,
                ..
            }
        )));
    }
    #[test]
    fn player_reconnect_while_paused() {
        let mut p = playing(true);
        p.step(Input::TogglePause, 10000);
        let a = p.step(Input::YtPlayerConnected, 20000);
        assert!(has(&a, |a| matches!(
            a,
            Action::YtPlay {
                start_ms: 10000,
                ..
            }
        )));
        assert!(has(&a, |a| matches!(a, Action::YtPause)));
        assert_eq!(
            p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Started { duration_ms: 60000 }
                },
                20001
            ),
            vec![Action::YtPause]
        );
        assert_eq!(p.mode(), Mode::Paused);
    }
    #[test]
    fn fallback_finishes_current_without_cancel() {
        let mut p = playing(false);
        let a = p.step(Input::Spotify(Err(())), 60000);
        no_cancel(&a);
        assert_eq!(p.mode(), Mode::Idle);
    }
    #[test]
    fn started_items_never_cancel_in_remaining_branches() {
        let (mut p, _) = restored(true);
        p.step(Input::SetMaxDuration(Some(10)), 0);
        no_cancel(&p.step(
            Input::Yt {
                item_id: 1,
                ev: YtEvent::Started { duration_ms: 11 },
            },
            1,
        ));
        assert_eq!(p.mode(), Mode::Idle);

        let mut p = playing(false);
        let mut queued = item(2, false);
        queued.started = true;
        p.step(Input::Enqueue(queued.clone()), 0);
        no_cancel(&p.step(Input::Remove(2), 1));
        p.step(Input::Enqueue(queued), 2);
        p.step(Input::Spotify(Err(())), 60000);
        no_cancel(&p.step(
            Input::FallbackResolved {
                item_id: 2,
                video_id: None,
            },
            60001,
        ));
        assert_eq!(p.mode(), Mode::Idle);
    }

    #[test]
    fn all_foreign_events_leave_current_unchanged() {
        for yt in [false, true] {
            let mut p = playing(yt);
            let before = p.snapshot();
            for input in [
                Input::PlayResult {
                    item_id: 99,
                    result: Ok(()),
                },
                Input::PlayResult {
                    item_id: 99,
                    result: Err(PlayError::NoDevice),
                },
                Input::Yt {
                    item_id: 99,
                    ev: YtEvent::Started { duration_ms: 1 },
                },
                Input::Yt {
                    item_id: 99,
                    ev: YtEvent::Ended,
                },
                Input::Yt {
                    item_id: 99,
                    ev: YtEvent::Error,
                },
            ] {
                assert!(p.step(input, 5000).is_empty());
                assert_eq!(p.snapshot(), before);
                assert_eq!(p.mode(), Mode::Playing);
            }
        }
    }

    #[test]
    fn youtube_snapshots_only_update_spotify_health() {
        for paused in [false, true] {
            let mut p = playing(true);
            if paused {
                p.step(Input::TogglePause, 1000);
            }
            let before = p.snapshot();
            let mode = p.mode();
            for input in [
                snap("foreign", true, 0),
                Input::Spotify(Ok(None)),
                Input::Spotify(Err(())),
            ] {
                assert!(p.step(input, 100000).is_empty());
                assert_eq!(p.snapshot(), before);
                assert_eq!(p.mode(), mode);
            }
            assert_eq!(p.last_ok, Some(100000));
            assert!(!p.fallback);
        }
        let mut p = setup(false);
        assert!(p.step(snap("foreign", true, 0), 100000).is_empty());
        assert_eq!(p.mode(), Mode::Starting);
    }

    #[test]
    fn spotify_retries_follow_full_backoff_and_keep_last_device() {
        let mut p = playing(false);
        p.step(snap("s1", true, 1000), 1000);
        p.step(
            Input::PlayResult {
                item_id: 1,
                result: Err(PlayError::Net),
            },
            1000,
        );
        let mut due = 3000;
        for delay in [4000, 8000, 16000, 30000, 30000] {
            assert!(p.step(Input::Tick, due - 1).is_empty());
            let a = p.step(Input::Tick, due);
            assert!(has(
                &a,
                |a| matches!(a, Action::SpotifyPlay { device_id: Some(d), position_ms: 1000, .. } if d == "dev")
            ));
            no_cancel(&a);
            assert_eq!(p.retry_at, Some(due + delay));
            due += delay;
        }
    }

    #[test]
    fn resolved_fallback_starts_muted_youtube_after_spotify_pause() {
        let mut p = fallback();
        let a = p.step(
            Input::FallbackResolved {
                item_id: 2,
                video_id: Some("video".into()),
            },
            60001,
        );
        assert_eq!(a[0], Action::Persist);
        assert_eq!(a[1], Action::SpotifyPause);
        assert!(
            matches!(&a[2], Action::YtPlay { item_id: 2, video_id, start_ms: 0 } if video_id == "video")
        );
        assert!(!has(&a, |a| matches!(
            a,
            Action::Started(_) | Action::YtUnmute
        )));
        assert_eq!(p.mode(), Mode::Starting);
    }

    #[test]
    fn reconnect_acknowledgement_preserves_watchdog_clock() {
        let mut p = playing(true);
        p.step(Input::YtPlayerConnected, 10000);
        let a = p.step(
            Input::Yt {
                item_id: 1,
                ev: YtEvent::Started { duration_ms: 60000 },
            },
            11000,
        );
        assert_eq!(a, vec![Action::YtUnmute]);
        p.step(Input::Tick, 70000);
        assert_eq!(p.mode(), Mode::Idle);
        let mut p = playing(false);
        assert!(p.step(Input::YtPlayerConnected, 10000).is_empty());
    }
    #[test]
    fn own_uri_progress_reset_after_end_is_end() {
        for queued in [false, true] {
            let mut p = playing(false);
            if queued {
                p.step(Input::Enqueue(item(2, false)), 0);
            }
            p.step(snap("s1", true, 54000), 54000);
            let a = p.step(snap("s1", false, 0), 55000);
            no_cancel(&a);
            assert_eq!(p.mode(), if queued { Mode::Starting } else { Mode::Idle });
            assert_eq!(
                p.snapshot().0.map(|i| i.id),
                if queued { Some(2) } else { None }
            );
        }
    }

    #[test]
    fn outage_during_starting_triggers_fallback() {
        for failed in [false, true] {
            let mut p = setup(false);
            if failed {
                p.step(
                    Input::PlayResult {
                        item_id: 1,
                        result: Err(PlayError::Net),
                    },
                    0,
                );
            }
            assert!(p.step(Input::Spotify(Err(())), OUTAGE_MS - 1).is_empty());
            let a = p.step(Input::Spotify(Err(())), OUTAGE_MS);
            assert!(has(&a, |a| matches!(a, Action::Warn(Some(_)))));
            assert!(has(
                &a,
                |a| matches!(a, Action::ResolveFallback(i) if i.id == 1)
            ));
            no_cancel(&a);
            assert!(!p.snapshot().0.unwrap().started);
            assert_eq!(p.retry_at, None);
            assert!(p.step(Input::Spotify(Err(())), OUTAGE_MS + 1).is_empty());
        }
    }

    #[test]
    fn resume_after_paused_reconnect_unmutes() {
        let mut p = playing(true);
        p.step(Input::TogglePause, 10000);
        p.step(Input::YtPlayerConnected, 20000);
        assert_eq!(
            p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Started { duration_ms: 60000 }
                },
                20001
            ),
            vec![Action::YtPause]
        );
        let a = p.step(Input::TogglePause, 30000);
        assert!(has(&a, |a| matches!(a, Action::YtResume)));
        assert!(has(&a, |a| matches!(a, Action::YtUnmute)));
        assert_eq!(p.progress(31000), 11000);
        p.step(Input::TogglePause, 32000);
        assert!(!has(&p.step(Input::TogglePause, 33000), |a| matches!(
            a,
            Action::YtUnmute
        )));
    }

    #[test]
    fn yt_ended_before_start_cancels() {
        for started in [false, true] {
            let mut p = if started {
                restored(true).0
            } else {
                setup(true)
            };
            p.step(Input::Enqueue(item(2, false)), 0);
            let a = p.step(
                Input::Yt {
                    item_id: 1,
                    ev: YtEvent::Ended,
                },
                1,
            );
            assert_eq!(
                has(
                    &a,
                    |a| matches!(a, Action::Cancel(i, CancelReason::YtUnavailable) if i.id == 1)
                ),
                !started
            );
            assert!(has(&a, |a| matches!(
                a,
                Action::SpotifyPlay { item_id: 2, .. }
            )));
        }
    }

    #[test]
    fn unknown_duration_not_treated_as_end() {
        for duration in [None, Some(0), Some(END_TOLERANCE_MS + POLL_MS)] {
            for foreign in [false, true] {
                let mut p = playing(false);
                p.current.as_mut().unwrap().track.duration_ms = duration;
                let input = if foreign {
                    snap("foreign", true, 0)
                } else {
                    Input::Spotify(Ok(None))
                };
                let a = p.step(input, 6000);
                assert_eq!(p.snapshot().0.unwrap().id, 1);
                assert_eq!(
                    p.mode(),
                    if foreign {
                        Mode::Suspended
                    } else {
                        Mode::Playing
                    }
                );
                no_cancel(&a);
            }
        }
    }

    #[test]
    fn duplicate_play_ok_ignored() {
        let mut p = playing(false);
        p.step(snap("s1", true, 10000), 10000);
        let progress = p.progress(15000);
        let confirmed_at = p.confirmed_at;
        assert!(
            p.step(
                Input::PlayResult {
                    item_id: 1,
                    result: Ok(())
                },
                15000
            )
            .is_empty()
        );
        assert_eq!(p.progress(15000), progress);
        assert_eq!(p.confirmed_at, confirmed_at);
    }
}
