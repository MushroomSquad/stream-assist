# Правила «триггер → действие» и редизайн панели — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Явная вертикаль «действие (одно на вид) ← список триггеров (вид + параметры)» в модели, маршрутизации, жизненном цикле наград и панели; затем полный редизайн панели по навыку `unslop` с экраном правил (раскладка A).

**Architecture:** Новый чистый модуль `src-tauri/src/rules.rs` (типы, умолчания, миграция, валидация, маршрутизация выкупа/чата, расчёт операций над наградами) заменяет `router.rs`. Драйвер исполняет `RewardOp` и маршруты, музыкальный автомат не меняется. Движок озвучки получает уже сматченный текст чата и флаг активности награды. Фаза 2 — только Svelte/CSS, команды Tauri не меняются, кроме выдачи ошибок наград по триггеру (Task 4).

**Tech Stack:** Rust (tokio, axum, serde, wiremock в тестах), Tauri 2, SvelteKit 2 + Svelte 5, обычный CSS. Новых крейтов нет.

**Spec:** `docs/superpowers/specs/2026-11-03-rules-and-redesign-design.md`

**Исполнение (решение пользователя):** план → критик + архитектор до консенсуса → верификатор → задачи исполняет codex через `omc ask codex` (`node ~/.claude/plugins/cache/omc/oh-my-claudecode/<ver>/bin/oh-my-claudecode.js ask codex -p ...`), ревью каждой задачи — субагент Claude. **Исключение:** Task 6 (аудит и направление) ведёт основная сессия Claude с навыком `unslop` и визуальным компаньоном — это интерактивная контрольная точка с пользователем.

## Global Constraints

- Новых крейтов нет; новых UI-библиотек нет. Шрифты — локальные файлы в `static/fonts/` (woff2), без загрузки по сети.
- Все существующие тесты остаются зелёными (адаптация под новую форму настроек допустима, смысл проверок — нет). Команды `cargo` — из `src-tauri/`; интеграционные тесты Voice Lab требуют `cargo build --examples`.
- Инварианты `docs/architecture.md` 1–8 не нарушаются: баллы возвращаются только до старта, FULFILLED только после подтверждённого старта, лок `Db` не держится через `.await`.
- Допустимые сочетания: `Music ← Reward`; `Tts ← Reward, ChatMarker`. Не больше 10 триггеров на действие.
- Наградный триггер: название 1–45 символов после `trim`, цена 1–2147483647, кулдаун 0–604800; названия уникальны среди **всех** наградных триггеров (сравнение `trim().to_lowercase()`).
- Метка: 1–3 символа, без пробельных; метки уникальны среди всех триггеров-меток. Озвучивается текст **до** метки (`filter::extract_marked`, цифра перед меткой — не метка).
- `TtsCfg`: скорость 0.7–1.5, кулдауны 0–3600, стоп-слов ≤ 200 по ≤ 50 символов, `lang` ∈ `supertonic3_tts::AVAILABLE_LANGS` (пределы как сейчас).
- Триггер активен ⇔ `trigger.enabled && action.enabled`. Награды обходят роли и кулдауны действия; музыкальные `Limits` (правила допуска трека) действуют на каждый заказ музыки.
- Награда на Twitch создаётся **лениво**: только для активного триггера без `reward_id`. Выключенный триггер без `reward_id` на Twitch ничего не создаёт.
- Выключение триггера/действия: `{"is_enabled":false}`, очередь доигрывается без возврата; выкуп, пришедший по неактивному триггеру, отменяется с возвратом.
- Удаление наградного триггера: локально снять не начатые элементы (музыка — `Input::Remove`, озвучка — `TtsMsg::DropReward`), затем в фоне: все `UNFULFILLED` выкупы этой награды → `CANCELED` (дождаться каждого), затем `DELETE` награды. Ошибка удаления → `{"is_enabled":false}` + ошибка у триггера. (Twitch при удалении переводит `UNFULFILLED` в `FULFILLED` — поэтому сначала отмена.)
- Выход и `TwitchLogout` — `{"is_paused":true}` всем наградам с `reward_id`, параллельно, бюджет 2 с.
- Миграция односторонняя: после загрузки старой формы настройки сразу пересохраняются в новой.
- Тексты UI — русские, в `src/lib/i18n/ru.ts`; тексты чата — `i18n.rs`.

## Review Focus

1. **Обновление со старым конфигом, где обе награды уже созданы** — те же `reward_id`, метка, лимиты; при подключении ни одного `POST` наград (Task 1 `migrate_legacy_full`, Task 4 `connect_after_migration_creates_nothing`).
2. **Стример выключил музыку посреди стрима** — очередь доигрывает, награда `is_enabled:false`, новый выкуп отменён с возвратом и не попал в очередь (Task 4 `inactive_music_redemption_refunded`).
3. **Удаление наградного триггера с элементами в очереди** — отмена всех `UNFULFILLED` строго до `DELETE`; сбой `DELETE` → выключение + ошибка у триггера (Task 3 `retire_cancels_before_delete`, `retire_delete_failure_disables`).
4. **Дубль названия/метки, недопустимое сочетание, смена вида у триггера** — сохранение отклонено целиком, ничего не записано (Task 1 `validate_*`); смена вида под тем же id трактуется как удаление + новый триггер (Task 2 `assign_ids_kind_change_is_new`).
5. **Награду удалили вручную на Twitch, триггер выключен** — не пересоздаётся, `reward_id` сбрасывается; при включении — создаётся (Task 2 `connect_ops_*`, Task 4 `ensure_missing_inactive_clears_id`).

---

## Фаза 1 — правила в ядре

### Task 1: `rules.rs` — модель, умолчания, валидация, миграция

**Files:** Create `src-tauri/src/rules.rs`; Modify `src-tauri/src/lib.rs` (`pub mod rules;`), `src-tauri/src/tts/mod.rs` (добавить `TtsCfg`, `TtsRuntime`; `TtsSettings` пока остаётся — нужна миграции и удаляется в Task 4)

**Interfaces — Produces:**
- `tts/mod.rs`: `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)] pub struct TtsCfg { pub profile_id: Option<i64>, pub lang: String, pub speed: f32, pub roles: filter::Roles, pub global_cooldown_s: u32, pub user_cooldown_s: u32, pub strip_links: bool, pub stop_words: Vec<String> }` — `Default` = значения из текущего `TtsSettings::default()`.
- `tts/mod.rs`: `#[derive(Clone, Debug, PartialEq)] pub struct TtsRuntime { pub cfg: TtsCfg, pub in_use: bool /* есть активный триггер озвучки */, pub chat_active: bool /* есть активная метка */ }`.
- `rules.rs`:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum ActionKind { Music, Tts }`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum TriggerTag { Reward, ChatMarker }`; `pub fn allowed(a: ActionKind) -> &'static [TriggerTag]`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(tag = "kind", rename_all = "snake_case")] pub enum TriggerKind { Reward { title: String, cost: u32, cooldown_s: u32, #[serde(default)] reward_id: Option<String> }, ChatMarker { marker: String } }` + `pub fn tag(&self) -> TriggerTag`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Trigger { #[serde(default)] pub id: String, pub enabled: bool, #[serde(flatten)] pub kind: TriggerKind }`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Action<C> { pub enabled: bool, pub cfg: C, pub triggers: Vec<Trigger> }`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)] pub struct MusicCfg { pub limits: crate::limits::Limits }`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Rules { pub music: Action<MusicCfg>, pub tts: Action<TtsCfg> }` + `Default`: music `enabled:true`, триггер `{id:"music-reward", enabled:true, Reward{"Заказать трек",500,0,None}}`; tts `enabled:true`, `{id:"tts-reward", enabled:false, Reward{"Озвучить текст",300,0,None}}`, `{id:"tts-chat", enabled:false, ChatMarker{"%"}}`.
  - `impl Rules`: `pub fn triggers(&self) -> Vec<(ActionKind, &Trigger)>` (порядок: music, затем tts, внутри — порядок списка); `pub fn is_active(&self, a: ActionKind, t: &Trigger) -> bool`; `pub fn reward_ids(&self) -> Vec<String>`; `pub fn set_reward_id(&mut self, trigger_id: &str, id: Option<String>) -> bool`; `pub fn tts_runtime(&self) -> TtsRuntime`.
  - `pub fn validate(r: &Rules) -> Result<(), String>` — ошибка всегда `"settings_invalid"`; правила из Global Constraints (сочетания, ≤ 10, пределы, уникальность).
  - `pub fn from_legacy(v: &serde_json::Value) -> Rules` — читает старые корневые `reward_title/reward_cost/reward_cooldown_s/music_reward_id/limits` и `tts` (`TtsSettings`, serde-default) по таблице спеки «Миграция»; отсутствующие поля → умолчания. Id триггеров: `music-reward`, `tts-reward`, `tts-chat`.

- [ ] **Step 1: Тесты** (`mod tests` в `rules.rs`):
  - `defaults_shape` — `Rules::default()` ровно как выше; `serde_json::to_value` триггера-награды == `{"id":"music-reward","enabled":true,"kind":"reward","title":"Заказать трек","cost":500,"cooldown_s":0,"reward_id":null}`, метки — `{"id":"tts-chat","enabled":false,"kind":"chat_marker","marker":"%"}`; round-trip `from_value(to_value(x)) == x`.
  - `migrate_legacy_full` — JSON старой формы с `music_reward_id:"m1"`, `reward_title:"Трек"`, `reward_cost:700`, `reward_cooldown_s:30`, `limits.no_repeat_last_n:5`, `tts:{reward_enabled:true, chat_enabled:false, reward_id:"t1", reward_title:"Скажи", reward_cost:100, marker:"!!", speed:1.2, stop_words:["x"]}` → музыка `Reward{"Трек",700,30,Some("m1")}` enabled, `limits.no_repeat_last_n==5`; tts-reward `enabled:true` с `Some("t1")`; tts-chat `enabled:false`, `marker:"!!"`; `cfg.speed==1.2`, `cfg.stop_words==["x"]`; оба действия `enabled:true`.
  - `migrate_legacy_empty` — `from_legacy(&json!({}))` == `Rules::default()`.
  - `validate_ok_default`; `validate_rejects` — по одному нарушению на кейс: метка-триггер у музыки; 11 триггеров; название `""`/`"   "`/46 символов; цена 0; кулдаун 604801; дубль названия `"Заказать трек"` vs `" заказать ТРЕК "`; две метки `"%"`; метка `""`, `"abcd"`, `"a b"`; `speed` 0.69 и 1.51; `global_cooldown_s` 3601; 201 стоп-слово; стоп-слово 51 символ; `lang:"xx"`.
  - `is_active_matrix` — 4 сочетания `trigger.enabled × action.enabled`.
  - `tts_runtime_flags` — `in_use`/`chat_active` при: всё выкл; только награда; только метка; действие выкл при включённых триггерах (оба `false`).
- [ ] **Step 2:** `cargo test --lib rules` → FAIL (нет модуля).
- [ ] **Step 3:** Реализовать типы и функции.
- [ ] **Step 4:** `cargo test --lib rules` → PASS; `cargo test` → всё зелёное.
- [ ] **Step 5:** Commit `feat(rules): model, defaults, validation, legacy migration`.

### Task 2: `rules.rs` — маршрутизация и операции над наградами

**Files:** Modify `src-tauri/src/rules.rs`

**Interfaces — Consumes:** Task 1 типы; `crate::tts::filter::{extract_marked, strip_emotes}`; `crate::model::ChatMessage`.
**Produces:**
- `#[derive(Clone, Debug, PartialEq)] pub struct RewardRoute { pub action: ActionKind, pub trigger_id: String, pub active: bool }`; `pub fn route_reward(r: &Rules, reward_id: &str) -> Option<RewardRoute>` — по `reward_id` любого наградного триггера (активного или нет).
- `#[derive(Clone, Debug, PartialEq)] pub struct ChatRoute { pub action: ActionKind, pub trigger_id: String, pub text: String }`; `pub fn route_chat(r: &Rules, m: &ChatMessage) -> Option<ChatRoute>` — `None`, если `source_broadcaster_id` есть и ≠ `broadcaster_id`; текст = `strip_emotes(&m.fragments)`; первая **активная** метка (порядок `triggers()`), для которой `extract_marked` даёт `Some`.
- `pub fn reward_body(title: &str, cost: u32, cooldown_s: u32, enabled: bool) -> serde_json::Value` — ровно текущий `TtsSettings::reward_body`: `{"title","cost","is_enabled","is_paused":false,"global_cooldown_setting":{"is_enabled":cooldown_s>0,"global_cooldown_seconds":cooldown_s}}`.
- `pub fn reward_prompt(a: ActionKind) -> &'static str` — Music `"Название или ссылка на трек"`, Tts `"Текст для озвучки"`.
- `#[derive(Clone, Debug, PartialEq)] pub enum RewardOp { Ensure { action: ActionKind, trigger_id: String, known_id: Option<String>, title: String, cost: u32, create: bool }, Patch { reward_id: String, body: serde_json::Value }, Delete { action: ActionKind, reward_id: String } }`
- `pub fn connect_ops(r: &Rules) -> Vec<RewardOp>` — для каждого наградного триггера: есть `reward_id` → `Ensure{known_id:Some, create: is_active}`; нет id и активен → `Ensure{known_id:None, create:true}`; нет id и неактивен → ничего.
- `pub fn save_ops(old: &Rules, new: &Rules) -> Vec<RewardOp>` — сопоставление по `Trigger.id`: был в old с `reward_id`, нет в new → `Delete`; в new с id: изменились `(title,cost,cooldown_s,active)` → `Patch{ body: if was_active && !now_active {{"is_enabled":false}} else reward_body(.., now_active) }`; в new без id: активен и (раньше не был активен или изменились title/cost/cooldown) → `Ensure{known_id:None, create:true}`.
- `pub fn assign_ids(current: &Rules, incoming: Rules) -> Rules` — для каждого триггера incoming: если `id` совпадает с триггером current того же действия **и того же `TriggerTag`** → `reward_id` берётся из current (incoming игнорируется); иначе (пустой, неизвестный id, другой вид) — новый id `"t{n}"`, `n` = 1 + максимум среди существующих `t<число>` (во всех действиях), `reward_id = None`.

- [ ] **Step 1: Тесты:**
  - `route_reward_cases` — id музыки → `{Music,"music-reward",active}`; id выключенной награды озвучки → `active:false`; неизвестный → `None`.
  - `route_chat_cases` — активная `%`: `"привет %"` → `text:"привет"`; `"на 100% согласен"` → `None`; метка выключена → `None`; действие выключено → `None`; чужой `source_broadcaster_id` → `None`; эмоут вырезан до поиска метки; две метки `"%"` и `"!!"` — срабатывает та, что находится в тексте.
  - `reward_body_shape` — побайтово как в существующем `tts_reward_body`.
  - `connect_ops_lazy` — `Rules::default()` без id → один `Ensure{music-reward, None, create:true}`, для выключенной tts-reward ничего; tts-reward выключена с id → `Ensure{known_id:Some, create:false}`.
  - `save_ops_cases` — без изменений → `[]`; смена цены у награды с id → `Patch(reward_body(..,true))`; выключение триггера с id → `Patch({"is_enabled":false})`; выключение действия → `Patch` для всех его наград с id; включение без id → `Ensure{create:true}`; смена названия у активной без id (повтор после сбоя) → `Ensure`; удаление триггера с id → `Delete`; удаление без id → `[]`.
  - `assign_ids_keeps_owned_reward_id` — incoming подменил `reward_id` → берётся из current.
  - `assign_ids_new_trigger` — пустой id при существующем `t3` → `t4`, `reward_id:None`.
  - `assign_ids_kind_change_is_new` — `tts-chat` пришёл как `Reward` → новый id; `save_ops(current, assigned)` при этом не содержит `Patch` по старому id.
- [ ] **Step 2–4:** FAIL → реализовать → `cargo test --lib rules` PASS.
- [ ] **Step 5:** Commit `feat(rules): routing and reward operations`.

### Task 3: Helix `delete_reward`/`retire_reward`, движок озвучки на `TtsRuntime`

**Files:** Modify `src-tauri/src/twitch/helix.rs`, `src-tauri/src/twitch/tests.rs`, `src-tauri/src/tts/engine.rs`, `src-tauri/src/tts/queue.rs`, `src-tauri/src/driver.rs` и `src-tauri/src/app.rs` (только временный адаптер, см. Step 3)

**Interfaces — Consumes:** `TtsRuntime`, `TtsCfg` (Task 1).
**Produces:**
- `Helix::delete_reward(&self, id: &str) -> Result<()>` — `DELETE /channel_points/custom_rewards?broadcaster_id&id`; 404 → `Ok`.
- `Helix::retire_reward(&self, id: &str) -> Result<()>` — `unfulfilled(id)` → для каждого `set_redemption_status(&r.r, false).await?` последовательно → `delete_reward(id)`. При ошибке `DELETE` → `update_reward(id, {"is_enabled":false})` и вернуть `Err`.
- `TtsMsg` меняется: `Settings(TtsRuntime)`, `Reward { r: Redemption, active: bool }`, `Chat { msg: ChatMessage, text: String }`, новое `DropReward(String /* reward_id */)`; остальные варианты без изменений.
- `engine::run(deps, settings: TtsRuntime, rx)`.
- `Queue::remove_pending(&mut self, pred: impl Fn(&Item) -> bool) -> Vec<Item>` — удаляет совпавшие элементы из ожидающих (не текущий).

Поведение движка: `Chat` — маркер и источник уже проверены маршрутизатором; движок игнорирует сообщение, если `!chat_active`; дальше как сейчас (стример обходит роли/кулдауны, `clean`, голос). `Reward` — `!active` → `Outcome::Disabled`, дальше как сейчас. `tts_no_voice` при `profile_id.is_none() && in_use`; `chat_warning` показывается при `chat_active`. `DropReward(id)` → `remove_pending` элементов `Origin::Reward(r)` с `r.reward_id == id` → `report(.., Outcome::Cleared)` каждому.

- [ ] **Step 1: Тесты:**
  - `twitch/tests.rs` (wiremock): `delete_reward_ok_and_404`; `retire_cancels_before_delete` — 2 `UNFULFILLED`, проверка порядка: оба `PATCH status=CANCELED` получены сервером раньше `DELETE` (сравнить порядок в `received_requests()`); `retire_delete_failure_disables` — `DELETE` 500 → `PATCH {"is_enabled":false}` и `Err`.
  - `engine.rs`: адаптировать существующие тесты к новым сообщениям (смысл не меняется); новые `drop_reward_cancels_pending_only` (текущий элемент этой награды остаётся и доигрывает, ожидающие этой награды → `CANCELED`, чужие и чатовые остаются); `inactive_reward_disabled`; `chat_ignored_when_chat_inactive`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Реализовать. Временный адаптер, чтобы сборка была зелёной до Task 4: драйвер/`app.rs` строят `TtsRuntime` через `rules::from_legacy(&serde_json::to_value(&settings)?).tts_runtime()`, `Reward{active: settings.tts.reward_enabled}`, `Chat` — драйвер вызывает `route_chat` на `from_legacy(..)`. Пометить `// TODO(Task 4)`; Task 4 удаляет адаптер.
- [ ] **Step 4:** `cargo test` → PASS.
- [ ] **Step 5:** Commit `feat(tts,twitch): engine on TtsRuntime, reward retire`.

### Task 4: `Settings.rules` — драйвер, приложение, жизненный цикл наград

**Files:** Modify `src-tauri/src/driver.rs`, `src-tauri/src/app.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/tts/mod.rs` (удалить `TtsSettings`, `tts_reward_patch`, их тесты — покрыты Task 1/2), `src-tauri/tests/core.rs`; Delete `src-tauri/src/router.rs`

**Interfaces — Consumes:** всё из Task 1–3.
**Produces:**
- `driver::Settings { #[serde(default)] pub rules: Rules, pub port, pub active_theme, pub youtube_api_key, pub obs, pub spotify_client_id }` (без `reward_*`, `music_reward_id`, `limits`, `tts`).
- `pub fn load_settings(db: &Db) -> Result<Settings>` (в `driver.rs`): читает `serde_json::Value`; нет записи → `Settings::default()`; есть ключ `"rules"` → `from_value`; иначе — `from_value` для общих полей + `rules = rules::from_legacy(&v)`. Используется во **всех** местах чтения `"settings"` (`app.rs` bootstrap, `exit`, `get_settings`, тесты). Bootstrap после загрузки legacy сразу пишет новую форму `set_setting("settings", ..)`.
- `app::validate_settings`: `port`, `obs.port` как сейчас + `rules::validate(&s.rules)`.
- `apply_settings`: `incoming.rules = rules::assign_ids(&current.rules, incoming.rules)`; ops = `save_ops(&old.rules, &new.rules)` → исполнить; `TtsMsg::Settings(new.rules.tts_runtime())`; `Input::SetMaxDuration(new.rules.music.cfg.limits.max_duration_ms)`.
- Исполнение `RewardOp` в драйвере (`fn run_op(ctx, op, tx)`):
  - `Ensure` → фон: `find_manageable_reward(known_id, title)`; найдено → `Ok(Some(id))`; не найдено и `create` → `create_reward(title, cost, reward_prompt(action))` (тексты ошибок `RewardError` — как сейчас); не найдено и `!create` → `Ok(None)`. Результат → `Internal::RewardReady { action, trigger_id, result: Result<Option<String>> }`.
  - `RewardReady(Ok(Some(id)))` → `rules.set_reward_id`, сохранить настройки, `Patch(reward_body(.., is_active))`, reconcile этой награды для её действия, снять ошибку триггера. `Ok(None)` → `set_reward_id(None)`, сохранить. `Err(e)` → ошибка у триггера (если id у триггера уже появился параллельно — ошибку игнорировать, как сейчас у TTS).
  - `Patch` → `background(update_reward)`.
  - `Delete{Music,id}` → для каждого элемента очереди `player.snapshot().1` с `redemption.reward_id == id` → `player.step(Input::Remove(item.id))` (+ `actions`); `Delete{Tts,id}` → `TtsMsg::DropReward(id)`; затем фон `retire_reward(id)` с таймаутом 30 с; ошибка → ошибка у триггера (по `reward_id`, триггер уже удалён — ключ `"deleted:<reward_id>"`).
- Ошибки наград по триггеру: `BTreeMap<String /* trigger_id */, String>` в драйвере; при изменении — событие Tauri `"rule_errors"` с картой; `DriverMsg::RuleErrors(oneshot::Sender<BTreeMap<String,String>>)` и команда `app::rule_errors` (регистрация в `lib.rs`). Ошибки наград больше не пишутся в `WidgetState.warning` и `TtsMsg::Warning`.
- `redeem`: `route_reward(&rules, &r.r.reward_id)`: `None` → игнор; `Tts` → `TtsMsg::Reward{r, active}`; `Music` и `!active` → если `redemption_state == Unknown` и id не в `inflight`: `mark_cancelled` + `status(false)`; `Music` и `active` → как сейчас.
- Чат: `EsSignal::Chat(m)` → `route_chat` → `Some(Tts)` → `TtsMsg::Chat{msg, text}`; иначе игнор.
- Reconcile/`Resync`: по каждому наградному триггеру с `reward_id` — музыка → существующий `Internal::Reconcile`, озвучка → `Internal::TtsReconcile`.
- `connect_twitch`: вместо двух блоков — `for op in connect_ops(&rules) { run_op(..) }`.
- `TwitchLogout` и `app::exit`: пауза для `rules.reward_ids()` (exit — `join_all`, бюджет 2 с).
- Удалить временный адаптер Task 3, `router.rs`, `keep_reward_id`, `tts_target`, `spawn_tts_reward`, `tts_settings_changed`, `Internal::Reward/TtsReward`.

- [ ] **Step 1: Тесты** (`driver.rs` `mod tests`, wiremock-хелперы как в существующих `apply_settings_*`):
  - `load_settings_migrates_legacy` — в БД старый JSON → `load_settings` даёт ожидаемые `rules`; `load_settings_new_form` — round-trip.
  - `connect_after_migration_creates_nothing` — legacy с обоими id, tts-reward выключена: при подключении сервер не получает ни одного `POST /channel_points/custom_rewards`; получает `PATCH` с `is_enabled:true` для музыки и `is_enabled:false` для озвучки.
  - `ensure_missing_inactive_clears_id` — награда с id не найдена, триггер выключен → нет `POST`, `reward_id` сброшен в БД.
  - `inactive_music_redemption_refunded` — музыка выключена, приходит выкуп → `PATCH status=CANCELED`, в очереди автомата его нет.
  - `apply_settings_keeps_reward_id_and_updates_reward` и `settings_change_keeps_*` — переписать на `rules` (подмена `reward_id` из панели игнорируется).
  - `delete_music_trigger_refunds_queue` — в очереди 2 не начатых элемента этой награды и 1 другой → 2 `Remove` (отмена), третий остаётся; затем `retire` (порядок: `CANCELED` до `DELETE`).
  - `chat_routed_only_on_marker` — сообщение без метки не доходит до движка (канал `TtsMsg` пуст).
  - `app.rs`: `validate_rejects_bad_tts` → переписать на `rules` (пределы те же).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Реализовать.
- [ ] **Step 4:** `cargo build --examples && cargo test` → всё зелёное; `cargo clippy --all-targets` без новых предупреждений.
- [ ] **Step 5:** Commit `feat(rules): settings.rules drives routing and reward lifecycle`.

### Task 5: панель на новой форме настроек (минимально) + документация ядра

**Files:** Modify `src/lib/api.ts`, `src/lib/ttsApi.ts`, `src/lib/views/Rewards.svelte`, `src/lib/views/Limits.svelte`, `src/lib/views/Tts.svelte`, `docs/architecture.md`

**Interfaces — Produces (TS):**
```ts
export type TriggerKind = { kind: 'reward'; title: string; cost: number; cooldown_s: number; reward_id: string | null } | { kind: 'chat_marker'; marker: string };
export type Trigger = { id: string; enabled: boolean } & TriggerKind;
export interface Action<C> { enabled: boolean; cfg: C; triggers: Trigger[] }
export interface Rules { music: Action<{ limits: Limits }>; tts: Action<TtsCfg> }
export interface Settings { rules: Rules; port: number; active_theme: string; youtube_api_key: string | null; obs: {...}; spotify_client_id: string | null }
export const rule_errors = () => isTauri() ? call<Record<string, string>>('rule_errors') : Promise.resolve({});
export const onRuleErrors = (cb: (e: Record<string, string>) => void) => ...listen('rule_errors')
```
`TtsSettings` → `TtsCfg` в `ttsApi.ts`; `defaultSettings` повторяет `Rules::default()`.

- [ ] **Step 1:** Обновить типы и `defaultSettings`.
- [ ] **Step 2:** Вкладки только перепривязать: «Награды» → первый `reward`-триггер музыки; «Лимиты» → `rules.music.cfg.limits`; «Озвучка» → `rules.tts.cfg`, первый `reward`- и первый `chat_marker`-триггер озвучки (их `enabled`). Новых элементов UI нет.
- [ ] **Step 3:** `npm run check` → 0 ошибок; `node --test tests/` → PASS.
- [ ] **Step 4:** `docs/architecture.md`: таблица модулей (`rules.rs` вместо `router.rs`), раздел «Как добавить новую награду» → «Как добавить вид триггера или действие» (вариант `TriggerKind`/`ActionKind`, строка в `allowed`, ветка в `route_*`, исполнение в драйвере), абзац про жизненный цикл наград и миграцию.
- [ ] **Step 5:** Ручная проверка оркестратором: `TWITCH_CLIENT_ID=… npm run tauri dev` со скопированной старой БД — настройки на вкладках те же.
- [ ] **Step 6:** Commit `feat(panel): settings.rules shape; docs: rules architecture`.

---

## Фаза 2 — редизайн

Общие правила фазы: навык `unslop` (`~/.claude/skills/unslop/SKILL.md` и `references/*.md`) — обязательное чтение для исполнителя каждой задачи 7–11; утверждённое направление `docs/superpowers/specs/2026-11-03-panel-direction.md` (Task 6) — закон. Логика компонентов и вызовы `api.ts`/`voiceApi.ts`/`ttsApi.ts` не меняются, кроме Task 8. Проверка экранов — `npm run dev` (Vite, `http://127.0.0.1:1420`, браузерный режим с `defaultSettings`), скриншоты снимает оркестратор (Claude, `claude-in-chrome`) на ширинах 1280 и 900 и прикладывает к ревью; ревьюер сверяет с направлением и `references/signs.md`.

### Task 6: аудит и направление — КОНТРОЛЬНАЯ ТОЧКА (исполняет основная сессия Claude)

**Files:** Create `docs/superpowers/specs/2026-11-03-panel-direction.md`

- [ ] **Step 1:** Скриншоты всех текущих экранов (`npm run dev`): Сейчас, Награды, Лимиты, Виджет, Подключения, Voice Lab (все подвкладки), Озвучка.
- [ ] **Step 2:** Аудит по `unslop` (каждый признак — где живёт).
- [ ] **Step 3:** Направление по `unslop`: характер, мир, где живёт цвет (1–2 момента), сигнатурная деталь, нейтральная шкала, акцент, шрифт(ы) с источником woff2 и лицензией, шкала отступов/радиусов/типографики, длительности и кривая анимации, иконки (inline SVG, без пакета), тёмная тема по умолчанию; набросок навигации «Сейчас · Правила · Виджет · Подключения · Voice Lab» и карточки действия (раскладка A) в визуальном компаньоне.
- [ ] **Step 4:** Показать пользователю, итерировать до **явного** «утверждаю». Без этого задачи 7–11 не начинаются.
- [ ] **Step 5:** Commit `docs: panel design direction`.

### Task 7: токены, шрифты, оболочка и общие элементы

**Files:** Create `src/app.css` (токены `:root`, сброс, базовые `button/input/select/textarea/label/.card/.muted` и т. п.), `static/fonts/*.woff2` + `@font-face`; Modify `src/routes/+layout.ts` или `+layout.svelte` (подключить `app.css`), `src/routes/+page.svelte` (навигация из 5 вкладок; `rewards`/`limits`/`tts` убираются из `tabs`, `rules` добавляется — пока заглушка с `Rewards`/`Limits`/`Tts` друг под другом, чтобы ничего не потерять до Task 8), `src/lib/i18n/ru.ts`

- [ ] **Step 1:** Токены и база ровно по направлению; все цвета/отступы/радиусы — только через переменные.
- [ ] **Step 2:** `npm run check` → 0; `grep -rnE '#[0-9a-fA-F]{3,6}\b' src/lib src/routes --include=*.svelte` — цвета только в `app.css` (исключение — превью виджета `Widget.svelte`, фон-подложка для темы).
- [ ] **Step 3:** Скриншоты оболочки → ревью.
- [ ] **Step 4:** Commit `feat(panel): design tokens, fonts and shell`.

### Task 8: экран «Правила»

**Files:** Create `src/lib/views/rules/Rules.svelte`, `src/lib/views/rules/ActionCard.svelte`, `src/lib/views/rules/TriggerRow.svelte`, `src/lib/views/rules/TriggerEditor.svelte`, `src/lib/views/rules/MusicSettings.svelte` (содержимое `Limits.svelte`), `src/lib/views/rules/TtsSettings.svelte` (настройки из `Tts.svelte`); Delete `src/lib/views/Rewards.svelte`, `src/lib/views/Limits.svelte`; Modify `src/lib/views/Tts.svelte` (остаются очередь, URL виджета, «Добавить в OBS», тест), `src/routes/+page.svelte`, `src/lib/i18n/ru.ts`

**Interfaces — Consumes:** TS-типы и `rule_errors`/`onRuleErrors` из Task 5; `save(settings)` как сейчас.

Поведение:
- Карточка на действие: заголовок, переключатель действия, список триггеров, «+ триггер», раскрываемые «Настройки и лимиты» (`MusicSettings`/`TtsSettings`). У «Озвучки» в карточке также блок очереди/виджета/теста (компонент `Tts.svelte`) — если направление Task 6 не решило иначе.
- Строка триггера: вид (подпись), параметры (награда — название, цена, кулдаун; метка — символ), переключатель, «Изменить», «Удалить», ошибка из `rule_errors[trigger.id]` у строки.
- «+ триггер»: выбор вида только из допустимых (`music: ['reward']`, `tts: ['reward','chat_marker']` — константа рядом с типами в `api.ts`, зеркало `rules::allowed`); затем поля; новый триггер с `id: ''`.
- Удаление наградного триггера с `reward_id` — подтверждение **в интерфейсе** (не `confirm()`): «Награда будет удалена на Twitch, не начатые заказы вернут баллы».
- Клиентская подсказка дублей названия/метки и пределов (HTML-атрибуты + проверка перед `save`); итоговая проверка — ядро (`settings_invalid` → общий текст ошибки).
- Недостижимые сочетания в UI не предлагаются; при 10 триггерах «+ триггер» неактивна.

- [ ] **Step 1:** Реализовать компоненты.
- [ ] **Step 2:** `npm run check` → 0.
- [ ] **Step 3:** Сценарий в браузере (оркестратор): добавить метку к озвучке, изменить цену награды, выключить действие, удалить триггер (видно подтверждение), дубль метки — подсказка; скриншоты → ревью.
- [ ] **Step 4:** Commit `feat(panel): rules screen`.

### Task 9: «Сейчас», «Виджет», «Подключения»

**Files:** Modify `src/lib/views/NowQueue.svelte`, `src/lib/views/Widget.svelte`, `src/lib/views/Connections.svelte`, `src/lib/i18n/ru.ts` (только тексты по проходу copy из `unslop`)

- [ ] **Step 1:** Проходы `unslop` по трём экранам (spacing → … → motion) в рамках направления; поведение и вызовы API не меняются.
- [ ] **Step 2:** `npm run check` → 0; скриншоты (с непустой очередью — временно подставить данные в браузерном режиме только при съёмке, не коммитить) → ревью.
- [ ] **Step 3:** Commit `feat(panel): redesign now, widget, connections`.

### Task 10: Voice Lab

**Files:** Modify `src/lib/views/voice/*.svelte`, `src/lib/i18n/ru.ts`

- [ ] **Step 1:** Проходы `unslop` по `VoiceLab`, `Setup`, `Sources`, `Voices`, `Mixer`, `Train`, `Renders`, `Explore`; логика, события, `canvas`/`audio` — без изменений.
- [ ] **Step 2:** `npm run check` → 0; `node --test tests/` → PASS; скриншоты каждой подвкладки → ревью.
- [ ] **Step 3:** Commit `feat(panel): redesign voice lab`.

### Task 11: финальный проход и документация

**Files:** Modify любые `src/**/*.svelte`, `src/app.css` (только находки аудита), `docs/user-guide.md`, `docs/architecture.md`, `README.md` (если упоминает старые вкладки)

- [ ] **Step 1:** Повторный аудит всей панели по `references/signs.md`; исправить находки.
- [ ] **Step 2:** Клавиатура: Tab проходит все экраны, фокус видим; у каждого поля подпись; контраст текста ≥ 4.5:1 (проверить пары токенов).
- [ ] **Step 3:** `docs/user-guide.md` — разделы про награды, лимиты и озвучку переписать под экран «Правила».
- [ ] **Step 4:** `npm run check`, `node --test tests/`, `cd src-tauri && cargo test` → всё зелёное; `TWITCH_CLIENT_ID=… npm run tauri dev` — ручной просмотр оркестратором.
- [ ] **Step 5:** Commit `feat(panel): final polish; docs: user guide for rules`.
