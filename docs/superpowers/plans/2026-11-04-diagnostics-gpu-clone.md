# Отладка, GPU-клон, прогресс тренажёра — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** лог-файл и `GET /debug` для отладки, клонирование голоса на NVIDIA GPU через скачиваемый пакет провайдера ORT, честный прогресс установки тренажёра.

**Architecture:** std-only логгер `diag.rs` (глобальный `OnceLock`), `/debug` на существующем axum с замыканием-снимком из `app.rs`. GPU: сборка с фичей `cuda` у `supertonic3-tts`, провайдер CUDA от Microsoft в `<app_data>/voice-lab/gpu/`, воркер запускается через симлинк в этом каталоге (ORT ищет провайдеры рядом с путём запуска). Прогресс `uv sync` — разбор `install.log` во время работы `uv`.

**Tech Stack:** Rust (Tauri 2, tokio, axum), SvelteKit/Svelte 5.

**Spec:** `docs/superpowers/specs/2026-11-04-diagnostics-gpu-clone.md`

## Global Constraints

- Ветка `feat/diag-gpu-progress`. Без push/merge.
- `ort` остаётся `=2.0.0-rc.13`; `supertonic3-tts` `=1.3.0`; без `cargo update`.
- Новых крейтов нет. Разрешено включить фичу `cuda` у `supertonic3-tts` (T5). Всё остальное — std и уже подключённые крейты (`chrono` и `flate2` есть в lock как транзитивные — не подключать напрямую, если можно без них).
- В лог и `/debug` не попадают токены, ключи, `client_secret`, коды OAuth, тексты сообщений чата.
- Сбой записи лога не роняет и не блокирует приложение.
- Тексты панели — только в `src/lib/i18n/ru.ts`.
- Музыкальная логика, озвучка и CPU-путь Voice Lab по поведению не меняются.
- Перед коммитом: `cd src-tauri && cargo build --examples && cargo test` (зелёные), `cargo clippy --all-targets` (без новых предупреждений); для задач с фронтендом ещё `npm run check` и `npm test`.

## Review Focus

1. Лог недоступен на запись (каталог только для чтения, диск полон) → приложение работает, строки идут в stderr. Тест в T1 `write_failure_is_silent`.
2. `/debug` при незапущенном движке TTS или зависшем драйвере → ответ ≤ ~2 с, части `null`. Тест в T3 `debug_parts_time_out_to_null`.
3. AppImage: путь `current_exe()` меняется на каждом запуске → симлинк пересоздаётся, а не переиспользуется устаревший. Тест в T6 `worker_link_replaces_stale_link`.
4. Пакет повреждён/обрезан (не тот SHA) → `pack: broken`, GPU недоступен, переустановка чинит. Тест в T5 `status_reports_broken_on_hash_mismatch`.
5. `install.log` в середине записи (обрезанная последняя строка) и пакеты с одинаковым префиксом имени → счётчик не скачет и не превышает total. Тест в T4 `uv_progress_ignores_partial_line`.

---

### Task 1: Логгер `diag`

**Files:**
- Create: `src-tauri/src/diag.rs`
- Modify: `src-tauri/src/lib.rs` (mod), `src-tauri/src/app.rs` (init в `setup`), все файлы с `eprintln!` (список ниже)

**Interfaces:**
- Produces:
  - `pub fn init(dir: &Path)` — один раз; создаёт `dir`, ставит хук паники. Повторный вызов — no-op.
  - `pub fn line(area: &str, msg: impl std::fmt::Display)` — пишет `<ts> <area> <msg>\n` в файл и stderr. До `init` — только stderr.
  - `pub fn log_path() -> Option<PathBuf>`
  - `pub fn tail(n: usize) -> Vec<String>` — последние `n` строк текущего файла (читает не больше 256 КБ с конца).
  - макрос `crate::dlog!(area, "fmt", args…)` → `diag::line(area, format_args!(…))`.
- Константы: `MAX_BYTES = 5 * 1024 * 1024`, файл `stream-assist.log`, ротация в `stream-assist.log.1`.

- [ ] **Step 1: тесты в `diag.rs` (модуль `tests`, функции принимают путь, глобальное состояние не трогают — вынести `struct Writer { path, max }` с `fn write(&mut self, line: &str)`)**

```rust
#[test] fn writes_line_with_area() { /* Writer::write("x tts hello") → файл содержит строку с "tts hello\n" */ }
#[test] fn rotates_over_limit() { /* max = 100; две записи по 80 байт → есть .log.1 с первой, .log со второй */ }
#[test] fn rotation_replaces_old_backup() { /* три записи по 80 → .log.1 содержит вторую, не первую */ }
#[test] fn write_failure_is_silent() { /* path в несуществующем каталоге без прав / под файлом → write не паникует */ }
#[test] fn tail_returns_last_lines() { /* 300 строк → tail(200).len()==200, последняя — "line 299" */ }
#[test] fn timestamp_is_utc_iso() { /* format_ts(SystemTime::UNIX_EPOCH + 1.5s) == "1970-01-01T00:00:01.500Z" */ }
```

- [ ] **Step 2:** `cargo test diag::` — FAIL (нет модуля).
- [ ] **Step 3: реализация.** Глобал: `static LOG: OnceLock<Mutex<Writer>>`. Время — `SystemTime` → UTC вручную (алгоритм дней civil-from-days), без `chrono`. Хук паники: `std::panic::set_hook`, сохраняет предыдущий хук и вызывает его после записи `panic <info>`. Lock poison игнорируется (`into_inner`).
- [ ] **Step 4:** `app::setup` первым делом `crate::diag::init(&dir.join("logs"))`, затем `dlog!("app", "start v{} data={}", env!("CARGO_PKG_VERSION"), dir.display())`. Порт — после старта сервера: `dlog!("server", "listening {port}")` или код предупреждения.
- [ ] **Step 5:** заменить все `eprintln!` в `app.rs`, `driver.rs`, `server/mod.rs`, `server/ws.rs`, `voice_lab/mod.rs` на `dlog!` с областями `app/music/server/tts/vl`. В `voice_lab/trainer.rs` тестовые `eprintln!` (строки ~517, ~527 внутри `#[cfg(test)]`) не трогать.
- [ ] **Step 6:** тесты и clippy зелёные.
- [ ] **Step 7: Commit** `feat(diag): file logger with rotation and panic hook`

---

### Task 2: Точки логирования

**Files:**
- Modify: `src-tauri/src/twitch/eventsub.rs`, `src-tauri/src/driver.rs`, `src-tauri/src/tts/engine.rs`, `src-tauri/src/obs.rs`, `src-tauri/src/spotify.rs`, `src-tauri/src/voice_lab/runner.rs`, `src-tauri/src/voice_lab/commands.rs`, `src-tauri/src/voice_lab/trainer.rs`

**Interfaces:**
- Consumes: `dlog!` из T1.
- Produces: ничего нового для других задач.

Строки (формат свободный, но только эти поля):

| Область | Событие | Поля |
|---|---|---|
| `twitch` | welcome, подписка ок/403/ошибка, reconnect, обрыв + бэкофф, reconcile (кол-во) | session id, тип подписки, HTTP-код |
| `music` | выкуп получен, исход | redemption id, reward id, исход/причина |
| `tts` | в очереди, синтез ошибка (код), play, ended/error/timeout, виджет connected/disconnected, исход погашения | item id, источник `reward/chat/test`, логин, длина текста |
| `obs`, `spotify` | ошибка запроса | операция, вид ошибки / HTTP-код |
| `vl` | задание старт/исход | job id, kind, статус, код ошибки |
| `train` | этапы установки, исход, код `uv` | stage, exit code |

- [ ] **Step 1: тест** `tts::engine` — `log_line_has_no_chat_text`: прогнать chat-сообщение с текстом `"секрет123"` через движок с тестовым логгером… Упрощение: вынести форматирование tts-строк в `fn describe(item: &Item) -> String` и тестировать её: результат содержит логин и `len=9`, не содержит `"секрет123"`.
- [ ] **Step 2:** FAIL → **Step 3:** реализовать `describe` и расставить `dlog!` по таблице. Ошибки reqwest логировать как `e.status()` / `is_timeout()` / `is_connect()`, не `{e}` (URL может содержать query с токеном).
- [ ] **Step 4:** тесты/clippy зелёные. **Step 5: Commit** `feat(diag): log connections, redemptions, tts and voice lab events`

---

### Task 3: `GET /debug`, блок «Отладка» в панели, docs

**Files:**
- Modify: `src-tauri/src/server/mod.rs`, `src-tauri/src/app.rs`, `src-tauri/src/lib.rs` (регистрация команды), `src/lib/views/Connections.svelte`, `src/lib/api.ts`, `src/lib/i18n/ru.ts`, `src/lib/mock.ts`
- Create: `docs/debugging.md`
- Modify: `README.md`, `docs/architecture.md` (раздел «Отладка», строка `diag.rs` в таблице модулей)

**Interfaces:**
- Produces:
  - `pub type DebugFn = Arc<dyn Fn() -> futures_util::future::BoxFuture<'static, serde_json::Value> + Send + Sync>;` в `server/mod.rs`.
  - `serve`/`start_or_disabled`/`build` получают `debug: DebugFn` последним параметром.
  - маршрут `GET /debug` → проверка `Host` (вынести из `ws::upgrade` в `fn host_allowed(headers, port) -> bool`, переиспользовать в обоих), затем `Json((debug)())` + `Cache-Control: no-store`; иначе 403.
  - `app.rs`: `fn debug_snapshot(app: AppHandle, started: Instant) -> DebugFn` — собирает JSON из спецификации (A2) через существующие `get_state`, `tts_state`, `twitch_status`, `spotify_status`, `running_port`, `diag::log_path`, `diag::tail(200)`; каждая асинхронная часть — `tokio::time::timeout(2s)` → `null`.
  - команда `#[tauri::command] pub fn open_logs() -> Result<(), String>` — `xdg-open`/`open`/`explorer` на каталог логов; `logs_unavailable` если `diag::log_path()` нет.
  - `api.ts`: `open_logs(): Promise<void>`, `log_dir(): Promise<string | null>`.

- [ ] **Step 1: тесты в `server/mod.rs` (через `tower::ServiceExt::oneshot`, как существующие)**

```rust
#[tokio::test] async fn debug_returns_snapshot_json() { /* DebugFn → json!({"ok":1}); GET /debug Host 127.0.0.1:<port> → 200, body {"ok":1}, Cache-Control no-store */ }
#[tokio::test] async fn debug_rejects_foreign_host() { /* Host evil.example → 403 */ }
```
  и в `app.rs`: `debug_parts_time_out_to_null` — вынести `async fn part<T: Serialize>(f: impl Future<Output=Result<T,String>>) -> Value` с таймаутом; с `tokio::time::pause()` будущий `pending()` → `Value::Null` после `advance(2s)`; `Ok(5)` → `5`; `Err` → `null`.
- [ ] **Step 2:** FAIL → **Step 3:** реализация. Секреты: в снимок не включать `Settings`.
- [ ] **Step 4: панель.** В `Connections.svelte` секция «Отладка»: путь к логу (команда `#[tauri::command] pub fn log_dir() -> Option<String>` → `diag::log_path()` родитель), кнопка «Открыть папку логов», адрес `http://127.0.0.1:<port>/debug` (порт из `running_port`) с кнопкой «Копировать» (как Copy URL в `Tts.svelte`). Тексты: `debugTitle: 'Отладка'`, `debugOpenLogs: 'Открыть папку логов'`, `debugUrl: 'Состояние для отладки'`, `debugCopy: 'Копировать'`, `logs_unavailable: 'Папка логов недоступна'`.
- [ ] **Step 5: `docs/debugging.md`** (≤ 60 строк): путь лога, `tail -f`, `grep` по областям, `curl -s http://127.0.0.1:<port>/debug | jq`, где `voice-lab/trainer/install.log` и `voice-lab/training/<id>/worker.log`, что секретов там нет. Ссылки из README и architecture.
- [ ] **Step 6:** cargo test/clippy, `npm run check`, `npm test` зелёные. **Step 7: Commit** `feat(diag): GET /debug snapshot and debug panel`

---

### Task 4: Прогресс установки тренажёра

**Files:**
- Modify: `src-tauri/src/voice_lab/trainer.rs`, `src-tauri/src/voice_lab/commands.rs`, `src-tauri/src/lib.rs`, `src/lib/voiceApi.ts`, `src/lib/views/voice/Train.svelte`, `src/lib/i18n/ru.ts`, `src/lib/mock.ts`, `docs/user-guide.md`

**Interfaces:**
- Produces:
  - `pub fn uv_progress(log: &str) -> UvProgress` где `pub struct UvProgress { pub done: u64, pub total: u64, pub item: Option<String> }`. Учитываются только полные строки (до последнего `\n`). `total` = число уникальных имён из `Downloading <name> (`; `done` = уникальные имена из ` Downloaded <name>`, которые были в `Downloading`; `item` — последнее начатое, ещё не скачанное.
  - `progress` у `trainer::install` получает третий вариант стадии: `progress("packages", done, total)` и отдельный колбэк элемента — сигнатуру колбэка расширить до `&(dyn Fn(&str, u64, u64, Option<&str>) + Send + Sync)`; существующие вызовы передают `None`.
  - Событие `vl-train-install`: `{stage, done, total, item?}`; стадия `venv` с `total == 0` значит «без значения».
  - `#[tauri::command] pub async fn vl_train_install_log(state) -> Result<String, String>` — последние ≤ 8 КБ `trainer/install.log` (по границе символа), пусто, если файла нет.

- [ ] **Step 1: тесты в `trainer.rs`**

```rust
#[test] fn uv_progress_counts_downloads() { /* лог из 3 Downloading + 1 Downloaded → done 1, total 3, item = третье имя */ }
#[test] fn uv_progress_ignores_partial_line() { /* последняя строка " Downloaded tor" без \n → не считается */ }
#[test] fn uv_progress_unique_names() { /* "nvidia-cublas-cu12" и "nvidia-cublas" — разные; повтор строки не удваивает */ }
#[test] fn uv_progress_ignores_unknown_downloaded() { /* Downloaded без Downloading → done не растёт, done <= total */ }
```
  и `vl_train_install_log_tails_file` в `commands.rs` (10 КБ кириллицы → ≤ 8192 байт, валидный UTF-8, конец совпадает).
- [ ] **Step 2:** FAIL → **Step 3:** реализация: в `install` вместо `child.wait()` в `select!` — цикл с `tokio::time::interval(500ms)`: на тик читать `install.log`, `uv_progress`, слать `packages` пока `done < total`, иначе `venv` с `(0, 0)`. Кеш `uv-cache` при ошибке не удалять (проверить существующий код очистки).
- [ ] **Step 4: панель `Train.svelte`:** при `packages` — `ru.vlTrainStages.packages` + `n/m` + имя; при `venv` и `total==0` — `<progress>` без `value`. При ошибке установки — `<pre>` с `vl_train_install_log()`. Подпись `vlTrainInstallNote: 'Установка прерывается при закрытии программы; повтор докачает уже скачанное'`. `vlTrainStages.packages: 'Скачивание пакетов'`.
- [ ] **Step 5: user-guide:** в разделе тренажёра — что окружение изолировано (свой Python и `.venv` в папке приложения, системный pip не используется) и как смотреть `install.log`.
- [ ] **Step 6:** проверки зелёные. **Step 7: Commit** `feat(train): real install progress from uv log`

---

### Task 5: Сборка с CUDA и GPU-пакет

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (только изменения от фичи)
- Create: `src-tauri/src/voice_lab/gpu.rs`
- Modify: `src-tauri/src/voice_lab/mod.rs`, `src-tauri/src/voice_lab/commands.rs`, `src-tauri/src/voice_lab/trainer.rs` (вынести `gpu_info`), `src-tauri/src/lib.rs`

**Interfaces:**
- Produces (`gpu.rs`):
  - `pub const SUPPORTED: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));`
  - `pub const ARCHIVE_URL`, `ARCHIVE_SHA256 = "84d28f27589090b280d4312743efd3d450cd4ac7d1e1d75e7d9076d9637bf9de"`, `ARCHIVE_SIZE` (байты — взять из `Content-Length`, записать в план исполнителем как отклонение), `FILES: [(&str /*имя*/, &str /*sha256*/); 2]` для `libonnxruntime_providers_cuda.so` и `libonnxruntime_providers_shared.so` (исполнитель вычисляет хэши из архива и фиксирует).
  - `pub fn dir(root: &Path) -> PathBuf` → `root/gpu`.
  - `#[derive(Serialize)] #[serde(rename_all="lowercase")] pub enum Pack { None, Ok, Broken }`; `pub fn pack(root: &Path) -> Pack` (размер+SHA каждого файла; результат кешировать по `(len, mtime)` в `Mutex<Option<…>>`, чтобы статус не хэшировал 267 МБ на каждый вызов).
  - `pub async fn install(root: &Path, progress: &(dyn Fn(u64,u64)+Send+Sync), cancel: oneshot::Receiver<()>) -> Result<(), VlError>` — `models::install_files` для архива в `root/gpu/tmp`, затем `tar -xzf <archive> -C <tmp>/x --strip-components=2 <top>/lib/libonnxruntime_providers_cuda.so <top>/lib/libonnxruntime_providers_shared.so` через `std::process::Command` (без shell), проверка SHA файлов, `rename` в `root/gpu/`, удаление `tmp`. Коды: `GPU_UNSUPPORTED`, `MODEL_DOWNLOAD_FAILED` (как у модели), `GPU_PACK_INVALID`, `JOB_CANCELLED`.
  - `pub fn uninstall(root: &Path) -> Result<(), VlError>`.
  - `pub fn worker_link(root: &Path, exe: &Path) -> std::io::Result<PathBuf>` — удаляет `root/gpu/stream-assist` (файл/ссылку), создаёт `std::os::unix::fs::symlink(exe, …)`; на не-unix — `Err(Unsupported)`.
- Produces (`commands.rs`): `vl_gpu_status() -> GpuStatus { supported, pack, installing, gpu: Option<GpuInfo> }`, `vl_gpu_install()`, `vl_gpu_install_cancel()`, `vl_gpu_uninstall()`; слот установки — как `train_install` (`tokio::Mutex<Option<oneshot::Sender<()>>>` в `Lab`); событие прогресса `vl-gpu-install` `{done,total}`.
- Produces (`trainer.rs`): `pub async fn gpu_info() -> Option<GpuInfo>` — вынесено из `status`, `status` её использует.

- [ ] **Step 1: Cargo.** `supertonic3-tts = { version = "=1.3.0", features = ["cuda"] }` + комментарий: «провайдер CUDA в GPU-пакете — onnxruntime 1.28.0 (Microsoft); версия ядра pyke ms@1.28.0 из ort-sys rc.13 — менять только вместе». `cargo build` — проверить, что собирается, и записать размер `target/release/stream-assist` до/после в отчёт (сборка release не обязательна — достаточно debug-сборки и `cargo test`; размер release замерит T7).
- [ ] **Step 2: тесты `gpu.rs`**

```rust
#[test] fn status_none_without_files()
#[test] fn status_ok_with_matching_files()        // FILES подменяются через внутреннюю fn pack_with(root, files)
#[test] fn status_reports_broken_on_hash_mismatch()
#[test] fn uninstall_removes_dir()
#[cfg(unix)] #[test] fn worker_link_points_to_exe()
#[cfg(unix)] #[test] fn worker_link_replaces_stale_link() // ссылка на старый путь → после вызова указывает на новый
#[tokio::test] async fn install_extracts_only_named_files() // локальный tgz (собрать в тесте системным tar) с lib/ и лишним файлом; wiremock раздаёт; после install в gpu/ только два .so; через install_from(url, sha, files) — внутренняя параметризованная fn
#[tokio::test] async fn install_rejects_wrong_file_hash()  // GPU_PACK_INVALID, gpu/ без .so
```
- [ ] **Step 3:** FAIL → **Step 4:** реализация. Удаление пакета запрещено (`JOB_BUSY`) при `runner.gpu_clone_running()` из T6 — до T6 вызывать заглушку нельзя, поэтому проверку добавить в T6.
- [ ] **Step 5:** проверки зелёные. **Step 6: Commit** `feat(vl): CUDA-enabled build and downloadable GPU provider pack`

---

### Task 6: Клонирование на GPU

**Files:**
- Modify: `src-tauri/src/voice_lab/worker.rs`, `src-tauri/src/voice_lab/runner.rs`, `src-tauri/src/voice_lab/commands.rs`, `src-tauri/examples/vl_fake_worker.rs` (если нужен новый режим)

**Interfaces:**
- Consumes: `gpu::{pack, Pack, worker_link}` (T5), `dlog!` (T1).
- Produces:
  - `#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)] #[serde(rename_all="lowercase")] pub enum CloneDevice { #[default] Cpu, Gpu }` в `worker.rs`.
  - `WorkerRequest::Clone { …, #[serde(default)] device: CloneDevice }`.
  - `WorkerEvent::Error { code: String, #[serde(default, skip_serializing_if = "Option::is_none")] detail: Option<String> }` — `detail` ≤ 2000 символов, только для `GPU_FAILED`.
  - Воркер: при `device == Gpu` TTS и `SpeakerEncoder` грузятся с `Device::Gpu`, `CloneOptions.device = Device::Gpu`; любая ошибка загрузки/клона в GPU-режиме → `Error { code: "GPU_FAILED", detail: Some(format!("{e:#}")) }`.
  - `Runner::run_worker_at(&self, exe: &Path, req, tmp, cancel, on_event)`; `run_worker` вызывает его с `&self.exe`.
  - `Runner::start_clone(…, device: CloneDevice, emit)` — новый параметр перед `emit`. Для `Gpu`: `gpu::pack(&root) != Ok` → `GPU_PACK_MISSING`; exe = `gpu::worker_link(&root, &self.exe)` (ошибка → `GPU_FAILED`). Пока идёт GPU-клон, `self.gpu_clone: AtomicBool` = true (сбрасывает `JobGuard`/завершение).
  - `pub fn gpu_clone_running(&self) -> bool`.
  - Родитель логирует `dlog!("vl", "clone {id} gpu failed: {detail}")`.
  - `vl_clone(…, device: Option<CloneDevice>)` — `None` = `Cpu` (совместимость фронтенда до T7). Перед стартом GPU-клона: `l.train.active().is_some()` или установка GPU-пакета идёт → `JOB_BUSY`.
  - `vl_train_start` и `vl_train_resume`: `l.runner.gpu_clone_running()` → `JOB_BUSY`. `vl_gpu_install`/`vl_gpu_uninstall`: `gpu_clone_running()` → `JOB_BUSY`.

- [ ] **Step 1: тесты**

```rust
// worker.rs
#[test] fn clone_request_defaults_to_cpu() // JSON без device → Cpu
#[test] fn error_event_detail_is_optional() // {"event":"error","code":"X"} парсится; detail None не сериализуется
// runner.rs (fake worker, как существующие тесты клона)
#[tokio::test] async fn gpu_clone_without_pack_fails() // GPU_PACK_MISSING, слот свободен
#[tokio::test] async fn gpu_clone_runs_through_link() // фейковый пакет: подложить файлы и подменить проверку через pack_with из T5 или тестовый хук; fake worker пишет argv[0] в файл → это root/gpu/stream-assist
#[tokio::test] async fn gpu_clone_flag_clears_after_finish()
#[tokio::test] async fn gpu_failed_detail_is_logged_and_code_kept() // fake worker режим "gpu-fail" шлёт Error{GPU_FAILED, detail} → job.error == "GPU_FAILED"
// commands.rs
#[test] fn train_start_blocked_during_gpu_clone()
```
- [ ] **Step 2:** FAIL → **Step 3:** реализация. CPU-путь (`device == Cpu`) — байт-в-байт прежний запрос, кроме поля `device` (serde default).
- [ ] **Step 4: ручной прогон (если у исполнителя есть NVIDIA + CUDA 13):** `#[ignore = "needs NVIDIA GPU, CUDA 13, cuDNN 9 and GPU pack"]` тест `real_gpu_clone_fast` в `acceptance_p2.rs`-стиле: клон «быстро» на GPU из фикстуры, профиль создан. Не обязателен для зелёного `cargo test`.
- [ ] **Step 5:** проверки зелёные. **Step 6: Commit** `feat(vl): clone on NVIDIA GPU via provider pack`

---

### Task 7: Панель «Записи», документация, приёмка

**Files:**
- Modify: `src/lib/views/voice/Sources.svelte`, `src/lib/voiceApi.ts`, `src/lib/i18n/ru.ts`, `src/lib/mock.ts`, `docs/voice-lab.md`, `docs/user-guide.md`, `docs/architecture.md`, `README.md`, `docs/backlog.md` (удалить сделанные пункты; файл удалить, если пуст), спецификация (статус «реализовано»)

**Interfaces:**
- Consumes: `vl_gpu_status/install/install_cancel/uninstall`, событие `vl-gpu-install`, `vl_clone(..., device)`.
- Produces: `voiceApi.ts`: `type CloneDevice = 'cpu' | 'gpu'`, `GpuStatus`, функции-обёртки, `onGpuInstall(cb)`.

- [ ] **Step 1: UI.** В форме клона — `<select>` «Устройство»: `Процессор` / `Видеокарта (NVIDIA)`; второй вариант `disabled`, пока `pack !== 'ok'`; если `!supported` — селекта нет. Рядом — кнопки «Установить GPU-пакет (≈230 МБ)» / «Отменить» / «Удалить GPU-пакет», `<progress>`, имя GPU и драйвер из статуса. Выбор в `localStorage` (`vl.cloneDevice`, `try/catch`). Подсказка: `vlCloneCpuNote` для CPU, `vlCloneGpuNote` для GPU. Ошибки: `GPU_FAILED: 'Не удалось запустить на видеокарте. Нужны драйвер NVIDIA 580+, CUDA 13 и cuDNN 9. Подробности — в логе'`, `GPU_PACK_MISSING: 'GPU-пакет не установлен'`, `GPU_PACK_INVALID: 'GPU-пакет повреждён — переустановите'`, `GPU_UNSUPPORTED: 'GPU поддерживается только на Linux x86_64'`. `pack === 'broken'` → тот же текст, что `GPU_PACK_INVALID`.
- [ ] **Step 2:** `npm run check`, `npm test`.
- [ ] **Step 3: приёмка вручную (исполнитель-человек/оркестратор):** release-сборка `npm run tauri build`; записать размер AppImage до/после и `objdump -T … | grep -o 'GLIBC_[0-9.]*' | sort -V | tail -1` ≤ 2.39; установить пакет, клон «быстро» и «точно» на GPU — время в `docs/voice-lab.md`; значение подставить в `vlCloneGpuNote`.
- [ ] **Step 4: docs.** `voice-lab.md`: GPU-клон, требования (драйвер 580+, CUDA 13, cuDNN 9), пакет, время. `user-guide.md`: раздел «Клонирование на видеокарте», строки в «Если что-то не так» про `GPU_FAILED` и отладку (ссылка на `debugging.md`). `architecture.md`: GPU-пакет, симлинк-запуск и почему (ORT ищет провайдеры рядом с путём запуска; провайдер pyke без sm_120).
- [ ] **Step 5: Commit** `feat(vl): GPU clone controls and docs`

---

Порядок выполнения: 1 → 2 → 3 → 4 → 5 → 6 → 7.
