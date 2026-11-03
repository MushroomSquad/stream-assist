# Озвучка стрима (TTS) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Озвучка голосом Voice Lab по награде, по метке в чате или обоими способами, с настраиваемыми лимитами, звук — отдельным источником OBS.

**Architecture:** Новый модуль `src-tauri/src/tts/`: чистые фильтры/гейт (`filter.rs`), очередь (`queue.rs`) и асинхронный движок (`engine.rs`) — отдельная задача tokio с внедрённой функцией синтеза. Драйвер маршрутизирует сырые события Twitch в движок, управляет жизненным циклом TTS-награды и выставляет статусы погашений по исходам, которые движок шлёт по собственному неограниченному каналу. Синтез — `Runner::synth_file` без записи в «Рендеры» и без публикации задания. Воспроизведение — страница `/widget/tts` через существующий WebSocket с ролью `tts`.

**Tech Stack:** как сейчас (Rust, tokio, axum, Tauri 2, Svelte 5). Новых зависимостей нет (`futures-util` уже есть — `BoxFuture`).

**Spec:** `docs/superpowers/specs/2026-11-03-stream-tts.md`

## Global Constraints

- Новых крейтов нет. Музыкальная часть по поведению не меняется; все существующие тесты зелёные.
- `driver::Settings.tts: TtsSettings` с `#[serde(default)]` на поле и на структуре — старые настройки читаются.
- Пределы: текст ≤ 300 символов после фильтров (обрезка по последнему пробелу в пределах 300, иначе по 300 символам), очередь ≤ 20 (текущий не считается; `Test` тоже подчиняется), метка 1–3 непробельных символа, кулдауны 0–3600 с, скорость 0.7–1.5, стоп-слов ≤ 200 по ≤ 50 символов, название награды 1–45 символов и ≠ названию музыкальной награды, цена 1–2147483647, кулдаун награды 0–604800, `lang` ∈ `supertonic3_tts::AVAILABLE_LANGS`.
- Ожидание слота синтеза: повтор раз в 1 с, максимум 60 с → `SynthFailed`. Страховка воспроизведения `duration_ms + 10 000` мс → `Played`.
- Итог награды: `Played`/`Skipped` → `FULFILLED`; `Rejected(_)`, `QueueFull`, `SynthFailed`, `Cleared`, `Disabled`, `Reconciled` → `CANCELED`.
- Чат: подписка на `channel.chat.message` v1 при каждом подключении EventSub; режим проверяет движок. **Никакой сбой подписки на чат не рвёт сессию**: успех → `EsSignal::ChatStatus(ChatStatus::Ok)`, 403 → `ChatStatus::Forbidden` (предупреждение `tts_chat_relogin`), любая другая ошибка или таймаут → `ChatStatus::Failed` (предупреждение `tts_chat_failed`, повтор при следующем переподключении). Любое уведомление, которое не удалось разобрать, и неизвестные типы → `EsMsg::Ignored`, а не разрыв.
- Канал в движок — `mpsc::unbounded_channel::<TtsMsg>()`; драйвер и команды панели шлют `send()` без ожидания (порядок сохраняется, блокировки нет; никаких `tokio::spawn` для отправки). Движок шлёт исходы в `mpsc::UnboundedSender<(RedemptionRef, bool)>`, который драйвер читает в своём `select!`.
- TTS-предупреждения живут только в `TtsView.warning` (не в `WidgetState.warning`).
- Тексты UI — русские, в `src/lib/i18n/ru.ts`. `SCOPES` += `user:read:chat`.

## Review Focus

1. **«100%» и метка в середине числа** — `extract_marked("на 100% согласен", "%") == None` (Task 1).
2. **Старый токен без `user:read:chat`** — подписка на чат 403 или 500 → `ChatStatus`, сессия жива, погашения приходят; кривой payload чата не рвёт сессию (Task 3).
3. **Повторная доставка и гонка с reconcile** — одно погашение не озвучивается дважды и не получает `FULFILLED` после `CANCELED` (Task 6).
4. **«Пропустить» во время синтеза и рендер в Voice Lab во время озвучки** — результат выбрасывается, WAV удаляется, следующий синтез не стартует, пока не вернулся прошлый; `JOB_BUSY` → ожидание до 60 с (Task 4, Task 6).
5. **Перезапуск** — TTS-награда снимается с паузы при подключении, если режим включён, и выключается (`is_enabled:false`), если нет (Task 2, Task 7).

---

### Task 1: `tts/filter.rs` — метка, фильтры, гейт

**Files:** Create `src-tauri/src/tts/mod.rs`, `src-tauri/src/tts/filter.rs`; Modify `src-tauri/src/lib.rs` (`pub mod tts;`), `src-tauri/src/model.rs`

**Interfaces — Produces:**
- `model.rs`: `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Fragment { pub kind: String, pub text: String }`.
- `pub fn extract_marked(text: &str, marker: &str) -> Option<String>` — первое вхождение `marker`, перед которым не ASCII-цифра; `text[..pos].trim()`, пустой → `None`.
- `pub fn strip_emotes(fragments: &[Fragment]) -> String` — склейка всех фрагментов, кроме `kind == "emote"`.
- `#[derive(Debug, Clone, PartialEq)] pub enum Reject { Empty, StopWord }`.
- `pub fn clean(text: &str, strip_links: bool, stop_words: &[String]) -> Result<String, Reject>` — порядок: стоп-слова (подстрока, `to_lowercase`, пустые после `trim` игнорируются) → `StopWord`; ссылки (токен по пробелам: содержит `://`, начинается с `www.` без учёта регистра, или вида `метка(.метка)+(/…)?`, где метки из букв/цифр/`-`, последняя — ≥2 буквы); повтор одного символа > 3 → 3; схлопнуть пробелы, `trim`; пусто → `Empty`; обрезка до 300.
- `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct Roles { pub everyone: bool, pub subscribers: bool, pub vips: bool, pub moderators: bool }`, `Default` — все `true`.
- `pub fn role_allowed(r: &Roles, badges: &[String]) -> bool` — `broadcaster` всегда; `everyone`; `subscriber`/`founder` при `subscribers`; `vip`; `moderator`/`lead_moderator` при `moderators`.
- `tts/mod.rs`: `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)] pub struct TtsSettings { pub reward_enabled: bool /*false*/, pub chat_enabled: bool /*false*/, pub profile_id: Option<i64>, pub lang: String /*"ru"*/, pub speed: f32 /*1.0*/, pub reward_title: String /*"Озвучить текст"*/, pub reward_cost: u32 /*300*/, pub reward_cooldown_s: u32 /*0*/, pub reward_id: Option<String>, pub marker: String /*"%"*/, pub roles: filter::Roles, pub global_cooldown_s: u32 /*10*/, pub user_cooldown_s: u32 /*60*/, pub strip_links: bool /*true*/, pub stop_words: Vec<String> }` + `Default`.
- `#[derive(Default)] pub struct Gate { last_global: Option<Instant>, last_user: HashMap<String, Instant> }`: `pub fn allowed(&self, user: &str, global_s: u32, user_s: u32, now: Instant) -> bool` и `pub fn mark(&mut self, user: &str, now: Instant, keep_s: u32)` — ключ — логин в нижнем регистре; `mark` удаляет записи старше `keep_s`. Стримера движок не проверяет и не отмечает.
- [ ] **Step 1: Тесты** (`mod tests` в `filter.rs`; `tts_settings_default_from_old_json` — в `tts/mod.rs`): `marker_basic` («привет всем % лол» → «привет всем»; без метки → None; «%» → None; «  %x» → None), `marker_after_digit_ignored` («на 100% согласен» → None; «скидка 50% а потом привет %» → «скидка 50% а потом привет»), `marker_multichar` (`"!!"`), `emotes_stripped`, `links_removed` (`https://x.y/z`, `www.Site.com`, `twitch.tv/abc`, `пример.рф` вырезаны; «т.е.», «3.14», «ok.» остаются), `repeats_collapsed` («ааааааа!!!!!» → «ааа!!!»), `stop_word_rejects`, `truncate_on_word`, `roles` (каждая роль, `founder`, `lead_moderator`, стример), `gate_cooldowns` (общий, личный, 0 = выкл, `allowed` не отмечает).
- [ ] **Step 2–4:** FAIL → реализовать → PASS (`cargo test --lib tts::filter`). **Step 5:** Commit `feat(tts): marker, filters and chat gate`.

### Task 2: настройки, маршрутизация, жизненный цикл награды

**Files:** Modify `src-tauri/src/tts/mod.rs`, `src-tauri/src/driver.rs`, `src-tauri/src/router.rs`, `src-tauri/src/app.rs`

**Interfaces — Produces:**
- `TtsSettings` (из Task 1) += `pub fn reward_body(&self, enabled: bool) -> serde_json::Value` → `{"title","cost","is_enabled":enabled,"is_paused":false,"global_cooldown_setting":{"is_enabled":cd>0,"global_cooldown_seconds":cd}}`; `pub fn tts_reward_patch(old: &TtsSettings, new: &TtsSettings) -> Option<serde_json::Value>` — `None`, если ни название/цена/кулдаун/`reward_enabled` не менялись; выключение → `{"is_enabled": false}`; иначе `reward_body(new.reward_enabled)`.
- `driver::Settings` += `#[serde(default)] pub tts: TtsSettings`; `keep_reward_id` сохраняет и `tts.reward_id`.
- `app::validate_settings` — пределы из Global Constraints.
- `router::FeatureKind::Tts`; `pub fn route(reward_id: &str, music: Option<&str>, tts: Option<&str>) -> Option<FeatureKind>`; обновить вызовы (`redeem`).
- `apply_settings(...)` остаётся «сохранить + патч музыкальной награды» и **возвращает `Result<TtsSettings>` — прежние** TTS-настройки. В `run` оба обработчика (`SaveSettings`, `SettingsChanged`) после успеха вызывают общий `fn tts_settings_changed(old: &TtsSettings, new: &Settings, helix: Option<Arc<Helix>>, tts_tx: &mpsc::UnboundedSender<TtsMsg>, tx: &mpsc::UnboundedSender<Internal>)` (явные параметры, без `DriverCtx`):
  - всегда `TtsMsg::Settings(new.tts)` движку;
  - если есть `helix` и `reward_id`: `tts_reward_patch(old, &new.tts)` → `update_reward`;
  - если `reward_enabled` стало `true` и `reward_id` нет → `spawn_tts_reward(helix, &new.tts, tx)`.
- `fn spawn_tts_reward(helix: Arc<Helix>, tts: &TtsSettings, tx: &mpsc::UnboundedSender<Internal>)` — `find_manageable_reward(tts.reward_id, &tts.reward_title)` → иначе `create_reward(title, cost, "Текст для озвучки")` → `Internal::TtsReward(Result<String>)` (тексты ошибок как у музыки). Обработчик `TtsReward(Ok(id))`: сохранить id в настройки (БД), `update_reward(id, reward_body(true))`, `TtsMsg::Settings` (сбрасывает это предупреждение), запустить TTS-reconcile (Task 7). `Err` → `TtsMsg::Warning(text)`.
- `connect_twitch`: если `tts.reward_enabled` → `spawn_tts_reward`; иначе если `tts.reward_id` есть → `update_reward(id, {"is_enabled": false})`.
- `app::exit` и `DriverMsg::TwitchLogout` ставят на паузу и TTS-награду (`{"is_paused": true}`), если id есть.
- `app::SCOPES` += ` user:read:chat`.
- Задача выполняется после Task 6: `tts_settings_changed` шлёт `TtsMsg::Settings` в канал движка (`mpsc::UnboundedSender<TtsMsg>`, создаётся в Task 7; до Task 7 — параметр функции, в тестах — тестовый канал).
- [ ] **Step 1: Тесты:** `route_tts`, `validate_rejects_bad_tts` (метка "" и "abcd", скорость 2.0, кулдаун 3601, одинаковые названия наград, язык "xx"), `settings_change_keeps_tts_reward_id`, `tts_reward_body` (enabled/disabled), `tts_reward_patch_cases` (без изменений → None; выключение → `{"is_enabled":false}`; включение и смена цены → тело с `is_paused:false`).
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(tts): settings and reward lifecycle`.

### Task 3: EventSub — сообщения чата

**Files:** Modify `src-tauri/src/twitch/eventsub.rs`, `src-tauri/src/twitch/helix.rs`, `src-tauri/src/twitch/tests.rs`, `src-tauri/src/model.rs`; Create `src-tauri/src/twitch/fixtures/chat_message.json` (основан на примере из документации Twitch, отредактирован: фрагменты text+emote, badges `[{"set_id":"subscriber",…}]`)

**Interfaces — Produces:**
- `model::ChatMessage { pub user_login: String /*chatter_user_login*/, pub broadcaster_id: String /*broadcaster_user_id*/, pub source_broadcaster_id: Option<String> /*source_broadcaster_user_id, null → None*/, pub fragments: Vec<Fragment> /*message.fragments[]: type→kind, text*/, pub badges: Vec<String> /*badges[].set_id*/ }`.
- `EsMsg::Chat(ChatMessage)`, `EsMsg::Ignored`. `parse_message`: уведомление `…redemption.add` → `Notification`; `channel.chat.message` → `Chat`, при ошибке разбора события → `Ignored`; прочие типы уведомлений → `Ignored`. Ошибки структуры конверта (нет `metadata`, неизвестный `message_type`) — как раньше.
- `Helix::subscribe_chat(&self, session_id: &str) -> Result<bool>` — `Ok(true)` успех, `Ok(false)` на 403, другой неуспех → `Err`.
- `pub enum ChatStatus { Ok, Forbidden, Failed }` (в `eventsub.rs`), `EsSignal::ChatStatus(ChatStatus)`, `EsSignal::Chat(ChatMessage)`. Тело: `{"type":"channel.chat.message","version":"1","condition":{"broadcaster_user_id":b,"user_id":b},"transport":{"method":"websocket","session_id":…}}`.
- `eventsub::run` (сигнатура не меняется): после `subscribe_redemptions` → `subscribe_chat` (таймаут 10 с); `Ok(true)` → `ChatStatus::Ok`, `Ok(false)` → `Forbidden`, `Err`/таймаут → `Failed`; во всех случаях сессия продолжается. `EsMsg::Chat` (только после welcome) → `EsSignal::Chat(ChatMessage)`; `Ignored` → продолжить.
- [ ] **Step 1: Тесты:** `parse_chat_message` (фикстура → логин, 2 фрагмента, badges `["subscriber"]`, `source_broadcaster_id == None`), `unknown_notification_ignored` (`channel.follow` → `Ignored`), `malformed_chat_ignored` (`channel.chat.message` без `message` → `Ignored`), `eventsub_chat_forbidden_keeps_session` (wiremock: redemptions 202, chat 403 → `ChatStatus(Forbidden)`, затем погашение доходит как `Redemption`), `eventsub_chat_500_keeps_session` (chat 500 → `Failed`, погашение доходит), `eventsub_chat_ok_signals_status`; существующие тесты EventSub обновить под второй POST (мок chat 202).
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(twitch): chat messages over EventSub`.

### Task 4: `Runner::synth_file` — синтез без «Рендеров»

**Files:** Modify `src-tauri/src/voice_lab/runner.rs`, `src-tauri/src/voice_lab/commands.rs`

**Interfaces — Produces:**
- Тело `synth` разделить: приватный `async fn synth_raw(&self, root: &Path, style_path: PathBuf, params: &RenderParams, out: &Path, emit: Option<&Emit>) -> Result<(i64 /*duration_ms*/, i64 /*synth_ms*/), VlError>` — занимает `synth_busy`, воркер, таймаут 300 с; при `emit == None` задание не попадает в `jobs` и не публикуется (`JobGuard` с no-op emit и без записи). `synth` = `synth_raw` во временный файл + регистрация (поведение и события не меняются).
- `pub async fn synth_file(&self, root: &Path, style_path: PathBuf, params: RenderParams, out: &Path) -> Result<i64, VlError>` — `emit = None`; при ошибке `out` не остаётся.
- `commands.rs`: `pub(crate) async fn tts_synth(lab: Arc<VoiceLab>, profile_id: i64, params: RenderParams, out: PathBuf) -> Result<i64, VlError>` — `validate_params`, `require_model` (→ `MODEL_MISSING` или существующий код), стиль профиля (нет профиля → существующий код ошибки профиля), затем `synth_file`. Футура должна быть `Send`: в тестах `fn assert_send<T: Send>(_: &T)` на `tts_synth(...)`. Если не `Send` — обернуть как `vl_render` (`spawn_blocking` + `Handle::block_on`), это допустимо: движок не прерывает синтез, а ждёт его.
- [ ] **Step 1: Тесты** (фейковый воркер, `cargo build --examples`): `synth_file_writes_without_render` (файл есть, `renders()` пуст, `jobs()` пуст), `synth_file_busy` (`JOB_BUSY`), `synth_file_failure_leaves_no_file`, `tts_synth_is_send` (компиляция); существующие тесты `synth` зелёные.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(voice-lab): synth to file for tts`.

### Task 5: сервер — `/widget/tts`, `/tts/{file}`, роль `tts`

**Files:** Modify `src-tauri/src/server/mod.rs`, `src-tauri/src/server/ws.rs`, `src-tauri/src/server/tests.rs`, `src-tauri/src/app.rs`, `src-tauri/src/driver.rs` (вызов `serve` в тесте); Create `src-tauri/assets/widget/tts.html`

**Interfaces — Produces:**
- `WsOut::TtsPlay { id: u64 }`, `WsOut::TtsStop`. Сессия `role=tts`: без начального `State`, получает только `TtsPlay`/`TtsStop`/`Reload`; остальные роли `Tts*` не получают.
- `pub enum TtsWidgetEvent { Connected, Ended(u64), Error(u64), Disconnected }`; `ServerChannels.tts_rx: Option<mpsc::Receiver<TtsWidgetEvent>>` (забирается `take()` в `app::start` до передачи `ws` в `DriverCtx`). Сообщение виджета `{"type":"tts","id":N,"event":"ended"|"error"}`. Одна активная `tts`-сессия (отдельный слот, как `player`; вытесненная закрывается 4001, её события игнорируются); `Connected` при захвате слота, `Disconnected` — только активной при выходе.
- `build`/`serve`/`start_or_disabled` получают `tts_dir: PathBuf` (обновить все вызовы, включая тесты). `GET /tts/{file}`: `file` строго `^[0-9]{1,20}\.wav$`, иначе 404; заголовки `Content-Type: audio/wav`, `Cache-Control: no-store`.
- `GET /widget/tts` → `include_str!("../../assets/widget/tts.html")`: без внешних ресурсов, прозрачный фон; WebSocket `ws://${location.host}/ws?role=tts`, переподключение раз в 2 с; `TtsPlay` → `new Audio('/tts/'+id+'.wav')`, `ended`/`error` → событие; `TtsStop` → пауза и сброс.
- [ ] **Step 1: Тесты:** `tts_wav_served_and_name_checked` (200 + оба заголовка; `a.wav`, `..%2Fx.wav` → 404), `tts_role_receives_only_tts` (нет начального `State`; `player` не получает `TtsPlay`; `tts` не получает `YtPlay`), `tts_events_forwarded`, `second_tts_widget_evicts_first`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(server): tts widget and audio route`.

### Task 6: очередь и движок озвучки

**Files:** Create `src-tauri/src/tts/queue.rs`, `src-tauri/src/tts/engine.rs`; Modify `src-tauri/src/tts/mod.rs`

**Interfaces — Produces:**
- `queue.rs`: `pub enum Origin { Chat, Reward(RedemptionRef), Test }`; `pub struct Item { pub id: u64, pub origin: Origin, pub user: String, pub text: String }`; `pub enum Outcome { Played, Skipped, Rejected, QueueFull, SynthFailed, Cleared, Disabled, Reconciled }`; `pub fn fulfilled(&Outcome) -> bool` (Played|Skipped). `pub struct Queue` (`VecDeque<Item>` + `current: Option<Item>`): `push(Item) -> Result<(), Item>` (≥ 20 в очереди → `Err`), `take_next() -> Option<Item>` (делает текущим, возвращает клон), `finish_current() -> Option<Item>`, `clear() -> Vec<Item>`, `view() -> (Option<ItemView>, Vec<ItemView>)`.
- `#[derive(Serialize, Clone, Debug)] pub struct TtsView { pub current: Option<ItemView>, pub queue: Vec<ItemView>, pub widget: bool, pub warning: Option<String> }`, `ItemView { id: u64, user: String, text: String, reward: bool, synthesizing: bool }`.
- `engine.rs`: `pub enum TtsMsg { Chat(ChatMessage), Reward(Redemption), Settings(TtsSettings), ChatStatus(ChatStatus), Warning(String), Widget(TtsWidgetEvent), Skip, Clear, Test(String), Reconcile(Vec<RedemptionRef>), State(oneshot::Sender<TtsView>) }`.
- `pub type SynthFn = Arc<dyn Fn(i64, String /*text*/, String /*lang*/, f32, PathBuf) -> BoxFuture<'static, Result<i64, String /*код*/>> + Send + Sync>`.
- `pub struct EngineDeps { pub synth: SynthFn, pub dir: PathBuf, pub ws: broadcast::Sender<WsOut>, pub outcome: mpsc::UnboundedSender<(RedemptionRef, bool)>, pub view: Arc<dyn Fn(&TtsView) + Send + Sync> }`; `pub async fn run(deps: EngineDeps, settings: TtsSettings, rx: mpsc::UnboundedReceiver<TtsMsg>)`.
- Правила `run`:
  - **Приём чата:** игнор, если `!chat_enabled` или `source_broadcaster_id.is_some_and(|s| s != broadcaster_id)`. Стример = badge `broadcaster`. Текст: `strip_emotes` → `extract_marked`; не стример: `role_allowed` и `gate.allowed`; затем `clean`; затем `push`; только при успешном `push` — `gate.mark` (не для стримера). Любой отказ — тихо.
  - **Приём награды:** id погашения в `seen` → игнор. Иначе добавить в `seen`. `!reward_enabled` → `Disabled`; `clean(user_input)` отказ → `Rejected`; голос недоступен (см. ниже) → `SynthFailed`; полна → `QueueFull`; иначе в очередь. Все отказы → `outcome(ref, false)`.
  - **`seen`:** `HashSet<String>` + `VecDeque` порядка, ≤ 500 (старейшие вытесняются).
  - **Голос недоступен:** `profile_id == None` → `warning = "tts_no_voice"`; синтез **текущего** элемента вернул `MODEL_MISSING` → `"tts_model_missing"`, `NOT_FOUND` (нет профиля, `store.rs`) → `"tts_voice_missing"`, `VOICE_LAB_UNAVAILABLE` → `"tts_unavailable"`; во всех трёх — флаг `voice_broken = true` (сброс при `Settings`). Пока голос недоступен, новые элементы отклоняются при приёме, а уже стоящие в очереди — `SynthFailed` по одному при попытке.
  - **Проигрывание:** если нет текущего, нет синтеза в полёте, есть элемент и виджет подключён → `take_next`, `tokio::spawn` синтеза в `dir/<id>.wav` (повтор на `"JOB_BUSY"` раз в 1 с до 60 с; между повторами проверяется общий с движком `Arc<AtomicBool>` отмены → немедленный возврат `"CANCELLED"`; остальные ошибки — сразу) → результат `(item_id, Result<i64,String>)` во внутренний канал. Результат для элемента, который больше не текущий → удалить WAV, ничего больше (в том числе не ставить `voice_broken`). Ошибка → `SynthFailed`. Успех → `TtsPlay{id}`, дедлайн `duration+10 с`. `Ended(id)` текущего или дедлайн → `Played`; `Error(id)` → `SynthFailed`. События с чужим id игнорируются. После завершения — удалить WAV, исход для награды, следующий.
  - **`Skip`:** текущий → `Skipped` (исход сразу), `TtsStop`; если его синтез ещё идёт — выставить флаг отмены (ожидание `JOB_BUSY` прерывается сразу; запущенный воркер дорабатывает 1–3 с), результат выбрасывается по правилу выше; следующий синтез не стартует, пока не вернулся прошлый.
  - **`Clear`:** все в очереди → `Cleared` (текущий не трогается).
  - **`Test(text)`:** `clean` без стоп-слов и ссылок → в очередь как `Origin::Test` (тот же предел 20).
  - **`Widget`:** `Connected` → `widget = true`, попробовать начать; `Disconnected` → `widget = false` (текущий доводит дедлайн).
  - **`ChatStatus`:** `Forbidden` → `"tts_chat_relogin"`, `Failed` → `"tts_chat_failed"` (оба показываются, только если `chat_enabled`), `Ok` → снять. **`Warning(text)`** → `warning = text`. Хранить два поля: `chat_warning` и `warning`; в `TtsView.warning` — первое непустое из (`warning`, `chat_warning` при `chat_enabled`).
  - **`Reconcile(refs)`:** для каждого ref, чьего id нет в `seen`, → добавить в `seen`, `outcome(ref, false)`.
  - **`Settings`:** заменить настройки, сбросить `voice_broken` и `warning` (кроме `chat_warning`).
  - После каждого изменения — `view(&view)`; `State(tx)` → ответить текущим видом.
- [ ] **Step 1: Тесты** (`#[tokio::test(start_paused = true)]`, фейковый `SynthFn` пишет пустой файл и отдаёт 1000 мс, считает вызовы): `chat_marked_message_plays`, `chat_cooldown_marks_only_accepted` (стоп-слово не тратит кулдаун), `chat_roles_and_broadcaster_bypass`, `shared_chat_ignored`, `chat_disabled_ignored`, `reward_rejected_refunds`, `reward_disabled_refunds`, `reward_played_fulfills`, `reward_duplicate_ignored`, `reconcile_then_late_event_ignored`, `queue_full_refunds_reward`, `skip_fulfills_clear_refunds`, `skip_during_synth_drops_result` (WAV удалён, `TtsPlay` не отправлен, следующий синтез — только после возврата прошлого), `skip_during_busy_wait_is_immediate` (фейк отвечает `JOB_BUSY`; Skip → следующий элемент стартует < 2 с), `stale_result_does_not_break_voice`, `widget_missing_waits`, `safety_timeout_advances`, `synth_busy_retries_then_fails` (61 × `JOB_BUSY` → `(ref,false)`), `missing_voice_rejects_at_intake`, `wav_deleted_after_play`, `chat_relogin_warning_only_when_enabled`, `settings_clears_warning`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(tts): queue and playback engine`.

### Task 7: связка в приложении и драйвере, команды панели

**Files:** Modify `src-tauri/src/app.rs`, `src-tauri/src/driver.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/obs.rs`

**Interfaces — Produces:**
- `app::start`: `create_dir_all(voice-lab/tmp/tts)` (сам `tmp` чистит `Store::open`); канал `mpsc::unbounded_channel::<TtsMsg>()`; `handle.manage(TtsHandle(UnboundedSender<TtsMsg>))`; `mpsc::unbounded_channel` исходов; `SynthFn` поверх `VlState` (`commands::tts_synth`, качество 8; `VlState(None)` → `"VOICE_LAB_UNAVAILABLE"`); `view` → `emit("tts", view)`; задача-переходник `ws.tts_rx.take()` → `TtsMsg::Widget`; `tokio::spawn(engine::run(deps, settings.tts.clone(), rx))`. `DriverCtx` += `tts: mpsc::UnboundedSender<TtsMsg>`, `tts_outcomes: mpsc::UnboundedReceiver<(RedemptionRef,bool)>` (или передать в `run` отдельно).
- Драйвер (`select!` += ветка исходов → `status(ref, ok)`):
  - `EsSignal::Chat(m)` → `TtsMsg::Chat(m)`; `EsSignal::ChatStatus(s)` → `TtsMsg::ChatStatus(s)`.
  - `redeem`: `route(...) == Some(Tts)` → `TtsMsg::Reward(r)` (без БД музыки и `inflight`); `Music` — как раньше.
  - TTS-reconcile: `fn tts_reconcile(ctx, tx)` — если есть `helix` и `tts.reward_id` → `unfulfilled(id)` → `Internal::TtsReconcile(Vec<Redemption>)` → `TtsMsg::Reconcile(refs)`. Вызывать при старте (после `connect_twitch`), на `EsSignal::Resync`, после `TwitchLogin` и после `TtsReward(Ok)`; не зависит от музыкальной награды и от `reward_enabled`.
  - Чистые хелперы под тест: `fn tts_target(r: &Redemption, s: &Settings) -> Option<FeatureKind>` (обёртка над `route`).
- Команды Tauri (`try_state::<TtsHandle>()`, нет → `"starting"`): `tts_state() -> TtsView` (через `TtsMsg::State`), `tts_skip()`, `tts_clear()`, `tts_test(text: String)` (1–300 символов после `trim`), `obs_add_tts()`; регистрация в `lib.rs`.
- `obs.rs`: вынести «создать или обновить источник + добавить в сцену + мониторинг» из `add_widget_to_current_scene` в `async fn upsert_browser_source(client, scene, name, settings, monitor: bool)`; `pub async fn add_tts_source(cfg, url)` — «Stream Assist — Озвучка», 400×100, `reroute_audio: true`, мониторинг и вывод. Музыкальный путь использует ту же функцию, поведение прежнее.
- [ ] **Step 1: Тесты:** `tts_target_routes_by_reward_id` (музыка/TTS/чужая), `tts_test_text_bounds`; ручная проверка в Task 8.
- [ ] **Step 2–4:** FAIL → реализовать → PASS; `cargo clippy --all-targets` без новых предупреждений; `cargo test` целиком зелёный. **Step 5:** Commit `feat(tts): wire engine into driver and panel commands`.

### Task 8: вкладка «Озвучка», документация

**Files:** Create `src/lib/views/Tts.svelte`, `src/lib/ttsApi.ts`; Modify `src/routes/+page.svelte`, `src/lib/api.ts` (`Settings.tts`, `defaultSettings.tts`), `src/lib/i18n/ru.ts`, `docs/user-guide.md`, `docs/architecture.md`

**Interfaces — Consumes:** команды Task 7, событие `tts` (`TtsView`), `voiceApi.vl_profiles()`.
- Вкладка `tts` («Озвучка») после «Голосовой лаборатории»: форма настроек (общий `save`), голос из профилей, язык, скорость (0.7–1.5), блоки «Награда», «Чат» (метка с подсказкой «Озвучивается текст до метки. „100%“ не считается», роли, кулдауны), «Фильтры» (ссылки, стоп-слова построчно); адрес `http://127.0.0.1:<port>/widget/tts` + «Добавить в OBS»; очередь (текущий с пометкой «синтез…», список, «Пропустить», «Очистить очередь»); поле + «Проверить». Предупреждения: `tts_no_voice`, `tts_voice_missing`, `tts_model_missing`, `tts_unavailable`, `tts_chat_failed`, `tts_chat_relogin` (подсказка: «Подключения» → выйти и войти в Twitch), «Источник OBS не подключён» при `widget == false` и непустой очереди. Примечание: «Пока идёт озвучка, рендер в Голосовой лаборатории может ответить „занято“».
- Состояние: `tts_state()` при монтировании + подписка на `tts`.
- `docs/user-guide.md`: раздел «Озвучка» (включая перелогин и известное ограничение про ответы программы в чат); `docs/architecture.md`: абзац про `tts`.
- [ ] **Step 1:** `npm run check` → 0/0; `node --test tests/*.mjs` зелёные.
- [ ] **Step 2:** Commit `feat(panel): stream tts tab`.

## Порядок выполнения

Task 1 → 3 → 4 → 5 → 6 → 2 → 7 → 8. (`TtsSettings` — в Task 1; Task 2 использует `TtsMsg` из Task 6; Task 3 не зависит от Task 2.)
