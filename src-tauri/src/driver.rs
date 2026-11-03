use crate::{
    db::{Db, RedemptionState},
    i18n,
    limits::Limits,
    model::*,
    music::{self, Decision, Resolution},
    obs::ObsCfg,
    player::{self, Action, Input, Player, WidgetState},
    request, router,
    server::{ServerChannels, WsOut},
    spotify::{self, Pkce, Spotify},
    twitch::{
        eventsub::{self, EsSignal},
        helix::{Helix, RewardError},
    },
    youtube,
};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::Emitter;
use tokio::sync::{mpsc, watch};
#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub limits: Limits,
    pub reward_title: String,
    pub reward_cost: u32,
    pub reward_cooldown_s: u32,
    pub music_reward_id: Option<String>,
    pub port: u16,
    pub active_theme: String,
    pub youtube_api_key: Option<String>,
    pub obs: ObsCfg,
    pub spotify_client_id: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            reward_title: "Заказать трек".into(),
            reward_cost: 500,
            reward_cooldown_s: 0,
            music_reward_id: None,
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
    SettingsChanged(Settings),
    SaveSettings(Settings, tokio::sync::oneshot::Sender<Result<(), String>>),
    SpotifyVolume(tokio::sync::oneshot::Sender<Option<u8>>),
    TwitchStatus(tokio::sync::oneshot::Sender<bool>),
    SpotifyStatus(tokio::sync::oneshot::Sender<crate::app::SpotifyStatus>),
}
pub struct DriverCtx {
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
            eprintln!("External service operation failed");
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
fn keep_reward_id(current: &Settings, mut incoming: Settings) -> Settings {
    incoming.music_reward_id = current.music_reward_id.clone();
    incoming
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
                eprintln!("Failed to save redemption decision: {error}");
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
    Reward(Result<String>),
    Spotify(Result<Tokens>),
}
fn redeem(
    c: &DriverCtx,
    r: Redemption,
    inflight: &mut HashSet<String>,
    tx: &mpsc::UnboundedSender<Internal>,
) {
    if router::route(&r.r.reward_id, c.settings.music_reward_id.as_deref()).is_none() {
        return;
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
fn reconcile(c: &DriverCtx, tx: &mpsc::UnboundedSender<Internal>) {
    if let (Some(h), Some(id)) = (c.helix.clone(), c.settings.music_reward_id.clone()) {
        let tx = tx.clone();
        background(async move {
            let rows = h.unfulfilled(&id).await?;
            let _ = tx.send(Internal::Reconcile(rows));
            Ok(())
        });
    }
}
fn connect_twitch(
    c: &DriverCtx,
    tx: &mpsc::UnboundedSender<Internal>,
) -> Option<tokio::task::JoinHandle<()>> {
    let h = c.helix.clone()?;
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
    let s = c.settings.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let res = bounded(async {
            match h.find_manageable_reward(s.music_reward_id.as_deref(), &s.reward_title).await? {
                Some(id) => Ok(id),
                None => h.create_reward(&s.reward_title, s.reward_cost, "Название или ссылка на трек")
                    .await.map_err(|error| anyhow!(match error {
                        RewardError::NotAffiliate => "нужен статус компаньона/партнёра",
                        RewardError::DuplicateTitle => "награда с таким названием уже есть, но создана не этой программой — переименуйте её или удалите",
                        RewardError::Other(_) => "Не удалось создать награду",
                    })),
            }
        }).await;
        let _ = tx.send(Internal::Reward(res));
    });
    Some(task)
}
pub async fn run(mut ctx: DriverCtx, mut rx: mpsc::Receiver<DriverMsg>) {
    let running_port = ctx.ws.running_port;
    let (tx, mut internal) = mpsc::unbounded_channel();
    let mut player = Player::new(ctx.settings.limits.max_duration_ms);
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
    let mut es = connect_twitch(&ctx, &tx);
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
            Internal::Msg(DriverMsg::Es(EsSignal::Resync)) => reconcile(&ctx, &tx),
            // Task 7 will forward these signals to the TTS engine.
            Internal::Msg(DriverMsg::Es(EsSignal::Chat(_) | EsSignal::ChatStatus(_))) => {}
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
                let (cur, q) = player.snapshot();
                let recent = ctx
                    .db
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .history_recent(ctx.settings.limits.no_repeat_last_n);
                let decision = match (res, recent) {
                    (Ok(res), Ok(mut recent)) => {
                        recent.reverse();
                        music::decide(
                            res,
                            &ctx.settings.limits,
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
            Internal::Reward(Ok(id)) => {
                ctx.settings.music_reward_id = Some(id);
                let _ = ctx
                    .db
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_setting("settings", &ctx.settings);
                if let Some(h) = ctx.helix.clone() {
                    let id = ctx.settings.music_reward_id.clone().unwrap();
                    let cooldown = ctx.settings.reward_cooldown_s;
                    background(async move {
                        h.update_reward(
                            &id,
                            &serde_json::json!({
                                "is_paused": false,
                                "global_cooldown_setting": {
                                    "is_enabled": cooldown > 0,
                                    "global_cooldown_seconds": cooldown
                                }
                            }),
                        )
                        .await
                    });
                }
                reconcile(&ctx, &tx);
            }
            Internal::Reward(Err(e)) => warn(&ctx, Some(e.to_string())),
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
            Internal::Msg(DriverMsg::SaveSettings(s, reply)) => {
                let result = apply_settings(
                    &mut ctx.settings,
                    &ctx.db,
                    &ctx.active_theme,
                    &ctx.ws,
                    ctx.helix.clone(),
                    s,
                )
                .await;
                if result.is_ok() {
                    let aa = player.step(
                        Input::SetMaxDuration(ctx.settings.limits.max_duration_ms),
                        now(),
                    );
                    actions(&ctx, &player, aa, &tx, &mut polls);
                }
                let _ = reply.send(result.map_err(|_| "settings_save_failed".into()));
            }
            Internal::Msg(DriverMsg::SettingsChanged(s)) => {
                if apply_settings(
                    &mut ctx.settings,
                    &ctx.db,
                    &ctx.active_theme,
                    &ctx.ws,
                    ctx.helix.clone(),
                    s,
                )
                .await
                .is_ok()
                {
                    let aa = player.step(
                        Input::SetMaxDuration(ctx.settings.limits.max_duration_ms),
                        now(),
                    );
                    actions(&ctx, &player, aa, &tx, &mut polls);
                } else {
                    warn(&ctx, Some("settings_save_failed".into()));
                }
            }

            Internal::Msg(DriverMsg::TwitchLogin(t)) => {
                if let Some(old) = es.take() {
                    old.abort();
                }
                ctx.helix = Some(crate::app::twitch_client(t, ctx.app.clone()));
                es = connect_twitch(&ctx, &tx);
            }
            Internal::Msg(DriverMsg::TwitchLogout) => {
                if let Some(old) = es.take() {
                    old.abort();
                }
                if let (Some(h), Some(id)) =
                    (ctx.helix.take(), ctx.settings.music_reward_id.clone())
                {
                    background(async move {
                        h.update_reward(&id, &serde_json::json!({"is_paused":true}))
                            .await
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
    helix: Option<Arc<Helix>>,
    incoming: Settings,
) -> Result<()> {
    let s = keep_reward_id(current, incoming);
    db.lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_setting("settings", &s)?;
    if s.active_theme != current.active_theme {
        *active_theme.write().unwrap_or_else(|e| e.into_inner()) = s.active_theme.clone();
        let _ = ws.out_tx.send(WsOut::Reload);
    }
    if let (Some(h), Some(id)) = (helix, s.music_reward_id.clone()) {
        if (s.reward_title.as_str(), s.reward_cost, s.reward_cooldown_s)
            != (
                current.reward_title.as_str(),
                current.reward_cost,
                current.reward_cooldown_s,
            )
        {
            let body = serde_json::json!({"title":s.reward_title,"cost":s.reward_cost,"global_cooldown_setting":{"is_enabled":s.reward_cooldown_s > 0,"global_cooldown_seconds":s.reward_cooldown_s}});
            background(async move { h.update_reward(&id, &body).await });
        }
    }
    *current = s;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let ws = crate::server::serve(0, std::env::temp_dir(), theme.clone(), state)
            .await
            .unwrap();
        // The server remains reachable throughout settings changes.
        {
            let port = ws.running_port;
            (current, db, theme, ws, port)
        }
    }
    #[tokio::test]
    async fn apply_settings_theme_change_sends_reload() {
        let (mut current, db, theme, ws, _) = settings_fixture().await;
        let mut events = ws.out_tx.subscribe();
        let mut incoming = current.clone();
        incoming.active_theme = "glass".into();
        apply_settings(&mut current, &db, &theme, &ws, None, incoming)
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
        apply_settings(&mut current, &db, &theme, &ws, None, incoming)
            .await
            .unwrap();
        assert_eq!(
            db.lock()
                .unwrap()
                .get_setting::<Settings>("settings")
                .unwrap()
                .unwrap()
                .port,
            port
        );
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
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, method, path, query_param},
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/helix/users"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data":[{"id":"b","login":"streamer"}]})),
            )
            .mount(&server)
            .await;
        Mock::given(method("PATCH")).and(path("/helix/channel_points/custom_rewards"))
            .and(query_param("id", "owned"))
            .and(body_json(serde_json::json!({"title":"New reward","cost":123,"global_cooldown_setting":{"is_enabled":true,"global_cooldown_seconds":60}})))
            .respond_with(ResponseTemplate::new(200)).expect(1).mount(&server).await;
        let helix = Arc::new(Helix::new(
            "client".into(),
            Tokens {
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                expires_at: i64::MAX,
            },
            &server.uri(),
            &server.uri(),
            |_| {},
        ));
        let (mut current, db, theme, ws, _) = settings_fixture().await;
        current.music_reward_id = Some("owned".into());
        let mut incoming = current.clone();
        incoming.music_reward_id = Some("stale".into());
        incoming.reward_title = "New reward".into();
        incoming.reward_cost = 123;
        incoming.reward_cooldown_s = 60;
        apply_settings(&mut current, &db, &theme, &ws, Some(helix), incoming)
            .await
            .unwrap();
        assert_eq!(current.music_reward_id.as_deref(), Some("owned"));
        assert_eq!(
            db.lock()
                .unwrap()
                .get_setting::<Settings>("settings")
                .unwrap()
                .unwrap()
                .music_reward_id
                .as_deref(),
            Some("owned")
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .any(|r| r.method == "PATCH")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        server.verify().await;
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
        current.music_reward_id = Some("owned".into());
        for id in [None, Some("stale".into())] {
            let mut incoming = Settings::default();
            incoming.music_reward_id = id;
            incoming.reward_cost = 123;
            let updated = keep_reward_id(&current, incoming);
            assert_eq!(updated.music_reward_id.as_deref(), Some("owned"));
            assert_eq!(updated.reward_cost, 123);
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
