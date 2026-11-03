# Правила «триггер → действие» и редизайн панели — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Явная вертикаль «действие (одно на вид) ← список триггеров (вид + параметры)» в модели, маршрутизации, жизненном цикле наград и панели; затем полный редизайн панели по навыку `unslop` с экраном правил (раскладка A).

**Architecture:** Чистый модуль `src-tauri/src/rules.rs` (типы, умолчания, миграция, валидация, маршрутизация выкупа/чата, расчёт операций над наградами) заменяет `router.rs`. Чистый `src-tauri/src/rewards.rs` ведёт учёт жизненного цикла наград (что в полёте, что удаляется, ошибки по триггерам) и отдаёт `RewardOp`; асинхронные исполнители операций — свободные функции над `Helix`, тестируемые на wiremock. Драйвер только склеивает: исполняет операции и маршруты, музыкальный автомат не меняется. Движок озвучки получает уже сматченный текст чата и флаг активности награды. Фаза 2 — только Svelte/CSS (+ dev-моки для браузерного предпросмотра).

**Tech Stack:** Rust (tokio, serde, wiremock в тестах), Tauri 2, SvelteKit 2 + Svelte 5, обычный CSS. Новых крейтов нет.

**Spec:** `docs/superpowers/specs/2026-11-03-rules-and-redesign-design.md`

**Исполнение (решение пользователя):** план → критик + архитектор до консенсуса → верификатор → задачи исполняет codex через `omc ask codex`, ревью каждой задачи — субагент Claude. **Исключение:** Task 7 (аудит и направление) ведёт основная сессия Claude с навыком `unslop` и визуальным компаньоном — интерактивная контрольная точка с пользователем; там же скачиваются и коммитятся шрифты (у codex может не быть сети).

## Global Constraints

- Новых крейтов нет; новых UI-библиотек нет. Шрифты — локальные woff2 **с кириллицей**, лицензия OFL или аналогичная, в `static/fonts/` вместе с файлом лицензии.
- Все существующие тесты остаются зелёными (адаптация под новую форму настроек допустима, смысл проверок — нет). Команды `cargo` — из `src-tauri/`; интеграционные тесты Voice Lab требуют `cargo build --examples`.
- Инварианты `docs/architecture.md` 1–8 не нарушаются: баллы возвращаются только до старта, FULFILLED только после подтверждённого старта, лок `Db` не держится через `.await`.
- Допустимые сочетания: `Music ← Reward`; `Tts ← Reward, ChatMarker`. Не больше 10 триггеров на действие.
- Все длины — в символах (`chars().count()` / `[...s].length`), не в байтах.
- Наградный триггер: название 1–45 символов после `trim`, цена 1–2147483647, кулдаун 0–604800; названия уникальны среди **всех** наградных триггеров (сравнение `trim().to_lowercase()`).
- Метка: 1–3 символа, без пробельных; метки уникальны среди всех меток. Озвучивается текст **до** метки (`filter::extract_marked`, цифра перед меткой — не метка).
- `TtsCfg`: скорость 0.7–1.5, кулдауны 0–3600, стоп-слов ≤ 200 по ≤ 50 символов, `lang` ∈ `supertonic3_tts::AVAILABLE_LANGS`.
- Триггер активен ⇔ `trigger.enabled && action.enabled`. Награды обходят роли и кулдауны действия; музыкальные `Limits` действуют на каждый заказ музыки.
- Награда на Twitch создаётся **лениво**: только для активного триггера без `reward_id`. Выключенный триггер без `reward_id` ничего не создаёт.
- Выключение триггера/действия: `{"is_enabled":false}`, очередь доигрывается без возврата; выкуп по неактивному триггеру (в т. ч. ещё резолвящийся) отменяется с возвратом.
- Удаление наградного триггера: `reward_id` переносится в **персистентный** `Rules.retiring`; локально снимаются не начатые элементы (музыка — `Input::Remove`, озвучка — `TtsMsg::DropReward`); выкупы по удаляемой награде отменяются с возвратом; в фоне `retire_reward`: `{"is_enabled":false}` → отмена всех `UNFULFILLED`, кроме `keep` (текущий элемент музыки/озвучки этой награды) → `DELETE`. Успех — убрать из `retiring`; неудача или нет подключения — остаётся в `retiring` и повторяется при следующем подключении. (Twitch при `DELETE` переводит `UNFULFILLED` в `FULFILLED` — поэтому сначала отмена.)
- Id триггеров не переиспользуются: персистентный счётчик `Rules.next_id`.
- Выход и `TwitchLogout` — `{"is_paused":true}` всем наградам из `rules.reward_ids()` (вкл. `retiring`), параллельно, бюджет 2 с.
- Миграция односторонняя: старый JSON сохраняется как есть под ключом `settings_legacy`, затем настройки пересохраняются в новой форме.
- Проверка источника чата (`source_broadcaster_id`) — часть маршрутизации (`route_chat`); роли, кулдауны, фильтры текста — в движке озвучки. *Отклонение от формулировки спеки «в одном месте» зафиксировано здесь: источник — это вопрос «чьё сообщение», а не лимит действия.*
- Тексты UI — русские, в `src/lib/i18n/ru.ts`; тексты чата — `i18n.rs`.

## Review Focus

1. **Обновление со старым конфигом, где обе награды уже созданы** — те же `reward_id`, метка, лимиты; при подключении ни одного `POST` наград (Task 1 `migrate_legacy_full`, Task 2 `connect_ops_after_migration_no_create`, Task 3 `ensure_known_found_no_post`).
2. **Стример выключил музыку посреди стрима, в т. ч. пока заказ резолвится** — очередь доигрывает, новый и резолвящийся выкупы отменены с возвратом (Task 2 `music_decision_*`, Task 5 вызов в `ResolveDone`).
3. **Удаление наградного триггера с очередью, в т. ч. без подключения к Twitch** — `disable` → отмена (кроме текущего) → `DELETE`; сбой/офлайн — награда остаётся в `retiring` и удаляется при следующем подключении (Task 3 `retire_*`, Task 4 `retiring_*`).
4. **Сохранение, пока награда ещё создаётся** — ровно один `POST`; результат для удалённого триггера уходит в `retiring` (Task 4 `save_during_ensure_*`, `ready_for_deleted_trigger_retires`).
5. **Дубль названия/метки, недопустимое сочетание, смена вида, два новых триггера за раз** — валидация отклоняет; id не повторяются (Task 1 `validate_*`, Task 2 `assign_ids_*`).

---

## Фаза 1 — правила в ядре

### Task 1: `rules.rs` — модель, умолчания, валидация, миграция

**Files:** Create `src-tauri/src/rules.rs`; Modify `src-tauri/src/lib.rs` (`pub mod rules;`), `src-tauri/src/limits.rs` (`Limits`: добавить `Debug, PartialEq` в derive), `src-tauri/src/tts/mod.rs` (добавить `TtsCfg`, `TtsRuntime`; `TtsSettings` пока остаётся до Task 5)

**Interfaces — Produces:**
- `tts/mod.rs`: `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)] pub struct TtsCfg { pub profile_id: Option<i64>, pub lang: String, pub speed: f32, pub roles: filter::Roles, pub global_cooldown_s: u32, pub user_cooldown_s: u32, pub strip_links: bool, pub stop_words: Vec<String> }`, `Default` = значения текущего `TtsSettings::default()` (`"ru"`, `1.0`, роли все `true`, `10`, `60`, `true`, `[]`).
- `tts/mod.rs`: `#[derive(Clone, Debug, PartialEq)] pub struct TtsRuntime { pub cfg: TtsCfg, pub in_use: bool /* есть активный триггер озвучки */, pub chat_active: bool /* есть активная метка */ }`.
- `rules.rs`:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum ActionKind { Music, Tts }`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum TriggerTag { Reward, ChatMarker }`; `pub fn allowed(a: ActionKind) -> &'static [TriggerTag]`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(tag = "kind", rename_all = "snake_case")] pub enum TriggerKind { Reward { title: String, cost: u32, cooldown_s: u32, #[serde(default)] reward_id: Option<String> }, ChatMarker { marker: String } }` + `pub fn tag(&self) -> TriggerTag`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Trigger { #[serde(default)] pub id: String, pub enabled: bool, #[serde(flatten)] pub kind: TriggerKind }`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Action<C> { pub enabled: bool, pub cfg: C, pub triggers: Vec<Trigger> }`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)] pub struct MusicCfg { pub limits: crate::limits::Limits }`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Retiring { pub action: ActionKind, pub reward_id: String }`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Rules { pub music: Action<MusicCfg>, pub tts: Action<TtsCfg>, #[serde(default)] pub retiring: Vec<Retiring>, #[serde(default = "one")] pub next_id: u32 }`. `Default`: music `enabled:true`, `{id:"music-reward", enabled:true, Reward{"Заказать трек",500,0,None}}`; tts `enabled:true`, `{id:"tts-reward", enabled:false, Reward{"Озвучить текст",300,0,None}}`, `{id:"tts-chat", enabled:false, ChatMarker{"%"}}`; `retiring: []`, `next_id: 1`.
  - `impl Rules`: `pub fn triggers(&self) -> Vec<(ActionKind, &Trigger)>` (music, затем tts, порядок списка); `pub fn trigger(&self, id: &str) -> Option<(ActionKind, &Trigger)>`; `pub fn is_active(&self, a: ActionKind, t: &Trigger) -> bool`; `pub fn reward_ids(&self) -> Vec<String>` (все `reward_id` триггеров + `retiring`); `pub fn set_reward_id(&mut self, trigger_id: &str, id: Option<String>) -> bool` (false, если триггера нет или он не `Reward`); `pub fn tts_runtime(&self) -> TtsRuntime`.
  - `pub fn validate(r: &Rules) -> Result<(), String>` — ошибка всегда `"settings_invalid"`; правила Global Constraints (сочетания, ≤ 10, пределы, уникальность названий и меток).
  - `pub fn from_legacy(v: &serde_json::Value) -> Rules` — по таблице спеки «Миграция». Каждое поле читается **отдельно** из `Value` (`v["tts"]["reward_id"].as_str()` и т. п.; неверный тип или отсутствие → умолчание только для этого поля). `limits` — `from_value`, при ошибке — умолчание. Значения приводятся к пределам: название — обрезка до 45 символов, пустое → умолчание; цена — `max(1)`; кулдаун — `min(604800)`; метка вне правил → `"%"`; `speed` → clamp 0.7–1.5; кулдауны чата → `min(3600)`; если названия музыки и озвучки совпали после `trim().to_lowercase()` — к названию озвучки добавляется `" (озвучка)"` с обрезкой до 45. Id триггеров: `music-reward`, `tts-reward`, `tts-chat`; `next_id: 1`.

- [ ] **Step 1: Тесты** (`mod tests` в `rules.rs`):
  - `defaults_shape` — `Rules::default()` как выше; `serde_json::to_value` наградного триггера == `{"id":"music-reward","enabled":true,"kind":"reward","title":"Заказать трек","cost":500,"cooldown_s":0,"reward_id":null}`, метки — `{"id":"tts-chat","enabled":false,"kind":"chat_marker","marker":"%"}`; round-trip `from_value(to_value(x)) == x`; JSON без `retiring`/`next_id` читается (`[]`, `1`).
  - `migrate_legacy_full` — старый JSON с `music_reward_id:"m1"`, `reward_title:"Трек"`, `reward_cost:700`, `reward_cooldown_s:30`, `limits.no_repeat_last_n:5`, `tts:{reward_enabled:true, chat_enabled:false, reward_id:"t1", reward_title:"Скажи", reward_cost:100, marker:"!!", speed:1.2, stop_words:["x"]}` → музыка `Reward{"Трек",700,30,Some("m1")}` enabled, `limits.no_repeat_last_n==5`; tts-reward `enabled:true`, `Some("t1")`; tts-chat `enabled:false`, `"!!"`; `cfg.speed==1.2`, `cfg.stop_words==["x"]`; оба действия `enabled:true`.
  - `migrate_legacy_empty` — `from_legacy(&json!({}))` == `Rules::default()`.
  - `migrate_legacy_lenient` — `tts.speed:"fast"` (неверный тип) не сбрасывает `tts.reward_id:"t1"`; `reward_title` из 60 символов → 45; `reward_cooldown_s: 999999` → `604800`; совпавшие названия разведены; результат проходит `validate`.
  - `validate_ok_default`; `validate_rejects` — по одному нарушению: метка у музыки; 11 триггеров; название `""`/`"   "`/46 × `"я"` (45 × `"я"` — ок); цена 0; кулдаун 604801; дубль названия `"Заказать трек"` vs `" заказать ТРЕК "`; две метки `"%"`; метка `""`, `"abcd"`, `"a b"`; `speed` 0.69 и 1.51; `global_cooldown_s` 3601; 201 стоп-слово; стоп-слово 51 × `"я"`; `lang:"xx"`.
  - `is_active_matrix` — 4 сочетания `trigger.enabled × action.enabled`.
  - `tts_runtime_flags` — `in_use`/`chat_active`: всё выкл; только награда; только метка; действие выкл при включённых триггерах (оба `false`).
  - `reward_ids_include_retiring`.
- [ ] **Step 2:** `cargo test --lib rules` → FAIL.
- [ ] **Step 3:** Реализовать.
- [ ] **Step 4:** `cargo test --lib rules` → PASS; `cargo test` → всё зелёное.
- [ ] **Step 5:** Commit `feat(rules): model, defaults, validation, legacy migration`.

### Task 2: `rules.rs` — маршрутизация, решения и операции над наградами

**Files:** Modify `src-tauri/src/rules.rs`

**Interfaces — Consumes:** Task 1; `crate::tts::filter::{extract_marked, strip_emotes}`; `crate::model::ChatMessage`.
**Produces:**
- `#[derive(Clone, Debug, PartialEq)] pub struct RewardRoute { pub action: ActionKind, pub trigger_id: Option<String> /* None для retiring */, pub active: bool }`; `pub fn route_reward(r: &Rules, reward_id: &str) -> Option<RewardRoute>` — наградный триггер (активный или нет) → его маршрут; id в `retiring` → `{action, trigger_id: None, active: false}`; иначе `None`.
- `#[derive(Clone, Copy, Debug, PartialEq)] pub enum MusicDecision { Ignore, Refund, Process }`; `pub fn music_decision(r: &Rules, reward_id: &str) -> MusicDecision` — нет маршрута или маршрут не к музыке → `Ignore`; музыка и `!active` → `Refund`; иначе `Process`.
- `#[derive(Clone, Debug, PartialEq)] pub struct ChatRoute { pub action: ActionKind, pub trigger_id: String, pub text: String }`; `pub fn route_chat(r: &Rules, m: &ChatMessage) -> Option<ChatRoute>` — `None`, если `source_broadcaster_id` есть и ≠ `broadcaster_id`; текст = `strip_emotes(&m.fragments)`; первая **активная** метка (порядок `triggers()`), для которой `extract_marked` → `Some`.
- `pub fn reconcile_targets(r: &Rules) -> Vec<(ActionKind, String)>` — все наградные триггеры с `reward_id` (не `retiring`, без id — пропуск).
- `pub fn reward_body(title: &str, cost: u32, cooldown_s: u32, enabled: bool) -> serde_json::Value` — ровно текущий `TtsSettings::reward_body`: `{"title","cost","is_enabled","is_paused":false,"global_cooldown_setting":{"is_enabled":cooldown_s>0,"global_cooldown_seconds":cooldown_s}}`.
- `pub fn reward_prompt(a: ActionKind) -> &'static str` — Music `"Название или ссылка на трек"`, Tts `"Текст для озвучки"`.
- `#[derive(Clone, Debug, PartialEq)] pub enum RewardOp { Ensure { action: ActionKind, trigger_id: String, known_id: Option<String>, title: String, cost: u32, create: bool }, Patch { reward_id: String, body: serde_json::Value }, Retire { action: ActionKind, reward_id: String } }`
- `pub fn connect_ops(r: &Rules) -> Vec<RewardOp>` — для каждого наградного триггера: есть `reward_id` → `Ensure{known_id:Some, create: is_active}`; нет id и активен → `Ensure{known_id:None, create:true}`; нет id и неактивен → ничего; плюс `Retire` для каждого `retiring`.
- `pub fn plan_save(current: &Rules, incoming: Rules) -> (Rules, Vec<RewardOp>)` — объединяет назначение id и расчёт операций:
  1. **Id.** Для каждого триггера incoming: id совпадает с триггером current **того же действия и того же `TriggerTag`** и ещё не встречался в этом проходе → `reward_id` из current (incoming игнорируется); иначе — новый id `format!("t{}", next_id)`, `next_id += 1`, `reward_id = None`. `retiring` и `next_id` берутся из current (из панели игнорируются).
  2. **Удалённые.** Триггер current с `reward_id`, чьего id нет в результате → `Retiring{action, reward_id}` в `retiring` + `RewardOp::Retire`.
  3. **Остальные** (сопоставление по id): с `reward_id` и изменились `(title,cost,cooldown_s,active)` → `Patch{ body: if was_active && !now_active {{"is_enabled":false}} else reward_body(.., now_active) }`; без id, активен и (раньше не был активен, или новый, или изменились title/cost/cooldown) → `Ensure{known_id:None, create:true}`.

- [ ] **Step 1: Тесты:**
  - `route_reward_cases` — id музыки → `{Music, Some("music-reward"), true}`; выключенная награда озвучки → `active:false`; id из `retiring` → `{.., None, false}`; неизвестный → `None`.
  - `music_decision_cases` — `Process`; действие выключено → `Refund`; триггер выключен → `Refund`; retiring музыки → `Refund`; награда озвучки → `Ignore`; неизвестный → `Ignore`.
  - `route_chat_cases` — активная `%`: `"привет %"` → `"привет"`; `"на 100% согласен"` → `None`; метка выключена → `None`; действие выключено → `None`; чужой `source_broadcaster_id` → `None`; эмоут вырезан до поиска метки; метки `"%"` и `"!!"` — срабатывает та, что в тексте.
  - `reconcile_targets_cases` — включены все наградные триггеры с `reward_id` (музыка и озвучка, в т. ч. выключенные), исключены `retiring` и триггеры без id, метки не входят.
  - `reward_body_shape` — побайтово как существующий тест `tts_reward_body`.
  - `connect_ops_lazy` — `Rules::default()` → один `Ensure{"music-reward", None, create:true}`; выключенная tts-reward с id → `Ensure{Some, create:false}`; `retiring` → `Retire`.
  - `connect_ops_after_migration_no_create` — `from_legacy(full)` (оба id) → только `Ensure{known_id:Some(..)}`, нет `known_id:None`.
  - `plan_save_ops` — без изменений → `[]`; смена цены у награды с id → `Patch(reward_body(..,true))`; выключение триггера с id → `Patch({"is_enabled":false})`; выключение действия → `Patch` для всех его наград с id; включение без id → `Ensure`; смена названия у активной без id → `Ensure`; удаление триггера с id → `Retire` + запись в `retiring`; удаление без id → `[]`.
  - `assign_ids_keeps_owned_reward_id` — incoming подменил `reward_id` → из current.
  - `assign_ids_two_new_triggers_distinct` — два пустых id → `t{n}`, `t{n+1}`, `next_id` вырос на 2.
  - `assign_ids_unknown_id_is_new` — incoming id, которого нет в current (`"t99"`, ≥ `next_id`) → новый id, `reward_id: None`; current `t4` с id отсутствует в incoming → `Retire`. Удалённый id никогда не выдаётся повторно (персистентный `next_id`).
  - `assign_ids_kind_change_is_new` — `tts-chat` пришёл как `Reward` → новый id; нет `Patch` по старому id.
  - `assign_ids_duplicate_incoming` — два триггера с одним известным id → второй получает новый id и `reward_id: None`.
- [ ] **Step 2–4:** FAIL → реализовать → `cargo test --lib rules` PASS.
- [ ] **Step 5:** Commit `feat(rules): routing, decisions and reward operations`.

### Task 3: исполнители операций над наградами (Helix) и движок озвучки на `TtsRuntime`

**Files:** Modify `src-tauri/src/twitch/helix.rs`, `src-tauri/src/twitch/tests.rs`, `src-tauri/src/tts/engine.rs`, `src-tauri/src/tts/queue.rs`; Create `src-tauri/src/rewards.rs` (только async-исполнители этой задачи); Modify `src-tauri/src/lib.rs` (`pub mod rewards;`), `src-tauri/src/driver.rs` и `src-tauri/src/app.rs` (только временный адаптер, Step 3)

**Interfaces — Consumes:** Task 1–2.
**Produces:**
- `Helix::find_manageable_reward(&self, id: Option<&str>, title: &str, exclude: &[String]) -> Result<Option<String>>` — как сейчас, но строки с `id` из `exclude` пропускаются (и при поиске по id, и по названию). Существующие вызовы передают `&[]`.
- `Helix::disable_reward(&self, id: &str) -> Result<bool>` — `PATCH {"is_enabled":false}`; 404 → `Ok(false)` (награды нет), успех → `Ok(true)`. `update_reward` **не меняется**.
- `Helix::delete_reward(&self, id: &str) -> Result<()>` — `DELETE /channel_points/custom_rewards?broadcaster_id&id`; 404 → `Ok`.
- `rewards.rs`:
  - `pub async fn ensure_reward(h: &Helix, action: ActionKind, known_id: Option<&str>, title: &str, cost: u32, create: bool, exclude: &[String]) -> Result<Option<String>>` — `find_manageable_reward(known_id, title, exclude)`; найдено → `Some(id)`; не найдено и `create` → `create_reward(title, cost, reward_prompt(action))` (тексты ошибок `RewardError` — как сейчас в `driver.rs`); иначе `None`.
  - `pub async fn retire_reward(h: &Helix, id: &str, keep: &HashSet<String>) -> Result<()>` — `disable_reward(id)` (`Ok(false)` → награды уже нет, вернуть `Ok(())`) → `unfulfilled(id)` → для каждой строки, чей `redemption_id` ∉ `keep`, `set_redemption_status(&r.r, false).await?` последовательно → `delete_reward(id)`. Любая ошибка → `Err` (награда уже выключена первым шагом).
- `TtsMsg`: `Settings(TtsRuntime)`, `Reward { r: Redemption, active: bool }`, `Chat { msg: ChatMessage, text: String }`, новое `ChatSeen` (сообщение чата без метки — только сбрасывает `tts_chat_failed`), новое `DropReward { reward_id: String, reply: oneshot::Sender<Option<String>> }` (ответ — `redemption_id` текущего элемента этой награды, если он есть); остальные варианты без изменений. `engine::run(deps, settings: TtsRuntime, rx)`.
- `Queue::remove_pending(&mut self, pred: impl Fn(&Item) -> bool) -> Vec<Item>` — удаляет совпавшие из ожидающих (не текущий).

Поведение движка: `Chat` — маркер и источник проверены маршрутизатором; игнор при `!chat_active`; сбрасывает `tts_chat_failed`; дальше как сейчас (стример обходит роли/кулдауны, `clean`, голос). `ChatSeen` — только сброс `tts_chat_failed`. `Reward` — `!active` → `Outcome::Disabled`, дальше как сейчас. `tts_no_voice` при `profile_id.is_none() && in_use`; `chat_warning` показывается при `chat_active`. `DropReward` → `remove_pending` элементов `Origin::Reward(r)` с `r.reward_id == reward_id` → `report(.., Outcome::Cleared)`; ответ — id текущего, если он этой награды.

- [ ] **Step 1: Тесты:**
  - `twitch/tests.rs` (wiremock): `delete_reward_ok_and_404`; `find_reward_excludes` — строка из `exclude` не находится ни по id, ни по названию; `ensure_known_found_no_post` — найдено по id → `Some`, ни одного `POST`; `ensure_missing_no_create` — не найдено, `create:false` → `None`, нет `POST`; `ensure_creates_when_missing` — `POST` с `prompt` по действию; `retire_order` — порядок в `received_requests()`: `PATCH is_enabled:false` → `GET` выкупов → `PATCH status=CANCELED` (по одному на строку не из `keep`; строка из `keep` не трогается) → `DELETE`; `retire_delete_failure_errs` — `DELETE` 500 → `Err`, отключение уже отправлено; `retire_missing_reward_ok` — `PATCH` отключения 404 → `Ok(())`, больше запросов нет.
  - `engine.rs`: адаптировать существующие тесты к новым сообщениям, смысл тот же; `shared_chat_ignored` удаляется (покрыт `route_chat_cases`), `chat_disabled_ignored` становится `chat_ignored_when_chat_inactive`; `chat_relogin_warning_only_when_enabled` сбрасывает предупреждение через `ChatSeen`; новые `drop_reward_cancels_pending_only` (текущий элемент этой награды доигрывает и возвращается в `reply`; ожидающие этой награды → `CANCELED`; чужие и чатовые остаются), `inactive_reward_disabled`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Реализовать. Временный адаптер до Task 5, помечен `// TODO(Task 5)`: драйвер и `app.rs` строят `TtsRuntime` через `rules::from_legacy(&serde_json::to_value(&settings)?).tts_runtime()`; `Reward{active: settings.tts.reward_enabled}`; чат — `route_chat` на `from_legacy(..)`, без маршрута → `ChatSeen`.
- [ ] **Step 4:** `cargo build --examples && cargo test` → PASS.
- [ ] **Step 5:** Commit `feat(tts,twitch): engine on TtsRuntime, reward ensure/retire executors`.

### Task 4: `rewards.rs` — учёт жизненного цикла (чистый)

**Files:** Modify `src-tauri/src/rewards.rs`

**Interfaces — Consumes:** `Rules`, `RewardOp`, `plan_save`, `connect_ops` (Task 1–2).
**Produces:** `#[derive(Default)] pub struct Lifecycle { ensuring: HashMap<String /* trigger_id */, bool /* dirty */>, retiring_now: HashSet<String>, pub errors: BTreeMap<String, String> }` с методами (все синхронные, без сети; возвращают операции, которые драйвер исполнит):
- `pub fn on_connect(&mut self, rules: &Rules) -> Vec<RewardOp>` — `connect_ops`, отфильтрованные через `admit`.
- `pub fn on_save(&mut self, current: &Rules, incoming: Rules) -> (Rules, Vec<RewardOp>)` — `plan_save`, затем `admit`; ошибки удалённых триггеров убрать из `errors`.
- `fn admit(&mut self, ops) -> Vec<RewardOp>` — `Ensure` для триггера, уже в `ensuring` → не выпускать, пометить `dirty = true`; иначе занести в `ensuring`. `Retire` для id в `retiring_now` → не выпускать; иначе занести.
- `pub fn release_ensure(&mut self, trigger_id: &str)` и `pub fn release_retire(&mut self, reward_id: &str)` — снять операцию из учёта «в полёте», если она не была запущена (нет подключения к Twitch).
- `pub fn exclude(&self, rules: &Rules) -> Vec<String>` — id из `rules.retiring` (передаётся в `ensure_reward`).
- `pub fn on_ready(&mut self, rules: &mut Rules, trigger_id: &str, known_id: Option<String>, result: Result<Option<String>, String>) -> (bool /* rules changed */, Vec<RewardOp>)`:
  - снять `ensuring[trigger_id]`, запомнить `dirty`;
  - `Ok(Some(id))` и `id` в `rules.retiring` → как `Ok(None)` (не привязывать; если триггер активен — новый `Ensure{create:true}`).
  - `Ok(Some(id))`: если триггер существует, это `Reward`, а его `reward_id` `None` или == `known_id` → `set_reward_id(Some(id))`, убрать ошибку, выпустить `Patch(reward_body(текущие параметры, is_active))`. Иначе, если `id` не принадлежит никакому триггеру и не в `retiring` → добавить `Retiring{action, id}` и выпустить `Retire` (сирота).
  - `Ok(None)`: только если `reward_id == known_id` → `set_reward_id(None)`; если триггер теперь активен → выпустить `Ensure{create:true}`.
  - `Err(e)`: если у триггера `reward_id` уже есть → игнор; иначе `errors[trigger_id] = e`.
  - если был `dirty` и триггер существует → пересчитать для него: есть id → `Patch(reward_body(.., is_active))`; нет id и активен → `Ensure{create:true}`.
  - всё выпускаемое — через `admit`.
- `pub fn on_retired(&mut self, rules: &mut Rules, reward_id: &str, result: Result<(), String>) -> bool` — снять из `retiring_now`; `Ok` → убрать из `rules.retiring` (true), убрать `errors["retire:<id>"]`; `Err(e)` → `errors["retire:<id>"] = e`, оставить в `retiring` (false).

- [ ] **Step 1: Тесты** (`mod tests` в `rewards.rs`):
  - `save_during_ensure_single_ensure` — `on_connect` выпустил `Ensure` для `music-reward`; `on_save` с новым названием → `Ensure` не выпущен; `on_ready(Ok(Some("r1")))` → id сохранён, выпущен ровно один `Patch` с **новым** названием, ни одного второго `Ensure`.
  - `ready_for_deleted_trigger_retires` — триггер удалён во время `Ensure`; `on_ready(Ok(Some("r9")))` → `"r9"` в `rules.retiring`, выпущен `Retire`.
  - `ready_none_keeps_foreign_id` — `reward_id` уже `"r2"`, `on_ready(known_id: Some("r1"), Ok(None))` → `"r2"` не тронут.
  - `ready_none_then_active_recreates` — `Ensure{create:false}` в полёте, триггер включили (dirty), `Ok(None)` → id сброшен, выпущен `Ensure{create:true}`.
  - `ready_err_sets_error_unless_id` — ошибка → в `errors`; если id уже есть → игнор.
  - `retiring_survives_failure` — `on_retired(Err)` → остаётся в `retiring`, ошибка `retire:<id>`; повторный `on_connect` выпускает `Retire`; `on_retired(Ok)` → убран.
  - `retire_not_duplicated_in_flight` — два `on_connect` подряд → один `Retire`.
  - `offline_release_then_connect_reissues` — `on_save` выпустил `Ensure` и `Retire`; `release_ensure`/`release_retire`; `on_connect` выпускает их снова; следующий `on_save` с изменением не помечается как `dirty`.
  - `ready_with_retiring_id_not_assigned` — `Ok(Some(id))`, где `id` в `retiring` → не привязан; активный триггер → новый `Ensure`.
- [ ] **Step 2–4:** FAIL → реализовать → `cargo test --lib rewards` PASS.
- [ ] **Step 5:** Commit `feat(rewards): reward lifecycle bookkeeping`.

### Task 5: `Settings.rules` — склейка в драйвере и приложении

**Files:** Modify `src-tauri/src/driver.rs`, `src-tauri/src/app.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/tts/mod.rs` (удалить `TtsSettings`, `tts_reward_patch` и их тесты — покрыты Task 1–2), `src-tauri/tests/core.rs`; Delete `src-tauri/src/router.rs`

**Interfaces — Consumes:** Task 1–4.
**Produces:**
- `driver::Settings { #[serde(default)] pub rules: Rules, pub port, pub active_theme, pub youtube_api_key, pub obs, pub spotify_client_id }` (без `reward_*`, `music_reward_id`, `limits`, `tts`).
- `driver::Settings` получает `#[serde(default)]` на структуре (как `TtsSettings`), чтобы отсутствие любого общего поля в старом JSON не ломало старт.
- `pub fn load_settings(db: &Db) -> Result<(Settings, bool /* migrated */)>` (в `driver.rs`): нет записи → `(default, false)`; есть ключ `"rules"` → `from_value`; иначе общие поля через `from_value` (serde-default) + `rules = from_legacy(&v)`, `migrated = true`. Используется во **всех** местах чтения `"settings"` (bootstrap, `exit`, `get_settings`, тесты). Bootstrap при `migrated`: `set_setting("settings_legacy", &raw)`, затем `set_setting("settings", &new)`.
- `app::validate_settings`: `port`, `obs.port` как сейчас + `rules::validate(&s.rules)`.
- `apply_settings(...) -> Result<(Settings /* old */, Vec<RewardOp>)>`: `let (rules, ops) = lifecycle.on_save(&current.rules, incoming.rules)`; сохранить; вернуть ops. В `run` после успешного `apply_settings`, по порядку: `TtsMsg::Settings(ctx.settings.rules.tts_runtime())`; `player.step(Input::SetMaxDuration(rules.music.cfg.limits.max_duration_ms))` + `actions`; исполнить ops (у `run` есть `player`, `polls`, `inflight`).
- `DriverMsg::SaveSettings` отвечает **сохранёнными** настройками: `oneshot::Sender<Result<Settings, String>>` (с назначенными id, `reward_id`, `retiring`, `next_id`); команда `app::save_settings` возвращает `Settings`. Остальные отправители `SaveSettings` (`spotify_login`) игнорируют значение.
- `async`-склейка в `run`: `fn exec(op, ctx, player, polls, inflight, lifecycle, tx)`:
  - `Ensure` → если `helix` нет — `lifecycle.release_ensure(trigger_id)`; иначе `tokio::spawn` `bounded(ensure_reward(.., lifecycle.exclude(&rules)))` → `Internal::RewardReady { trigger_id, known_id, result: Result<Option<String>, String> }`.
  - `Patch` → `background(update_reward)` при наличии `helix`.
  - `Retire{Music, id}` → для элементов `player.snapshot().1` (очередь, не текущий) с `redemption.reward_id == id` → `player.step(Input::Remove(item.id))` + `actions`; `keep` = `redemption_id` текущего элемента музыки, если он этой награды. `Retire{Tts, id}` → `TtsMsg::DropReward{reward_id:id, reply}`; `keep` из `reply` (движок остановлен или `reply` закрыт → `keep = ∅`, без ошибки и ожидания). Затем, если `helix` есть, — `tokio::spawn`: дождаться `reply` (для Tts), `timeout(30 s, retire_reward(h, id, &keep))` → `Internal::Retired { reward_id, result }`; если `helix` нет — `lifecycle.release_retire(id)` (повтор при подключении).
  - `RewardReady` → `lifecycle.on_ready` → при изменении сохранить настройки, `TtsMsg::Settings(rules.tts_runtime())`, если `Ok(Some(id))` применён — reconcile этой награды; исполнить выпущенные ops; опубликовать ошибки.
  - `Retired` → `lifecycle.on_retired` → сохранить, опубликовать ошибки.
- Публикация ошибок: при изменении `lifecycle.errors` — событие Tauri `"rule_errors"` (карта); `DriverMsg::RuleErrors(oneshot::Sender<BTreeMap<String,String>>)` и команда `app::rule_errors` (регистрация в `lib.rs`). Ошибки наград больше не пишутся в `WidgetState.warning` и `TtsMsg::Warning`.
- `redeem`: `music_decision`/`route_reward`: Tts → `TtsMsg::Reward{r, active}`; Music `Refund` → если `redemption_state == Unknown` и id не в `inflight` → `mark_cancelled` + `status(false)`; `Process` → как сейчас; `Ignore` → игнор.
- `ResolveDone`: перед `music::decide` — `music_decision(&rules, &red.r.reward_id)`; не `Process` → `mark_cancelled` + `status(false)`, `inflight.remove`, без постановки в очередь.
- Чат: `route_chat` → `Some(Tts)` → `TtsMsg::Chat{msg, text}`; иначе → `TtsMsg::ChatSeen`.
- Reconcile/`Resync`: `for (action, id) in reconcile_targets(&rules)` — Music → существующий `Internal::Reconcile`, Tts → `Internal::TtsReconcile`.
- `connect_twitch`: `for op in lifecycle.on_connect(&rules) { exec(..) }`.
- `TwitchLogout` и `app::exit`: пауза для `rules.reward_ids()`.
- Удалить: временный адаптер Task 3, `router.rs`, `keep_reward_id`, `tts_target`, `spawn_tts_reward`, `tts_settings_changed`, `Internal::Reward/TtsReward`, `DriverMsg::SettingsChanged` (отправителя нет, валидацию обходит).

- [ ] **Step 1: Тесты** (`driver.rs` `mod tests` — только функции без `AppHandle`):
  - `load_settings_migrates_legacy` — старый JSON в БД → ожидаемые `rules`, `migrated:true`; `load_settings_new_form` — round-trip, `migrated:false`.
  - `bootstrap_resaves_new_form` — функция миграции на старте (вынести в `fn migrate_stored(db: &Db) -> Result<Settings>`): после вызова в БД `settings` в новой форме, `settings_legacy` — исходный JSON.
  - `apply_settings_keeps_reward_id_and_updates_reward` и `settings_change_keeps_*` — переписать на `rules`: подмена `reward_id` из панели игнорируется; смена цены → `Patch` в возвращённых ops.
  - `app.rs`: `validate_rejects_bad_tts` → на `rules` (пределы те же).
  - Поведение `redeem`/`ResolveDone`/чата покрыто чистыми тестами Task 2/4; здесь — `cargo test` целиком зелёный.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Реализовать.
- [ ] **Step 4:** `cargo build --examples && cargo test` → всё зелёное; `cargo clippy --all-targets` без новых предупреждений.
- [ ] **Step 5:** Ручная проверка оркестратором: копия реальной старой БД, `TWITCH_CLIENT_ID=… npm run tauri dev` — приложение стартует, `settings_legacy` записан, награды не задвоились.
- [ ] **Step 6:** Commit `feat(rules): settings.rules drives routing and reward lifecycle`.

### Task 6: панель на новой форме настроек (минимально), dev-моки, документация ядра

**Files:** Modify `src/lib/api.ts`, `src/lib/ttsApi.ts`, `src/lib/voiceApi.ts`, `src/lib/views/Rewards.svelte`, `src/lib/views/Limits.svelte`, `src/lib/views/Tts.svelte`, `src/lib/i18n/ru.ts`, `docs/architecture.md`; Create `src/lib/mock.ts`

**Interfaces — Produces (TS):**
```ts
export type TriggerKind = { kind: 'reward'; title: string; cost: number; cooldown_s: number; reward_id: string | null } | { kind: 'chat_marker'; marker: string };
export type Trigger = { id: string; enabled: boolean } & TriggerKind;
export interface Action<C> { enabled: boolean; cfg: C; triggers: Trigger[] }
export interface Rules { music: Action<{ limits: Limits }>; tts: Action<TtsCfg>; retiring: { action: 'music' | 'tts'; reward_id: string }[]; next_id: number }
export interface Settings { rules: Rules; port: number; active_theme: string; youtube_api_key: string | null; obs: { host: string; port: number; password: string | null }; spotify_client_id: string | null }
export const allowedTriggers = { music: ['reward'], tts: ['reward', 'chat_marker'] } as const;
export const rule_errors = () => call<Record<string, string>>('rule_errors');
export const onRuleErrors = (cb: (e: Record<string, string>) => void) => isTauri() ? listen<Record<string, string>>('rule_errors', e => cb(e.payload)) : Promise.resolve(() => {});
```
`TtsSettings` → `TtsCfg` в `ttsApi.ts`; `defaultSettings` повторяет `Rules::default()`.
- `save_settings(s): Promise<Settings>` — возвращает сохранённые настройки; **каждый** путь сохранения (`+page.svelte` `save()`, сохранение в `Connections.svelte`) делает `settings = result`. Перечитывать настройки по `rule_errors` **не нужно** (ядро всегда берёт `reward_id`/`retiring`/`next_id` у себя, а перечитывание стёрло бы несохранённую форму); `Connections.svelte` получает результат через колбэк или `$bindable`. Иначе новый триггер с `id: ''` получал бы новый id на каждом сохранении, а его награда удалялась и создавалась заново.
- Обёртки с браузерным фолбэком (`get_state`, `get_settings`, `running_port`, `spotify_volume`, `list_themes`): `isTauri() || mockEnabled ? call(..) : fallback`.
- `src/lib/mock.ts`: `export const mockEnabled = !isTauri() && import.meta.env.DEV;` `export function mockCall<T>(name: string, args?: Record<string, unknown>): Promise<T>` — фикстуры: `get_state` (текущий трек + 3 в очереди), `get_settings`/`save_settings` (в памяти; `save_settings` заменяет пустые id триггеров на `t${next_id++}` и возвращает результат, как ядро), `rule_errors` (`{}`; ошибка у `tts-reward` при `?mock=errors` в URL), `tts_state` (текущий + 2 в очереди), `vl_*`, нужные для отрисовки Voice Lab (модель установлена, 3 профиля, 2 источника, 3 рендера, пустые задания), остальные — `Promise.resolve(undefined)`. Функции `call` в `api.ts`, `ttsApi.ts`, `voiceApi.ts`: `isTauri() ? invoke(..) : mockEnabled ? mockCall(..) : Promise.reject(new Error('browser'))`. В production-сборке (`import.meta.env.DEV === false`) моки не используются.

- [ ] **Step 1:** Типы, `defaultSettings`, `mock.ts`, ветка `mockEnabled` в трёх `call`.
- [ ] **Step 2:** Вкладки только перепривязать: «Награды» → первый `reward`-триггер музыки; «Лимиты» → `rules.music.cfg.limits`; «Озвучка» → `rules.tts.cfg`, первый `reward`- и первый `chat_marker`-триггер (их `enabled`). Под каждой формой наград — строка ошибки `rule_errors[trigger.id]` (подписка `onRuleErrors`), иначе ошибки создания награды не видны до фазы 2. Других изменений UI нет.
- [ ] **Step 3:** `npm run check` → 0 ошибок; `node --test tests/` → PASS; `npm run build` проходит.
- [ ] **Step 4:** `docs/architecture.md`: таблица модулей (`rules.rs`, `rewards.rs` вместо `router.rs`); «Как добавить новую награду» → «Как добавить вид триггера или действие» (вариант `TriggerKind`/`ActionKind`, строка в `allowed`, ветка в `route_*`, исполнение в драйвере); жизненный цикл наград (`retiring`, `ensuring`, порядок retire), миграция и `settings_legacy`; известное ограничение: текущий элемент удаляемой награды (трек или фраза озвучки) остаётся `UNFULFILLED` до конца и становится FULFILLED при `DELETE`; если он затем не запустится/не синтезируется, возврата не будет.
- [ ] **Step 5:** Commit `feat(panel): settings.rules shape, dev mocks; docs: rules architecture`.

---

## Фаза 2 — редизайн

Общие правила фазы: навык `unslop` (`~/.claude/skills/unslop/SKILL.md` и `references/*.md`) — обязательное чтение для исполнителя каждой задачи 8–12; утверждённое направление `docs/superpowers/specs/2026-11-03-panel-direction.md` (Task 7) — закон. Логика компонентов и вызовы `api.ts`/`voiceApi.ts`/`ttsApi.ts` не меняются, кроме Task 9. Проверка экранов — `npm run dev` (`http://127.0.0.1:1420`, браузерный режим с моками Task 6); скриншоты на ширинах 1280 и 900 снимает оркестратор (Claude, `claude-in-chrome`) и прикладывает к ревью; ревьюер сверяет с направлением и `references/signs.md`. Финальный просмотр в `tauri dev` — Task 12.

### Task 7: аудит и направление — КОНТРОЛЬНАЯ ТОЧКА (исполняет основная сессия Claude)

**Files:** Create `docs/superpowers/specs/2026-11-03-panel-direction.md`, `static/fonts/*.woff2`, `static/fonts/LICENSE*`

- [x] **Step 1:** Скриншоты всех текущих экранов (`npm run dev` с моками): Сейчас, Награды, Лимиты, Виджет, Подключения, Voice Lab (все подвкладки), Озвучка.
- [x] **Step 2:** Аудит по `unslop` (каждый признак — где живёт).
- [x] **Step 3:** Направление по `unslop`: характер, мир, где живёт цвет (1–2 момента), сигнатурная деталь, нейтральная шкала, акцент, шрифт(ы), шкала отступов/радиусов/типографики, длительности и кривая анимации, иконки (inline SVG, без пакета), тёмная тема по умолчанию; набросок навигации «Сейчас · Правила · Виджет · Подключения · Voice Lab» и карточки действия (раскладка A) в визуальном компаньоне.
- [x] **Step 4:** Показать пользователю, итерировать до **явного** «утверждаю». Без этого задачи 8–12 не начинаются.
- [x] **Step 5:** Скачать выбранные шрифты (woff2 с кириллицей, OFL) и лицензию в `static/fonts/`.
- [x] **Step 6:** Commit `docs: panel design direction; fonts`.

### Task 8: токены, шрифты, оболочка и общие элементы

**Files:** Create `src/app.css` (токены `:root`, `@font-face` на `static/fonts`, сброс, базовые `button/input/select/textarea/label/.card/.muted` и т. п.); Create `src/routes/+layout.svelte` (`import '../app.css'` + `{@render children()}`; `+layout.ts` не трогать), `src/routes/+page.svelte` (навигация из 5 вкладок; `rewards`/`limits`/`tts` убираются из `tabs`, `rules` добавляется — временно рендерит `Rewards`/`Limits`/`Tts` друг под другом до Task 9), `src/lib/i18n/ru.ts`

- [ ] **Step 1:** Токены и база ровно по направлению; цвета/отступы/радиусы — только через переменные.
- [ ] **Step 2:** `npm run check` → 0; `grep -rnE '(color|background|border|fill|stroke)[^;]*#[0-9a-fA-F]{3,8}' src/lib src/routes` — нет совпадений, кроме подложки превью в `Widget.svelte`.
- [ ] **Step 3:** Скриншоты оболочки → ревью.
- [ ] **Step 4:** Commit `feat(panel): design tokens, fonts and shell`.

### Task 9: экран «Правила»

**Files:** Create `src/lib/views/rules/Rules.svelte`, `ActionCard.svelte`, `TriggerRow.svelte`, `TriggerEditor.svelte`, `MusicSettings.svelte` (содержимое `Limits.svelte`), `TtsSettings.svelte` (настройки из `Tts.svelte`) — все в `src/lib/views/rules/`; Delete `src/lib/views/Rewards.svelte`, `src/lib/views/Limits.svelte`; Modify `src/lib/views/Tts.svelte` (остаются очередь, URL виджета, «Добавить в OBS», тест), `src/routes/+page.svelte`, `src/lib/i18n/ru.ts`

**Interfaces — Consumes:** TS-типы, `allowedTriggers`, `rule_errors`/`onRuleErrors` (Task 6); `save(settings)` как сейчас.

Поведение:
- Карточка на действие: заголовок, переключатель действия, список триггеров, «+ триггер», раскрываемые «Настройки и лимиты». У «Озвучки» в карточке также блок очереди/виджета/теста (`Tts.svelte`) — если направление Task 7 не решило иначе.
- Строка триггера: вид, параметры (награда — название, цена, кулдаун; метка — символ), переключатель, «Изменить», «Удалить», ошибка `rule_errors[trigger.id]` у строки.
- Ошибки `retire:<reward_id>` — плашкой на карточке действия, указанного в `rules.retiring` для этого id: «Не удалось удалить награду на Twitch, повторим при следующем подключении».
- «+ триггер»: вид только из `allowedTriggers[action]`; затем поля; новый триггер с `id: ''`; при 10 триггерах кнопка неактивна.
- Удаление наградного триггера с `reward_id` — подтверждение **в интерфейсе** (не `confirm()`): «Награда будет удалена на Twitch, не начатые заказы вернут баллы».
- Подсказки дублей названия/метки и пределов на клиенте (HTML-атрибуты + проверка перед `save`); `settings_invalid` от ядра → общий текст ошибки.

- [ ] **Step 1:** Реализовать компоненты.
- [ ] **Step 2:** `npm run check` → 0.
- [ ] **Step 3:** Сценарий в браузере с моками (оркестратор): добавить метку к озвучке, изменить цену награды, выключить действие, удалить триггер (подтверждение), дубль метки — подсказка, `?mock=errors` — ошибка у строки; добавить триггер и сохранить дважды — id стабилен, в `retiring` ничего не появилось; скриншоты → ревью.
- [ ] **Step 4:** Commit `feat(panel): rules screen`.

### Task 10: «Сейчас», «Виджет», «Подключения»

**Files:** Modify `src/lib/views/NowQueue.svelte`, `src/lib/views/Widget.svelte`, `src/lib/views/Connections.svelte`, `src/lib/i18n/ru.ts` (только тексты по проходу copy)

- [ ] **Step 1:** Проходы `unslop` (spacing → … → motion) в рамках направления; поведение и вызовы API не меняются.
- [ ] **Step 2:** `npm run check` → 0; скриншоты (моки) → ревью.
- [ ] **Step 3:** Commit `feat(panel): redesign now, widget, connections`.

### Task 11: Voice Lab

**Files:** Modify `src/lib/views/voice/*.svelte`, `src/lib/i18n/ru.ts`

- [ ] **Step 1:** Проходы `unslop` по `VoiceLab`, `Setup`, `Sources`, `Voices`, `Mixer`, `Train`, `Renders`, `Explore`; логика, события, `canvas`/`audio` — без изменений.
- [ ] **Step 2:** `npm run check` → 0; `node --test tests/` → PASS; скриншоты каждой подвкладки (моки) → ревью.
- [ ] **Step 3:** Commit `feat(panel): redesign voice lab`.

### Task 12: финальный проход и документация

**Files:** Modify любые `src/**/*.svelte`, `src/app.css` (только находки аудита), `docs/user-guide.md`, `docs/architecture.md`, `README.md` (если упоминает старые вкладки)

- [ ] **Step 1:** Повторный аудит всей панели по `references/signs.md`; исправить находки.
- [ ] **Step 2:** Клавиатура: Tab проходит все экраны, фокус видим; у каждого поля подпись; контраст текста ≥ 4.5:1 (пары токенов).
- [ ] **Step 3:** `docs/user-guide.md` — разделы про награды, лимиты и озвучку переписать под экран «Правила».
- [ ] **Step 4:** `npm run check`, `node --test tests/`, `npm run build`, `cd src-tauri && cargo test` → всё зелёное.
- [ ] **Step 5:** Пользователь смотрит панель в `TWITCH_CLIENT_ID=… npm run tauri dev` (реальные данные).
- [ ] **Step 6:** Commit `feat(panel): final polish; docs: user guide for rules`.
