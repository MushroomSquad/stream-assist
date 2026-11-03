# Озвучка стрима (TTS) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Озвучка голосом Voice Lab по награде, по метке в чате или обоими способами, с настраиваемыми лимитами, звук — отдельным источником OBS.

**Architecture:** Новый модуль `src-tauri/src/tts/`: чистые фильтры/гейт (`filter.rs`), очередь-автомат (`queue.rs`) и асинхронный движок (`engine.rs`) с внедрённой функцией синтеза. Драйвер только пересылает сырые события Twitch в движок и выставляет статусы погашений по его ответам. Синтез — новый `Runner::synth_file` без записи в «Рендеры». Воспроизведение — страница `/widget/tts` через существующий WebSocket с ролью `tts`.

**Tech Stack:** как сейчас (Rust, tokio, axum, Tauri 2, Svelte 5). Новых зависимостей нет.

**Spec:** `docs/superpowers/specs/2026-11-03-stream-tts.md`

## Global Constraints

- Новых крейтов нет. Музыкальная часть (очередь, лимиты, награда музыки) по поведению не меняется; все существующие тесты зелёные.
- `Settings.tts: TtsSettings` с `#[serde(default)]` — старые сохранённые настройки читаются без ошибки.
- Пределы: текст ≤ 300 символов после фильтров (обрезка по границе слова, иначе по символу), очередь ≤ 20 (текущий не считается), метка 1–3 непробельных символа, кулдауны 0–3600 с, скорость 0.7–1.5, стоп-слов ≤ 200, каждое ≤ 50 символов, название награды 1–45 символов, цена 1–2147483647, кулдаун награды 0–604800.
- Ожидание слота синтеза: попытка раз в 1 с, максимум 60 с → отказ `SynthBusy`. Страховка воспроизведения: `duration_ms + 10 000` мс.
- Итог награды: `Played`/`Skipped` → `FULFILLED`; `Rejected(_)`, `QueueFull`, `SynthFailed`, `Cleared`, `Reconciled` → `CANCELED`.
- Чат: только `channel.chat.message` v1; сообщения с `source_broadcaster_user_id` ≠ мой id игнорируются. Ошибка подписки на чат не рвёт EventSub-сессию (награды работают), а даёт предупреждение `tts_chat_relogin`.
- `SCOPES` += `user:read:chat`.
- Тексты UI — русские, в `src/lib/i18n/ru.ts`.

## Review Focus

1. **«100%» и метка в середине числа** — `extract_marked("на 100% согласен", "%") == None`, `"привет 50%% всем"` → по первому `%` не после цифры (Task 1).
2. **Старый токен без `user:read:chat`** — подписка на чат 403 → `EsSignal::ChatUnavailable`, сессия жива, погашения продолжают приходить (Task 3).
3. **Источник OBS не подключён / отключился посреди фразы** — очередь не теряется, текущий элемент по страховочному таймауту завершается, награда `FULFILLED` только если `ended` получен, иначе повторного проигрывания нет и итог `Played` по таймауту (Task 5, Task 6).
4. **Пользователь рендерит в Voice Lab во время озвучки** — `JOB_BUSY` → ожидание, не отказ; через 60 с → `SynthFailed` → `CANCELED` (Task 4, Task 6).
5. **Перезапуск с непогашенными наградами** — reconcile отменяет только те, которых нет в очереди/текущем (Task 6).

---

### Task 1: `tts/filter.rs` — метка, фильтры, гейт

**Files:** Create `src-tauri/src/tts/mod.rs`, `src-tauri/src/tts/filter.rs`; Modify `src-tauri/src/lib.rs` (`mod tts;`)

**Interfaces — Produces:**
- `pub fn extract_marked(text: &str, marker: &str) -> Option<String>` — первое вхождение `marker`, перед которым не ASCII-цифра; результат `text[..pos].trim()`, пустой → `None`.
- `pub struct Fragment { pub kind: String, pub text: String }` (`kind`: `"text" | "emote" | "mention" | "cheermote"`); `pub fn strip_emotes(fragments: &[Fragment]) -> String` — склейка всех фрагментов, кроме `emote`.
- `pub enum Reject { Empty, StopWord, Role, Cooldown }` (`Debug, Clone, PartialEq`).
- `pub fn clean(text: &str, strip_links: bool, stop_words: &[String]) -> Result<String, Reject>` — порядок: стоп-слова (подстрока, `to_lowercase`, пустые записи игнорируются) → `StopWord`; вырезать ссылки (токен по пробелам: содержит `://`, начинается с `www.` без учёта регистра, или вида `метка(.метка)+(/…)?`, где метки из букв/цифр/`-`, последняя — ≥2 букв); сжать повтор одного символа > 3 до 3; схлопнуть пробелы; пусто → `Empty`; обрезать до 300 символов по последнему пробелу в пределах 300 (если нет — по 300 символам).
- `pub struct Roles { pub everyone: bool, pub subscribers: bool, pub vips: bool, pub moderators: bool }` (`Serialize, Deserialize, Clone, Debug, PartialEq`, `Default` = everyone true, остальные true).
- `pub fn role_allowed(r: &Roles, badges: &[String]) -> bool` — `broadcaster` всегда; `everyone`; `subscriber`/`founder` при `subscribers`; `vip`; `moderator`.
- `pub struct Gate { last_global: Option<Instant>, last_user: HashMap<String, Instant> }` + `pub fn check_and_mark(&mut self, user: &str, is_broadcaster: bool, global_s: u32, user_s: u32, now: Instant) -> Result<(), Reject>` — стример всегда `Ok` без отметки; иначе `Cooldown`, если `now - last_global < global_s` или `now - last_user[user] < user_s`; при `Ok` отмечает оба. Ключ пользователя — логин в нижнем регистре. Записи старше `max(global_s,user_s)` удаляются при каждой отметке.
- [ ] **Step 1: Тесты** (`#[cfg(test)] mod tests` в `filter.rs`): `marker_basic` («привет всем % лол» → «привет всем»; «без метки» → None; «%» → None; «  %x» → None), `marker_after_digit_ignored` («на 100% согласен» → None; «скидка 50% а потом привет %» → «скидка 50% а потом привет»), `marker_multichar` (`"!!"`), `emotes_stripped` (фрагменты `[text "hi ", emote "Kappa", text " all"]` → «hi  all»), `links_removed` (`https://x.y/z`, `www.Site.com`, `twitch.tv/abc`, `пример.рф` вырезаны; «т.е.», «3.14», «ok.» — остаются), `repeats_collapsed` («ааааааа!!!!!» → «ааа!!!»), `stop_word_rejects` (без учёта регистра; пустое стоп-слово ничего не блокирует), `truncate_on_word` (310 символов словами → ≤ 300, без обрыва слова; одно слово 400 символов → ровно 300), `roles` (каждая роль, `founder` = саб, стример всегда), `gate_cooldowns` (общий и личный, стример без кулдауна и без отметки, 0 = выкл).
- [ ] **Step 2–4:** FAIL → реализовать → PASS (`cargo test --lib tts::filter`). **Step 5:** Commit `feat(tts): marker, filters and chat gate`.

### Task 2: настройки, награда TTS, маршрутизация

**Files:** Modify `src-tauri/src/driver.rs`, `src-tauri/src/router.rs`, `src-tauri/src/app.rs`, `src-tauri/src/tts/mod.rs`

**Interfaces — Produces:**
- В `tts/mod.rs`: `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)] pub struct TtsSettings { pub reward_enabled: bool /*false*/, pub chat_enabled: bool /*false*/, pub profile_id: Option<i64>, pub lang: String /*"ru"*/, pub speed: f32 /*1.0*/, pub reward_title: String /*"Озвучить текст"*/, pub reward_cost: u32 /*300*/, pub reward_cooldown_s: u32 /*0*/, pub reward_id: Option<String>, pub marker: String /*"%"*/, pub roles: filter::Roles, pub global_cooldown_s: u32 /*10*/, pub user_cooldown_s: u32 /*60*/, pub strip_links: bool /*true*/, pub stop_words: Vec<String> }` + `Default`.
- `driver::Settings` += `#[serde(default)] pub tts: TtsSettings`; `keep_reward_id` сохраняет и `tts.reward_id`.
- `app::validate_settings` проверяет пределы из Global Constraints (`lang` ∈ `voice_lab::models::AVAILABLE_LANGS` или где он определён).
- `router::FeatureKind::Tts`; `pub fn route(reward_id: &str, music: Option<&str>, tts: Option<&str>) -> Option<FeatureKind>`; все вызовы обновить.
- `connect_twitch`: после музыкальной — если `tts.reward_enabled`: найти/создать награду `tts.reward_title` (prompt «Текст для озвучки»), результат — `Internal::TtsReward(Result<String>)`, драйвер сохраняет id в настройки (как музыкальный) и шлёт `update_reward(id, {"is_enabled": true})`.
- `apply_settings`: изменение названия/цены/кулдауна → `update_reward` (тело как у музыки); `reward_enabled` true→false → `{"is_enabled": false}`; false→true при отсутствии id → тот же поиск/создание; изменение `chat_enabled` → переподключение EventSub (как при `TwitchLogin`: abort старой задачи, `connect_twitch`).
- `app::exit` ставит на паузу и TTS-награду.
- `app::SCOPES` += ` user:read:chat`.
- [ ] **Step 1: Тесты:** `tts_settings_default_from_old_json` (JSON настроек без `tts` десериализуется, `tts == TtsSettings::default()`), `route_tts`, `validate_rejects_bad_tts` (метка "" и "abcd", скорость 2.0, кулдаун 3601), `settings_change_keeps_tts_reward_id`, расширить `apply_settings_keeps_reward_id_and_updates_reward` (wiremock: PATCH с `is_enabled:false` при выключении).
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(tts): settings and reward lifecycle`.

### Task 3: EventSub — сообщения чата

**Files:** Modify `src-tauri/src/twitch/eventsub.rs`, `src-tauri/src/twitch/helix.rs`, `src-tauri/src/twitch/tests.rs`; Create `src-tauri/src/twitch/fixtures/chat_message.json` (пример из документации Twitch `channel.chat.message`, с фрагментами text+emote и badges `subscriber`)

**Interfaces — Produces:**
- `crate::model::ChatMessage { pub user_login: String, pub fragments: Vec<tts::filter::Fragment>, pub badges: Vec<String> /*set_id*/, pub source_broadcaster_id: Option<String>, pub broadcaster_id: String }`.
- `EsMsg::Chat(ChatMessage)`; `parse_message` различает `subscription_type` (`…redemption.add` → `Notification`, `channel.chat.message` → `Chat`); прочие типы уведомлений → `EsMsg::Keepalive`-подобный пропуск (`EsMsg::Ignored`), **не** ошибка (раньше `bail!` рвал сессию).
- `Helix::subscribe_chat(&self, session_id: &str) -> Result<()>` — `{"type":"channel.chat.message","version":"1","condition":{"broadcaster_user_id":b,"user_id":b},"transport":{…}}`.
- `eventsub::run(helix, ws_url, out, chat: bool)`: после `subscribe_redemptions` при `chat` → `subscribe_chat`; ошибка → `out.send(EsSignal::ChatUnavailable)`, сессия продолжается. `EsSignal::Chat(ChatMessage)` для `EsMsg::Chat`. Фильтр общего чата здесь не делается (делает движок).
- [ ] **Step 1: Тесты:** `parse_chat_message` (фикстура → логин, 2 фрагмента, badges `["subscriber"]`), `unknown_notification_ignored` (тип `channel.follow` → `Ignored`, не `Err`), `eventsub_chat_subscription_forbidden_keeps_session` (wiremock: redemptions 202, chat 403 → первым сигналом `ChatUnavailable`, затем уведомление о погашении доходит как `Redemption`), `eventsub_chat_disabled_does_not_subscribe` (`chat=false` → POST на chat не вызывается; `expect(0)`).
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(twitch): chat messages over EventSub`.

### Task 4: `Runner::synth_file` — синтез без «Рендеров»

**Files:** Modify `src-tauri/src/voice_lab/runner.rs`, `src-tauri/src/voice_lab/commands.rs`

**Interfaces — Produces:**
- Тело `synth` разделить: приватный `async fn synth_raw(&self, root, style_path, params, out: &Path, kind: &str, emit) -> Result<(i64 /*duration_ms*/, i64 /*synth_ms*/), VlError>` (занимает `synth_busy`, `JobGuard`, воркер, таймаут 300 с); `synth` = `synth_raw` во временный файл + регистрация (поведение не меняется).
- `pub async fn synth_file(&self, root: &Path, style_path: PathBuf, params: RenderParams, out: &Path) -> Result<i64 /*duration_ms*/, VlError>` — `kind = "tts"`, emit — no-op; при ошибке файла `out` не остаётся.
- В `commands.rs`: `pub(crate) async fn tts_synth(lab: &VoiceLab, profile_id: i64, params: RenderParams, out: &Path) -> Result<i64, VlError>` — `validate_params`, `require_model`, путь стиля профиля (как `render`), затем `synth_file`.
- [ ] **Step 1: Тесты** (фейковый воркер, `cargo build --examples`): `synth_file_writes_without_render` (файл есть, `store.renders()` пуст), `synth_file_busy` (занятый слот → `JOB_BUSY`), `synth_file_failure_leaves_no_file`; существующие тесты `synth` зелёные.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(voice-lab): synth to file for tts`.

### Task 5: сервер — `/widget/tts`, `/tts/{id}.wav`, роль `tts`

**Files:** Modify `src-tauri/src/server/mod.rs`, `src-tauri/src/server/ws.rs`, `src-tauri/src/server/tests.rs`, `src-tauri/src/app.rs` (передать каталог и канал); Create `src-tauri/assets/widget/tts.html`

**Interfaces — Produces:**
- `WsOut::TtsPlay { id: u64 }`, `WsOut::TtsStop`. Сессии с `role=tts` получают только `Tts*` и `Reload`; остальные роли `Tts*` не получают.
- `pub enum TtsWidgetEvent { Connected, Ended(u64), Error(u64), Disconnected }`; `ServerChannels.tts_rx: mpsc::Receiver<TtsWidgetEvent>`. Сообщение от виджета: `{"type":"tts","id":N,"event":"ended"|"error"}`. Вытеснение: одна активная `tts`-сессия (как `player`, отдельный слот), вытесненная закрывается кодом 4001 и её события игнорируются; `Disconnected` шлётся только активной сессией при выходе.
- `build`/`serve`/`start_or_disabled` получают `tts_dir: PathBuf`; маршрут `GET /tts/{file}`: `file` строго `^[0-9]{1,20}\.wav$`, иначе 404; `audio/wav`; файл читается из `tts_dir`.
- `GET /widget/tts` → `tts.html`: без внешних ресурсов; WebSocket `ws://${location.host}/ws?role=tts` с переподключением раз в 2 с; на `TtsPlay` — `new Audio('/tts/'+id+'.wav')`, `ended`/`error` → отправить событие; на `TtsStop` — пауза и сброс.
- [ ] **Step 1: Тесты:** `tts_wav_served_and_name_checked` (`/tts/5.wav` 200 `audio/wav`; `/tts/..%2Fx.wav`, `/tts/a.wav` → 404), `tts_role_receives_only_tts` (`player`-сессия не получает `TtsPlay`, `tts` не получает `YtPlay`), `tts_events_forwarded` (ended/error/Connected/Disconnected), `second_tts_widget_evicts_first`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(server): tts widget and audio route`.

### Task 6: очередь и движок озвучки

**Files:** Create `src-tauri/src/tts/queue.rs`, `src-tauri/src/tts/engine.rs`; Modify `src-tauri/src/tts/mod.rs`

**Interfaces — Produces:**
- `queue.rs`: `pub enum Origin { Chat, Reward(RedemptionRef), Test }`; `pub struct Item { pub id: u64, pub origin: Origin, pub user: String, pub text: String }`; `pub enum Outcome { Played, Skipped, Rejected(filter::Reject), QueueFull, SynthFailed, Cleared, Reconciled }`; `pub fn fulfilled(o: &Outcome) -> bool` (Played|Skipped). `pub struct Queue { items: VecDeque<Item>, current: Option<Item> }` с `push(item) -> Result<(), Item>` (полна при 20; `Test` не ограничен), `start_next() -> Option<&Item>`, `finish() -> Option<Item>`, `clear() -> Vec<Item>`, `contains_redemption(&str) -> bool`, `view() -> TtsView`.
- `#[derive(Serialize, Clone)] pub struct TtsView { pub current: Option<ItemView>, pub queue: Vec<ItemView>, pub widget: bool, pub warning: Option<String> }`, `ItemView { id, user, text, reward: bool }`.
- `engine.rs`: `pub enum TtsMsg { Chat(ChatMessage), Reward(Redemption), Settings(TtsSettings), BroadcasterId(String), Widget(TtsWidgetEvent), Skip, Clear, Test(String), Reconcile(Vec<RedemptionRef>), State(oneshot::Sender<TtsView>) }`.
- `pub type SynthFn = Arc<dyn Fn(i64 /*profile*/, String /*text*/, String /*lang*/, f32, PathBuf /*out*/) -> BoxFuture<'static, Result<i64, String /*code*/>> + Send + Sync>`.
- `pub struct EngineDeps { pub synth: SynthFn, pub dir: PathBuf /*tts wav dir*/, pub ws: broadcast::Sender<WsOut>, pub outcome: Arc<dyn Fn(RedemptionRef, bool) + Send + Sync>, pub view: Arc<dyn Fn(&TtsView) + Send + Sync> }`; `pub async fn run(deps: EngineDeps, rx: mpsc::Receiver<TtsMsg>)`.
- Правила `run`:
  - `Chat`: игнор, если `!chat_enabled`, `source_broadcaster_id` задан и ≠ `broadcaster_id`; `strip_emotes` → `extract_marked` → `role_allowed` → `Gate::check_and_mark` → `clean`; любой отказ — тихо. Очередь полна — тихо.
  - `Reward`: игнор, если `!reward_enabled` или id награды ≠ `reward_id` (драйвер всё равно фильтрует); `clean(user_input)`; отказ/полная очередь → `outcome(ref,false)`.
  - Голос не выбран → `warning = "tts_no_voice"`, элементы отклоняются (`SynthFailed`).
  - Проигрывание: если нет текущего, есть элемент и виджет подключён → синтез в `dir/<id>.wav` (повтор при `"JOB_BUSY"` раз в 1 с до 60 с; остальные ошибки — сразу `SynthFailed`), затем `TtsPlay{id}`, ждать `Ended(id)` или `duration+10 с` → `Played`; `Error(id)` → `SynthFailed` (возврат баллов). После — удалить WAV, `outcome` для наград, следующий.
  - Синтез идёт в отдельной задаче; во время синтеза/проигрывания цикл продолжает принимать сообщения (`select!`).
  - `Skip` → текущий `Skipped`, `TtsStop`; `Clear` → все в очереди `Cleared` (текущий не трогается).
  - `Widget(Disconnected)` во время проигрывания → ничего, страховочный таймаут доведёт; `Connected` → попробовать начать.
  - `Reconcile(refs)` → для каждого ref, которого нет в очереди/текущем → `outcome(ref,false)`.
  - После каждого изменения — `view(&view)`.
- [ ] **Step 1: Тесты** (`tokio::test(start_paused = true)`, фейковый `SynthFn` пишет пустой файл и отдаёт 1000 мс): `chat_marked_message_plays`, `chat_cooldown_and_roles`, `shared_chat_ignored`, `reward_rejected_refunds` (стоп-слово → outcome false), `reward_played_fulfills` (Ended → true), `queue_full_refunds_reward`, `skip_fulfills_clear_refunds`, `widget_missing_waits` (без `Connected` синтез не вызывается; после — вызывается), `safety_timeout_advances` (без Ended через 11 с → следующий), `synth_busy_retries_then_fails` (фейк 61 раз `JOB_BUSY` → outcome false), `reconcile_cancels_only_unknown`, `wav_deleted_after_play`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(tts): queue and playback engine`.

### Task 7: связка в драйвере и приложении, команды панели

**Files:** Modify `src-tauri/src/app.rs`, `src-tauri/src/driver.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/obs.rs`

**Interfaces — Produces:**
- `app::start`: каталог `voice-lab/tmp/tts` (очистить при старте), канал `TtsMsg` (128), `handle.manage(TtsHandle(mpsc::Sender<TtsMsg>))`, задача `tts::engine::run` с `SynthFn` поверх `VlState` (`commands::tts_synth`, качество 8; `VlState(None)` → `"VL_UNAVAILABLE"`), `outcome` → `DriverMsg::TtsOutcome(RedemptionRef, bool)`, `view` → `emit("tts", view)`; виджетные события из `ServerChannels.tts_rx` пересылаются в движок; начальные `Settings(tts)`.
- Драйвер: `EsSignal::Chat` → `TtsMsg::Chat`; погашение с `route == Tts` → `TtsMsg::Reward` (без записи в БД музыки); `TtsOutcome` → `status(ref, ok)`; `EsSignal::ChatUnavailable` → `warn("tts_chat_relogin")`; после `connect_twitch` — `helix.me()` → `TtsMsg::BroadcasterId`; reconcile — также `unfulfilled(tts.reward_id)` → `TtsMsg::Reconcile`; применение настроек → `TtsMsg::Settings`.
- Команды Tauri: `tts_state() -> TtsView`, `tts_skip()`, `tts_clear()`, `tts_test(text: String)` (1–300 символов), `obs_add_tts()`; регистрация в `lib.rs`.
- `obs::add_tts_source(cfg, url)` — браузерный источник «Stream Assist — Озвучка», 400×100, `reroute_audio: true`, мониторинг и вывод (переиспользовать логику «создать или обновить» из `add_widget_to_current_scene`, вынеся её в общую функцию).
- [ ] **Step 1: Тесты:** `driver_routes_tts_redemption_to_engine` (погашение TTS-награды не попадает в музыкальную очередь/БД), `tts_outcome_sets_status` (wiremock PATCH `FULFILLED`/`CANCELED`), `chat_unavailable_warns`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS; `cargo clippy --all-targets` без новых предупреждений. **Step 5:** Commit `feat(tts): wire engine into driver and panel commands`.

### Task 8: вкладка «Озвучка», документация

**Files:** Create `src/lib/views/Tts.svelte`, `src/lib/ttsApi.ts`; Modify `src/routes/+page.svelte`, `src/lib/api.ts` (`Settings.tts`, `defaultSettings.tts`), `src/lib/i18n/ru.ts`, `docs/user-guide.md`, `docs/architecture.md`

**Interfaces — Consumes:** команды Task 7, событие `tts` (`TtsView`), `voiceApi.vl_profiles()`.
- Вкладка `tts` («Озвучка») после «Голосовой лаборатории»: форма настроек (сохраняется общим `save`), выбор голоса из профилей, язык, скорость (ползунок 0.7–1.5), блоки «Награда» и «Чат» (метка, роли, кулдауны), «Фильтры» (ссылки, стоп-слова построчно), адрес `http://127.0.0.1:<port>/widget/tts` + «Добавить в OBS»; блок очереди (текущий, список, «Пропустить», «Очистить очередь»), поле + «Проверить». Предупреждения: `tts_no_voice`, `tts_chat_relogin` (с кнопкой перелогина из «Подключений»), «Источник OBS не подключён» при `widget == false` и непустой очереди. Подсказка под меткой: «Озвучивается текст до метки. „100%“ не считается».
- `docs/user-guide.md`: раздел «Озвучка»; `docs/architecture.md`: абзац про модуль `tts`.
- [ ] **Step 1:** `npm run check` → 0 ошибок/предупреждений; `node --test tests/*.mjs` зелёные.
- [ ] **Step 2:** Ручная проверка (пользователь): см. чек-лист в отчёте.
- [ ] **Step 3:** Commit `feat(panel): stream tts tab`.
