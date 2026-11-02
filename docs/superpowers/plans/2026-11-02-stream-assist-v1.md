# Stream Assist v1 (музыка по наградам) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Десктоп-приложение на Tauri: зрители заказывают треки за баллы Twitch, треки играют через Spotify стримера (YouTube — запасной), есть очередь, сторож воспроизведения, виджет OBS с темами и спокойная панель управления.

**Architecture:** Rust-ядро (tokio) внутри Tauri v2. Вся логика очереди и сторожа — чистый детерминированный автомат `Player::step(input, now) -> Vec<Action>`, который тестируется без сети. Async-драйвер исполняет `Action` через HTTP-клиенты Spotify/Twitch/YouTube/OBS и скармливает результаты обратно в автомат. Локальный axum-сервер на `127.0.0.1` отдаёт виджет, `sdk.js`, темы, WebSocket состояния и OAuth-callback. Панель — Svelte, общается с ядром через Tauri-команды.

**Tech Stack:** Rust 1.93, Tauri 2.x, tokio, axum 0.8, reqwest (rustls, json), rusqlite (bundled), keyring 3, tokio-tungstenite, obws, sha2, base64, rand, include_dir, zip, serde/serde_json, thiserror; dev: wiremock, tokio-test. Фронт: Node 24, Svelte 5 + Vite + TypeScript (шаблон create-tauri-app `svelte-ts`), npm.

**Spec:** `docs/superpowers/specs/2026-11-02-stream-assist-design.md`

## Global Constraints

- Сервер слушает только `127.0.0.1`, порт по умолчанию `47821`, настраивается.
- Spotify redirect URI: `http://127.0.0.1:<port>/callback/spotify`. PKCE, без client secret. Client ID вводит стример.
- Spotify scopes: `user-read-playback-state user-modify-playback-state user-read-currently-playing`.
- Twitch: Device Code Flow, публичный клиент; Client ID берётся при сборке из `option_env!("TWITCH_CLIENT_ID")`.
- Twitch scopes: `channel:read:redemptions channel:manage:redemptions user:write:chat` (`user:read:chat` из спеки не нужен — чат не читаем; YAGNI).
- Refresh-токены — только в системном keyring (service `stream-assist`), никогда в SQLite/файлах.
- Очередь своя (не очередь Spotify); следующий трек — `PUT /v1/me/player/play` с `uris`.
- Опрос Spotify раз в 2.5 с; refresh токена за 5 мин до истечения.
- Баллы возвращаются (CANCELED) **только до старта** трека; трек стартовал → FULFILLED сразу.
- Ручная смена трека в Spotify → очередь `Suspended`, кнопка «Вернуть очередь»; ручная пауза → `Paused`, сторож не трогает.
- Фолбэк YouTube включается, только когда API Spotify недоступен **и** прошло ожидаемое время конца текущего трека.
- Пустая очередь → тишина, виджет прячется.
- Тема по умолчанию `underground`; встроенные: `underground`, `glass`, `crt`, `block`. Никаких логотипов/названий EA/NFS.
- Строки UI панели на русском в `src/lib/i18n/ru.ts`; сообщения в чат — в `src-tauri/src/i18n.rs`.
- Каждый Spotify/Twitch HTTP-клиент принимает `base_url` (для wiremock в тестах).

## Review Focus

1. **Конец трека vs ручная пауза у самого конца.** Spotify после трека с выключенным автоплеем отдаёт `is_playing=false` на том же URI — это конец, не пауза. Тест в Task 5: `track_end_near_duration_is_end_not_pause`.
2. **Грязный ввод заказа:** `open.spotify.com/intl-ru/track/ID?si=…`, `youtu.be/ID?t=10`, `youtube.com/shorts/ID`, `music.youtube.com/watch?v=ID`, эмодзи, 500 символов, пустая строка. Тесты в Task 3.
3. **Перезапуск программы посреди трека:** очередь и текущий элемент восстанавливаются из SQLite, повторного FULFILLED/CANCELED нет. Тест в Task 2 `current_item_survives_reopen` и Task 5 `restore_playing_does_not_refulfill`.
4. **Дубли выкупов от EventSub** (переподключение, повторная доставка, разбор UNFULFILLED при старте): дедупликация по `redemption_id`. Тест в Task 2 `duplicate_redemption_rejected`.
5. **Кастомная тема-zip с `../` путями или без `theme.json`:** импорт отклоняется, ничего не пишется вне `themes/`. Тест в Task 6 `zip_slip_rejected`.

---

## File Structure

```
package.json, vite.config.ts, src/                 # Svelte панель
  App.svelte, lib/api.ts, lib/i18n/ru.ts
  views/{NowQueue,Rewards,Limits,Widget,Connections}.svelte
src-tauri/
  Cargo.toml, tauri.conf.json
  assets/widget/sdk.js
  assets/widget/themes/{underground,glass,crt,block}/{theme.json,index.html,style.css}
  src/
    main.rs            # tauri bootstrap
    app.rs             # AppState, запуск сервера/драйвера, tauri-команды
    model.rs           # общие типы (Track, QueueItem, Source, Redemption)
    db.rs              # SQLite: settings, queue, history, seen_redemptions
    request.rs         # разбор текста заказа
    limits.rs          # Limits + check
    player.rs          # чистый автомат очереди/сторожа
    driver.rs          # async-цикл: опрос, исполнение Action
    secrets.rs         # keyring
    i18n.rs            # тексты чата
    server/{mod.rs,ws.rs,themes.rs}
    spotify.rs         # PKCE + Web API
    twitch/{auth.rs,helix.rs,eventsub.rs}
    youtube.rs         # oEmbed, Data API search
    obs.rs
    music.rs           # фича «музыка»: Redemption → Track → Player
    feature.rs         # трейт Feature + реестр
```

---

### Task 1: Каркас Tauri + Svelte, общие типы, шина событий

**Files:**
- Create: весь шаблон `npm create tauri-app@latest . -- --template svelte-ts --manager npm --identifier app.streamassist --yes` (если не принимает флаги — интерактивно с теми же значениями, имя `stream-assist`)
- Create: `src-tauri/src/model.rs`, `src-tauri/src/feature.rs`
- Modify: `src-tauri/src/main.rs`, `src-tauri/Cargo.toml`, `.gitignore`

**Interfaces:**
- Produces:
```rust
// model.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Source { Spotify { uri: String }, YouTube { video_id: String } }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track { pub source: Source, pub title: String, pub artist: String,
  pub duration_ms: Option<u64>, pub explicit: Option<bool>, pub cover_url: Option<String> }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueItem { pub id: i64, pub track: Track, pub requester: String,
  pub redemption: Option<RedemptionRef> }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RedemptionRef { pub reward_id: String, pub redemption_id: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Redemption { pub r: RedemptionRef, pub user_login: String, pub user_input: String }
#[derive(Clone, Debug)]
pub enum Event { RedemptionAdded(Redemption) }   // расширяется следующими фичами
// feature.rs
#[async_trait::async_trait]  // или нативный async fn + Box<dyn> через enum — на выбор исполнителя
pub trait Feature: Send + Sync { fn id(&self) -> &'static str; async fn on_event(&self, e: &Event); }
pub struct Bus { pub tx: tokio::sync::broadcast::Sender<Event> }  // capacity 256
```
Трейт `Feature` — требование спеки (расширяемость), не преждевременная абстракция.

- [ ] **Step 1:** сгенерировать шаблон, добавить в `.gitignore` `src-tauri/target/`, `src-tauri/gen/`.
- [ ] **Step 2:** добавить зависимости из Tech Stack в `src-tauri/Cargo.toml`; создать `model.rs`, `feature.rs`.
- [ ] **Step 3: тест** в `model.rs`: `source_roundtrip_json` — `Source::YouTube{video_id:"abc"}` сериализуется и десериализуется обратно равным.
- [ ] **Step 4:** `cd src-tauri && cargo test` → PASS; `npm install && npm run build` → успешно; `npm run tauri build -- --debug --no-bundle` → бинарник собирается.
- [ ] **Step 5:** `git add -A && git commit -m "chore: scaffold tauri+svelte, core model types"`

---

### Task 2: SQLite-хранилище

**Files:** Create `src-tauri/src/db.rs` (тесты внутри `#[cfg(test)]`, БД в `:memory:` и во временном файле).

**Interfaces:**
- Consumes: `model::*`
- Produces:
```rust
pub struct Db(rusqlite::Connection);
impl Db {
  pub fn open(path: &Path) -> Result<Db>;          // создаёт схему, user_version=1
  pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>>;
  pub fn set_setting<T: Serialize>(&self, key: &str, v: &T) -> Result<()>;
  pub fn queue_push(&self, track: &Track, requester: &str, r: Option<&RedemptionRef>) -> Result<QueueItem>;
  pub fn queue_list(&self) -> Result<Vec<QueueItem>>;     // по position
  pub fn queue_remove(&self, id: i64) -> Result<()>;
  pub fn queue_move(&self, id: i64, to_index: usize) -> Result<()>;
  pub fn set_current(&self, item: Option<&QueueItem>) -> Result<()>;  // играющий сейчас, вне очереди
  pub fn current(&self) -> Result<Option<QueueItem>>;
  pub fn history_push(&self, track: &Track) -> Result<()>;
  pub fn history_recent_uris(&self, n: usize) -> Result<Vec<Source>>;
  pub fn mark_redemption_seen(&self, redemption_id: &str) -> Result<bool>; // false если уже был
}
```
Настройки хранятся JSON-ом в таблице `settings(key TEXT PK, value TEXT)`.

- [ ] **Step 1: тесты:** `queue_push_list_order` (3 элемента в порядке добавления), `queue_move_reorders` (move id3→0 даёт [3,1,2]), `queue_remove_works`, `current_item_survives_reopen` (файл, set_current, drop, open → тот же item), `duplicate_redemption_rejected` (второй `mark_redemption_seen("x")` → false), `settings_roundtrip`.
- [ ] **Step 2:** `cargo test db::` → FAIL.
- [ ] **Step 3:** реализовать.
- [ ] **Step 4:** `cargo test db::` → PASS.
- [ ] **Step 5:** commit `feat(db): sqlite storage for queue, settings, history`.

---

### Task 3: Разбор заказа и лимиты

**Files:** Create `src-tauri/src/request.rs`, `src-tauri/src/limits.rs`.

**Interfaces:**
- Produces:
```rust
// request.rs
#[derive(Debug, PartialEq)]
pub enum RequestInput { SpotifyTrack(String /*track id*/), YouTube(String /*video id*/), Query(String), Empty }
pub fn parse_request(input: &str) -> RequestInput;
// limits.rs
#[derive(Clone, Serialize, Deserialize)]
pub struct Limits { pub max_duration_ms: Option<u64>, pub no_repeat_last_n: usize,
  pub max_per_user_in_queue: Option<usize>, pub block_explicit: bool,
  pub blocklist: Vec<String> /* подстроки, без регистра, по "artist — title" */ }
impl Default for Limits { /* 7 мин, 10, Some(2), false, [] */ }
#[derive(Debug, PartialEq)]
pub enum LimitViolation { TooLong, Repeat, UserQuota, Explicit, Blocked }
pub fn check(l: &Limits, t: &Track, requester: &str, queue: &[QueueItem],
             current: Option<&QueueItem>, recent: &[Source]) -> Result<(), LimitViolation>;
```
`duration_ms: None` → проверка длины пропускается (YouTube без ключа, длина проверится в плеере — Task 5). `explicit: None` → не блокируется.

- [ ] **Step 1: тесты request:** 
```rust
assert_eq!(parse_request("https://open.spotify.com/intl-ru/track/4uLU6hMCjMI75M1A2tKUQC?si=x"), RequestInput::SpotifyTrack("4uLU6hMCjMI75M1A2tKUQC".into()));
assert_eq!(parse_request("spotify:track:4uLU6hMCjMI75M1A2tKUQC"), SpotifyTrack(..same..));
assert_eq!(parse_request("https://youtu.be/dQw4w9WgXcQ?t=10"), YouTube("dQw4w9WgXcQ".into()));
assert_eq!(parse_request("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=x"), YouTube(..));
assert_eq!(parse_request("https://youtube.com/shorts/dQw4w9WgXcQ"), YouTube(..));
assert_eq!(parse_request("https://music.youtube.com/watch?v=dQw4w9WgXcQ"), YouTube(..));
assert_eq!(parse_request("  "), RequestInput::Empty);
assert_eq!(parse_request("Daft Punk 🎧 around the world"), Query("Daft Punk 🎧 around the world".into()));
// >200 символов обрезается до 200 по границе char, без паники
assert!(matches!(parse_request(&"я".repeat(500)), Query(s) if s.chars().count()==200));
assert_eq!(parse_request("https://open.spotify.com/album/xyz"), Query("https://open.spotify.com/album/xyz".into()));
```
- [ ] **Step 2: тесты limits:** `too_long`, `repeat_in_queue`, `repeat_in_recent`, `repeat_is_current`, `user_quota` (2 в очереди, третий → UserQuota), `explicit_blocked_only_when_flag_true`, `unknown_duration_passes`, `blocklist_case_insensitive`.
- [ ] **Step 3:** `cargo test request:: limits::` → FAIL; реализовать (`url` crate для ссылок); → PASS.
- [ ] **Step 4:** commit `feat: request parsing and viewer limits`.

---

### Task 4: Секреты и HTTP-клиент Spotify (PKCE + Web API)

**Files:** Create `src-tauri/src/secrets.rs`, `src-tauri/src/spotify.rs`.

**Interfaces:**
- Produces:
```rust
// secrets.rs
pub fn store(key: &str, value: &str) -> Result<()>;   // keyring service "stream-assist"
pub fn load(key: &str) -> Result<Option<String>>;
pub fn delete(key: &str) -> Result<()>;
// spotify.rs
pub struct Pkce { pub verifier: String, pub challenge: String, pub state: String }
pub fn new_pkce() -> Pkce;                            // verifier 64 симв. [A-Za-z0-9-._~], challenge = base64url_nopad(sha256)
pub fn authorize_url(client_id: &str, redirect: &str, p: &Pkce) -> String;
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Tokens { pub access_token: String, pub refresh_token: String, pub expires_at: i64 /*unix сек*/ }
pub struct Spotify { /* reqwest::Client, base_api, base_accounts, client_id, tokens: Mutex<Tokens> */ }
impl Spotify {
  pub fn new(client_id: String, tokens: Tokens, api: &str, accounts: &str) -> Self; // прод: https://api.spotify.com, https://accounts.spotify.com
  pub async fn exchange_code(client_id:&str, code:&str, redirect:&str, verifier:&str, accounts:&str) -> Result<Tokens>;
  pub async fn ensure_fresh(&self, now: i64) -> Result<()>;   // refresh если expires_at - now < 300; сохраняет новый refresh_token в secrets, если пришёл
  pub async fn me_product(&self) -> Result<String>;            // "premium" | ...
  pub async fn search_track(&self, q: &str) -> Result<Vec<Track>>;  // limit=5, market=from_token
  pub async fn track(&self, id: &str) -> Result<Track>;
  pub async fn player(&self) -> Result<Option<PlaybackSnapshot>>;  // 204 → None
  pub async fn play(&self, uri: &str, device_id: Option<&str>, position_ms: u64) -> Result<()>;
  pub async fn pause(&self) -> Result<()>;
  pub async fn resume(&self) -> Result<()>;
  pub async fn set_volume(&self, pct: u8) -> Result<()>;
}
// общая для player.rs:
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackSnapshot { pub device_id: Option<String>, pub is_playing: bool,
  pub track_uri: Option<String>, pub progress_ms: u64, pub duration_ms: u64 }
```
`PlaybackSnapshot` положить в `model.rs` (используется автоматом без зависимости от HTTP).

- [ ] **Step 1: тесты (wiremock):** `pkce_challenge_matches_rfc7636_vector` (verifier `dBjftJeZ4CVP-mJ92K9q2W8ivJXR5zGPRgo0vJyj6Mw` → challenge `E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM`), `player_204_is_none`, `player_parses_snapshot`, `refresh_called_when_expiring` (expires_at = now+100 → POST /api/token grant_type=refresh_token выполнен), `search_maps_track_fields` (title, artist через ", ", duration_ms, explicit, cover = самая большая image).
- [ ] **Step 2:** FAIL → реализовать → `cargo test spotify::` PASS. `secrets` без тестов (тонкая обёртка keyring).
- [ ] **Step 3:** commit `feat(spotify): pkce auth and web api client`.

---

### Task 5: Автомат Player (очередь + сторож) — ядро надёжности

**Files:** Create `src-tauri/src/player.rs` (только чистая логика, без async/сети).

**Interfaces:**
- Consumes: `QueueItem`, `PlaybackSnapshot`, `Source`.
- Produces:
```rust
pub enum Mode { Idle, Playing, Paused /*ручная пауза в Spotify*/, Suspended /*стример взял управление*/ }
pub enum Input {
  Enqueue(QueueItem), Skip, Remove(i64), Move(i64, usize), ResumeQueue, TogglePause,
  Spotify(Result<Option<PlaybackSnapshot>, ()>),     // результат опроса
  YtStarted, YtDuration(u64), YtEnded, YtError,     // события плеера виджета
  FallbackResolved { item_id: i64, video_id: Option<String> },
  Restore { current: Option<QueueItem>, queue: Vec<QueueItem> },  // при запуске
}
pub enum Action {
  SpotifyPlay { uri: String, device_id: Option<String>, position_ms: u64 },
  SpotifyPause, SpotifyResume,
  YtPlay { video_id: String }, YtStop,
  ResolveFallback(QueueItem),            // драйвер ищет на YouTube, потом шлёт FallbackResolved
  Fulfill(RedemptionRef), Cancel(RedemptionRef, CancelReason),
  Widget(WidgetState),                   // полное состояние для виджета/панели
  Persist,                               // драйвер сохраняет current+queue в Db
  Warn(Option<String>),                  // Some — показать плашку в панели, None — убрать
}
pub enum CancelReason { RemovedByStreamer, TooLong, YtUnavailable }
#[derive(Serialize, Clone)] pub struct WidgetState { pub visible: bool, pub now: Option<QueueItem>, pub queue: Vec<QueueItem>, pub progress_ms: u64, pub mode: &'static str, pub warning: Option<String> }
pub struct Player { /* mode, current, queue: VecDeque, last_device, last_snapshot, last_ok_ms, play_sent_ms, expected_end_ms, fallback: bool, max_duration_ms: Option<u64> */ }
impl Player {
  pub fn new(max_duration_ms: Option<u64>) -> Self;
  pub fn step(&mut self, input: Input, now_ms: i64) -> Vec<Action>;
}
pub const PLAY_GRACE_MS: i64 = 4000;      // после нашего play игнорируем рассинхрон
pub const END_TOLERANCE_MS: u64 = 3000;
pub const OUTAGE_MS: i64 = 30_000;
```
Правила (каждое — тест, имена ниже):
- Enqueue в Idle → старт трека; в остальных режимах → в конец (`Persist`, `Widget`).
- Старт Spotify-трека → `SpotifyPlay` + `Fulfill` сразу. Старт YouTube → `YtPlay`; `Fulfill` только на `YtStarted`; `YtError` до старта → `Cancel(YtUnavailable)` + следующий; `YtDuration(d)` > max до старта → `YtStop` + `Cancel(TooLong)` + следующий.
- Конец: snapshot на нашем URI, `!is_playing`, `progress_ms >= duration_ms - END_TOLERANCE_MS` **или** `progress_ms == 0 && last_progress >= duration - END_TOLERANCE_MS` **или** `track_uri == None` после такого прогресса → следующий трек / Idle + `Widget{visible:false}`.
- Ручная пауза: наш URI, `!is_playing`, прогресс далеко от конца → `Paused`, никаких действий кроме `Widget`. `is_playing` снова true → `Playing`.
- Чужой URI вне `PLAY_GRACE_MS` после нашего play → `Suspended`, `Warn(None)`, `Widget`; дальнейшие опросы ничего не делают. `ResumeQueue` → старт следующего из очереди (текущий уже fulfilled, не повторяется).
- Пропало устройство (`Ok(None)` или `device_id None`) при Playing вне grace → `SpotifyPlay{uri, device_id: last_device, position_ms: last_progress}`.
- `Err(())` подряд: ничего, пока `now - last_ok >= OUTAGE_MS` **и** `now >= expected_end_ms`; тогда `fallback=true`, `Warn(Some(..))`, для следующего Spotify-элемента → `ResolveFallback(item)`. `FallbackResolved{None}` → элемент не стартовал, значит `Cancel(YtUnavailable)` + следующий. Первый `Ok` после outage → `fallback=false`, `Warn(None)`; текущий YouTube-трек доигрывает, следующий снова Spotify.
- Skip/Remove текущего → `SpotifyPause`/`YtStop` + следующий; Remove из очереди (не текущий) → `Cancel(RemovedByStreamer)`.
- Пока играет YouTube, наш собственный `SpotifyPause` не считается ручной паузой (`Paused` только когда источник текущего — Spotify).
- `Restore{current: Some(..)}` → режим Playing **без** `Fulfill` и без нового play (сторож разберётся по следующему опросу).

- [ ] **Step 1: тесты** (по одному на правило): `enqueue_idle_starts_and_fulfills`, `enqueue_busy_appends`, `track_end_plays_next`, `track_end_near_duration_is_end_not_pause`, `last_track_end_goes_idle_and_hides`, `manual_pause_does_nothing`, `manual_resume_back_to_playing`, `foreign_track_suspends`, `foreign_track_within_grace_ignored`, `resume_queue_plays_next`, `device_lost_replays_at_position`, `short_outage_no_fallback`, `outage_before_expected_end_no_fallback`, `outage_after_end_triggers_fallback`, `fallback_unresolved_cancels`, `recovery_clears_warning`, `yt_error_before_start_cancels`, `yt_too_long_cancels`, `yt_started_fulfills`, `remove_queued_cancels`, `skip_current_no_refund`, `own_pause_during_youtube_not_manual`, `restore_playing_does_not_refulfill`.
- [ ] **Step 2:** `cargo test player::` → FAIL.
- [ ] **Step 3:** реализовать `step`.
- [ ] **Step 4:** `cargo test player::` → PASS (все 23).
- [ ] **Step 5:** commit `feat(player): queue and watchdog state machine`.

---

### Task 6: Локальный сервер: виджет, WS, темы, OAuth-callback

**Files:** Create `src-tauri/src/server/{mod.rs,ws.rs,themes.rs}`, `src-tauri/assets/widget/sdk.js`, `src-tauri/assets/widget/themes/underground/{theme.json,index.html,style.css}` (остальные темы — Task 10).

**Interfaces:**
- Consumes: `WidgetState`, `Input::Yt*`.
- Produces:
```rust
pub struct ServerHandle { pub widget_tx: tokio::sync::watch::Sender<WidgetState>,
  pub yt_rx: tokio::sync::mpsc::Receiver<player::Input>,
  pub spotify_code_rx: tokio::sync::mpsc::Receiver<(String /*code*/, String /*state*/)> }
pub async fn serve(port: u16, themes_dir: PathBuf, active_theme: Arc<RwLock<String>>) -> Result<ServerHandle>;
pub fn import_theme_zip(zip: &Path, themes_dir: &Path) -> Result<String /*theme id*/>;
pub fn list_themes(themes_dir: &Path) -> Vec<ThemeMeta>;   // встроенные + кастомные
#[derive(Serialize, Deserialize)] pub struct ThemeMeta { pub id: String, pub name: String, pub author: String, pub width: u32, pub height: u32 }
```
Маршруты: `GET /widget/music` → `index.html` активной темы; `GET /widget/theme/<id>/<file>`; `GET /widget/sdk.js`; `GET /ws` (сервер шлёт `{"type":"state", ...WidgetState}` при каждом изменении и сразу при подключении; клиент шлёт `{"type":"yt","event":"started|ended|error|duration","value":N}`); `GET /callback/spotify?code&state` → отдаёт страницу «Готово, вернитесь в программу».
Встроенные темы — `include_dir!`; кастомная тема с тем же id перекрывает встроенную.
`sdk.js` API для тем: `StreamAssist.on(name, fn)` где name ∈ `state|nowPlaying|queue|progress|hidden`; переподключение WS каждые 2 с; YouTube IFrame Player в контейнере `#sa-youtube` (если тема не дала — создаёт 200×200 за краем экрана), команды `yt_play`/`yt_stop` приходят от сервера в WS (`{"type":"yt_play","video_id":..}`); порядок: `cueVideoById` → отправить `duration` → `playVideo` → `started`.
Тема `underground` — порт макета `ug2-theme-v2.html` (см. спеку «Тема A»): плашка с круглой обложкой (`now.track.cover_url`, иначе заглушка — салатовая нота на тёмном), строки артист / «название» / «Заказал: ник», язычок MUSIC, прогресс; табличка «Очередь».

- [ ] **Step 1: тесты (axum + tower::ServiceExt):** `widget_serves_active_theme`, `sdk_served`, `ws_sends_state_on_connect`, `callback_forwards_code`, `zip_slip_rejected` (zip с `../evil.txt` → Err, файл не создан), `zip_without_theme_json_rejected`, `custom_theme_overrides_builtin`.
- [ ] **Step 2:** FAIL → реализовать → `cargo test server::` PASS.
- [ ] **Step 3: ручная проверка** (на этом этапе — временный `#[tokio::test] #[ignore]` или пример `cargo run --example widget_demo`, который поднимает сервер и шлёт фейковый `WidgetState`): открыть `http://127.0.0.1:47821/widget/music` в браузере — видна плашка и очередь в стиле UG2.
- [ ] **Step 4:** commit `feat(server): widget server, sdk, ws, theme import, underground theme`.

---

### Task 7: Twitch — Device Code, Helix, EventSub, чат

**Files:** Create `src-tauri/src/twitch/{mod.rs,auth.rs,helix.rs,eventsub.rs}`, `src-tauri/src/i18n.rs`.

**Interfaces:**
- Produces:
```rust
// auth.rs
pub struct DeviceStart { pub device_code: String, pub user_code: String, pub verification_uri: String, pub interval: u64, pub expires_in: u64 }
pub async fn device_start(client_id: &str, scopes: &str, base: &str) -> Result<DeviceStart>;  // POST /oauth2/device
pub async fn device_poll(client_id: &str, device_code: &str, scopes: &str, base: &str) -> Result<Option<Tokens>>; // authorization_pending → None
pub async fn refresh(client_id: &str, refresh_token: &str, base: &str) -> Result<Tokens>;   // без client_secret
// helix.rs
pub struct Helix { /* client_id, tokens, base, broadcaster_id */ }
impl Helix {
  pub async fn me(&self) -> Result<(String /*id*/, String /*login*/)>;
  pub async fn create_reward(&self, title: &str, cost: u32, prompt: &str) -> Result<String /*reward_id*/>; // is_user_input_required=true
  pub async fn update_reward(&self, id: &str, patch: &serde_json::Value) -> Result<()>;   // cost/title/is_paused/cooldown
  pub async fn reward_exists_and_manageable(&self, id: &str) -> Result<bool>;              // GET ?id&only_manageable_rewards=true
  pub async fn set_redemption_status(&self, r: &RedemptionRef, fulfilled: bool) -> Result<()>;
  pub async fn unfulfilled(&self, reward_id: &str) -> Result<Vec<Redemption>>;
  pub async fn chat(&self, text: &str) -> Result<()>;            // POST /helix/chat/messages sender_id=broadcaster
  pub async fn subscribe_redemptions(&self, session_id: &str) -> Result<()>;
}
// eventsub.rs
pub async fn run(helix: Arc<Helix>, ws_url: String, out: tokio::sync::mpsc::Sender<Redemption>) -> !; // welcome→subscribe; keepalive; session_reconnect; при обрыве — переподключение с backoff 1..30 с
pub fn parse_message(text: &str) -> Result<EsMsg>;  // Welcome{session_id}|Keepalive|Notification(Redemption)|Reconnect{url}|Revocation
// i18n.rs
pub fn chat_added(user: &str, artist: &str, title: &str, pos: usize) -> String;   // "@user трек «artist — title» в очереди, позиция N"
pub fn chat_rejected(user: &str, why: &str) -> String;                            // "@user заказ отклонён: why. Баллы возвращены."
pub fn violation_text(v: &LimitViolation) -> &'static str; // "слишком длинный трек", ...
```
- [ ] **Step 1: тесты:** `device_poll_pending_is_none`, `device_poll_success_tokens` (wiremock), `parse_welcome`, `parse_redemption_notification` (фикстура из документации Twitch: user_login, user_input, reward.id, id), `parse_reconnect`, `set_status_sends_patch` (PATCH `/channel_points/custom_rewards/redemptions?id&broadcaster_id&reward_id` body `{"status":"FULFILLED"}`), `unmanageable_reward_detected` (пустой data → false).
- [ ] **Step 2:** FAIL → реализовать → `cargo test twitch::` PASS.
- [ ] **Step 3:** commit `feat(twitch): device auth, helix, eventsub, chat`.

---

### Task 8: YouTube и OBS

**Files:** Create `src-tauri/src/youtube.rs`, `src-tauri/src/obs.rs`.

**Interfaces:**
```rust
// youtube.rs
pub async fn oembed(video_id: &str, base: &str) -> Result<Option<Track>>;   // 401/404 → None (не встраивается/нет); title/author_name/thumbnail_url; duration None
pub async fn search(api_key: &str, q: &str, base: &str) -> Result<Option<String /*video_id*/>>; // videoEmbeddable=true, type=video, maxResults=1
pub fn split_title(title: &str, channel: &str) -> (String /*artist*/, String /*title*/); // "A - B" / "A — B" → (A,B); иначе (channel без " - Topic", title)
// obs.rs
pub struct ObsCfg { pub host: String, pub port: u16, pub password: Option<String> }
pub async fn check(cfg: &ObsCfg) -> Result<String /*obs version*/>;
pub async fn add_widget_to_current_scene(cfg: &ObsCfg, url: &str, w: u32, h: u32) -> Result<()>; // input kind "browser_source", имя "Stream Assist — Music"; если уже есть — обновить url
```
- [ ] **Step 1: тесты:** `oembed_404_none`, `oembed_maps_fields`, `split_title_dash`, `split_title_topic_channel`, `search_returns_first_id`. OBS — без юнит-тестов (тонкая обёртка `obws`), проверка вручную в Task 11.
- [ ] **Step 2:** FAIL → реализовать → PASS.
- [ ] **Step 3:** commit `feat: youtube oembed/search, obs browser source`.

---

### Task 9: Фича «Музыка» и драйвер

**Files:** Create `src-tauri/src/music.rs`, `src-tauri/src/driver.rs`, `src-tauri/src/app.rs`; Modify `main.rs`.

**Interfaces:**
- Consumes: всё выше.
- Produces:
```rust
// music.rs — чистая часть «заказ → решение», тестируемая с фейковым резолвером
pub enum Resolved { Ok(Track), NotFound }
pub async fn resolve(input: RequestInput, sp: &Spotify, yt_base: &str) -> Result<Resolved>;
//   SpotifyTrack → sp.track; Query → sp.search_track, первый; YouTube → oembed; затем sp.search_track("artist title"),
//   если первый результат: artist совпадает (без регистра, contains в любую сторону) И title совпадает после удаления
//   скобок/"official video"/"lyrics" → Spotify-трек, иначе YouTube-трек.
pub enum Decision { Enqueue(Track), Reject(String /*причина для чата*/) }
pub fn decide(res: Resolved, limits: &Limits, requester: &str, queue: &[QueueItem], current: Option<&QueueItem>, recent: &[Source]) -> Decision;
// driver.rs
pub async fn run(state: Arc<AppState>) ;   // один tokio-цикл select!: tick 2.5 c (ensure_fresh + player()), redemption rx, yt rx, команды панели, исполнение Action
// app.rs
pub struct AppState { pub db: Mutex<Db>, pub player: Mutex<Player>, pub cmd_tx: mpsc::Sender<player::Input>, /* клиенты Option<…> после авторизации, настройки */ }
```
Поток выкупа: `mark_redemption_seen` (false → игнор) → `parse_request` → `resolve` → `decide` → `Enqueue`: `db.queue_push` + `player.step(Enqueue)` + чат `chat_added`; `Reject`: `set_redemption_status(false)` + чат `chat_rejected`. `Empty`/`NotFound` → Reject «ничего не найдено».
Исполнение `Action`: `Fulfill/Cancel` → Helix; `Persist` → `db.set_current` + перезапись очереди; `Widget` → `widget_tx`; `Yt*` → WS; `ResolveFallback` → `youtube::search` (нет ключа → `FallbackResolved{None}`).
Feature: `MusicFeature` реализует `Feature`, `on_event(RedemptionAdded)` только для `reward_id == settings.music_reward_id`.
Запуск: `Restore` из Db; если Twitch подключён — проверить `reward_exists_and_manageable`, иначе `create_reward` (новый id в settings); `update_reward{is_paused:false}`; разобрать `unfulfilled()` через тот же поток. Выход (`RunEvent::ExitRequested`) → `update_reward{is_paused:true}` (best-effort, таймаут 2 с).

- [ ] **Step 1: тесты music:** `decide_rejects_on_violation`, `decide_not_found_rejects`, `resolve_youtube_matches_spotify` (wiremock: oembed «Daft Punk - Around The World (Official Video)» + search вернул Daft Punk / Around the World → Track с Source::Spotify), `resolve_youtube_no_match_stays_youtube`.
- [ ] **Step 2:** FAIL → реализовать → `cargo test` (весь крейт) PASS.
- [ ] **Step 3:** commit `feat(music): redemption pipeline and driver`.

---

### Task 10: Остальные темы виджета

**Files:** Create `src-tauri/assets/widget/themes/{glass,crt,block}/{theme.json,index.html,style.css}`; `docs/themes.md` (контракт `sdk.js` для авторов тем: события, поля `WidgetState`, `#sa-youtube`, структура zip).

- [ ] **Step 1:** перенести B/C/D из `.superpowers/brainstorm/*/content/widget-themes.html` на `sdk.js`. Обложка из `cover_url` с заглушкой.
- [ ] **Step 2:** расширить тест `custom_theme_overrides_builtin` / добавить `builtin_themes_listed` → `list_themes` возвращает 4 id. `cargo test server::` PASS.
- [ ] **Step 3:** commit `feat(widget): glass, crt, block themes + theme authoring doc`.

---

### Task 11: Панель управления (Svelte) и Tauri-команды

**Files:** Create `src/lib/api.ts`, `src/lib/i18n/ru.ts`, `src/views/{NowQueue,Rewards,Limits,Widget,Connections}.svelte`; Modify `src/App.svelte`, `src-tauri/src/app.rs`.

**Interfaces (Tauri commands, `#[tauri::command]`, имена = функции `api.ts`):**
`get_state() -> WidgetState`, `skip()`, `remove_item(id:i64)`, `move_item(id:i64,to:usize)`, `toggle_pause()`, `resume_queue()`, `set_volume(pct:u8)`,
`get_settings() -> Settings`, `save_settings(s:Settings)` (Limits, reward title/cost/cooldown, port, active_theme, youtube_api_key, obs cfg),
`twitch_login_start() -> DeviceStart`, `twitch_login_poll() -> bool`, `twitch_logout()`,
`spotify_login(client_id:String) -> String /*url, открыть в браузере через plugin-opener*/`, `spotify_status() -> {connected, premium:bool}`,
`obs_check() -> String`, `obs_add_widget()`, `list_themes() -> Vec<ThemeMeta>`, `import_theme(path:String) -> String`, `widget_test()` (шлёт фейковый трек в виджет на 10 с).
Событие `state` (Tauri emit) при каждом `Action::Widget` — панель не опрашивает.

Экраны (спокойный UI, светлая/тёмная по системе, все строки из `ru.ts`):
- **Сейчас** — текущий трек, прогресс, кнопки Пропустить/Пауза, громкость; при `Suspended` — заметная кнопка «Вернуть очередь»; плашка предупреждения из `warning`; очередь с перетаскиванием (нативный HTML5 DnD) и кнопкой удалить.
- **Награды** — название, стоимость, кулдаун награды «Заказать трек».
- **Лимиты** — поля `Limits`.
- **Виджет** — выбор темы, URL для OBS с кнопкой «Копировать», «Добавить в OBS», «Тест», импорт zip.
- **Подключения** — Twitch (код + ссылка из Device Flow), Spotify-мастер (шаги: открыть dashboard, создать приложение, вставить Redirect URI `http://127.0.0.1:<port>/callback/spotify` с кнопкой копирования, вставить Client ID, войти; предупреждение если не Premium; совет выключить «Автовоспроизведение»), OBS (хост/порт/пароль, проверка), YouTube API key (необязательно, с пояснением зачем), подсказка про мониторинг аудио Browser Source.

- [ ] **Step 1:** реализовать команды в `app.rs` (тонкие: шлют `player::Input` в `cmd_tx` или читают/пишут Db).
- [ ] **Step 2:** реализовать экраны.
- [ ] **Step 3:** `npm run check` (svelte-check) — 0 ошибок; `cargo test` PASS; `npm run tauri dev` — ручной прогон чек-листа: подключить Twitch, Spotify, OBS; «Добавить в OBS» создаёт источник; «Тест» показывает плашку; выкуп с тестового аккаунта → трек играет, плашка видна; второй выкуп → в очередь; пропуск, удаление (баллы вернулись), ручная смена трека в Spotify → «Вернуть очередь»; выключить сеть на 40 с во время трека → предупреждение, после конца трека YouTube-фолбэк (если есть ключ); закрыть программу → награда на паузе в Twitch.
- [ ] **Step 4:** commit `feat(panel): control panel and commands`.

---

### Task 12: Сборка и README

**Files:** Create `README.md`; Modify `src-tauri/tauri.conf.json` (productName `Stream Assist`, иконка по умолчанию).

- [ ] **Step 1:** README: что это; как собрать (`TWITCH_CLIENT_ID=… npm run tauri build`); как зарегистрировать Twitch-приложение (Public client, Device Code Flow включён); ограничения Spotify (своё приложение, Premium); ссылка на `docs/themes.md`.
- [ ] **Step 2:** `TWITCH_CLIENT_ID=dummy npm run tauri build` → артефакты в `src-tauri/target/release/bundle/` (AppImage/deb на Linux).
- [ ] **Step 3:** commit `docs: readme and build config`.
