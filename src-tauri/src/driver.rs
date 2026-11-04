use crate::{
    db::{Db, RedemptionState},
    i18n,
    model::*,
    music::{self, Decision, Resolution},
    obs::ObsCfg,
    player::{self, Action, Input, Player, WidgetState},
    request,
    rewards::{Lifecycle, ensure_reward, retire_reward},
    rules::{self, ActionKind, MusicDecision, RewardOp, Rules},
    server::{ServerChannels, WsOut},
    spotify::{self, Pkce, Spotify},
    tts::engine::TtsMsg,
    twitch::{
        eventsub::{self, EsSignal},
        helix::Helix,
    },
    youtube,
};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::Emitter;
use tokio::sync::{mpsc, watch};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(default)]
    pub rules: Rules,
    pub port: u16,
    pub active_theme: String,
    pub youtube_api_key: Option<String>,
    pub obs: ObsCfg,
    pub spotify_client_id: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            rules: Rules::default(),
            port: 47821,
            active_theme: "underground".into(),
            youtube_api_key: None,
            obs: ObsCfg::default(),
            spotify_client_id: None,
        }
    }
}
pub enum DriverMsg {
    Cmd(Input),
    Es(EsSignal),
    ResolveDone {
        red: Redemption,
        res: Result<Resolution>,
    },
    TwitchLogin(Tokens),
    TwitchLogout,
    SpotifyLoginStart(Pkce),
    SetVolume(u8),
    WidgetTest,
    Warning(String),
    SaveSettings(
        Settings,
        tokio::sync::oneshot::Sender<Result<Settings, String>>,
    ),
    RuleErrors(tokio::sync::oneshot::Sender<BTreeMap<String, String>>),
    SpotifyVolume(tokio::sync::oneshot::Sender<Option<u8>>),
    TwitchStatus(tokio::sync::oneshot::Sender<bool>),
    SpotifyStatus(tokio::sync::oneshot::Sender<crate::app::SpotifyStatus>),
}
pub struct DriverCtx {
    pub tts: mpsc::UnboundedSender<TtsMsg>,
    pub tts_outcomes: mpsc::UnboundedReceiver<(RedemptionRef, bool)>,
    pub db: Arc<Mutex<Db>>,
    pub active_theme: Arc<std::sync::RwLock<String>>,
    pub state_tx: watch::Sender<WidgetState>,
    pub ws: ServerChannels,
    pub app: tauri::AppHandle,
    pub settings: Settings,
    pub spotify: Option<Arc<Spotify>>,
    pub helix: Option<Arc<Helix>>,
}
pub enum ReconcileStep {
    Skip,
    Fulfill(RedemptionRef),
    Recancel(RedemptionRef),
    Process(Redemption),
}
pub fn plan_reconcile(states: &[(Redemption, RedemptionState)]) -> Vec<ReconcileStep> {
    states
        .iter()
        .map(|(r, s)| match s {
            RedemptionState::Queued => ReconcileStep::Skip,
            RedemptionState::Started => ReconcileStep::Fulfill(r.r.clone()),
            RedemptionState::Cancelled => ReconcileStep::Recancel(r.r.clone()),
            RedemptionState::Unknown => ReconcileStep::Process(r.clone()),
        })
        .collect()
}
const WAIT: Duration = Duration::from_secs(10);
async fn bounded<T>(f: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(WAIT, f)
        .await
        .map_err(|_| anyhow!("service timeout"))?
}
fn background(f: impl std::future::Future<Output = Result<()>> + Send + 'static) {
    tokio::spawn(async move {
        if bounded(f).await.is_err() {
            crate::dlog!("music", "External service operation failed");
        }
    });
}
fn publish(c: &DriverCtx, s: WidgetState) {
    c.state_tx.send_replace(s.clone());
    let _ = c.ws.out_tx.send(WsOut::State(s.clone()));
    let _ = c.app.emit("state", s);
}
fn warn(c: &DriverCtx, text: Option<String>) {
    let mut s = c.state_tx.borrow().clone();
    s.warning = text;
    publish(c, s);
}
fn status(c: &DriverCtx, r: RedemptionRef, ok: bool) {
    if let Some(h) = c.helix.clone() {
        background(async move { h.set_redemption_status(&r, ok).await });
    }
}
fn chat(c: &DriverCtx, text: String) {
    if let Some(h) = c.helix.clone() {
        background(async move { h.chat(&text).await });
    }
}
#[derive(Default)]
struct PollGate {
    in_flight: bool,
    last_play: Option<std::time::Instant>,
}
impl PollGate {
    fn start(&mut self) -> Option<std::time::Instant> {
        if self.in_flight {
            return None;
        }
        self.in_flight = true;
        Some(std::time::Instant::now())
    }
    fn finish(&mut self, issued: std::time::Instant) -> bool {
        self.in_flight = false;
        self.last_play.is_none_or(|play| issued >= play)
    }
}
pub fn load_settings(db: &Db) -> Result<(Settings, bool)> {
    let Some(raw) = db.get_setting::<serde_json::Value>("settings")? else {
        return Ok((Settings::default(), false));
    };
    let migrated = raw.get("rules").is_none();
    let mut settings: Settings = serde_json::from_value(raw.clone())?;
    if migrated {
        settings.rules = rules::from_legacy(&raw);
    }
    Ok((settings, migrated))
}
pub(crate) fn migrate_stored(db: &Db) -> Result<Settings> {
    let (settings, migrated) = load_settings(db)?;
    if migrated {
        let raw = db
            .get_setting::<serde_json::Value>("settings")?
            .expect("loaded settings");
        db.set_setting("settings_legacy", &raw)?;
        db.set_setting("settings", &settings)?;
    }
    Ok(settings)
}
fn prepare_actions(
    mut aa: Vec<Action>,
    mut mark: impl FnMut(&Action) -> bool,
    mut persist: impl FnMut(),
) -> Vec<Action> {
    aa.retain(|a| match a {
        // Fulfill even if history cannot be saved: Twitch must not replay this redemption.
        Action::Started(_) => {
            mark(a);
            true
        }
        Action::Cancel(_, _) => mark(a),
        _ => true,
    });
    aa.retain(|a| {
        if matches!(a, Action::Persist) {
            persist();
            false
        } else {
            true
        }
    });
    aa
}
fn actions(
    c: &DriverCtx,
    p: &Player,
    aa: Vec<Action>,
    tx: &mpsc::UnboundedSender<Internal>,
    polls: &mut PollGate,
) {
    // Commit local redemption decisions before replacing the queue snapshot.
    let aa = prepare_actions(
        aa,
        |a| {
            let result = match a {
                Action::Started(i) => {
                    c.db.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .history_push(i, now())
                }
                Action::Cancel(i, _) => {
                    if let Some(r) = &i.redemption {
                        c.db.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .mark_cancelled(&r.redemption_id)
                    } else {
                        Ok(())
                    }
                }
                _ => return true,
            };
            if let Err(error) = &result {
                crate::dlog!("music", "Failed to save redemption decision: {error}");
                warn(c, Some("Не удалось сохранить решение по заказу".into()));
            }
            result.is_ok()
        },
        || {
            let (cur, q) = p.snapshot();
            if c.db
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .save_state(cur.as_ref(), &q)
                .is_err()
            {
                warn(c, Some("Не удалось сохранить очередь".into()));
            }
        },
    );
    for a in aa {
        match a {
            Action::Persist => unreachable!("persistence already completed"),
            Action::Started(i) => {
                if let Some(r) = i.redemption {
                    status(c, r, true)
                }
            }
            Action::Cancel(i, why) => {
                if let Some(r) = i.redemption {
                    status(c, r, false);
                    chat(
                        c,
                        i18n::chat_rejected(&i.requester, i18n::cancel_text(&why)),
                    );
                }
            }
            Action::SpotifyPlay {
                item_id,
                uri,
                device_id,
                position_ms,
            } => {
                polls.last_play = Some(std::time::Instant::now());
                let sp = c.spotify.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let result = tokio::time::timeout(WAIT, async {
                        let sp = sp.ok_or(PlayError::Unauthorized)?;
                        let device = match device_id {
                            Some(d) => Some(d),
                            None => spotify::pick_device(
                                &sp.devices().await.map_err(|_| PlayError::Net)?,
                            ),
                        };
                        if device.is_none() {
                            return Err(PlayError::NoDevice);
                        }
                        sp.play(&uri, device.as_deref(), position_ms).await
                    })
                    .await
                    .unwrap_or(Err(PlayError::Net));
                    let _ = tx.send(Internal::Input(Input::PlayResult { item_id, result }));
                });
            }
            Action::SpotifyPause | Action::SpotifyResume => {
                if let Some(sp) = c.spotify.clone() {
                    background(async move {
                        if matches!(a, Action::SpotifyPause) {
                            sp.pause().await
                        } else {
                            sp.resume().await
                        }
                    });
                }
            }
            Action::ResolveFallback(i) => {
                let key = c.settings.youtube_api_key.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let video_id = bounded(async {
                        match key {
                            Some(k) => {
                                youtube::search(
                                    &k,
                                    &format!("{} {}", i.track.artist, i.track.title),
                                    "https://www.googleapis.com",
                                )
                                .await
                            }
                            None => Err(anyhow!("YouTube key unavailable")),
                        }
                    })
                    .await
                    .map_err(|_| ());
                    let _ = tx.send(Internal::Input(Input::FallbackResolved {
                        item_id: i.id,
                        video_id,
                    }));
                });
            }
            Action::Widget(mut s) => {
                // Keep the ten-second visual test on screen while playback continues.
                if c.state_tx.borrow().now.as_ref().is_some_and(|i| i.id == -1) {
                    continue;
                }
                if s.warning.is_none() {
                    s.warning = c.state_tx.borrow().warning.clone();
                }
                publish(c, s);
            }
            Action::Warn(w) => warn(c, w),
            Action::YtPlay {
                item_id,
                video_id,
                start_ms,
            } => {
                let _ = c.ws.out_tx.send(WsOut::YtPlay {
                    item_id,
                    video_id,
                    start_ms,
                });
            }
            Action::YtUnmute => {
                let _ = c.ws.out_tx.send(WsOut::YtUnmute);
            }
            Action::YtStop => {
                let _ = c.ws.out_tx.send(WsOut::YtStop);
            }
            Action::YtPause => {
                let _ = c.ws.out_tx.send(WsOut::YtPause);
            }
            Action::YtResume => {
                let _ = c.ws.out_tx.send(WsOut::YtResume);
            }
        }
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
enum Internal {
    Input(Input),
    Poll {
        issued: std::time::Instant,
        result: Result<Option<PlaybackSnapshot>, ()>,
    },
    WidgetReset(u64),
    Msg(DriverMsg),
    Reconcile(Vec<Redemption>),
    RewardReady {
        trigger_id: String,
        known_id: Option<String>,
        result: Result<Option<String>, String>,
    },
    Retired {
        reward_id: String,
        result: Result<(), String>,
    },
    TtsReconcile(Vec<Redemption>),
    TtsOutcome(RedemptionRef, bool),
    Spotify(Result<Tokens>),
}
fn redeem(
    c: &DriverCtx,
    r: Redemption,
    inflight: &mut HashSet<String>,
    tx: &mpsc::UnboundedSender<Internal>,
) {
    if let Some(route) = rules::route_reward(&c.settings.rules, &r.r.reward_id)
        && route.action == ActionKind::Tts
    {
        let _ = c.tts.send(TtsMsg::Reward {
            r,
            active: route.active,
        });
        return;
    }
    match rules::music_decision(&c.settings.rules, &r.r.reward_id) {
        MusicDecision::Ignore => return,
        MusicDecision::Refund => {
            let db = c.db.lock().unwrap_or_else(|e| e.into_inner());
            if !inflight.contains(&r.r.redemption_id)
                && db.redemption_state(&r.r.redemption_id).ok() == Some(RedemptionState::Unknown)
                && db.mark_cancelled(&r.r.redemption_id).is_ok()
            {
                status(c, r.r, false);
            }
            return;
        }
        MusicDecision::Process => {}
    }
    if c.db
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .redemption_state(&r.r.redemption_id)
        .ok()
        != Some(RedemptionState::Unknown)
        || !inflight.insert(r.r.redemption_id.clone())
    {
        return;
    }
    let sp = c.spotify.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let res = bounded(async {
            music::resolve(
                request::parse_request(&r.user_input),
                sp.as_deref(),
                "https://www.youtube.com",
            )
            .await
        })
        .await;
        let _ = tx.send(Internal::Msg(DriverMsg::ResolveDone { red: r, res }));
    });
}
fn reconcile_reward(
    c: &DriverCtx,
    action: ActionKind,
    id: String,
    tx: &mpsc::UnboundedSender<Internal>,
) {
    if let Some(h) = c.helix.clone() {
        let tx = tx.clone();
        background(async move {
            let rows = h.unfulfilled(&id).await?;
            let _ = tx.send(match action {
                ActionKind::Music => Internal::Reconcile(rows),
                ActionKind::Tts => Internal::TtsReconcile(rows),
            });
            Ok(())
        });
    }
}
fn reconcile(c: &DriverCtx, tx: &mpsc::UnboundedSender<Internal>) {
    for (action, id) in rules::reconcile_targets(&c.settings.rules) {
        reconcile_reward(c, action, id, tx);
    }
}
fn connect_twitch(
    c: &DriverCtx,
    player: &mut Player,
    polls: &mut PollGate,
    lifecycle: &mut Lifecycle,
    tx: &mpsc::UnboundedSender<Internal>,
) -> Option<tokio::task::JoinHandle<()>> {
    let h = c.helix.clone()?;
    for op in lifecycle.on_connect(&c.settings.rules) {
        exec(op, c, player, polls, lifecycle, tx);
    }
    let (out, mut rx) = mpsc::channel(64);
    let forward = tx.clone();
    let eventsub_helix = h.clone();
    let task = tokio::spawn(async move {
        tokio::select! {
            _ = eventsub::run(eventsub_helix, "wss://eventsub.wss.twitch.tv/ws".into(), out) => {},
            _ = async {
                while let Some(signal) = rx.recv().await {
                    if forward.send(Internal::Msg(DriverMsg::Es(signal))).is_err() { break; }
                }
            } => {},
        }
    });
    Some(task)
}
pub async fn run(mut ctx: DriverCtx, mut rx: mpsc::Receiver<DriverMsg>) {
    let running_port = ctx.ws.running_port;
    let (tx, mut internal) = mpsc::unbounded_channel();
    let mut player = Player::new(ctx.settings.rules.music.cfg.limits.max_duration_ms);
    let mut pending = None;
    let mut polls = PollGate::default();
    let mut widget_test = 0u64;
    let mut inflight = HashSet::new();
    let restored = ctx
        .db
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .load_state();
    if let Ok((current, queue)) = restored {
        let aa = player.step(Input::Restore { current, queue }, now());
        actions(&ctx, &player, aa, &tx, &mut polls);
    }
    let mut lifecycle = Lifecycle::default();
    let mut es = connect_twitch(&ctx, &mut player, &mut polls, &mut lifecycle, &tx);
    reconcile(&ctx, &tx);
    let mut tick = tokio::time::interval(Duration::from_millis(player::POLL_MS));
    loop {
        let event = tokio::select! {
            _ = tick.tick() => {
                if let Some(issued) = polls.start() {
                let sp = ctx.spotify.clone();
                let tx = tx.clone();
                // Poll even while Idle: successful polls clear the outage/fallback flag.
                tokio::spawn(async move {
                    let result = bounded(async {
                        match sp {
                            Some(s) => s.player().await,
                            None => Err(anyhow!("Spotify disconnected")),
                        }
                    }).await.map_err(|_| ());
                    let _ = tx.send(Internal::Poll { issued, result });
                });
                }
                Internal::Input(Input::Tick)
            },
            message = rx.recv() => match message {
                Some(message) => Internal::Msg(message),
                None => break,
            },
            Some(message) = internal.recv() => message,
            Some((r, ok)) = ctx.tts_outcomes.recv() => Internal::TtsOutcome(r, ok),
            Some(input) = ctx.ws.yt_rx.recv() => Internal::Input(input),
            Some((code, state)) = ctx.ws.oauth_rx.recv() => {
                match code {
                    Err(_) => {
                        if spotify::take_pending_if_state_matches(&mut pending, &state).is_some() {
                            warn(&ctx, Some("Вход Spotify отменён".into()));
                        }
                    },
                    Ok(code) => {
                        if let Some(pkce) = spotify::take_pending_if_state_matches(&mut pending, &state) {
                            if let Some(id) = ctx.settings.spotify_client_id.clone() {
                                let tx = tx.clone();
                                let redirect = format!("http://127.0.0.1:{}/callback/spotify", running_port);
                                tokio::spawn(async move {
                                    let result = bounded(Spotify::exchange_code(
                                        &id, &code, &redirect, &pkce.verifier, "https://accounts.spotify.com"
                                    )).await;
                                    let _ = tx.send(Internal::Spotify(result));
                                });
                            }
                        }
                    },
                }
                continue
            }

        };
        let event = match event {
            Internal::Poll { issued, result } => {
                if !polls.finish(issued) {
                    continue;
                }
                Internal::Input(Input::Spotify(result))
            }
            other => other,
        };
        match event {
            Internal::Poll { .. } => unreachable!(),
            Internal::WidgetReset(generation) => {
                if generation == widget_test {
                    publish(&ctx, player.widget_state(now()));
                }
            }
            Internal::Input(i) | Internal::Msg(DriverMsg::Cmd(i)) => {
                let aa = player.step(i, now());
                actions(&ctx, &player, aa, &tx, &mut polls);
            }
            Internal::Msg(DriverMsg::Es(EsSignal::Redemption(r))) => {
                redeem(&ctx, r, &mut inflight, &tx)
            }
            Internal::Msg(DriverMsg::Es(EsSignal::Resync)) => {
                reconcile(&ctx, &tx);
            }
            Internal::Msg(DriverMsg::Es(EsSignal::Chat(m))) => {
                let msg = match rules::route_chat(&ctx.settings.rules, &m) {
                    Some(route) if route.action == ActionKind::Tts => TtsMsg::Chat {
                        msg: m,
                        text: route.text,
                    },
                    _ => TtsMsg::ChatSeen,
                };
                let _ = ctx.tts.send(msg);
            }
            Internal::Msg(DriverMsg::Es(EsSignal::ChatStatus(s))) => {
                let _ = ctx.tts.send(TtsMsg::ChatStatus(s));
            }
            Internal::TtsOutcome(r, ok) => status(&ctx, r, ok),
            Internal::TtsReconcile(rows) => {
                let _ = ctx
                    .tts
                    .send(TtsMsg::Reconcile(rows.into_iter().map(|r| r.r).collect()));
            }

            Internal::Reconcile(rows) => {
                let states: Result<Vec<_>> = rows
                    .into_iter()
                    .map(|r| {
                        let s = ctx
                            .db
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .redemption_state(&r.r.redemption_id)?;
                        Ok((r, s))
                    })
                    .collect();
                match states {
                    Ok(states) => {
                        for step in plan_reconcile(&states) {
                            match step {
                                ReconcileStep::Skip => {}
                                ReconcileStep::Fulfill(r) => status(&ctx, r, true),
                                ReconcileStep::Recancel(r) => status(&ctx, r, false),
                                ReconcileStep::Process(r) => redeem(&ctx, r, &mut inflight, &tx),
                            }
                        }
                    }
                    Err(_) => warn(&ctx, Some("Не удалось сверить заказы".into())),
                }
            }
            Internal::Msg(DriverMsg::ResolveDone { red, res }) => {
                let id = red.r.redemption_id.clone();
                if !inflight.contains(&id) {
                    continue;
                }
                if rules::music_decision(&ctx.settings.rules, &red.r.reward_id)
                    != MusicDecision::Process
                {
                    if ctx
                        .db
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .mark_cancelled(&id)
                        .is_ok()
                    {
                        status(&ctx, red.r, false);
                    }
                    inflight.remove(&id);
                    continue;
                }
                let (cur, q) = player.snapshot();
                let recent = ctx
                    .db
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .history_recent(ctx.settings.rules.music.cfg.limits.no_repeat_last_n);
                let decision = match (res, recent) {
                    (Ok(res), Ok(mut recent)) => {
                        recent.reverse();
                        music::decide(
                            res,
                            &ctx.settings.rules.music.cfg.limits,
                            &red.user_login,
                            &q,
                            cur.as_ref(),
                            &recent,
                        )
                    }
                    _ => Decision::Reject(i18n::TEXT_SERVICE_DOWN.into()),
                };
                match decision {
                    Decision::Enqueue(t) => {
                        let inserted = ctx
                            .db
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert_item(&t, &red.user_login, Some(&red.r));
                        match inserted {
                            Ok(item) => {
                                let aa = player.step(Input::Enqueue(item), now());
                                actions(&ctx, &player, aa, &tx, &mut polls);
                                chat(
                                    &ctx,
                                    i18n::chat_added(&red.user_login, &t.artist, &t.title, {
                                        let (current, queue) = player.snapshot();
                                        queue.len() + usize::from(current.is_some())
                                    }),
                                );
                            }
                            Err(crate::db::DbError::Duplicate) => {}
                            Err(_) => {
                                let _ = ctx
                                    .db
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .mark_cancelled(&id);
                                status(&ctx, red.r, false);
                                chat(
                                    &ctx,
                                    i18n::chat_rejected(&red.user_login, i18n::TEXT_SERVICE_DOWN),
                                );
                            }
                        }
                    }
                    Decision::Reject(why) => {
                        if ctx
                            .db
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .mark_cancelled(&id)
                            .is_ok()
                        {
                            status(&ctx, red.r, false);
                            chat(&ctx, i18n::chat_rejected(&red.user_login, &why));
                        }
                    }
                }
                inflight.remove(&id);
            }
            Internal::RewardReady {
                trigger_id,
                known_id,
                result,
            } => {
                let errors = lifecycle.errors.clone();
                let applied = result.as_ref().ok().and_then(|id| id.clone());
                let (changed, ops) =
                    lifecycle.on_ready(&mut ctx.settings.rules, &trigger_id, known_id, result);
                if changed {
                    persist_settings(&ctx);
                }
                let _ = ctx
                    .tts
                    .send(TtsMsg::Settings(ctx.settings.rules.tts_runtime()));
                if let Some(id) = applied
                    && let Some((action, trigger)) = ctx.settings.rules.trigger(&trigger_id)
                    && matches!(&trigger.kind, rules::TriggerKind::Reward {reward_id:Some(current),..} if current == &id)
                {
                    reconcile_reward(&ctx, action, id, &tx);
                }
                for op in ops {
                    exec(op, &ctx, &mut player, &mut polls, &mut lifecycle, &tx);
                }
                publish_errors(&ctx, &lifecycle, &errors);
            }
            Internal::Retired { reward_id, result } => {
                let errors = lifecycle.errors.clone();
                if lifecycle.on_retired(&mut ctx.settings.rules, &reward_id, result) {
                    persist_settings(&ctx);
                }
                publish_errors(&ctx, &lifecycle, &errors);
            }

            Internal::Spotify(result) => match result {
                Ok(t) => {
                    crate::app::save_tokens("spotify", &t, ctx.app.clone());
                    if let Some(id) = ctx.settings.spotify_client_id.clone() {
                        ctx.spotify = Some(crate::app::spotify_client(id, t, ctx.app.clone()));
                    }
                }
                Err(_) => warn(&ctx, Some("Не удалось войти в Spotify".into())),
            },
            Internal::Msg(DriverMsg::SpotifyLoginStart(p)) => pending = Some(p),
            Internal::Msg(DriverMsg::SetVolume(v)) => {
                if let Some(s) = ctx.spotify.clone() {
                    background(async move { s.set_volume(v).await });
                }
            }
            Internal::Msg(DriverMsg::Warning(message)) => warn(&ctx, Some(message)),
            Internal::Msg(DriverMsg::WidgetTest) => {
                let mut s = ctx.state_tx.borrow().clone();
                widget_test += 1;
                let generation = widget_test;
                let reset = tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    let _ = reset.send(Internal::WidgetReset(generation));
                });
                s.visible = true;
                s.now = Some(widget_test_item());
                publish(&ctx, s);
            }
            Internal::Msg(DriverMsg::SpotifyVolume(reply)) => {
                let sp = ctx.spotify.clone();
                tokio::spawn(async move {
                    let volume = if let Some(sp) = sp {
                        bounded(sp.volume()).await.ok().flatten()
                    } else {
                        None
                    };
                    let _ = reply.send(volume);
                });
            }
            Internal::Msg(DriverMsg::TwitchStatus(reply)) => {
                let _ = reply.send(ctx.helix.is_some());
            }
            Internal::Msg(DriverMsg::SpotifyStatus(reply)) => {
                let sp = ctx.spotify.clone();
                tokio::spawn(async move {
                    let connected = sp.is_some();
                    let premium = if let Some(sp) = sp {
                        bounded(sp.me_product()).await.ok().map(|p| p == "premium")
                    } else {
                        None
                    };
                    let _ = reply.send(crate::app::SpotifyStatus { connected, premium });
                });
            }
            Internal::Msg(DriverMsg::RuleErrors(reply)) => {
                let _ = reply.send(lifecycle.errors.clone());
            }
            Internal::Msg(DriverMsg::SaveSettings(s, reply)) => {
                let errors = lifecycle.errors.clone();
                let result = apply_settings(
                    &mut ctx.settings,
                    &ctx.db,
                    &ctx.active_theme,
                    &ctx.ws,
                    &mut lifecycle,
                    s,
                )
                .await;
                if let Ok((_, ops)) = &result {
                    let _ = ctx
                        .tts
                        .send(TtsMsg::Settings(ctx.settings.rules.tts_runtime()));
                    let aa = player.step(
                        Input::SetMaxDuration(ctx.settings.rules.music.cfg.limits.max_duration_ms),
                        now(),
                    );
                    actions(&ctx, &player, aa, &tx, &mut polls);
                    for op in ops.clone() {
                        exec(op, &ctx, &mut player, &mut polls, &mut lifecycle, &tx);
                    }
                }
                publish_errors(&ctx, &lifecycle, &errors);
                let _ = reply.send(
                    result
                        .map(|_| ctx.settings.clone())
                        .map_err(|_| "settings_save_failed".into()),
                );
            }

            Internal::Msg(DriverMsg::TwitchLogin(t)) => {
                if let Some(old) = es.take() {
                    old.abort();
                }
                ctx.helix = Some(crate::app::twitch_client(t, ctx.app.clone()));
                es = connect_twitch(&ctx, &mut player, &mut polls, &mut lifecycle, &tx);
                reconcile(&ctx, &tx);
            }
            Internal::Msg(DriverMsg::TwitchLogout) => {
                let _ = ctx.tts.send(TtsMsg::ChatStatus(eventsub::ChatStatus::Ok));
                if let Some(old) = es.take() {
                    old.abort();
                }
                if let Some(h) = ctx.helix.take() {
                    let ids = ctx.settings.rules.reward_ids();
                    tokio::spawn(async move {
                        let pause = serde_json::json!({"is_paused":true});
                        let _ = tokio::time::timeout(
                            Duration::from_secs(2),
                            futures_util::future::join_all(
                                ids.iter().map(|id| h.update_reward(id, &pause)),
                            ),
                        )
                        .await;
                    });
                }
                let _ = crate::secrets::write("twitch", None);
            }
        }
    }
    if let Some(es) = es {
        es.abort();
    }
}

fn widget_test_item() -> QueueItem {
    QueueItem {
        id: -1,
        track: Track {
            source: Source::YouTube {
                video_id: String::new(),
            },
            title: "Тест виджета".into(),
            artist: "Stream Assist".into(),
            duration_ms: Some(10_000),
            explicit: None,
            cover_url: None,
        },
        requester: "Stream Assist".into(),
        redemption: None,
        started: true,
    }
}

async fn apply_settings(
    current: &mut Settings,
    db: &Arc<Mutex<Db>>,
    active_theme: &Arc<std::sync::RwLock<String>>,
    ws: &ServerChannels,
    lifecycle: &mut Lifecycle,
    mut incoming: Settings,
) -> Result<(Settings, Vec<RewardOp>)> {
    let old = current.clone();
    let (rules, ops) = lifecycle.on_save(&current.rules, incoming.rules);
    incoming.rules = rules;
    if let Err(error) = db
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_setting("settings", &incoming)
    {
        for op in &ops {
            match op {
                RewardOp::Ensure { trigger_id, .. } => lifecycle.release_ensure(trigger_id),
                RewardOp::Retire { reward_id, .. } => lifecycle.release_retire(reward_id),
                RewardOp::Patch { .. } => {}
            }
        }
        return Err(error.into());
    }
    if incoming.active_theme != current.active_theme {
        *active_theme.write().unwrap_or_else(|e| e.into_inner()) = incoming.active_theme.clone();
        let _ = ws.out_tx.send(WsOut::Reload);
    }
    *current = incoming;
    Ok((old, ops))
}
fn persist_settings(ctx: &DriverCtx) {
    if let Err(error) = ctx
        .db
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_setting("settings", &ctx.settings)
    {
        crate::dlog!("music", "Failed to save settings: {error}");
    }
}
fn publish_errors(ctx: &DriverCtx, lifecycle: &Lifecycle, previous: &BTreeMap<String, String>) {
    if previous != &lifecycle.errors {
        let _ = ctx.app.emit("rule_errors", &lifecycle.errors);
    }
}
fn exec(
    op: RewardOp,
    ctx: &DriverCtx,
    player: &mut Player,
    polls: &mut PollGate,
    lifecycle: &mut Lifecycle,
    tx: &mpsc::UnboundedSender<Internal>,
) {
    match op {
        RewardOp::Ensure {
            action,
            trigger_id,
            known_id,
            title,
            cost,
            create,
        } => {
            let Some(h) = ctx.helix.clone() else {
                lifecycle.release_ensure(&trigger_id);
                return;
            };
            let exclude = lifecycle.exclude(&ctx.settings.rules);
            let tx = tx.clone();
            tokio::spawn(async move {
                let result = bounded(ensure_reward(
                    &h,
                    action,
                    known_id.as_deref(),
                    &title,
                    cost,
                    create,
                    &exclude,
                ))
                .await
                .map_err(|e| e.to_string());
                let _ = tx.send(Internal::RewardReady {
                    trigger_id,
                    known_id,
                    result,
                });
            });
        }
        RewardOp::Patch { reward_id, body } => {
            if let Some(h) = ctx.helix.clone() {
                background(async move { h.update_reward(&reward_id, &body).await });
            }
        }
        RewardOp::Retire { action, reward_id } => {
            let mut keep = HashSet::new();
            let mut tts_reply = None;
            match action {
                ActionKind::Music => {
                    let (current, queue) = player.snapshot();
                    for item in queue {
                        if item
                            .redemption
                            .as_ref()
                            .is_some_and(|r| r.reward_id == reward_id)
                        {
                            let aa = player.step(Input::Remove(item.id), now());
                            actions(ctx, player, aa, tx, polls);
                        }
                    }
                    if let Some(r) = current
                        .filter(|item| item.started)
                        .and_then(|i| i.redemption)
                        .filter(|r| r.reward_id == reward_id)
                    {
                        keep.insert(r.redemption_id);
                    }
                }
                ActionKind::Tts => {
                    let (reply, rx) = tokio::sync::oneshot::channel();
                    if ctx
                        .tts
                        .send(TtsMsg::DropReward {
                            reward_id: reward_id.clone(),
                            reply,
                        })
                        .is_ok()
                    {
                        tts_reply = Some(rx);
                    }
                }
            }
            let Some(h) = ctx.helix.clone() else {
                lifecycle.release_retire(&reward_id);
                return;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let result = tokio::time::timeout(Duration::from_secs(30), async {
                    if let Some(rx) = tts_reply
                        && let Ok(Some(id)) = rx.await
                    {
                        keep.insert(id);
                    }
                    retire_reward(&h, &reward_id, &keep).await
                })
                .await
                .map_err(|_| "service timeout".to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
                let _ = tx.send(Internal::Retired { reward_id, result });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn load_settings_migrates_legacy() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let raw = serde_json::json!({"music_reward_id":"m1", "reward_cost":700,
            "tts":{"reward_id":"t1", "reward_enabled":true,"marker":"!!"}});
        db.set_setting("settings", &raw).unwrap();
        let (settings, migrated) = load_settings(&db).unwrap();
        assert!(migrated);
        assert_eq!(settings.rules, rules::from_legacy(&raw));
        assert_eq!(settings.port, Settings::default().port);
    }
    #[test]
    fn load_settings_new_form() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        assert!(!load_settings(&db).unwrap().1);
        let mut settings = Settings::default();
        settings
            .rules
            .set_reward_id("music-reward", Some("m1".into()));
        db.set_setting("settings", &settings).unwrap();
        let (loaded, migrated) = load_settings(&db).unwrap();
        assert!(!migrated);
        assert_eq!(
            serde_json::to_value(loaded).unwrap(),
            serde_json::to_value(settings).unwrap()
        );
    }
    #[test]
    fn bootstrap_resaves_new_form() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let raw = serde_json::json!({"music_reward_id":"m1","tts":{"reward_id":"t1"}});
        db.set_setting("settings", &raw).unwrap();
        let settings = migrate_stored(&db).unwrap();
        assert_eq!(
            db.get_setting::<serde_json::Value>("settings_legacy")
                .unwrap(),
            Some(raw.clone())
        );
        assert_eq!(load_settings(&db).unwrap().0.rules, settings.rules);
        assert!(!load_settings(&db).unwrap().1);
        migrate_stored(&db).unwrap();
        assert_eq!(
            db.get_setting::<serde_json::Value>("settings_legacy")
                .unwrap(),
            Some(raw)
        );
    }
    #[tokio::test]
    async fn settings_change_keeps_tts_reward_id() {
        let (mut current, db, theme, ws, _) = settings_fixture().await;
        current
            .rules
            .set_reward_id("tts-reward", Some("owned".into()));
        let mut incoming = current.clone();
        incoming
            .rules
            .set_reward_id("tts-reward", Some("stale".into()));
        apply_settings(
            &mut current,
            &db,
            &theme,
            &ws,
            &mut Lifecycle::default(),
            incoming,
        )
        .await
        .unwrap();
        let saved = load_settings(&db.lock().unwrap()).unwrap().0;
        assert_eq!(saved.rules, current.rules);
        assert!(
            matches!(&current.rules.tts.triggers[0].kind, rules::TriggerKind::Reward {reward_id:Some(id),..} if id == "owned")
        );
        ws.task.unwrap().abort();
    }
    async fn settings_fixture() -> (
        Settings,
        Arc<Mutex<Db>>,
        Arc<std::sync::RwLock<String>>,
        ServerChannels,
        u16,
    ) {
        let current = Settings::default();
        let db = Arc::new(Mutex::new(
            Db::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let theme = Arc::new(std::sync::RwLock::new(current.active_theme.clone()));
        let (_, state) = watch::channel(WidgetState {
            visible: false,
            now: None,
            queue: vec![],
            progress_ms: 0,
            mode: "Idle".into(),
            warning: None,
        });
        let ws = crate::server::serve(
            0,
            std::env::temp_dir(),
            theme.clone(),
            state,
            std::env::temp_dir(),
        )
        .await
        .unwrap();
        // The server remains reachable throughout settings changes.
        {
            let port = ws.running_port;
            (current, db, theme, ws, port)
        }
    }
    #[tokio::test]
    async fn apply_settings_failed_write_releases_reward_ops() {
        let (mut current, db, theme, ws, _) = settings_fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.db");
        *db.lock().unwrap() = Db::open(&path).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_settings BEFORE INSERT ON settings
                 BEGIN SELECT RAISE(FAIL, 'test write failure'); END;",
            )
            .unwrap();
        current
            .rules
            .set_reward_id("music-reward", Some("owned".into()));
        let original = current.clone();
        let mut incoming = current.clone();
        incoming.rules.music.triggers.clear();
        incoming.rules.tts.triggers[0].enabled = true;
        incoming.active_theme = "glass".into();
        let mut events = ws.out_tx.subscribe();
        let mut lifecycle = Lifecycle::default();
        let (_, expected_ops) =
            Lifecycle::default().on_save(&current.rules, incoming.rules.clone());
        assert!(
            expected_ops
                .iter()
                .any(|op| matches!(op, RewardOp::Ensure { .. }))
        );
        assert!(
            expected_ops
                .iter()
                .any(|op| matches!(op, RewardOp::Retire { .. }))
        );

        assert!(
            apply_settings(
                &mut current,
                &db,
                &theme,
                &ws,
                &mut lifecycle,
                incoming.clone(),
            )
            .await
            .is_err()
        );
        assert_eq!(current.rules, original.rules);
        assert_eq!(current.active_theme, original.active_theme);
        assert_eq!(*theme.read().unwrap(), original.active_theme);
        assert!(events.try_recv().is_err());

        connection
            .execute_batch("DROP TRIGGER fail_settings;")
            .unwrap();
        let (_, retry_ops) =
            apply_settings(&mut current, &db, &theme, &ws, &mut lifecycle, incoming)
                .await
                .unwrap();
        assert_eq!(retry_ops, expected_ops);
        assert_eq!(
            load_settings(&db.lock().unwrap()).unwrap().0.rules,
            current.rules
        );
        ws.task.unwrap().abort();
    }
    #[tokio::test]
    async fn apply_settings_theme_change_sends_reload() {
        let (mut current, db, theme, ws, _) = settings_fixture().await;
        let mut events = ws.out_tx.subscribe();
        let mut incoming = current.clone();
        incoming.active_theme = "glass".into();
        apply_settings(
            &mut current,
            &db,
            &theme,
            &ws,
            &mut Lifecycle::default(),
            incoming,
        )
        .await
        .unwrap();
        assert!(matches!(events.recv().await.unwrap(), WsOut::Reload));
        assert_eq!(*theme.read().unwrap(), "glass");
        ws.task.unwrap().abort();
    }
    #[tokio::test]
    async fn apply_settings_port_change_deferred() {
        let (mut current, db, theme, ws, running_port) = settings_fixture().await;
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = occupied.local_addr().unwrap().port();
        let task_id = ws.task.as_ref().unwrap().id();
        let mut incoming = current.clone();
        incoming.port = port;
        // Saving an occupied port succeeds because no bind is attempted.
        apply_settings(
            &mut current,
            &db,
            &theme,
            &ws,
            &mut Lifecycle::default(),
            incoming,
        )
        .await
        .unwrap();
        assert_eq!(load_settings(&db.lock().unwrap()).unwrap().0.port, port);
        assert_eq!(ws.running_port, running_port);
        assert!(
            reqwest::get(format!("http://127.0.0.1:{running_port}/widget/sdk.js"))
                .await
                .unwrap()
                .status()
                .is_success()
        );
        assert_eq!(ws.task.as_ref().unwrap().id(), task_id);
        assert!(!ws.task.as_ref().unwrap().is_finished());
        ws.task.unwrap().abort();
    }
    #[tokio::test]
    async fn apply_settings_keeps_reward_id_and_updates_reward() {
        let (mut current, db, theme, ws, _) = settings_fixture().await;
        current
            .rules
            .set_reward_id("music-reward", Some("owned".into()));
        let mut incoming = current.clone();
        incoming
            .rules
            .set_reward_id("music-reward", Some("stale".into()));
        if let rules::TriggerKind::Reward { cost, .. } = &mut incoming.rules.music.triggers[0].kind
        {
            *cost = 123;
        }
        let (_, ops) = apply_settings(
            &mut current,
            &db,
            &theme,
            &ws,
            &mut Lifecycle::default(),
            incoming,
        )
        .await
        .unwrap();
        assert!(ops.iter().any(|op| matches!(op, RewardOp::Patch {reward_id,body} if reward_id == "owned" && body["cost"] == 123)));
        assert!(
            matches!(&current.rules.music.triggers[0].kind, rules::TriggerKind::Reward {reward_id:Some(id),..} if id == "owned")
        );
        assert_eq!(
            load_settings(&db.lock().unwrap()).unwrap().0.rules,
            current.rules
        );
        ws.task.unwrap().abort();
    }

    #[test]
    fn widget_test_uses_fake_track_without_redemption() {
        let sample = widget_test_item();
        assert_eq!(sample.id, -1);
        assert_eq!(sample.track.duration_ms, Some(10_000));
        assert!(!sample.track.title.is_empty());
        assert!(sample.redemption.is_none());
    }
    #[test]
    fn stale_poll_after_play_is_dropped() {
        let mut gate = PollGate::default();
        let issued = gate.start().unwrap();
        assert!(gate.start().is_none());
        gate.last_play = Some(issued + Duration::from_millis(1));
        assert!(!gate.finish(issued));
        let next = gate.start().unwrap();
        gate.last_play = Some(next);
        assert!(gate.finish(next));
    }
    #[test]
    fn settings_change_keeps_music_reward_id() {
        let mut current = Settings::default();
        current
            .rules
            .set_reward_id("music-reward", Some("owned".into()));
        for id in [None, Some("stale".into())] {
            let mut incoming = current.clone();
            incoming.rules.set_reward_id("music-reward", id);
            let (updated, _) = Lifecycle::default().on_save(&current.rules, incoming.rules);
            assert!(
                matches!(&updated.music.triggers[0].kind, rules::TriggerKind::Reward {reward_id:Some(id),..} if id == "owned")
            );
        }
    }
}

#[cfg(test)]
mod action_order_tests {
    use super::*;
    #[test]
    fn started_kept_when_history_push_fails() {
        let item = QueueItem {
            id: 1,
            track: Track {
                source: Source::Spotify { uri: "s1".into() },
                title: "song".into(),
                artist: "artist".into(),
                duration_ms: None,
                explicit: None,
                cover_url: None,
            },
            requester: "viewer".into(),
            redemption: Some(RedemptionRef {
                reward_id: "reward".into(),
                redemption_id: "redemption".into(),
            }),
            started: false,
        };
        let mut started = item.clone();
        started.started = true;
        let mut marks = 0;
        let mut persisted = false;
        let aa = prepare_actions(
            vec![
                Action::Started(started.clone()),
                Action::Cancel(item, player::CancelReason::YtUnavailable),
                Action::Persist,
            ],
            |_| {
                marks += 1;
                false
            },
            || persisted = true,
        );
        assert_eq!(marks, 2);
        assert!(persisted);
        assert_eq!(aa, vec![Action::Started(started)]);
    }
    #[test]
    fn local_marks_then_persist_then_network() {
        let trace = std::cell::RefCell::new(Vec::new());
        // A mixed batch deliberately places network work before persistence.
        let item = QueueItem {
            id: 1,
            track: crate::model::Track {
                source: crate::model::Source::Spotify {
                    uri: "spotify:track:1".into(),
                },
                title: "song".into(),
                artist: "artist".into(),
                duration_ms: None,
                explicit: None,
                cover_url: None,
            },
            requester: "viewer".into(),
            redemption: None,
            started: false,
        };
        let aa = prepare_actions(
            vec![
                Action::SpotifyPause,
                Action::Persist,
                Action::Cancel(item.clone(), crate::player::CancelReason::YtUnavailable),
                Action::Started(item),
            ],
            |_| {
                trace.borrow_mut().push("mark");
                true
            },
            || trace.borrow_mut().push("persist"),
        );
        for _ in aa {
            trace.borrow_mut().push("network");
        }
        assert_eq!(
            *trace.borrow(),
            ["mark", "mark", "persist", "network", "network", "network"]
        );
    }
}
