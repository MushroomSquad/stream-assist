# Stream Assist v1 (музыка по наградам) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Десктоп-приложение на Tauri. Зрители заказывают треки за баллы Twitch, треки играют через Spotify стримера, YouTube — запасной источник. Есть очередь, сторож воспроизведения, виджет OBS с темами и спокойная панель управления.

**Architecture:** Rust-ядро внутри Tauri v2. Логика очереди и сторожа — чистый детерминированный автомат `Player::step(input, now) -> Vec<Action>`, который тестируется без сети. Им единолично владеет async-драйвер (без мьютекса). Драйвер исполняет `Action` через HTTP-клиенты и **возвращает результаты в автомат как `Input`**: результат play, события YouTube, опрос Spotify. Локальный axum-сервер на `127.0.0.1` отдаёт виджет, `sdk.js`, темы, WebSocket и OAuth-callback. Панель на SvelteKit (SPA, adapter-static), с ядром общается через Tauri-команды и событие `state`.

**Tech Stack:** Rust 1.93, Tauri 2.x (+ `tauri-plugin-opener`, `tauri-plugin-dialog`), tokio, axum 0.8, reqwest (rustls, json), rusqlite (bundled), keyring 3 с features `apple-native windows-native sync-secret-service crypto-rust`, tokio-tungstenite (rustls), obws, sha2, base64, rand, include_dir, zip, url, serde/serde_json, thiserror; dev: wiremock. Фронт: Node 24, SvelteKit + Svelte 5 + TypeScript (шаблон create-tauri-app `svelte-ts`), npm.

**Spec:** `docs/superpowers/specs/2026-11-02-stream-assist-design.md`

## Global Constraints

- Сервер слушает только `127.0.0.1`, порт по умолчанию `47821`, настраивается. При смене порта панель предупреждает, что Redirect URI в Spotify надо обновить, и открывает мастер.
- WebSocket `/ws` принимает соединение, только если `Origin` равен `http://127.0.0.1:<port>` или `http://localhost:<port>`, а `Host` — `127.0.0.1:<port>` или `localhost:<port>`. Иначе 403.
- Spotify: redirect URI `http://127.0.0.1:<port>/callback/spotify`. PKCE без client secret, Client ID вводит стример. Параметр `state` сверяется точно и действует один раз; если логин не начат, callback отклоняется.
- Spotify scopes: `user-read-playback-state user-modify-playback-state user-read-currently-playing user-read-private`.
- Twitch: Device Code Flow, публичный клиент, Client ID из `option_env!("TWITCH_CLIENT_ID")`. Scopes: `channel:read:redemptions channel:manage:redemptions user:write:chat`.
- Refresh-токены лежат только в keyring (service `stream-assist`). Ротация refresh-токена сохраняется сразу. Refresh однопоточный под `tokio::Mutex`: перед каждым запросом, если до истечения меньше 300 с, плюс один повтор после 401. Если keyring недоступен (на Linux нет Secret Service), панель показывает понятную ошибку.
- Очередь своя, не очередь Spotify. Следующий трек — `PUT /v1/me/player/play` с `uris`.
- Опрос Spotify раз в 2.5 с.
- **FULFILLED — только после подтверждённого старта**: успешный `SpotifyPlay` или `YtStarted`. CANCELED, с возвратом баллов, — только до старта. После старта возврата нет никогда.
- Ручная смена трека в Spotify → `Suspended` и кнопка «Вернуть очередь». Ручная пауза → `Paused`, сторож ничего не делает. Оба правила работают **только если текущий трек из Spotify**.
- Фолбэк на YouTube: API Spotify недоступен ≥ 30 с **и** прошло ожидаемое время конца текущего трека.
- Пустая очередь → тишина, виджет прячется.
- Встроенные темы: `underground` (по умолчанию), `glass`, `crt`, `block`. Никаких логотипов и названий EA/NFS.
- Строки панели — `src/lib/i18n/ru.ts`. Сообщения в чат — `src-tauri/src/i18n.rs`.
- Каждый HTTP-клиент принимает `base_url`, чтобы в тестах подставить wiremock.
- `Db` под `std::sync::Mutex`, лок никогда не держится через `.await`.

## Review Focus

1. **Конец трека против ручной паузы у самого конца и против автоплея Spotify**, который сам включает чужой трек после нашего. Тесты в Task 5: `track_end_near_duration_is_end_not_pause`, `autoplay_after_end_is_end_not_suspend`.
2. **Грязный ввод заказа** (intl-ссылки, `?si=`, shorts, youtu.be, music.youtube, эмодзи, 500 символов, пустая строка). Тесты в Task 3.
3. **Перезапуск посреди трека:** без повторного FULFILLED, YouTube-трек перезапускается. Тесты `restore_spotify_no_refulfill`, `restore_youtube_replays` в Task 5 и `state_survives_reopen` в Task 2.
4. **Потерянные и дублированные выкупы** (падение между приёмом и записью, переподключение EventSub, разбор при старте). Тесты `duplicate_redemption_rejected` в Task 2 и `reconcile_*` в Task 9.
5. **Чужие локальные страницы и пути с `../`**: WS с чужим Origin, zip-slip, `../` в маршруте темы. Тесты в Task 6.

---

## File Structure

```
src/                                   # SvelteKit SPA (adapter-static, ssr=false)
  routes/+layout.ts, routes/+page.svelte
  lib/api.ts, lib/i18n/ru.ts
  lib/views/{NowQueue,Rewards,Limits,Widget,Connections}.svelte
src-tauri/
  Cargo.toml, tauri.conf.json, capabilities/default.json
  assets/widget/sdk.js
  assets/widget/themes/{underground,glass,crt,block}/{theme.json,index.html,style.css}
  src/
    main.rs, lib.rs     # tauri bootstrap (шаблон), setup → spawn сервер+драйвер
    app.rs              # AppHandle-состояние, tauri-команды
    model.rs            # Track, QueueItem, Source, RedemptionRef, Redemption, PlaybackSnapshot
    db.rs               # SQLite
    request.rs          # разбор текста заказа
    limits.rs
    player.rs           # чистый автомат
    driver.rs           # async-цикл
    router.rs           # reward_id → фича (точка расширения)
    secrets.rs, i18n.rs
    server/{mod.rs,ws.rs,themes.rs}
    spotify.rs
    twitch/{mod.rs,auth.rs,helix.rs,eventsub.rs}
    youtube.rs, obs.rs
    music.rs            # resolve + decide
```

**Расширяемость** — это требование спеки. Её даёт `router.rs`: `enum FeatureKind { Music }` и `fn route(reward_id, &Settings) -> Option<FeatureKind>`. Новая награда добавляется так: вариант enum, модуль, ветка `match` в драйвере. Шина событий и трейт плагина в v1 не нужны (YAGNI; broadcast к тому же теряет события при `Lagged`).

---

### Task 1: Каркас Tauri + SvelteKit, общие типы

**Files:**
- Шаблон: `npm create tauri-app@latest stream-assist -- --template svelte-ts --manager npm --identifier app.streamassist --yes`. Запускать в scratch-каталоге, потом перенести содержимое в корень репо, не трогая `docs/` и `.gitignore`. Если флаги не приняты, запустить интерактивно с теми же значениями.
- Create: `src-tauri/src/model.rs`, `src-tauri/src/router.rs`
- Modify: `src-tauri/Cargo.toml` (зависимости из Tech Stack), `src-tauri/capabilities/default.json` (`opener:allow-open-url`, `dialog:allow-open`), `.gitignore` (`src-tauri/target/`, `src-tauri/gen/`, `build/`, `.svelte-kit/`)

**Interfaces — Produces (`model.rs`):**
```rust
pub enum Source { Spotify { uri: String }, YouTube { video_id: String } }
pub struct Track { pub source: Source, pub title: String, pub artist: String,
  pub duration_ms: Option<u64>, pub explicit: Option<bool>, pub cover_url: Option<String> }
pub struct RedemptionRef { pub reward_id: String, pub redemption_id: String }
pub struct QueueItem { pub id: i64, pub track: Track, pub requester: String,
  pub redemption: Option<RedemptionRef>, pub started: bool }
pub struct Redemption { pub r: RedemptionRef, pub user_login: String, pub user_input: String }
pub struct PlaybackSnapshot { pub device_id: Option<String>, pub is_playing: bool,
  pub track_uri: Option<String>, pub progress_ms: u64, pub duration_ms: u64 }
pub struct Tokens { pub access_token: String, pub refresh_token: String, pub expires_at: i64 /*unix c*/ } // общий для Spotify и Twitch
pub enum PlayError { NoDevice, Unauthorized, Api(u16), Net }
```
Все типы с derive `Clone, Debug, PartialEq, Serialize, Deserialize`. `router.rs`: `pub enum FeatureKind { Music }`, `pub fn route(reward_id: &str, music_reward_id: Option<&str>) -> Option<FeatureKind>`.

- [ ] **Step 1:** сгенерировать шаблон и перенести; добавить зависимости, `model.rs`, `router.rs`.
- [ ] **Step 2: тесты:** `source_roundtrip_json`; `route_music` (совпадает → `Some(Music)`, другой id → `None`).
- [ ] **Step 3:** `cd src-tauri && cargo test` → PASS. `npm install && npm run build` → OK. `npm run tauri build -- --debug --no-bundle` → бинарник собран.
- [ ] **Step 4:** commit `chore: scaffold tauri+sveltekit, core model types`.

---

### Task 2: SQLite-хранилище

**Files:** Create `src-tauri/src/db.rs`, тесты в `#[cfg(test)]`.

**Interfaces — Produces:**
```rust
pub struct Db(rusqlite::Connection);
impl Db {
  pub fn open(path: &Path) -> Result<Db>;     // схема + PRAGMA user_version=1
  pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>>;
  pub fn set_setting<T: Serialize>(&self, key: &str, v: &T) -> Result<()>;
  pub fn insert_item(&self, track: &Track, requester: &str, r: Option<&RedemptionRef>) -> Result<QueueItem>;
     // INSERT в queue с позицией в конце; UNIQUE(redemption_id) → Err(Duplicate)
  pub fn save_state(&self, current: Option<&QueueItem>, queue: &[QueueItem]) -> Result<()>;
     // одна транзакция: current (вкл. started) + порядок очереди; удалённые из queue строки удаляются
  pub fn load_state(&self) -> Result<(Option<QueueItem>, Vec<QueueItem>)>;
  pub fn history_push(&self, item: &QueueItem, now: i64) -> Result<()>;
  pub fn history_recent(&self, n: usize) -> Result<Vec<Source>>;
  pub fn redemption_state(&self, redemption_id: &str) -> Result<RedemptionState>;
  pub fn mark_cancelled(&self, redemption_id: &str) -> Result<()>;  // драйвер зовёт перед любым Cancel в Twitch
}
pub enum RedemptionState { Unknown, Queued, Started /* current со started=true или history */, Cancelled }
pub enum DbError { Duplicate, Sql(rusqlite::Error), Json(serde_json::Error) }
```
Единственный источник правды об очереди — `Player`. `Db` её только сохраняет через `save_state`. Исключение — `insert_item`: он выдаёт id и ловит дубли.

- [ ] **Step 1: тесты:**
  - `insert_assigns_ids_in_order`;
  - `duplicate_redemption_rejected` (второй `insert_item` с тем же redemption_id → `Duplicate`);
  - `save_state_reorders_and_deletes`;
  - `state_survives_reopen` (файл: save_state с `started=true` → reopen → `load_state` равен исходному);
  - `redemption_state_tracks_lifecycle` (Unknown → Queued → Started после history_push; mark_cancelled → Cancelled);
  - `settings_roundtrip`.
- [ ] **Step 2:** `cargo test db::` → FAIL → реализовать → PASS.
- [ ] **Step 3:** commit `feat(db): sqlite storage`.

---

### Task 3: Разбор заказа и лимиты

**Files:** Create `src-tauri/src/request.rs`, `src-tauri/src/limits.rs`.

**Interfaces — Produces:**
```rust
pub enum RequestInput { SpotifyTrack(String), YouTube(String), Query(String), Empty }
pub fn parse_request(input: &str) -> RequestInput;
pub struct Limits { pub max_duration_ms: Option<u64>, pub no_repeat_last_n: usize,
  pub max_per_user_in_queue: Option<usize>, pub block_explicit: bool, pub blocklist: Vec<String> }
impl Default for Limits { /* Some(420_000), 10, Some(2), false, vec![] */ }
pub enum LimitViolation { TooLong, Repeat, UserQuota, Explicit, Blocked }
pub fn check(l: &Limits, t: &Track, requester: &str, queue: &[QueueItem],
             current: Option<&QueueItem>, recent: &[Source]) -> Result<(), LimitViolation>;
```
Если `duration_ms` равен `None`, длина не проверяется; её проверит плеер (Task 5). Если `explicit` равен `None`, трек не блокируется. Blocklist ищет подстроку без учёта регистра в `"{artist} — {title}"`.

- [ ] **Step 1: тесты request:**
```rust
assert_eq!(parse_request("https://open.spotify.com/intl-ru/track/4uLU6hMCjMI75M1A2tKUQC?si=x"), SpotifyTrack("4uLU6hMCjMI75M1A2tKUQC".into()));
assert_eq!(parse_request("spotify:track:4uLU6hMCjMI75M1A2tKUQC"), SpotifyTrack("4uLU6hMCjMI75M1A2tKUQC".into()));
assert_eq!(parse_request("https://youtu.be/dQw4w9WgXcQ?t=10"), YouTube("dQw4w9WgXcQ".into()));
assert_eq!(parse_request("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=x"), YouTube("dQw4w9WgXcQ".into()));
assert_eq!(parse_request("https://youtube.com/shorts/dQw4w9WgXcQ"), YouTube("dQw4w9WgXcQ".into()));
assert_eq!(parse_request("https://music.youtube.com/watch?v=dQw4w9WgXcQ"), YouTube("dQw4w9WgXcQ".into()));
assert_eq!(parse_request("  "), Empty);
assert_eq!(parse_request("Daft Punk 🎧 around the world"), Query("Daft Punk 🎧 around the world".into()));
assert!(matches!(parse_request(&"я".repeat(500)), Query(s) if s.chars().count() == 200));
assert_eq!(parse_request("https://open.spotify.com/album/xyz"), Query("https://open.spotify.com/album/xyz".into()));
```
- [ ] **Step 2: тесты limits:** `too_long`, `repeat_in_queue`, `repeat_in_recent`, `repeat_is_current`, `user_quota`, `explicit_blocked_only_when_flag_true`, `unknown_duration_passes`, `blocklist_case_insensitive`.
- [ ] **Step 3:** FAIL → реализовать (`url` crate) → `cargo test request:: limits::` PASS.
- [ ] **Step 4:** commit `feat: request parsing and viewer limits`.

---

### Task 4: Секреты и клиент Spotify

**Files:** Create `src-tauri/src/secrets.rs`, `src-tauri/src/spotify.rs`.

**Interfaces — Produces:**
```rust
// secrets.rs — keyring service "stream-assist"
pub fn store(key: &str, value: &str) -> Result<()>;  pub fn load(key: &str) -> Result<Option<String>>;  pub fn delete(key: &str) -> Result<()>;
// spotify.rs
pub struct Pkce { pub verifier: String, pub challenge: String, pub state: String }
pub fn new_pkce() -> Pkce;   // verifier 64 симв. [A-Za-z0-9-._~], challenge = base64url_nopad(sha256(verifier)), state 32 симв.
pub fn authorize_url(client_id: &str, redirect: &str, p: &Pkce) -> String;
// Tokens и PlayError — из model.rs
pub struct Device { pub id: String, pub is_active: bool, pub kind: String }
pub struct Spotify { /* client, api, accounts, client_id, tokens: tokio::Mutex<Tokens>, on_rotate: Box<dyn Fn(&Tokens)+Send+Sync> */ }
impl Spotify {
  pub fn new(client_id: String, tokens: Tokens, api: &str, accounts: &str, on_rotate: impl Fn(&Tokens)+Send+Sync+'static) -> Self;
  pub async fn exchange_code(client_id: &str, code: &str, redirect: &str, verifier: &str, accounts: &str) -> Result<Tokens>;
  // каждый метод ниже сначала зовёт приватный auth(): single-flight refresh, если до expires_at < 300 c; на 401 — refresh + один повтор
  pub async fn me_product(&self) -> Result<String>;
  pub async fn search_track(&self, q: &str) -> Result<Vec<Track>>;   // type=track, limit=5, market=from_token
  pub async fn track(&self, id: &str) -> Result<Track>;
  pub async fn player(&self) -> Result<Option<PlaybackSnapshot>>;    // 204 → None
  pub async fn devices(&self) -> Result<Vec<Device>>;
  pub async fn play(&self, uri: &str, device_id: Option<&str>, position_ms: u64) -> Result<(), PlayError>; // 404 NO_ACTIVE_DEVICE → NoDevice
  pub async fn pause(&self) -> Result<()>;  pub async fn resume(&self) -> Result<()>;  pub async fn set_volume(&self, pct: u8) -> Result<()>;
}
pub fn pick_device(devs: &[Device]) -> Option<String>;  // активное, иначе первое kind=="Computer", иначе первое
```
- [ ] **Step 1: тесты (wiremock):**
  - `pkce_challenge_matches_rfc7636_vector`: verifier `dBjftJeZ4CVP-mJ92K9q2W8ivJXR5zGPRgo0vJyj6Mw` → challenge `E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM`;
  - `player_204_is_none`;
  - `player_parses_snapshot`;
  - `refresh_when_expiring_and_rotates` (новый refresh_token → вызван on_rotate);
  - `concurrent_calls_refresh_once` (два параллельных запроса → один POST /api/token);
  - `retry_once_on_401`;
  - `play_404_is_no_device`;
  - `search_maps_track_fields` (артисты через ", ", обложка — самая большая картинка);
  - `pick_device_prefers_active_then_computer`.
- [ ] **Step 2:** FAIL → реализовать → `cargo test spotify::` PASS. Для `secrets` тестов нет, это тонкая обёртка.
- [ ] **Step 3:** commit `feat(spotify): pkce auth and web api client`.

---

### Task 5: Автомат Player — очередь и сторож

**Files:** Create `src-tauri/src/player.rs`. Только чистая логика.

**Interfaces — Produces:**
```rust
pub enum Mode { Idle, Starting /*play отправлен, ждём подтверждения*/, Playing, Paused, Suspended }
pub enum Input {
  Enqueue(QueueItem), Skip, Remove(i64), Move(i64, usize), ResumeQueue, TogglePause,
  SetMaxDuration(Option<u64>),
  PlayResult { item_id: i64, result: Result<(), PlayError> },   // результат SpotifyPlay
  Spotify(Result<Option<PlaybackSnapshot>, ()>),
  Yt { item_id: i64, ev: YtEvent },                              // события от единственного player-соединения
  YtPlayerConnected,                                             // новое player-соединение (OBS перезагрузил источник и т.п.)
  FallbackResolved { item_id: i64, video_id: Option<String> },
  Restore { current: Option<QueueItem>, queue: Vec<QueueItem> },
  Tick,                                                          // для таймаутов и бэкоффа
}
pub enum YtEvent { Started { duration_ms: u64 }, Ended, Error }
pub enum Action {
  Persist,                                    // драйвер исполняет его первым в пачке
  SpotifyPlay { item_id: i64, uri: String, device_id: Option<String>, position_ms: u64 },
  SpotifyPause, SpotifyResume,
  YtPlay { item_id: i64, video_id: String, start_ms: u64 }, YtUnmute, YtStop, YtPause, YtResume,
  ResolveFallback(QueueItem),
  Started(QueueItem),                         // драйвер: Fulfill + history_push
  Cancel(QueueItem, CancelReason),            // драйвер: Cancel в Twitch + сообщение в чат
  Widget(WidgetState),
  Warn(Option<String>),
}
pub enum CancelReason { RemovedByStreamer, TooLong, YtUnavailable }
pub struct WidgetState { pub visible: bool, pub now: Option<QueueItem>, pub queue: Vec<QueueItem>,
  pub progress_ms: u64, pub mode: String, pub warning: Option<String> }
impl Player { pub fn new(max_duration_ms: Option<u64>) -> Self; pub fn step(&mut self, input: Input, now_ms: i64) -> Vec<Action>;
              pub fn snapshot(&self) -> (Option<QueueItem>, Vec<QueueItem>); }
pub const PLAY_GRACE_MS: i64 = 4000;
pub const END_TOLERANCE_MS: u64 = 3000;
pub const POLL_MS: u64 = 2500;
pub const OUTAGE_MS: i64 = 30_000;
pub const YT_START_TIMEOUT_MS: i64 = 20_000;
pub const YT_END_SLACK_MS: i64 = 10_000;     // нет Ended к started_at + duration (без пауз) + slack → считаем концом
pub const RETRY_BACKOFF_MS: [i64; 5] = [2000, 4000, 8000, 16000, 30000]; // дальше 30000
```
Каждое изменение очереди или текущего трека даёт `Persist` и `Widget`. `PlayError` и `Tokens` объявлены в `model.rs`, чтобы `player.rs` не зависел от HTTP-клиентов.

**Главный инвариант:** `Cancel(item, _)` выдаётся **только если `!item.started`**. Во всех ветках ниже, где написано «Cancel + следующий», для `started=true` выдаётся просто «следующий», без Cancel и без сообщения в чат.

**Правила:**
- **Старт элемента, Spotify.** `SpotifyPlay` (device = last_device), режим `Starting`. `PlayResult Ok` → `Started(item)`, `started=true`, режим `Playing`. `PlayResult Err` → `Warn(Some("Spotify: нет активного устройства / ошибка"))` и повтор по `RETRY_BACKOFF_MS` на `Tick`, без отмены, элемент остаётся текущим. При `NoDevice` ещё и `last_device = None`, чтобы драйвер выбрал устройство заново.
- **Старт элемента, YouTube.** `YtPlay{start_ms:0}` (плеер стартует без звука), режим `Starting`. `Yt Started{d}`: если `d > max` → `YtStop` + `Cancel(TooLong)` + следующий; иначе `YtUnmute` + `Started(item)` (если ещё не started) + `Playing`. `Yt Error` до старта → `Cancel(YtUnavailable)` + следующий. Нет `Started` за `YT_START_TIMEOUT_MS` → `YtStop` + `Cancel(YtUnavailable)` + следующий.
- **`PlayResult` и Yt-события с чужим `item_id`** игнорируются.
- **YouTube-сторож.** В `Playing` для YouTube: если `Ended` не пришёл к `started_at + duration + YT_END_SLACK_MS` (время на паузе не считается), то это конец трека. Проверяется на `Tick`.
- **`YtPlayerConnected`.** Если текущий трек из YouTube, то `YtPlay{start_ms: прогресс по часам}` повторно, а для `Paused` — ещё и `YtPause`. Иначе ничего. `Yt Started` в режиме `Paused` → только `YtPause`, режим не меняется, звук не включается.
- **Правила по снимку Spotify работают только если текущий трек из Spotify и режим `Playing`, `Paused` или `Suspended`.** Пока играет YouTube, снимки игнорируются, кроме учёта `last_ok`.
- **Конец трека.** Снимок на нашем URI, `!is_playing`, `progress >= duration - END_TOLERANCE_MS`. Либо снимок на чужом URI или `None` при `last_progress >= duration - (END_TOLERANCE_MS + POLL_MS)`: это автоплей или конец. Действие: следующий трек, либо `Idle` + `Widget{visible:false}`.
- **Ручная пауза.** Наш URI, `!is_playing`, прогресс далеко от конца → `Paused`. `is_playing` снова true → `Playing`.
- **Чужой URI** вне `PLAY_GRACE_MS` после `PlayResult Ok` и не у конца трека → `Suspended`. Дальше снимки игнорируются. `ResumeQueue` → следующий элемент очереди.
- **Устройство пропало** (`Ok(None)` или `device_id None`) в `Playing` вне grace → `SpotifyPlay{position_ms: last_progress}` с бэкоффом `RETRY_BACKOFF_MS`.
- **Outage.** `Err(())`: если `now - last_ok >= OUTAGE_MS` и `now >= expected_end`, то `fallback=true`, `Warn(Some(..))`; текущий Spotify-элемент считается доигранным (без Cancel); следующий Spotify-элемент → `ResolveFallback`. `FallbackResolved{None}` → `Cancel(YtUnavailable)` + следующий. `FallbackResolved{Some}` → старт как YouTube. Первый `Ok` → `fallback=false`, `Warn(None)`.
- **`TogglePause`.** Spotify → `SpotifyPause`/`SpotifyResume`, YouTube → `YtPause`/`YtResume`. Режим `Paused` ставится сразу, поэтому собственная пауза не считается чужой.
- **`Skip` текущего** → `SpotifyPause`/`YtStop` + следующий, без Cancel. Если текущий ещё не стартовал (`!started`), то `Cancel(RemovedByStreamer)`.
- **`Remove`** элемента из очереди → `Cancel(RemovedByStreamer)`.
- **`Restore`.** Текущий `started=true` из Spotify → `Playing`, без `Started` и без play, дальше разбирается сторож. Из YouTube → `YtPlay{start_ms:0}`, а последующий `Yt Started` не даёт повторного `Started`. Ошибка или таймаут такого элемента ведёт просто к следующему, без Cancel (инвариант). `started=false` → обычный старт.

- [ ] **Step 1: тесты:**
  - `enqueue_idle_starts_without_fulfill`, `play_ok_emits_started`, `play_failure_does_not_fulfill_and_retries`, `enqueue_busy_appends`;
  - `track_end_plays_next`, `track_end_near_duration_is_end_not_pause`, `autoplay_after_end_is_end_not_suspend`, `last_track_end_goes_idle_and_hides`;
  - `manual_pause_does_nothing`, `manual_resume_back_to_playing`, `foreign_track_suspends`, `foreign_track_within_grace_ignored`, `resume_queue_plays_next`;
  - `device_lost_replays_with_backoff`, `short_outage_no_fallback`, `outage_before_expected_end_no_fallback`, `outage_after_end_triggers_fallback`, `fallback_unresolved_cancels`, `recovery_clears_warning`;
  - `youtube_playing_ignores_spotify_snapshot`, `yt_error_before_start_cancels`, `yt_too_long_cancels`, `yt_start_timeout_cancels`, `yt_stale_item_ignored`;
  - `toggle_pause_youtube_uses_yt_actions`, `remove_queued_cancels`, `skip_started_no_refund`, `skip_starting_cancels`;
  - `restore_spotify_no_refulfill`, `restore_youtube_replays`, `persist_first_in_batch`;
  - `play_result_stale_ignored`, `no_device_clears_last_device`, `yt_unmute_only_after_length_ok`;
  - `restored_started_yt_error_no_cancel`, `skip_restored_started_no_cancel`;
  - `yt_missing_end_times_out`, `yt_missing_end_ignores_paused_time`, `player_reconnect_replays_current_yt`, `player_reconnect_while_paused` (`Yt Started` в `Paused` → только `YtPause`, без `YtUnmute`), `fallback_finishes_current_without_cancel`.
- [ ] **Step 2:** `cargo test player::` → FAIL → реализовать → PASS.
- [ ] **Step 3:** commit `feat(player): queue and watchdog state machine`.

---

### Task 6: Локальный сервер — виджет, WS, темы, OAuth-callback

**Files:** Create `src-tauri/src/server/{mod.rs,ws.rs,themes.rs}`, `src-tauri/assets/widget/sdk.js`, `src-tauri/assets/widget/themes/underground/*`.

**Interfaces — Produces:**
```rust
pub enum WsOut { State(WidgetState), YtPlay { item_id: i64, video_id: String, start_ms: u64 }, YtUnmute, YtStop, YtPause, YtResume, Reload }
pub struct ServerChannels {
  pub out_tx: tokio::sync::broadcast::Sender<WsOut>,   // State/Reload — всем; Yt* сервер шлёт только player-соединению
  pub yt_rx: tokio::sync::mpsc::Receiver<player::Input>,             // Input::Yt{..}
  pub oauth_rx: tokio::sync::mpsc::Receiver<(String, String)>,       // (code, state) — сверку делает драйвер
}
pub async fn serve(port: u16, themes_dir: PathBuf, active_theme: Arc<std::sync::RwLock<String>>,
                   last_state: tokio::sync::watch::Receiver<WidgetState>) -> Result<ServerChannels>;
pub fn import_theme_zip(zip: &Path, themes_dir: &Path) -> Result<String>;
pub fn list_themes(themes_dir: &Path) -> Vec<ThemeMeta>;
pub struct ThemeMeta { pub id: String, pub name: String, pub author: String, pub width: u32, pub height: u32 }
```
Потеря `State` при lag некритична: следующее состояние полное, а при подключении отправляется `last_state`.

**Маршруты:**
- `GET /widget/music` → `index.html` активной темы.
- `GET /widget/theme/{id}/{*file}`: путь нормализуется, компоненты `..` и абсолютные пути → 404.
- `GET /widget/sdk.js`.
- `GET /ws?role=player|view`. Проверка Origin и Host — см. Global Constraints. Player-соединение одно, новое вытесняет старое: старое закрывается с кодом `4001` и после этого переподключается только как `view`, поэтому взаимного выбивания нет. Каждое новое player-соединение шлёт в `yt_rx` `Input::YtPlayerConnected`. Остальные соединения получают только `State` и `Reload`.
- `GET /callback/spotify?code&state` → передаёт в `oauth_rx`, отвечает страницей «Готово, вернитесь в программу».

Встроенные темы подключаются через `include_dir!`. Кастомная тема с тем же id перекрывает встроенную.

**`sdk.js`:**
- API для тем: `StreamAssist.on(name, fn)`, где name ∈ `state|nowPlaying|queue|progress|hidden`.
- Переподключается к WS каждые 2 с.
- `role=player` запрашивается, только если `window.obsstudio` определён (так бывает только в браузере OBS). Любая другая страница, включая превью в панели, подключается как `role=view`, поэтому случайная вкладка звук не перехватит.
- YouTube IFrame Player размещается в `#sa-youtube`. Если тема его не дала, sdk создаёт контейнер 200×200 за краем экрана.
- На `YtPlay{start_ms}`: `mute()` + `loadVideoById({videoId, startSeconds})`. На первом PLAYING отправляется `{"type":"yt","item_id":..,"event":"started","duration_ms":getDuration()*1000}`, **звук остаётся выключенным**. `YtUnmute` → `unMute()`, `YtStop` → `stopVideo()`. Решение о длине принимает автомат.
- `ENDED` → `ended`, `onError` → `error`, всегда с `item_id`.
- `Reload` → `location.reload()`.

**Тема `underground`** — порт макета `ug2-theme-v2.html` (раздел «Тема A» в спеке): круглая обложка из `cover_url`, без неё заглушка — салатовая нота на тёмном фоне; строки «исполнитель», «название», «Заказал: ник»; язычок MUSIC; полоса прогресса; табличка «Очередь».

- [ ] **Step 1: тесты (axum + `tower::ServiceExt` / tokio-tungstenite к поднятому серверу):**
  - `widget_serves_active_theme`, `sdk_served`;
  - `ws_rejects_foreign_origin` (Origin `https://evil.example` → 403), `ws_sends_state_on_connect`;
  - `yt_commands_only_to_player` (два соединения, `YtPlay` получает только player);
  - `new_player_evicts_old_with_4001_and_signals_connected`;
  - `callback_forwards_code_and_state`;
  - `theme_route_traversal_404` (`/widget/theme/underground/..%2F..%2Fsecret`);
  - `zip_slip_rejected`, `zip_without_theme_json_rejected`, `custom_theme_overrides_builtin`.
- [ ] **Step 2:** FAIL → реализовать → `cargo test server::` PASS.
- [ ] **Step 3: ручная проверка.** Пример `src-tauri/examples/widget_demo.rs` поднимает сервер и шлёт фейковый `WidgetState`. Открыть `http://127.0.0.1:47821/widget/music` → видны плашка и очередь в стиле UG2.
- [ ] **Step 4:** commit `feat(server): widget server, sdk, ws, themes`.

---

### Task 7: Twitch — Device Code, Helix, EventSub, чат

**Files:** Create `src-tauri/src/twitch/{mod.rs,auth.rs,helix.rs,eventsub.rs}`, `src-tauri/src/i18n.rs`.

**Interfaces — Produces:**
```rust
// auth.rs (base = https://id.twitch.tv)
pub struct DeviceStart { pub device_code: String, pub user_code: String, pub verification_uri: String, pub interval: u64, pub expires_in: u64 }
pub async fn device_start(client_id: &str, scopes: &str, base: &str) -> Result<DeviceStart>;
pub async fn device_poll(client_id: &str, device_code: &str, scopes: &str, base: &str) -> Result<Option<Tokens>>; // authorization_pending → None
pub async fn refresh(client_id: &str, refresh_token: &str, base: &str) -> Result<Tokens>;  // без secret; refresh-токен одноразовый
// helix.rs — тот же паттерн auth(): single-flight refresh + повтор на 401 + on_rotate
pub enum RewardError { NotAffiliate, DuplicateTitle, Other(String) }
impl Helix {
  pub fn new(client_id: String, tokens: Tokens, api: &str, id_base: &str, on_rotate: impl Fn(&Tokens)+Send+Sync+'static) -> Self;
  pub async fn me(&self) -> Result<(String, String)>;                       // (id, login) → broadcaster_id
  pub async fn find_manageable_reward(&self, id: Option<&str>, title: &str) -> Result<Option<String>>;
      // GET custom_rewards?only_manageable_rewards=true; 404 → None; ищет по id, затем по title
  pub async fn create_reward(&self, title: &str, cost: u32, prompt: &str) -> Result<String, RewardError>; // 403 → NotAffiliate; 400 duplicate → DuplicateTitle
  pub async fn update_reward(&self, id: &str, patch: &serde_json::Value) -> Result<()>;
  pub async fn set_redemption_status(&self, r: &RedemptionRef, fulfilled: bool) -> Result<()>; // 400/404 «не UNFULFILLED» → Ok (идемпотентно)
  pub async fn unfulfilled(&self, reward_id: &str) -> Result<Vec<Redemption>>;  // все страницы, first=50, sort=OLDEST
  pub async fn chat(&self, text: &str) -> Result<()>;
  pub async fn subscribe_redemptions(&self, session_id: &str) -> Result<()>;
}
// eventsub.rs
pub enum EsMsg { Welcome { session_id: String }, Keepalive, Notification(Redemption), Reconnect { url: String }, Revocation }
pub fn parse_message(text: &str) -> Result<EsMsg>;
pub enum EsSignal { Redemption(Redemption), Resync /* после жёсткого переподключения — драйвер делает reconcile */ }
pub async fn run(helix: Arc<Helix>, ws_url: String, out: tokio::sync::mpsc::Sender<EsSignal>);
```
**Поведение `run`:**
- На `Welcome` новой сессии вызвать `subscribe_redemptions`.
- На `Reconnect{url}` подключиться к новому url и **не** подписываться заново. Старое соединение закрыть после welcome нового.
- Обрыв или таймаут keepalive (`keepalive_timeout_seconds` + 5 с) → новая сессия с бэкоффом 1–30 с, подписка, затем `EsSignal::Resync`.

**`i18n.rs`:**
- `chat_added(user, artist, title, pos)` → «@user трек «artist — title» в очереди, позиция N»;
- `chat_rejected(user, why)` → «@user заказ отклонён: why. Баллы возвращены.»;
- `violation_text(&LimitViolation)` → тексты: «слишком длинный трек», «этот трек недавно играл или уже в очереди», «у тебя уже максимум заказов в очереди», «explicit запрещён», «трек в чёрном списке»;
- `cancel_text(&CancelReason)` → «стример удалил заказ», «трек слишком длинный», «видео недоступно»;
- `TEXT_NOT_FOUND` = «ничего не найдено», `TEXT_SERVICE_DOWN` = «музыкальный сервис недоступен».

- [ ] **Step 1: тесты:**
  - `device_poll_pending_is_none`, `device_poll_success_tokens`;
  - `parse_welcome`, `parse_redemption_notification` (фикстура из документации Twitch), `parse_reconnect`;
  - `set_status_sends_patch` (PATCH `/helix/channel_points/custom_rewards/redemptions?id=…&broadcaster_id=…&reward_id=…`, body `{"status":"FULFILLED"}`), `set_status_already_done_is_ok`;
  - `find_reward_404_is_none`, `create_reward_403_not_affiliate`;
  - `unfulfilled_follows_pagination`;
  - `helix_refresh_rotates_token`.
- [ ] **Step 2:** FAIL → реализовать → `cargo test twitch::` PASS.
- [ ] **Step 3:** commit `feat(twitch): device auth, helix, eventsub, chat`.

---

### Task 8: YouTube и OBS

**Files:** Create `src-tauri/src/youtube.rs`, `src-tauri/src/obs.rs`.

**Interfaces — Produces:**
```rust
pub async fn oembed(video_id: &str, base: &str) -> Result<Option<Track>>;  // 401/403/404 → None; duration None; cover = thumbnail_url
pub async fn search(api_key: &str, q: &str, base: &str) -> Result<Option<String>>; // type=video, videoEmbeddable=true, maxResults=1
pub fn split_title(title: &str, channel: &str) -> (String, String); // "A - B"/"A — B" → (A,B); иначе (channel без " - Topic", title)
pub struct ObsCfg { pub host: String, pub port: u16, pub password: Option<String> }
pub async fn check(cfg: &ObsCfg) -> Result<String>;
pub async fn add_widget_to_current_scene(cfg: &ObsCfg, url: &str, w: u32, h: u32) -> Result<()>;
```
`add_widget_to_current_scene`:
- Input `browser_source` с именем «Stream Assist — Music», настройки `{url, width, height, reroute_audio: true}`.
- Если input уже есть: обновить настройки (`SetInputSettings`); если его нет в текущей сцене — `CreateSceneItem`.
- `SetInputAudioMonitorType(MonitorAndOutput)`, чтобы стример слышал YouTube-треки.

- [ ] **Step 1: тесты:** `oembed_404_none`, `oembed_maps_fields`, `split_title_dash`, `split_title_topic_channel`, `search_returns_first_id`. OBS проверяется вручную в Task 11.
- [ ] **Step 2:** FAIL → реализовать → PASS.
- [ ] **Step 3:** commit `feat: youtube oembed/search, obs browser source`.

---

### Task 9: Фича «Музыка» и драйвер

**Files:** Create `src-tauri/src/music.rs`, `src-tauri/src/driver.rs`, `src-tauri/src/app.rs`; Modify `src-tauri/src/lib.rs`.

**Interfaces — Produces:**
```rust
// music.rs
pub enum Resolved { Ok(Track), NotFound }
pub async fn resolve(input: RequestInput, sp: &Spotify, yt_base: &str) -> Result<Resolved>;
pub fn normalize_title(s: &str) -> String;  // lower, без (...)/[...], без "official video|audio|lyrics|mv|hd"
pub enum Decision { Enqueue(Track), Reject(String) }
pub fn decide(res: Resolved, limits: &Limits, requester: &str, queue: &[QueueItem], current: Option<&QueueItem>, recent: &[Source]) -> Decision;
// driver.rs
pub enum DriverMsg { Cmd(player::Input), Es(EsSignal), Resolved { red: Redemption, res: Result<Resolved> },
  TwitchLogin(Tokens), SpotifyLoginStart(Pkce), SettingsChanged }
pub async fn run(ctx: DriverCtx, rx: mpsc::Receiver<DriverMsg>);
pub struct DriverCtx { pub db: Arc<std::sync::Mutex<Db>>, pub state_tx: watch::Sender<WidgetState>, pub ws: ServerChannels,
  pub app: tauri::AppHandle /* emit("state") */, /* клиенты Option<Arc<..>>, Settings */ }
```
Драйвер владеет `Player` и `Option<Pkce>` для ожидающего логина Spotify. `select!` ждёт: tick `POLL_MS` (`Input::Tick` + опрос `player()` → `Input::Spotify`), `rx`, `yt_rx`, `oauth_rx`. Сверка OAuth: `state == pending.state`, после проверки `pending.take()`; иначе событие игнорируется.

**Выкуп** (`EsSignal::Redemption` или reconcile):
- `route(reward_id)` ≠ Music → игнор. `db.redemption_state(id)` ≠ Unknown или id в `inflight: HashSet` → игнор.
- Иначе id добавляется в `inflight` и запускается `tokio::spawn(resolve)`; результат приходит как `DriverMsg::Resolved`.
- На `Resolved`: `decide` по **текущему** снимку `Player` (без гонок квоты), затем:
  - `Enqueue` → `db.insert_item` (`Duplicate` → игнор) → `player.step(Enqueue)` → чат `chat_added`;
  - `Reject` → `db.mark_cancelled` → Cancel в Twitch + `chat_rejected`;
  - `Err` (сеть, Spotify не подключён) → `db.mark_cancelled` → Cancel + `TEXT_SERVICE_DOWN`.
  - В конце id убирается из `inflight`.

**Исполнение `Action`.** Пачка сортируется так, что `Persist` идёт первым: `db.save_state(player.snapshot())`. Остальные действия:
- `Started(item)` → `set_redemption_status(true)` + `history_push`;
- `Cancel(item, why)` → `db.mark_cancelled` (до сетевого вызова) → `set_redemption_status(false)` + чат `chat_rejected(user, cancel_text(why))`;
- `SpotifyPlay` → если `device_id` None, то `pick_device(devices())`, затем `play`; результат → `Input::PlayResult`;
- `Yt*` → `ws.out_tx`;
- `Widget` → `state_tx` + `ws.out_tx(State)` + `app.emit("state")`;
- `ResolveFallback` → spawn `youtube::search` (без ключа → `None`) → `FallbackResolved`;
- `Warn` → входит в `WidgetState.warning`.

Ошибки Twitch при Fulfill/Cancel логируются и не валят цикл.

**Reconcile**, при старте и на `EsSignal::Resync`. Для каждого `unfulfilled(reward_id)`:
- `Queued` → ничего;
- `Started` → Fulfill;
- `Cancelled` → Cancel повторно, без чата;
- `Unknown` → обычный путь выкупа.

**Запуск** (в `setup` через `tauri::async_runtime::spawn`):
- `Db::open(app_data_dir/stream-assist.db)` → `Restore`.
- Сервер.
- Клиенты из keyring.
- Награда:
  - `find_manageable_reward(saved_id, title)`;
  - найдена → сохранить id и `update_reward{is_paused:false}`;
  - не найдена → `create_reward`. Ошибки `NotAffiliate` и `DuplicateTitle` идут в `Warn` с текстом: «нужен статус компаньона/партнёра» или «награда с таким названием уже есть, но создана не этой программой — переименуйте её или удалите».
- Reconcile.

**Выход:** `RunEvent::ExitRequested { code: None, api }` → `api.prevent_exit()` → `update_reward{is_paused:true}` (таймаут 2 с) → `app.exit(0)`. `ExitRequested` с `code: Some(_)` пропускается, иначе выход зациклится.

- [ ] **Step 1: тесты music:** `decide_rejects_on_violation`, `decide_not_found_rejects`, `normalize_title_strips_noise`, `resolve_youtube_matches_spotify` (wiremock: oEmbed «Daft Punk - Around The World (Official Video)», поиск вернул Daft Punk / Around the World → `Source::Spotify`), `resolve_youtube_no_match_stays_youtube`.
- [ ] **Step 2: тесты driver** (функция `fn plan_reconcile(states: &[(Redemption, RedemptionState)]) -> Vec<ReconcileStep>` вынесена отдельно, чтобы тестировать без сети): `reconcile_queued_skipped`, `reconcile_started_fulfilled`, `reconcile_cancelled_recancelled`, `reconcile_unknown_processed`.
- [ ] **Step 3:** FAIL → реализовать → `cargo test` (весь крейт) PASS.
- [ ] **Step 4:** commit `feat(music): redemption pipeline and driver`.

---

### Task 10: Остальные темы виджета

**Files:** Create `src-tauri/assets/widget/themes/{glass,crt,block}/*`, `docs/themes.md` (контракт `sdk.js`: события, поля `WidgetState`, `#sa-youtube`, структура zip, `theme.json`).

- [ ] **Step 1:** перенести B/C/D из `.superpowers/brainstorm/*/content/widget-themes.html` на `sdk.js`, обложки через `cover_url` с заглушкой.
- [ ] **Step 2: тест** `builtin_themes_listed` → `list_themes` даёт 4 id. `cargo test server::` PASS.
- [ ] **Step 3:** commit `feat(widget): glass, crt, block themes + authoring doc`.

---

### Task 11: Панель управления и Tauri-команды

**Files:** Create `src/lib/api.ts`, `src/lib/i18n/ru.ts`, `src/lib/views/*.svelte`; Modify `src/routes/+page.svelte`, `src-tauri/src/app.rs`, `src-tauri/tauri.conf.json` (CSP: `frame-src http://127.0.0.1:*` для превью виджета).

**Команды** (`#[tauri::command]`, имена совпадают с функциями в `api.ts`). Все шлют `DriverMsg` в mpsc или читают `watch`/`Db`, без мьютекса `Player`:
- очередь: `get_state() -> WidgetState`, `skip()`, `remove_item(id: i64)`, `move_item(id: i64, to: usize)`, `toggle_pause()`, `resume_queue()`, `set_volume(pct: u8)`;
- настройки: `get_settings() -> Settings`, `save_settings(s: Settings)`. `Settings` = { limits, reward_title, reward_cost, reward_cooldown_s, port, active_theme, youtube_api_key, obs }. Смена `max_duration` → `SetMaxDuration`. Смена темы → `WsOut::Reload`. Смена награды → `update_reward`. Смена порта → предупреждение и мастер Spotify;
- вход: `twitch_login_start() -> DeviceStart`, `twitch_login_poll() -> bool`, `twitch_logout()`, `spotify_login(client_id: String) -> String` (URL открывается через plugin-opener), `spotify_status() -> SpotifyStatus { connected: bool, premium: bool }`;
- OBS и темы: `obs_check() -> String`, `obs_add_widget()`, `list_themes() -> Vec<ThemeMeta>`, `import_theme(path: String) -> String` (путь из plugin-dialog), `widget_test()` (фейковый трек в виджет на 10 с).

Событие `state` (Tauri emit) — панель ничего не опрашивает.

**Экраны.** Спокойный UI, светлая или тёмная тема по системе, все строки из `ru.ts`:
- **Сейчас** — трек, прогресс, Пропустить/Пауза, громкость. В `Suspended` — заметная «Вернуть очередь». Плашка `warning`. Очередь с HTML5 drag-and-drop и кнопкой удаления.
- **Награды** — название, стоимость, кулдаун.
- **Лимиты** — поля `Limits`.
- **Виджет** — выбор темы, URL для OBS + «Копировать», «Добавить в OBS», «Тест», импорт zip, превью `/widget/music?preview=1` в iframe.
- **Подключения:**
  - Twitch: код и ссылка из Device Flow;
  - мастер Spotify по шагам: открыть dashboard → создать приложение → вставить Redirect URI с кнопкой копирования → вставить Client ID → войти; предупреждение, если нет Premium; совет выключить «Автовоспроизведение»;
  - OBS: хост, порт, пароль, проверка;
  - YouTube API key (необязательный, с пояснением, зачем он нужен);
  - ошибка keyring, если хранилище недоступно.

- [ ] **Step 1:** команды в `app.rs`.
- [ ] **Step 2:** экраны.
- [ ] **Step 3: проверка.** `npm run check` → 0 ошибок, `cargo test` → PASS. Затем `npm run tauri dev` и ручной чек-лист:
  1. Подключить Twitch, Spotify, OBS.
  2. «Добавить в OBS» создаёт источник.
  3. «Тест» показывает плашку.
  4. Выкуп с тестового аккаунта → трек играет, выкуп FULFILLED. Второй выкуп → в очередь.
  5. Удаление из очереди → баллы вернулись.
  6. Закрыть Spotify и сделать выкуп → предупреждение «нет устройства», выкуп остаётся UNFULFILLED, возврата пока нет. Открыть Spotify → трек стартует, выкуп FULFILLED.
  7a. Во время YouTube-трека обновить источник в OBS → трек продолжается с той же позиции.
  7. Сменить трек вручную в Spotify → «Вернуть очередь».
  8. Выключить сеть на 40 с во время трека → предупреждение; после конца трека YouTube-фолбэк (если есть ключ).
  9. Закрыть программу → награда на паузе.
- [ ] **Step 4:** commit `feat(panel): control panel and commands`.

---

### Task 12: Сборка и README

**Files:** Create `README.md`; Modify `src-tauri/tauri.conf.json` (productName `Stream Assist`).

- [ ] **Step 1: README:**
  - что это;
  - сборка: `TWITCH_CLIENT_ID=… npm run tauri build`;
  - регистрация Twitch-приложения: Client Type «Public», Device Code Flow;
  - ограничения Spotify: своё приложение, Premium;
  - на Linux нужен Secret Service (gnome-keyring или KWallet);
  - ссылка на `docs/themes.md`.
- [ ] **Step 2:** `TWITCH_CLIENT_ID=dummy npm run tauri build` → артефакты в `src-tauri/target/release/bundle/`.
- [ ] **Step 3:** commit `docs: readme and build config`.
