use super::filter::{self, Gate};
use super::queue::{Item, Origin, Outcome, Queue, fulfilled};
use super::{TtsSettings, TtsView};
use crate::model::{ChatMessage, Redemption, RedemptionRef};
use crate::server::{TtsWidgetEvent, WsOut};
use crate::twitch::eventsub::ChatStatus;
use futures_util::future::BoxFuture;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};
use tokio::time::Instant;

pub enum TtsMsg {
    Chat(ChatMessage),
    Reward(Redemption),
    Settings(TtsSettings),
    ChatStatus(ChatStatus),
    Warning(String),
    Widget(TtsWidgetEvent),
    Skip,
    Clear,
    Test(String),
    Reconcile(Vec<RedemptionRef>),
    State(oneshot::Sender<TtsView>),
}
pub type SynthFn = Arc<
    dyn Fn(i64, String, String, f32, PathBuf) -> BoxFuture<'static, Result<i64, String>>
        + Send
        + Sync,
>;
pub struct EngineDeps {
    pub synth: SynthFn,
    pub dir: PathBuf,
    pub ws: broadcast::Sender<WsOut>,
    pub outcome: mpsc::UnboundedSender<(RedemptionRef, bool)>,
    pub view: Arc<dyn Fn(&TtsView) + Send + Sync>,
}

pub async fn run(
    deps: EngineDeps,
    mut settings: TtsSettings,
    mut rx: mpsc::UnboundedReceiver<TtsMsg>,
) {
    let mut queue = Queue::default();
    let mut gate = Gate::default();
    let mut seen = HashSet::new();
    let mut order = VecDeque::new();
    let mut next_id = 0;
    let mut widget = false;
    let mut warning = None;
    let mut chat_warning = None;
    let mut voice_broken = false;
    let mut flight: Option<(u64, Arc<AtomicBool>, Arc<Notify>)> = None;
    let mut deadline = None;
    let mut play_ms = Duration::ZERO;
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<(u64, Result<i64, String>)>();
    loop {
        // Drain unavailable items individually; never overlap synthesis workers.
        if flight.is_none() && queue.view().0.is_none() && widget {
            while let Some(item) = queue.take_next() {
                if settings.profile_id.is_none() || voice_broken {
                    if settings.profile_id.is_none() {
                        warning = Some("tts_no_voice".into());
                    }
                    finish(&deps, &mut queue, Outcome::SynthFailed);
                    continue;
                }
                let cancel = Arc::new(AtomicBool::new(false));
                let wake = Arc::new(Notify::new());
                flight = Some((item.id, cancel.clone(), wake.clone()));
                let synth = deps.synth.clone();
                let tx = done_tx.clone();
                let path = deps.dir.join(format!("{}.wav", item.id));
                let profile = settings.profile_id.unwrap();
                let lang = settings.lang.clone();
                let speed = settings.speed;
                tokio::spawn(async move {
                    let started = Instant::now();
                    let result = loop {
                        if cancel.load(Ordering::SeqCst) {
                            break Err("CANCELLED".into());
                        }
                        let result = synth(
                            profile,
                            item.text.clone(),
                            lang.clone(),
                            speed,
                            path.clone(),
                        )
                        .await;
                        if result.as_ref().err().is_none_or(|code| code != "JOB_BUSY")
                            || started.elapsed() >= Duration::from_secs(60)
                        {
                            break result;
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                            _ = wake.notified() => {},
                        }
                    };
                    if tx.send((item.id, result)).is_err() {
                        let _ = std::fs::remove_file(path);
                    }
                });
                break;
            }
        }
        let (mut current, pending) = queue.view();
        if let Some(item) = &mut current {
            item.synthesizing = flight.as_ref().is_some_and(|(id, _, _)| *id == item.id);
        }
        let view = TtsView {
            current,
            queue: pending,
            widget,
            warning: warning.clone().or_else(|| {
                if settings.chat_enabled {
                    chat_warning.clone()
                } else {
                    None
                }
            }),
        };
        (deps.view)(&view);
        tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else {
                    if let Some((_, cancel, wake)) = &flight { cancel.store(true, Ordering::SeqCst); wake.notify_one(); }
                    // Keep the receiver alive until the worker returns so its WAV can be removed.
                    if flight.is_some() && let Some((id, _)) = done_rx.recv().await {
                        let _ = std::fs::remove_file(deps.dir.join(format!("{id}.wav")));
                    }
                    if let Some(item) = queue.finish_current() { let _ = std::fs::remove_file(deps.dir.join(format!("{}.wav", item.id))); }
                    break;
                };
                match msg {
                    TtsMsg::State(tx) => { let _ = tx.send(view); }
                    TtsMsg::Settings(new) => {
                        settings = new; voice_broken = false;
                        warning = (settings.profile_id.is_none() && (settings.reward_enabled || settings.chat_enabled)).then(|| "tts_no_voice".into());
                    }
                    TtsMsg::Warning(text) => warning = Some(text),
                    TtsMsg::ChatStatus(status) => chat_warning = match status {
                        ChatStatus::Ok => None,
                        ChatStatus::Forbidden => Some("tts_chat_relogin".into()),
                        ChatStatus::Failed => Some("tts_chat_failed".into()),
                    },
                    TtsMsg::Chat(m) => {
                        if chat_warning.as_deref() == Some("tts_chat_failed") { chat_warning = None; }
                        if !settings.chat_enabled || m.source_broadcaster_id.as_ref().is_some_and(|s| s != &m.broadcaster_id) { continue; }
                        let broadcaster = m.badges.iter().any(|b| b == "broadcaster");
                        let Some(text) = filter::extract_marked(&filter::strip_emotes(&m.fragments), &settings.marker) else { continue; };
                        if !broadcaster && (!filter::role_allowed(&settings.roles, &m.badges) || !gate.allowed(&m.user_login, settings.global_cooldown_s, settings.user_cooldown_s, Instant::now())) { continue; }
                        let Ok(text) = filter::clean(&text, settings.strip_links, &settings.stop_words) else { continue; };
                        if settings.profile_id.is_none() { warning = Some("tts_no_voice".into()); continue; }
                        if voice_broken { continue; }
                        next_id += 1;
                        let user = m.user_login;
                        if queue.push(Item { id: next_id, origin: Origin::Chat, user: user.clone(), text }).is_ok() && !broadcaster {
                            gate.mark(&user, Instant::now(), settings.user_cooldown_s);
                        }
                    }
                    TtsMsg::Reward(r) => {
                        if !remember(&mut seen, &mut order, &r.r.redemption_id) { continue; }
                        let text = filter::clean(&r.user_input, settings.strip_links, &settings.stop_words);
                        let rejection = if !settings.reward_enabled { Some(Outcome::Disabled) }
                            else if text.is_err() { Some(Outcome::Rejected) }
                            else if settings.profile_id.is_none() || voice_broken {
                                if settings.profile_id.is_none() { warning = Some("tts_no_voice".into()); }
                                Some(Outcome::SynthFailed)
                            } else { None };
                        if let Some(outcome) = rejection { report(&deps, Item { id: 0, origin: Origin::Reward(r.r), user: r.user_login, text: r.user_input }, outcome); }
                        else {
                            next_id += 1;
                            if let Err(item) = queue.push(Item { id: next_id, origin: Origin::Reward(r.r), user: r.user_login, text: text.unwrap() }) { report(&deps, item, Outcome::QueueFull); }
                        }
                    }
                    TtsMsg::Test(text) => {
                        if let Ok(text) = filter::clean(&text, false, &[]) {
                            if settings.profile_id.is_none() { warning = Some("tts_no_voice".into()); continue; }
                            if voice_broken { continue; }
                            next_id += 1;
                            let _ = queue.push(Item { id: next_id, origin: Origin::Test, user: String::new(), text });
                        }
                    }
                    TtsMsg::Reconcile(refs) => for r in refs {
                        if remember(&mut seen, &mut order, &r.redemption_id) { let _ = deps.outcome.send((r, fulfilled(&Outcome::Reconciled))); }
                    },
                    TtsMsg::Clear => for item in queue.clear() { report(&deps, item, Outcome::Cleared); },
                    TtsMsg::Skip => {
                        if queue.current().is_some() {
                            if let Some((_, cancel, wake)) = &flight { cancel.store(true, Ordering::SeqCst); wake.notify_one(); }
                            let _ = deps.ws.send(WsOut::TtsStop);
                            deadline = None;
                            finish(&deps, &mut queue, Outcome::Skipped);
                        }
                    }
                    TtsMsg::Widget(event) => match event {
                        TtsWidgetEvent::Connected => {
                            widget = true;
                            // A reloaded OBS source (scene switch) replays the clip that was playing.
                            if deadline.is_some() && let Some(item) = queue.current() {
                                let _ = deps.ws.send(WsOut::TtsPlay { id: item.id });
                                deadline = Some(Instant::now() + play_ms);
                            }
                        }
                        TtsWidgetEvent::Disconnected => widget = false,
                        TtsWidgetEvent::Ended(id) | TtsWidgetEvent::Error(id) => {
                            if deadline.is_some() && queue.current().is_some_and(|item| item.id == id) {
                                let outcome = if matches!(event, TtsWidgetEvent::Ended(_)) { Outcome::Played } else { Outcome::SynthFailed };
                                deadline = None;
                                finish(&deps, &mut queue, outcome);
                            }
                        }
                    },
                }
            }
            Some((id, result)) = done_rx.recv(), if flight.is_some() => {
                flight = None;
                if queue.current().is_none_or(|item| item.id != id) {
                    let _ = std::fs::remove_file(deps.dir.join(format!("{id}.wav")));
                    continue;
                }
                match result {
                    Ok(duration) => {
                        let _ = deps.ws.send(WsOut::TtsPlay { id });
                        play_ms = Duration::from_millis(duration.max(0) as u64) + Duration::from_secs(10);
                        deadline = Some(Instant::now() + play_ms);
                    }
                    Err(code) => {
                        let broken = match code.as_str() {
                            "MODEL_MISSING" => Some("tts_model_missing"),
                            "NOT_FOUND" => Some("tts_voice_missing"),
                            "VOICE_LAB_UNAVAILABLE" => Some("tts_unavailable"),
                            _ => None,
                        };
                        if let Some(text) = broken { warning = Some(text.into()); voice_broken = true; }
                        finish(&deps, &mut queue, Outcome::SynthFailed);
                    }
                }
            }
            _ = async { match deadline { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } } => {
                deadline = None;
                finish(&deps, &mut queue, Outcome::Played);
            }
        }
    }
}
fn remember(seen: &mut HashSet<String>, order: &mut VecDeque<String>, id: &str) -> bool {
    if !seen.insert(id.into()) {
        return false;
    }
    order.push_back(id.into());
    if order.len() > 500
        && let Some(old) = order.pop_front()
    {
        seen.remove(&old);
    }
    true
}
fn report(deps: &EngineDeps, item: Item, outcome: Outcome) {
    if let Origin::Reward(r) = item.origin {
        let _ = deps.outcome.send((r, fulfilled(&outcome)));
    }
}
fn finish(deps: &EngineDeps, queue: &mut Queue, outcome: Outcome) {
    if let Some(item) = queue.finish_current() {
        let _ = std::fs::remove_file(deps.dir.join(format!("{}.wav", item.id)));
        report(deps, item, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Fragment;
    use std::sync::atomic::AtomicUsize;

    struct Harness {
        tx: mpsc::UnboundedSender<TtsMsg>,
        outcomes: mpsc::UnboundedReceiver<(RedemptionRef, bool)>,
        ws: broadcast::Receiver<WsOut>,
        calls: Arc<AtomicUsize>,
        dir: tempfile::TempDir,
    }
    impl Harness {
        fn new(settings: TtsSettings) -> Self {
            Self::custom(settings, None)
        }
        fn custom(settings: TtsSettings, synth: Option<SynthFn>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let synth = synth.unwrap_or_else(|| {
                Arc::new(move |_, _, _, _, out| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async move {
                        std::fs::write(out, []).unwrap();
                        Ok(1000)
                    })
                })
            });
            let (tx, rx) = mpsc::unbounded_channel();
            let (outcome, outcomes) = mpsc::unbounded_channel();
            let (ws_tx, ws) = broadcast::channel(100);
            tokio::spawn(run(
                EngineDeps {
                    synth,
                    dir: dir.path().into(),
                    ws: ws_tx,
                    outcome,
                    view: Arc::new(|_| {}),
                },
                settings,
                rx,
            ));
            Self {
                tx,
                outcomes,
                ws,
                calls,
                dir,
            }
        }
        fn send(&self, msg: TtsMsg) {
            self.tx.send(msg).unwrap();
        }
        async fn state(&self) -> TtsView {
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            let (tx, rx) = oneshot::channel();
            self.send(TtsMsg::State(tx));
            rx.await.unwrap()
        }
        async fn connect(&self) {
            self.send(TtsMsg::Widget(TtsWidgetEvent::Connected));
            self.state().await;
        }
        async fn outcome(&mut self, id: &str, ok: bool) {
            self.state().await;
            let (r, actual) = self.outcomes.try_recv().unwrap();
            assert_eq!(r.redemption_id, id);
            assert_eq!(actual, ok);
        }
    }
    fn settings() -> TtsSettings {
        TtsSettings {
            profile_id: Some(1),
            chat_enabled: true,
            reward_enabled: true,
            ..Default::default()
        }
    }
    fn reward(id: &str, text: &str) -> TtsMsg {
        TtsMsg::Reward(Redemption {
            r: reference(id),
            user_login: "alice".into(),
            user_input: text.into(),
        })
    }
    fn reference(id: &str) -> RedemptionRef {
        RedemptionRef {
            reward_id: "reward".into(),
            redemption_id: id.into(),
        }
    }
    fn chat(text: &str, badges: &[&str]) -> ChatMessage {
        ChatMessage {
            user_login: "alice".into(),
            broadcaster_id: "b".into(),
            source_broadcaster_id: None,
            fragments: vec![Fragment {
                kind: "text".into(),
                text: text.into(),
            }],
            badges: badges.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn chat_marked_message_plays() {
        let mut h = Harness::new(settings());
        h.connect().await;
        h.send(TtsMsg::Chat(chat("привет % хвост", &[])));
        let v = h.state().await;
        assert_eq!(v.current.unwrap().text, "привет");
        assert!(matches!(h.ws.try_recv(), Ok(WsOut::TtsPlay { .. })));
    }
    #[tokio::test(start_paused = true)]
    async fn chat_cooldown_marks_only_accepted() {
        let mut s = settings();
        s.stop_words = vec!["плохо".into()];
        let h = Harness::new(s);
        for text in ["плохо %", "хорошо %", "ещё %"] {
            h.send(TtsMsg::Chat(chat(text, &[])));
        }
        let v = h.state().await;
        assert_eq!(v.queue.len(), 1);
        assert_eq!(v.queue[0].text, "хорошо");
    }
    #[tokio::test(start_paused = true)]
    async fn chat_roles_and_broadcaster_bypass() {
        let mut s = settings();
        s.roles.everyone = false;
        s.roles.subscribers = false;
        s.roles.vips = false;
        s.roles.moderators = false;
        let h = Harness::new(s);
        h.send(TtsMsg::Chat(chat("нет %", &[])));
        for _ in 0..2 {
            h.send(TtsMsg::Chat(chat("да %", &["broadcaster"])));
        }
        assert_eq!(h.state().await.queue.len(), 2);
    }
    #[tokio::test(start_paused = true)]
    async fn shared_chat_ignored() {
        let h = Harness::new(settings());
        let mut m = chat("привет %", &[]);
        m.source_broadcaster_id = Some("other".into());
        h.send(TtsMsg::Chat(m));
        assert!(h.state().await.queue.is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn chat_disabled_ignored() {
        let mut s = settings();
        s.chat_enabled = false;
        let h = Harness::new(s);
        h.send(TtsMsg::Chat(chat("привет %", &[])));
        assert!(h.state().await.queue.is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn reward_rejected_refunds() {
        let mut h = Harness::new(settings());
        h.send(reward("1", " "));
        h.outcome("1", false).await;
    }
    #[tokio::test(start_paused = true)]
    async fn reward_disabled_refunds() {
        let mut s = settings();
        s.reward_enabled = false;
        let mut h = Harness::new(s);
        h.send(reward("1", "текст"));
        h.outcome("1", false).await;
    }
    #[tokio::test(start_paused = true)]
    async fn reward_played_fulfills() {
        let mut h = Harness::new(settings());
        h.connect().await;
        h.send(reward("1", "текст"));
        let id = h.state().await.current.unwrap().id;
        h.send(TtsMsg::Widget(TtsWidgetEvent::Ended(id + 1)));
        assert!(h.state().await.current.is_some());
        h.send(TtsMsg::Widget(TtsWidgetEvent::Ended(id)));
        h.outcome("1", true).await;
    }
    #[tokio::test(start_paused = true)]
    async fn reward_duplicate_ignored() {
        let h = Harness::new(settings());
        for _ in 0..2 {
            h.send(reward("1", "текст"));
        }
        assert_eq!(h.state().await.queue.len(), 1);
    }
    #[tokio::test(start_paused = true)]
    async fn reconcile_then_late_event_ignored() {
        let mut h = Harness::new(settings());
        h.send(TtsMsg::Reconcile(vec![reference("1"), reference("1")]));
        h.send(reward("1", "текст"));
        h.outcome("1", false).await;
        assert!(h.state().await.queue.is_empty());
        assert!(h.outcomes.try_recv().is_err());
        h.send(reward("2", "текст"));
        h.send(TtsMsg::Reconcile(vec![reference("2")]));
        assert_eq!(h.state().await.queue.len(), 1);
        assert!(h.outcomes.try_recv().is_err());
    }
    #[tokio::test(start_paused = true)]
    async fn queue_full_refunds_reward() {
        let mut h = Harness::new(settings());
        for _ in 0..20 {
            h.send(TtsMsg::Test("тест".into()));
        }
        h.send(TtsMsg::Test("лишний".into()));
        h.send(reward("1", "текст"));
        h.outcome("1", false).await;
        assert_eq!(h.state().await.queue.len(), 20);
    }
    #[tokio::test(start_paused = true)]
    async fn skip_fulfills_clear_refunds() {
        let mut h = Harness::new(settings());
        h.connect().await;
        h.send(reward("1", "один"));
        h.state().await;
        h.send(reward("2", "два"));
        h.send(TtsMsg::Clear);
        h.outcome("2", false).await;
        assert!(h.state().await.current.is_some());
        h.send(TtsMsg::Skip);
        h.outcome("1", true).await;
    }
    #[tokio::test(start_paused = true)]
    async fn skip_during_synth_drops_result() {
        stale_synth(Ok(1000)).await;
    }
    #[tokio::test(start_paused = true)]
    async fn stale_result_does_not_break_voice() {
        stale_synth(Err("MODEL_MISSING".into())).await;
    }
    async fn stale_synth(result: Result<i64, String>) {
        let (release, wait) = oneshot::channel();
        let wait = Arc::new(std::sync::Mutex::new(Some(wait)));
        let synth: SynthFn = Arc::new(move |_, _, _, _, out| {
            let wait = wait.lock().unwrap().take();
            let result = result.clone();
            Box::pin(async move {
                if let Some(wait) = wait {
                    wait.await.unwrap();
                    std::fs::write(out, []).unwrap();
                    result
                } else {
                    Ok(1000)
                }
            })
        });
        let mut h = Harness::custom(settings(), Some(synth));
        h.connect().await;
        h.send(reward("1", "один"));
        let id = h.state().await.current.unwrap().id;
        h.send(TtsMsg::Skip);
        h.outcome("1", true).await;
        h.send(reward("2", "два"));
        assert!(h.state().await.current.is_none());
        release.send(()).unwrap();
        let v = h.state().await;
        assert_eq!(v.current.unwrap().text, "два");
        assert!(v.warning.is_none());
        assert!(!h.dir.path().join(format!("{id}.wav")).exists());
        while let Ok(msg) = h.ws.try_recv() {
            assert!(!matches!(msg, WsOut::TtsPlay { id: played } if played == id));
        }
    }
    #[tokio::test(start_paused = true)]
    async fn skip_during_busy_wait_is_immediate() {
        let synth: SynthFn = Arc::new(|_, text, _, _, _| {
            Box::pin(async move {
                if text == "busy" {
                    Err("JOB_BUSY".into())
                } else {
                    Ok(1000)
                }
            })
        });
        let mut h = Harness::custom(settings(), Some(synth));
        h.connect().await;
        h.send(reward("1", "busy"));
        h.state().await;
        h.send(reward("2", "next"));
        h.send(TtsMsg::Skip);
        h.outcome("1", true).await;
        tokio::time::advance(Duration::from_millis(10)).await;
        assert_eq!(h.state().await.current.unwrap().text, "next");
        assert!(matches!(h.ws.try_recv(), Ok(WsOut::TtsStop)));
        assert!(matches!(h.ws.try_recv(), Ok(WsOut::TtsPlay { .. })));
    }
    #[tokio::test(start_paused = true)]
    async fn widget_missing_waits() {
        let h = Harness::new(settings());
        h.send(reward("1", "текст"));
        assert_eq!(h.state().await.queue.len(), 1);
        assert_eq!(h.calls.load(Ordering::SeqCst), 0);
        h.connect().await;
        assert_eq!(h.calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test(start_paused = true)]
    async fn safety_timeout_advances() {
        let mut h = Harness::new(settings());
        h.connect().await;
        h.send(reward("1", "текст"));
        h.state().await;
        h.send(TtsMsg::Widget(TtsWidgetEvent::Disconnected));
        h.state().await;
        tokio::time::advance(Duration::from_secs(11)).await;
        h.outcome("1", true).await;
    }
    #[tokio::test(start_paused = true)]
    async fn synth_busy_retries_then_fails() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let synth: SynthFn = Arc::new(move |_, _, _, _, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err("JOB_BUSY".into()) })
        });
        let mut h = Harness::custom(settings(), Some(synth));
        h.connect().await;
        h.send(reward("1", "текст"));
        h.state().await;
        for _ in 0..60 {
            tokio::time::advance(Duration::from_secs(1)).await;
            h.state().await;
        }
        h.outcome("1", false).await;
        assert_eq!(calls.load(Ordering::SeqCst), 61);
    }
    #[tokio::test(start_paused = true)]
    async fn missing_voice_rejects_at_intake() {
        let mut s = settings();
        s.profile_id = None;
        let mut h = Harness::new(s);
        h.send(reward("1", "текст"));
        h.send(TtsMsg::Test("тест".into()));
        h.send(TtsMsg::Chat(chat("чат %", &[])));
        h.outcome("1", false).await;
        let v = h.state().await;
        assert!(v.queue.is_empty());
        assert_eq!(v.warning.as_deref(), Some("tts_no_voice"));
    }
    #[tokio::test(start_paused = true)]
    async fn current_model_missing_breaks_voice() {
        let synth: SynthFn =
            Arc::new(|_, _, _, _, _| Box::pin(async { Err("MODEL_MISSING".into()) }));
        let mut h = Harness::custom(settings(), Some(synth));
        h.connect().await;
        h.send(reward("1", "текст"));
        h.outcome("1", false).await;
        assert_eq!(
            h.state().await.warning.as_deref(),
            Some("tts_model_missing")
        );
        h.send(reward("2", "ещё"));
        h.outcome("2", false).await;
        h.send(TtsMsg::Settings(settings()));
        assert_eq!(h.state().await.warning, None);
    }
    #[tokio::test(start_paused = true)]
    async fn widget_error_refunds() {
        let mut h = Harness::new(settings());
        h.connect().await;
        h.send(reward("1", "текст"));
        let id = h.state().await.current.unwrap().id;
        h.send(TtsMsg::Widget(TtsWidgetEvent::Error(id)));
        h.outcome("1", false).await;
    }
    #[tokio::test(start_paused = true)]
    async fn widget_reconnect_replays_current() {
        let mut h = Harness::new(settings());
        h.connect().await;
        h.send(reward("1", "текст"));
        let id = h.state().await.current.unwrap().id;
        assert!(matches!(h.ws.try_recv(), Ok(WsOut::TtsPlay { id: got }) if got == id));
        h.send(TtsMsg::Widget(TtsWidgetEvent::Disconnected));
        tokio::time::advance(Duration::from_secs(9)).await;
        h.connect().await;
        assert!(matches!(h.ws.try_recv(), Ok(WsOut::TtsPlay { id: got }) if got == id));
        tokio::time::advance(Duration::from_secs(9)).await;
        assert!(
            h.state().await.current.is_some(),
            "deadline restarted on replay"
        );
        h.send(TtsMsg::Widget(TtsWidgetEvent::Ended(id)));
        h.outcome("1", true).await;
    }
    #[tokio::test(start_paused = true)]
    async fn settings_without_voice_warns_immediately() {
        let h = Harness::new(settings());
        let mut s = settings();
        s.profile_id = None;
        s.chat_enabled = true;
        h.send(TtsMsg::Settings(s));
        assert_eq!(h.state().await.warning.as_deref(), Some("tts_no_voice"));
    }
    #[tokio::test(start_paused = true)]
    async fn wav_deleted_after_play() {
        let h = Harness::new(settings());
        h.connect().await;
        h.send(TtsMsg::Test("текст".into()));
        let id = h.state().await.current.unwrap().id;
        let path = h.dir.path().join(format!("{id}.wav"));
        assert!(path.exists());
        h.send(TtsMsg::Widget(TtsWidgetEvent::Ended(id)));
        h.state().await;
        assert!(!path.exists());
    }
    #[tokio::test(start_paused = true)]
    async fn chat_relogin_warning_only_when_enabled() {
        let mut s = settings();
        s.chat_enabled = false;
        let h = Harness::new(s.clone());
        h.send(TtsMsg::ChatStatus(ChatStatus::Forbidden));
        assert!(h.state().await.warning.is_none());
        s.chat_enabled = true;
        h.send(TtsMsg::Settings(s));
        assert_eq!(h.state().await.warning.as_deref(), Some("tts_chat_relogin"));
        h.send(TtsMsg::ChatStatus(ChatStatus::Failed));
        h.send(TtsMsg::Chat(chat("без метки", &[])));
        assert!(h.state().await.warning.is_none());
        h.send(TtsMsg::ChatStatus(ChatStatus::Forbidden));
        h.send(TtsMsg::ChatStatus(ChatStatus::Ok));
        assert!(h.state().await.warning.is_none());
    }
    #[tokio::test(start_paused = true)]
    async fn settings_clears_warning() {
        let h = Harness::new(settings());
        h.send(TtsMsg::Warning("ошибка".into()));
        assert!(h.state().await.warning.is_some());
        h.send(TtsMsg::Settings(settings()));
        assert!(h.state().await.warning.is_none());
    }
}
