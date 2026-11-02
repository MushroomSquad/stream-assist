# Voice Lab P2 — Experimental Voice Design Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Интерполяция A→B, случайные смеси с seed, сортировка по сходству с записью и перенос проекта `.svlab` — без изменения БД и формата рецепта.

**Architecture:** Веса считаются чистыми функциями в `mixer.rs`; пачка — новая операция P0-воркера `synthBatch` (модель и энкодер грузятся один раз), регистрация рендеров — после выхода воркера. Проект — ZIP, читаемый строго по манифесту.

**Tech Stack:** как P0 + `rand_chacha = "0.9"` (уже в `Cargo.lock` транзитивно), `zip 6` (есть).

**Spec:** `docs/superpowers/specs/2026-11-02-voice-lab-p2-design.md` (+ P0 spec)

**Предусловие:** P0 выполнен. От P1 не зависит.

## Global Constraints

- Ограничения P0 действуют целиком. Схема БД и `Recipe` (schemaVersion 1) не меняются.
- Пачка: 1–9 элементов; текст: `trim().chars().count()` вне 1..=300 → `BATCH_INVALID` (проверка до `validate_params`), остальные параметры — `validate_params` P0; интерполяция N 3–9, всегда `DpStrategy::Weighted`; случайные: K 2–8 от 2–8 профилей, seed `u32`, `DpStrategy::Select` → `BATCH_INVALID`.
- Таймаут пачки `300 + 60·N` с. Пачка занимает слот синтеза.
- Референс сходства: регион 1000–60000 мс, `consent` источника обязателен (`NO_CONSENT`), энкодер установлен (`ENCODER_MISSING`).
- `.svlab`: манифест ≤ 1 МБ, профиль ≤ 2 МБ, WAV ≤ 50 МБ, всего ≤ 500 МБ (по распакованным байтам), записей ≤ 2000; имена только `profiles/[0-9a-f]{64}\.json`, `renders/[0-9]{1,9}\.wav`.
- Новые коды в `ru.ts`: `BATCH_INVALID`, `ENCODER_MISSING`, `PROJECT_INVALID`, `PROJECT_TOO_LARGE`.
- UI P2 помечен «экспериментально».

## Review Focus

1. **Seed-воспроизводимость весов** (`dirichlet_is_seeded`, Task 1). Звук — нет, UI это говорит.
2. **Враждебный `.svlab`**: имя вне шаблона, zip-bomb (заголовок врёт о размере), подменённый профиль, `origin: builtin`, 3000 записей → `PROJECT_INVALID`/`PROJECT_TOO_LARGE`, ничего не применено (Task 4).
3. **Отмена пачки на середине** → готовые элементы — рендеры, остальные нет, слот свободен (Task 2).
4. **Импортированные избранные переживают `prune_renders`** (Task 4).
5. **Интерполяция без скачка длительностей на t=0.5** — DP всегда weighted (Task 3).

---

### Task 1: mixer — веса исследования

**Files:** Modify `src-tauri/src/voice_lab/mixer.rs`, `src-tauri/Cargo.toml` (`rand_chacha = "0.9"`)

**Interfaces — Produces:**
- `pub fn interp_weights(points: usize) -> Vec<[f64; 2]>` — `i/(N−1)`, `[100·(1−t), 100·t]`.
- `pub fn dirichlet_weights(n: usize, k: usize, seed: u32) -> Vec<Vec<f64>>` — `ChaCha8Rng::from_seed(key)`, где `key` = 32 байта: `seed.to_le_bytes()` и нули; порядок выборок: набор за набором, внутри — по профилям; `u = (rng.next_u64() >> 11) as f64 / 2^53`, `e = −ln(1 − u)`; веса `100·e/Σe` без округления.
- [ ] **Step 1: Тесты:** `interp_endpoints` (N=5 → [100,0], [75,25], …, [0,100]), `dirichlet_is_seeded` (равно при одном seed, иначе разные; эталон для `seed=7, n=3, k=1` зашит в тест после первой реализации), `dirichlet_sums_to_100` (допуск 1e-9, все > 0), `dirichlet_mixes_ok` (каждый набор проходит `mix`).
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(voice-lab): exploration weights`.

### Task 2: worker/runner — `synthBatch` и отмена

**Files:** Modify `worker.rs`, `runner.rs`, `examples/vl_fake_worker.rs`

**Interfaces — Produces:**
- `WorkerRequest::SynthBatch { model_dir, items: Vec<BatchItem>, params: RenderParams, encoder: Option<PathBuf>, reference_wav: Option<PathBuf> }`; `#[derive(Serialize, Deserialize, Clone, Debug)] #[serde(rename_all="camelCase")] pub struct BatchItem { pub style: PathBuf, pub out_wav: PathBuf }`.
- `WorkerEvent::Item { index: usize, duration_ms: i64, synth_ms: i64, similarity: Option<f32> }`.
- Воркер: модель грузится как для `Synth`; если `encoder` и `reference_wav` заданы — `SpeakerEncoder::on_device(encoder, Device::Cpu)`, `embed` референса один раз; для каждого элемента — синтез, `write_wav_file`, `embed` сэмплов в памяти, сходство = скалярное произведение эмбеддингов (они L2-нормированы; нефинитное → `None`), `Item`; в конце `Done { все None }`. `encoder` и `reference_wav` — оба `Some` или оба `None` (иначе `WORKER_FAILED`). В `execute` добавить `SynthBatch` в разбор `model_dir`; в `sanitize` ветка `Item` (нефинитное `similarity` → `None`). `embed(samples, sample_rate)`: для синтеза `tts.sample_rate as u32`, для референса — `Audio.sample_rate` из `features::load_wav`; сходство — `supertonic3_tts::speaker::cosine`; ошибка `embed` у элемента (короче ~100 мс) → `similarity = None`, пачка продолжается; ошибка `embed` референса → `WORKER_FAILED`.
- `runner.rs`: `JobGuard` различает слоты через `enum Slot { Clone, Synth, Batch }`; новое поле `synth_cancel: Mutex<Option<(u64, oneshot::Sender<()>)>>`, которое занимает только пачка; `cancel(id)` проверяет `clone_slot`, затем `synth_cancel`.
- `pub async fn batch(&self, store: &Arc<StdMutex<Store>>, root: &Path, items: Vec<(PathBuf /*style*/, Recipe)>, params: RenderParams, reference: Option<PathBuf /*готовый WAV*/>, encoder: Option<PathBuf>, emit: Emit) -> Result<BatchResult, VlError>` — `synth_busy` занят → `JOB_BUSY`; `JobState.kind = "batch"`, прогресс = `Item`; `on_event` только собирает `Item` (проверка `index < N`, без повторов — иначе `WORKER_CRASHED`); после `run_worker` (при `Ok`, `JOB_CANCELLED` или таймауте) для каждого собранного элемента — `add_render(None, Some(&recipe), &params, duration_ms, synth_ms)`, rename, при сбое `rollback_render`; вернуть `BatchResult { items: Vec<(Render, Option<f32>)>, cancelled: bool }`: при `JOB_CANCELLED` — `Ok` с частичным списком и `cancelled = true` (`JobState.status = "cancelled"`); при таймауте/падении воркера готовые всё равно регистрируются, затем возвращается ошибка. Стили передаются в `batch` путями в `tmp/`; первым действием `batch` (до проверки `synth_busy`) забирает их в `TempFiles` вместе со своими WAV — очистка на любом выходе на стороне `batch`. После регистрации — `prune_renders(200)`.
- `pub(super) fn cut_reference(store, root, source_id, start_ms, end_ms, min_ms, max_ms) -> Result<PathBuf, VlError>` — вынести вырезку из `start_clone` (consent, `validate_range`, `read_segment`, запись в уникальный `tmp/ref-<rand>.wav`; при ошибке файла не остаётся), `start_clone` сохраняет порядок проверок (consent, диапазон, `transcript_required`) до вызова `cut_reference`, сразу берёт файл в `TempFiles` и вызывает её до захвата `clone_slot` (тест `clone_slot_not_held_during_segment` остаётся зелёным). Вызывающий держит файл в `TempFiles`.
- Фейк: ветка `SynthBatch` в любом режиме, кроме `batch_slow`/`batch_dup`, ведёт себя как `batch_ok` (тесты Task 3 идут с `VL_FAKE_MODE=ok`); `batch_ok` (Item на каждый с тишиной, similarity = `0.1·index`, Done), `batch_slow` (Item раз в 1 с), `batch_dup` (два Item с одним индексом).
- [ ] **Step 1: Тесты:** `batch_registers_after_exit`, `batch_cancel_keeps_finished` (`batch_slow`, отмена после 2-го Item → `Ok`, `cancelled == true`, 2 рендера, слот свободен), `batch_busy_blocks_single_synth`, `batch_duplicate_index_crashes`, `batch_similarity_passed_through`; unit-тесты в `mod tests` файла `runner.rs` (функция `pub(super)`): `cut_reference_requires_consent`, `cut_reference_bad_region_leaves_no_file` (вне 1–60 с → `REGION_INVALID`, `tmp/` пуст).
- [ ] **Step 2–4:** FAIL → реализовать → PASS; `#[ignore]` `worker_batch_real` (3 элемента + энкодер; печать времени против 3 одиночных). **Step 5:** Commit `feat(voice-lab): batch synthesis with similarity`.

### Task 3: команда `vl_explore`

**Files:** Modify `commands.rs`, `src-tauri/src/lib.rs` (зарегистрировать `vl_explore` в `generate_handler!`)

**Interfaces — Produces:**
- `#[derive(Deserialize)] #[serde(tag="mode", rename_all="lowercase", rename_all_fields="camelCase")] pub enum Explore { Interp { a: i64, b: i64, points: usize }, Random { ids: Vec<i64>, k: usize, seed: u32, dp: DpStrategy } }`
- `#[derive(Deserialize)] #[serde(rename_all="camelCase")] pub struct RefRegion { pub source_id: i64, pub start_ms: u64, pub end_ms: u64 }`
- `#[derive(Serialize)] #[serde(rename_all="camelCase")] pub struct ExploreItem { pub render: Render, pub similarity: Option<f32> }` (веса — из `render.recipe`); `#[derive(Serialize)] #[serde(rename_all="camelCase")] pub struct ExploreResult { pub items: Vec<ExploreItem>, pub cancelled: bool }`
- `vl_explore(explore: Explore, params: RenderParams, reference: Option<RefRegion>) -> ExploreResult` — `require_model`; проверки из Global Constraints (`BATCH_INVALID`); веса из Task 1 → `build_mix` P0 для каждого (Interp — `Weighted`) → стили в `tmp/` → `cut_reference` при `reference` (энкодер = `encoder_path`; `models::status(root).encoder_installed == false` → `ENCODER_MISSING`; референс держится в `TempFiles` до конца команды) → `Runner::batch`. Порядок результата = порядок весов.
- [ ] **Step 1: Тесты** (фейк): `interp_returns_points_in_order`, `interp_uses_weighted_dp` (рецепт каждого рендера `dpStrategy: weighted`), `random_same_seed_same_weights`, `random_select_rejected`, `explore_text_over_300_rejected`, `explore_reference_without_encoder`, `explore_cancel_returns_partial`. `validate_params` сделать `pub(crate)`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(voice-lab): explore command`.

### Task 4: project.rs — `.svlab`

**Files:** Create `src-tauri/src/voice_lab/project.rs`; Modify `mod.rs`, `commands.rs`, `src-tauri/src/lib.rs` (зарегистрировать `vl_export_project`, `vl_import_project`)

**Interfaces — Produces:**
- Манифест: `{"kind":"voice-lab-project","schemaVersion":1,"assetRevision","createdAt","profiles":[{"file","sha256","name","tags","origin","recipe","meta"}],"renders":[{"file","text","lang","speed","quality","recipe","profileSha256"}]}`.
- `pub fn export_project(store: &Mutex<Store>, dest: &Path) -> Result<(), VlError>` — под коротким локом собрать список: все профили кроме `builtin` (`profiles()`), избранные рендеры (`renders(usize::MAX)` с фильтром `favorite`; `profileSha256` — sha профиля рендера, если есть) с путями файлов; превышение лимитов импорта (500 МБ, 2000 записей) → `PROJECT_TOO_LARGE`; ZIP писать без лока потоком в `File` `<dest>.tmp`, затем rename (исчезнувший файл → `EXPORT_FAILED`, `<dest>.tmp` удаляется). Проверку пути вынести из `export_bytes` P0 в `pub(super) fn check_export_path(path: &Path, ext: &str) -> Result<(), VlError>` в `commands.rs` (используется из `project.rs`) (абсолютный, родитель есть, расширение `svlab`).
- `#[derive(Serialize)] #[serde(rename_all="camelCase")] pub struct ImportReport { pub profiles: usize, pub renders: usize, pub skipped: usize, pub revision_mismatch: bool }`
- `pub fn import_project(store: &Mutex<Store>, src: &Path) -> Result<ImportReport, VlError>` — фаза проверки (без лока): размер файла ≤ 500 МБ (`PROJECT_TOO_LARGE`); `ZipArchive`, записей ≤ 2000; `manifest.json` через `by_name` + `take(1 МБ + 1)`; `kind == "voice-lab-project"` и `schemaVersion == 1`; `profiles` + `renders` ≤ 2000, все `file` уникальны; каждый `recipe` (профиля и рендера) проверяется: `schemaVersion == 1`, `kind == "voice-lab-recipe"`, 1–8 входов, веса конечны и в 0–100, иначе `PROJECT_INVALID`; каждый `file` по шаблону, читается `by_name` + `take(limit+1)`, суммарно ≤ 500 МБ; профиль (в памяти, ≤ 2 МБ): `parse_style`, `sha256(canonical_bytes)` = манифест; имя/теги по лимитам; рендер: параметры через `validate_params`, WAV сразу потоком в `tmp/import-<rand>/<i>.wav` (путь из индекса), проверка `hound` (mono, PCM16, частота 8000–48000, длительность из файла). Каталог `tmp/import-*` удаляется guard'ом при любом исходе. Любая ошибка → `PROJECT_INVALID`/`PROJECT_TOO_LARGE`, ничего не применено. Фаза применения: для каждого профиля `profile_by_sha(sha)` — есть → `skipped += 1`, ничего не трогать; иначе `put_profile(style, name, "import", recipe, meta = исходная meta без `sourceId` + `{"importedOrigin": origin}`; не-объект meta → `{"original": meta, …}`)` и `rename_profile` для тегов только у созданного; рендеры — под одним локом `add_render(profile_id по sha или None, recipe, …)`, rename из `tmp/import-*`, `set_favorite(id, true)`; сбой rename → `rollback_render`.
- Команды `vl_export_project(path)`, `vl_import_project(path) -> ImportReport`.
- [ ] **Step 1: Тесты:** `roundtrip_profiles_and_favorites` (+ после импорта и `prune_renders(0)` избранные на месте), `builtin_not_exported`, `bad_entry_name_rejected` (`../x`, `profiles/x.json`), `zip_bomb_rejected` (запись, распаковывающаяся больше лимита), `tampered_profile_rejected_nothing_applied`, `builtin_origin_imported_as_import`, `too_many_entries_rejected`, `duplicate_profiles_skipped` (имя и теги локального профиля, в т.ч. встроенного с тем же sha, не меняются), `wrong_kind_rejected`, `revision_mismatch_reported`.
- [ ] **Step 2–4:** FAIL → реализовать → PASS. **Step 5:** Commit `feat(voice-lab): project export/import`.

### Task 5: UI «Исследование» и проект

**Files:** Create `src/lib/views/voice/Explore.svelte`; Modify `VoiceLab.svelte`, `Voices.svelte`, `voiceApi.ts`, `ru.ts`

- [ ] **Step 1:** `Explore.svelte` («экспериментально»): режим «Интерполяция» (A, B, точек 3–9) или «Случайные смеси» (профили, K, seed с кнопкой «новый» = `Math.floor(Math.random()*2**32)`, DP без `select`); текст ≤ 300, язык, скорость, качество; необязательное «Сравнить с записью» (источник + регион 1–60 с); «Сгенерировать», прогресс пачки и «Отменить» (id задачи — из событий `vl-job` с `kind: "batch"`; при `ExploreResult.cancelled` — подпись «Отменено, показаны готовые»); лента карточек (подпись «t=0.25» / «#3 · seed 7», веса, сходство, плеер, «Сохранить снимком» → `vl_save_snapshot` с id профилей по `profileSha256` из рецепта; у крайних точек интерполяции кнопки нет — подпись «это профиль A/B»); сортировка по сходству. Подсказки: «t — вес смеси, а не доля пути»; «Одинаковый seed повторяет веса, но не звук».
- [ ] **Step 2:** `Voices.svelte`: «Экспорт проекта» (`save`, `.svlab`) и «Импорт проекта» (`open`) с отчётом (сколько профилей/рендеров, пропущено, предупреждение о другой ревизии модели).
- [ ] **Step 3:** `ru.ts`; `npm run check` — 0 ошибок. **Step 4:** Commit `feat(panel): voice lab exploration`.

### Task 6: Документация и приёмка

- [ ] **Step 1:** `docs/voice-lab.md` — раздел «Эксперименты»: что воспроизводимо, смысл `t`, сходство — подсказка, формат и ограничения `.svlab` (рендеры при повторном импорте дублируются).
- [ ] **Step 2:** Приёмка: `#[ignore]` `acceptance_p2` на реальной модели (`SUPERTONIC_MODEL_DIR`, энкодер): два профиля → `vl_explore` Interp 5 точек на русской фразе → 5 рендеров по порядку, у всех `dpStrategy: weighted`; снимок t=0.25 через `vl_save_snapshot` → новый профиль; Random K=4 от трёх профилей seed 7 дважды → одинаковые веса в рецептах; пачка с референсом → у всех `similarity` не `None`, список сортируется; экспорт `.svlab` → новый каталог данных с копией модели → импорт → профили и избранное на месте, синтез снимком даёт WAV. Ручная проверка UI: отмена пачки на середине оставляет готовые карточки с подписью, крайние точки без «Сохранить». **Step 3:** `cargo test`, `npm run check` зелёные. Commit `docs: voice lab experiments`.
