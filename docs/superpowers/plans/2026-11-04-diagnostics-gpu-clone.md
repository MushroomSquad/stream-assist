# Отладка, GPU-клон, прогресс тренажёра — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** лог-файл и `GET /debug` для отладки, клонирование голоса на NVIDIA GPU через скачиваемый пакет провайдера ORT, честный прогресс установки тренажёра.

**Architecture:** std-only логгер `diag.rs` (глобальный `OnceLock<Mutex<Writer>>`), `/debug` на существующем axum с замыканием-снимком из `app.rs`. GPU: фича `cuda` у `supertonic3-tts` только для Linux x86_64, провайдер CUDA от Microsoft в `<app_data>/voice-lab/gpu/`; воркер запускается с `argv[0] = <gpu>/stream-assist` (`Command::arg0`) — ORT ищет провайдеры в `dirname(argv[0])`. Прогресс `uv sync` — разбор `install.log` в байтах во время работы `uv`.

**Tech Stack:** Rust (Tauri 2, tokio, axum), SvelteKit/Svelte 5.

**Spec:** `docs/superpowers/specs/2026-11-04-diagnostics-gpu-clone.md`

## Global Constraints

- Ветка `feat/diag-gpu-progress`. Без push/merge.
- `ort` остаётся `=2.0.0-rc.13`; `supertonic3-tts` `=1.3.0`; без `cargo update`.
- Новых крейтов нет. Фича `cuda` у `supertonic3-tts` — **только** в `[target.'cfg(all(target_os = "linux", target_arch = "x86_64"))'.dependencies]`; общая строка в `[dependencies]` без фич остаётся. Windows/macOS-сборка не меняется.
- В лог и `/debug` не попадают токены, ключи, `client_secret`, коды OAuth, URL с query, тексты сообщений чата и наград (только длина).
- Сбой записи лога не роняет, не блокирует и не паникует.
- Тексты панели — только в `src/lib/i18n/ru.ts`; переиспользовать существующие ключи (`ru.copy` и т. п.).
- Музыка, озвучка и CPU-путь Voice Lab по поведению не меняются.
- При смене сигнатур чинить **все** места вызова, включая `src-tauri/tests/*.rs`, `server/tests.rs`, `acceptance_p2.rs`, примеры — даже если файла нет в списке задачи.
- Перед коммитом: `cd src-tauri && cargo build --examples && cargo test` (зелёные), `cargo clippy --all-targets` (без новых предупреждений); для задач с фронтендом ещё `npm run check` и `npm test`.

## Review Focus

1. Лог недоступен на запись (каталог только для чтения, диск полон, stderr закрыт) → приложение работает. Тест T1 `write_failure_is_silent`.
2. `/debug` при незапущенном движке TTS или зависшем драйвере → ответ ≤ ~2 с, части `null`. Тест T3 `debug_parts_time_out_to_null`.
3. Падение (SIGSEGV/abort) CUDA-провайдера во время клона → `GPU_FAILED` с хвостом stderr воркера в логе, слот клона свободен. Тест T6 `gpu_worker_crash_maps_to_gpu_failed`.
4. Пакет повреждён/неполон → `pack: broken`, GPU недоступен, переустановка чинит. Тест T5 `status_reports_broken_on_hash_mismatch`, `status_broken_when_one_file_missing`.
5. `install.log` с обрезанной последней строкой и Python-строкой `cpython… (download) (32.3MiB)` → байты не скачут, `done ≤ total`. Тесты T4 на фикстуре реального лога.

---

### Task 1: Логгер `diag`

**Files:**
- Create: `src-tauri/src/diag.rs`
- Modify: `src-tauri/src/lib.rs` (mod + `diag::init` в `.setup` **до** `voice_lab::init`), `src-tauri/src/app.rs`, все файлы с `eprintln!` вне `#[cfg(test)]`: `app.rs`, `driver.rs`, `server/mod.rs`, `server/ws.rs`, `voice_lab/mod.rs`

**Interfaces:**
- Produces:
  - `pub fn init(dir: &Path)` — один раз; создаёт `dir`, ставит хук паники. Повторный вызов — no-op.
  - `pub fn line(area: &str, msg: impl std::fmt::Display)` — пишет `<ts> <area> <msg>\n` в файл и stderr. До `init` — только stderr.
  - `pub fn log_path() -> Option<PathBuf>`
  - `pub fn tail(n: usize) -> Vec<String>` — последние `n` строк, читает ≤ 256 КБ с конца, под тем же lock, что запись; нет файла → пусто.
  - `pub fn http_err(e: &anyhow::Error) -> String` — для `reqwest::Error` (через `downcast_ref`, также `e.chain()`) возвращает `without_url()`-текст + статус; иначе `format!("{e:#}")` с вырезанием `?…` до пробела у подстрок, похожих на URL.
  - макрос `crate::dlog!(area, "fmt", args…)` → `diag::line(area, format_args!(…))`.
- Константы: `MAX_BYTES = 5 * 1024 * 1024`, файл `stream-assist.log`, ротация в `stream-assist.log.1`.

- [ ] **Step 1: тесты в `diag.rs`** (логика в `struct Writer { path: PathBuf, max: u64, file: Option<File> }` с `fn write(&mut self, line: &str)`; тесты глобал не трогают)

```rust
#[test] fn writes_line_with_area()          // Writer::write("… tts hello") → файл кончается на "tts hello\n"
#[test] fn rotates_over_limit()             // max = 100; две записи по 80 байт → .log.1 с первой, .log со второй
#[test] fn rotation_replaces_old_backup()   // три записи по 80 → .log.1 содержит вторую, не первую
#[test] fn write_failure_is_silent()        // path = <файл>/x.log (родитель — обычный файл) → write не паникует
#[test] fn tail_returns_last_lines()        // 300 строк → tail_of(path, 200).len()==200, последняя "line 299"
#[test] fn timestamp_is_utc_iso()           // format_ts(UNIX_EPOCH + 1.5s) == "1970-01-01T00:00:01.500Z"; format_ts(UNIX_EPOCH + 1_793_800_000s) == "2026-11-04T…" (точное значение исполнитель считает и фиксирует)
#[test] fn http_err_strips_query()          // anyhow!("GET https://x/y?token=abc failed") → нет "abc"
```

- [ ] **Step 2:** `cargo test diag::` — FAIL.
- [ ] **Step 3: реализация.**
  - Время — `SystemTime` → UTC вручную (civil-from-days), без `chrono`.
  - Строку форматировать **до** взятия lock. stderr — `let _ = writeln!(std::io::stderr().lock(), …)`, никогда `eprintln!`.
  - Ротация: закрыть `file` (`take()`), `rename`, открыть новый (на Windows rename открытого файла падает).
  - Хук паники: `std::panic::set_hook`; сохраняет предыдущий хук, пишет `panic <info>` через `try_lock` (не удалось — только stderr), затем вызывает предыдущий. Poison lock — `into_inner`.
- [ ] **Step 4:** `lib.rs` `.setup`: `crate::diag::init(&app.path().app_data_dir()?.join("logs"))` первой строкой. `app::setup`: `dlog!("app", "start v{} data={}", env!("CARGO_PKG_VERSION"), dir.display())`; после старта сервера `dlog!("server", "listening {port}")` или `dlog!("server", "{code}")`.
- [ ] **Step 5:** заменить перечисленные `eprintln!` на `dlog!` (области `app/music/server/tts/vl`). `#[cfg(test)]`-вывод в `voice_lab/trainer.rs` не трогать.
- [ ] **Step 6:** проверки зелёные. **Step 7: Commit** `feat(diag): file logger with rotation and panic hook`

---

### Task 2: Точки логирования

**Files:**
- Modify: `src-tauri/src/twitch/eventsub.rs`, `src-tauri/src/driver.rs`, `src-tauri/src/tts/engine.rs`, `src-tauri/src/tts/mod.rs`, `src-tauri/src/obs.rs`, `src-tauri/src/spotify.rs`, `src-tauri/src/voice_lab/runner.rs`, `src-tauri/src/voice_lab/trainer.rs`

**Interfaces:**
- Consumes: `dlog!`, `diag::http_err` (T1).
- Produces: `pub fn describe(id, user: &str, src: &str, text: &str) -> String` в `tts/mod.rs` — `id=… src=… user=… len=N` (без текста). Движок зовёт её для `queue::Item` (`src` из `Origin`: `reward|chat|test`), T3 — для `ItemView` (`src` = `reward|chat` по полю `reward`).

| Область | Событие | Поля |
|---|---|---|
| `twitch` | welcome, подписка ок/403/ошибка, reconnect, обрыв + бэкофф, reconcile (кол-во) | session id, тип подписки, HTTP-код |
| `music` | выкуп получен, исход | redemption id, reward id, исход/причина |
| `tts` | в очереди, ошибка синтеза (код), play, ended/error/timeout, виджет connected/disconnected, исход погашения | `describe(item)`, исход |
| `obs`, `spotify` | ошибка запроса | операция, `http_err` |
| `vl` | задание старт/исход | job id, kind, статус, код ошибки |
| `train` | этапы установки, исход, exit code `uv` | stage, code |

- [ ] **Step 1: тест** в `tts/mod.rs` `describe_has_no_text`: `describe(1, "bob", "chat", "секрет123")` → строка содержит `user=bob` и `len=9`, не содержит `секрет123`.
- [ ] **Step 2:** FAIL → **Step 3:** реализовать `describe`, расставить `dlog!` по таблице.
- [ ] **Step 4:** проверки зелёные. **Step 5: Commit** `feat(diag): log connections, redemptions, tts and voice lab events`

---

### Task 3: `GET /debug`, блок «Отладка», docs

**Files:**
- Modify: `src-tauri/src/server/mod.rs`, `src-tauri/src/server/ws.rs`, `src-tauri/src/server/tests.rs`, `src-tauri/src/app.rs`, `src-tauri/src/driver.rs` (вызов `serve` в тестах ~1159 и новый `DriverMsg`), `src-tauri/src/lib.rs`, `src/lib/views/Connections.svelte`, `src/lib/api.ts`, `src/lib/i18n/ru.ts`, `src/lib/mock.ts`
- Create: `docs/debugging.md`
- Modify: `README.md`, `docs/architecture.md` (раздел «Отладка», строка `diag.rs` в таблице модулей)

**Interfaces:**
- Produces:
  - `pub type DebugFn = Arc<dyn Fn() -> futures_util::future::BoxFuture<'static, serde_json::Value> + Send + Sync>;` и `pub fn noop_debug() -> DebugFn` (→ `json!({})`, для тестов) в `server/mod.rs`.
  - `build`/`serve`/`start_or_disabled` получают `debug: DebugFn` последним параметром.
  - `fn host_allowed(headers: &HeaderMap, port: u16) -> bool` — вынести из `ws::upgrade`, использовать в обоих.
  - `GET /debug`: 403, если `!host_allowed` **или** заголовок `Sec-Fetch-Site` есть и не равен `none` (страницы тем с того же origin не читают `/debug`; curl заголовка не шлёт). Иначе `Json((debug)())` + `Cache-Control: no-store`.
  - `DriverMsg::SpotifyConnected(oneshot::Sender<bool>)` — есть ли клиент Spotify, без сетевого запроса (`spotify_status` не использовать: он ходит в API).
  - `app.rs`: `fn debug_snapshot(app: AppHandle, started: Instant) -> DebugFn` (клон `AppHandle` в `async move`) — JSON из спецификации A2: `version, port, uptimeS, dataDir, logFile, widget (get_state), tts (redact_tts), twitch (twitch_status), spotify (SpotifyConnected), logTail (diag::tail(200))`.
  - `async fn part<T: Serialize>(f: impl Future<Output = Result<T, String>>) -> Value` — `tokio::time::timeout(2s)`; `Err`/таймаут → `Value::Null`.
  - `fn redact_tts(v: &TtsView) -> Value` — все поля `TtsView`, кроме очереди/текущего элемента, которые заменяются на `describe(item)` из T2.
  - команды `#[tauri::command] pub fn open_logs(app) -> Result<(), String>` через уже подключённый `tauri-plugin-opener` (`app.opener().open_path(dir, None::<&str>)`), ошибка → `logs_unavailable`; `#[tauri::command] pub fn log_dir() -> Option<String>`.
  - `api.ts`: `open_logs(): Promise<void>`, `log_dir(): Promise<string | null>`.

- [ ] **Step 1: тесты** (`server/tests.rs` поднимает реальный listener, как существующие)

```rust
#[tokio::test] async fn debug_returns_snapshot_json()      // DebugFn → json!({"ok":1}); GET /debug с Host 127.0.0.1:<port> → 200, {"ok":1}, Cache-Control: no-store
#[tokio::test] async fn debug_rejects_foreign_host()       // Host evil.example → 403
#[tokio::test] async fn debug_rejects_cross_site_fetch()   // Sec-Fetch-Site: same-origin → 403; none → 200
// app.rs
#[tokio::test] async fn debug_parts_time_out_to_null()     // tokio::time::pause(); part(pending()) после advance(2s) → Null; part(Ok(5)) → 5; part(Err) → Null
#[test] fn debug_snapshot_has_no_chat_text()                // TtsView с элементом text "секрет123" → serde_json::to_string(&redact_tts(..)) не содержит "секрет123"
```
- [ ] **Step 2:** FAIL → **Step 3:** реализация. `Settings` в снимок не включать.
- [ ] **Step 4: панель.** Вкладки «Настройки» нет — блок «Отладка» в конце `Connections.svelte` (отклонение от формулировки спецификации, записать в спецификацию): путь из `log_dir()`, кнопка «Открыть папку логов», адрес `http://127.0.0.1:<port>/debug` (`running_port`) с кнопкой `ru.copy`. Новые ключи: `debugTitle: 'Отладка'`, `debugOpenLogs: 'Открыть папку логов'`, `debugUrl: 'Состояние для отладки'`, `logs_unavailable: 'Папка логов недоступна'`.
- [ ] **Step 5: `docs/debugging.md`** (≤ 60 строк): путь лога, `tail -f`, `grep` по областям, `curl -s http://127.0.0.1:<port>/debug | jq`, где `voice-lab/trainer/install.log`, `voice-lab/training/<id>/worker.log`, `voice-lab/gpu/worker.log`; что секретов и текстов чата там нет. Ссылки из README и architecture.
- [ ] **Step 6:** проверки зелёные. **Step 7: Commit** `feat(diag): GET /debug snapshot and debug panel`

---

### Task 4: Прогресс установки тренажёра

**Files:**
- Modify: `src-tauri/src/voice_lab/trainer.rs`, `src-tauri/src/voice_lab/commands.rs`, `src-tauri/src/lib.rs`, `src/lib/voiceApi.ts`, `src/lib/views/voice/Train.svelte`, `src/lib/i18n/ru.ts`, `src/lib/mock.ts`, `docs/user-guide.md`
- Fixture (уже в репо): `src-tauri/src/voice_lab/fixtures/uv_install_partial.log` — реальный лог прерванного `uv sync` (27 `Downloading`, 12 ` Downloaded`, первая пара — `cpython-3.12.12-linux-x86_64-gnu (download) (32.3MiB)`).

**Interfaces:**
- Produces:
  - `pub struct UvProgress { pub done: u64, pub total: u64, pub item: Option<String> }` — **байты**.
  - `pub fn uv_progress(log: &str) -> UvProgress`: только полные строки (до последнего `\n`). `Downloading <name> … (<size>)` — `name` = первый токен после `Downloading `, `size` = содержимое **последних** скобок, единицы `B|KiB|MiB|GiB` (×1024ⁿ). `total` = Σ size уникальных имён; `done` = Σ size уникальных имён из ` Downloaded <name>` (первый токен), которые есть среди `Downloading`; `item` — последнее начатое и ещё не скачанное имя.
  - Колбэк `trainer::install`: `&(dyn Fn(&str, u64, u64, Option<&str>) + Send + Sync)`; существующие вызовы передают `None`. Событие `vl-train-install`: `{stage, done, total, item?}`; стадия `venv` с `total == 0` — «без значения».
  - `#[tauri::command] pub async fn vl_train_install_log(state) -> Result<String, String>` — последние ≤ 8 КБ `trainer/install.log` (по границе символа), пусто, если файла нет.

- [ ] **Step 1: тесты в `trainer.rs`**

```rust
const FIXTURE: &str = include_str!("fixtures/uv_install_partial.log");
#[test] fn uv_progress_on_real_log()            // total == Σ размеров 27 пакетов (исполнитель считает и фиксирует), done == Σ 12 скачанных, 0 < done < total, item == Some(имя одного из нескачанных — последнего начатого)
#[test] fn uv_progress_parses_python_line()      // "Downloading cpython-3.12.12-linux-x86_64-gnu (download) (32.3MiB)\n Downloaded cpython-3.12.12-linux-x86_64-gnu (download)\n" → done == total == (32.3 * 1048576.0) as u64
#[test] fn uv_progress_ignores_partial_line()    // "Downloading torch (991.1MiB)\n Downloaded tor" → done 0
#[test] fn uv_progress_unique_names()            // повтор строки Downloading не удваивает total; "nvidia-cublas-cu12" и "nvidia-cublas" — разные
#[test] fn uv_progress_ignores_unknown_downloaded() // Downloaded без Downloading → done не растёт
#[test] fn uv_progress_units()                   // 512B, 1.5KiB, 2GiB
```
  и в `commands.rs` `vl_train_install_log_tails_file` (10 КБ кириллицы → ≤ 8192 байт, валидный UTF-8, конец совпадает).
- [ ] **Step 2:** FAIL → **Step 3:** реализация: в `install` добавить `.env("NO_COLOR", "1").arg("--color").arg("never")` к `uv sync`; вместо `child.wait()` в `select!` — `loop { select! { r = child.wait() => …, _ = cancel => …, _ = interval.tick() => { читать install.log; uv_progress; done < total → progress("packages", done, total, item) иначе progress("venv", 0, 0, None) } } }` (500 мс). `uv-cache` при ошибке не удаляется (сейчас удаляется только `tmp` — не менять).
- [ ] **Step 4: `Train.svelte`:** `packages` → `ru.vlTrainStages.packages` + `МБ n / m` + имя; `venv` с `total == 0` — отдельная ветка `{#if}` с `<progress>` без `value`. При ошибке установки — `<pre>` с последними 20 строками `vl_train_install_log()` (резать на фронтенде). Подпись `vlTrainInstallNote: 'Установка прерывается при закрытии программы; повтор докачает уже скачанное'`; `vlTrainStages.packages: 'Скачивание пакетов'`.
- [ ] **Step 5: user-guide:** окружение изолировано (свой Python и `.venv` в папке приложения, системный pip не используется), где `install.log`.
- [ ] **Step 6:** проверки зелёные. **Step 7: Commit** `feat(train): real install progress from uv log`

---

### Task 5: Сборка с CUDA и GPU-пакет

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/.cargo/config.toml` (создать, если нет; если есть — дописать `[env]`)
- Create: `src-tauri/src/voice_lab/gpu.rs`
- Modify: `src-tauri/src/voice_lab/mod.rs` (поле `gpu_install` в `VoiceLab` + литералы в `voice_lab/mod.rs`, `commands.rs` тестах, `acceptance_p2.rs`), `src-tauri/src/voice_lab/commands.rs`, `src-tauri/src/voice_lab/models.rs` (тип sha), `src-tauri/src/voice_lab/trainer.rs` (вынести `gpu_info`), `src-tauri/src/lib.rs`

**Interfaces:**
- Produces (`gpu.rs`):
  - `pub const SUPPORTED: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));`
  - `ARCHIVE_URL = "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-gpu_cuda13-1.28.0.tgz"`, `ARCHIVE_SHA256 = "84d28f27589090b280d4312743efd3d450cd4ac7d1e1d75e7d9076d9637bf9de"`, `ARCHIVE_SIZE = 240_874_643`, `ARCHIVE_TOP = "onnxruntime-linux-x64-gpu_cuda13-1.28.0"`.
  - `FILES: [(&str /*путь в архиве от ARCHIVE_TOP*/, &str /*имя в gpu/*/, &str /*sha256*/); 4]` — `lib/libonnxruntime_providers_cuda.so`, `lib/libonnxruntime_providers_shared.so`, `LICENSE`, `ThirdPartyNotices.txt`; хэши исполнитель вычисляет из архива и фиксирует; если какого-то из текстовых файлов в архиве нет — убрать его из `FILES` и записать отклонение.
  - `pub fn dir(root: &Path) -> PathBuf` → `root/gpu`.
  - `#[derive(Serialize, Clone, Copy, PartialEq, Debug)] #[serde(rename_all = "lowercase")] pub enum Pack { None, Ok, Broken }`.
  - `pub fn pack(root: &Path) -> Pack` — без кеша: ни одного файла → `None`; все есть и SHA совпадают → `Ok`; иначе `Broken`. Внутренняя `fn pack_with(root, files: &[(…)]) -> Pack` для тестов. Вызывать только из `blocking`/`spawn_blocking` (хэширует ~280 МБ).
  - `pub fn missing_libs(root: &Path) -> Vec<String>` — `ldd gpu/libonnxruntime_providers_cuda.so` (без shell), имена из строк `… => not found`; ошибка `ldd` → пусто.
  - `pub async fn install(root: &Path, progress: &(dyn Fn(u64, u64) + Send + Sync), cancel: oneshot::Receiver<()>) -> Result<(), VlError>`: свободно < 600 МБ (`trainer::disk_free_mb`, сделать `pub(crate)`) → `DISK_FULL`; `models::install_files` для архива в `gpu/tmp` внутри `select!` с `cancel`; затем `tar -xzf <archive> -C <gpu/tmp/x> --strip-components=1 <ARCHIVE_TOP>/<путь>…` через `std::process::Command` (без shell); проверка SHA `FILES`; `rename` каждого в `gpu/`; удаление `gpu/tmp` **в любом исходе** (успех, ошибка, отмена). Коды: `GPU_UNSUPPORTED`, `DISK_FULL`, `MODEL_DOWNLOAD_FAILED`, `MODEL_CHECKSUM_MISMATCH`, `GPU_PACK_INVALID`, `JOB_CANCELLED`. Внутренняя `install_from(root, url, sha, size, top, files, progress, cancel)` для тестов.
  - `pub fn uninstall(root: &Path) -> Result<(), VlError>` — удаляет `gpu/` целиком.
- `models::install_files`: тип sha `&'static str` → `String` (или `Cow<'static, str>`); существующие вызовы — `.into()`.
- Produces (`commands.rs`): `vl_gpu_status() -> GpuStatus { supported, pack, installing, gpu: Option<GpuInfo>, missingLibs: Vec<String> }` (`gpu_info().await` снаружи, `pack`/`missing_libs` — в `blocking`; `missing_libs` только при `pack == Ok`), `vl_gpu_install()`, `vl_gpu_install_cancel()`, `vl_gpu_uninstall()`; слот установки — как `train_install`; событие `vl-gpu-install` `{done,total}`.
- Produces (`trainer.rs`): `pub async fn gpu_info() -> Option<GpuInfo>` — вынесено из `status`.

- [ ] **Step 1: сборка.**
  - `Cargo.toml`: общая строка `supertonic3-tts = "=1.3.0"` остаётся; добавить
    ```toml
    [target.'cfg(all(target_os = "linux", target_arch = "x86_64"))'.dependencies]
    # GPU-пакет — onnxruntime 1.28.0 (Microsoft, cuda13); ядро — pyke ms@1.28.0 из ort-sys rc.13. Менять только вместе.
    supertonic3-tts = { version = "=1.3.0", features = ["cuda"] }
    ```
  - `.cargo/config.toml`: `[env] ORT_CUDA_VERSION = "13"`.
  - Проверки (записать вывод в отчёт): `cargo tree -e features -i ort-sys --target x86_64-pc-windows-msvc | grep -c cuda` → `0`; `cargo tree -e features -i ort-sys | grep cuda` → есть; `cargo build`; `readelf -d target/debug/stream-assist | grep NEEDED` — нет `libcu*`/`libcudnn*`.
- [ ] **Step 2: тесты `gpu.rs`**

```rust
#[test] fn status_none_without_files()
#[test] fn status_ok_with_matching_files()            // pack_with с хэшами тестовых файлов
#[test] fn status_reports_broken_on_hash_mismatch()
#[test] fn status_broken_when_one_file_missing()
#[test] fn uninstall_removes_dir()
#[tokio::test] async fn install_extracts_only_named_files() // tgz собирается в тесте системным tar (TOP/lib/a.so, TOP/lib/b.so, TOP/LICENSE, TOP/extra.bin); wiremock раздаёт; sha архива считается в тесте; после install_from в gpu/ только 3 нужных файла, gpu/tmp нет
#[tokio::test] async fn install_rejects_wrong_file_hash()   // GPU_PACK_INVALID, в gpu/ нет .so, gpu/tmp нет
#[tokio::test] async fn install_cancel_cleans_tmp()         // cancel до ответа wiremock (delay) → JOB_CANCELLED, gpu/tmp нет
```
- [ ] **Step 3:** FAIL → **Step 4:** реализация.
- [ ] **Step 5:** проверки зелёные. **Step 6: Commit** `feat(vl): CUDA-enabled Linux build and downloadable GPU provider pack`
- [ ] **Step 7: Gate (оркестратор, до T6).** В `podman run docker.io/library/ubuntu:24.04` с зависимостями из `.github/workflows/ci.yml`: `cargo build --release --bin stream-assist`; `objdump -T target/release/stream-assist | grep -o 'GLIBC_[0-9.]*' | sort -V | tail -1` ≤ `GLIBC_2.39`; `readelf -d` без `libcu*`; CPU-синтез через `stream-assist --vl-worker <synth.json>` в контейнере без CUDA проходит. Провал → T5 откатывается, T6–T7 не начинаются.

---

### Task 6: Клонирование на GPU

**Files:**
- Modify: `src-tauri/src/voice_lab/worker.rs`, `src-tauri/src/voice_lab/runner.rs`, `src-tauri/src/voice_lab/commands.rs`, `src-tauri/src/voice_lab/mod.rs`, `src-tauri/examples/vl_fake_worker.rs`, все вызовы `start_clone` (`commands.rs`, `runner.rs` тесты, `tests/voice_lab_runner.rs`, `tests/voice_lab_model.rs`, `tests/voice_lab_train.rs`), `src/lib/voiceApi.ts` (тип `detail?` в событии ошибки)

**Interfaces:**
- Consumes: `gpu::{dir, pack, Pack}` (T5), `dlog!` (T1).
- Produces:
  - `worker.rs`: `#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)] #[serde(rename_all = "lowercase")] pub enum CloneDevice { #[default] Cpu, Gpu }`; `WorkerRequest::Clone { …, #[serde(default)] device: CloneDevice }`.
  - `WorkerEvent::Error { code: String, #[serde(default, skip_serializing_if = "Option::is_none")] detail: Option<String> }`; `detail` ≤ 2000 символов.
  - Воркер в GPU-режиме: `load_text_to_speech_on(…, Device::Gpu)`, `SpeakerEncoder::on_device(…, Device::Gpu)`, после `clone_options(…)` — `opts.device = Device::Gpu` (`clone_options` не менять). Ошибки **загрузки моделей и самого клона** → `("GPU_FAILED", Some(format!("{e:#}")))`; ошибки чтения wav/style/записи результата сохраняют свои коды. CPU-режим — как сейчас.
  - `runner.rs`: `"GPU_FAILED"` в `WORKER_CODES`; разбор `WorkerEvent::Error { code, .. }`.
  - `Runner::run_worker_as(&self, arg0: Option<&Path>, stderr: Option<&Path>, req, tmp, cancel, on_event)`: `command.arg0(arg0)` под `#[cfg(unix)]` (на других ОС `arg0` игнорируется); тесты и режим `argv0` fake-воркера — тоже `#[cfg(unix)]` (Windows-CI делает `cargo check --all-targets`), stderr — в файл (truncate) вместо `null`. `run_worker` = `run_worker_as(None, None, …)`.
  - `clone_slot: Mutex<Option<(u64, oneshot::Sender<()>, CloneDevice)>>`; `pub fn gpu_clone_running(&self) -> bool` = слот занят с `Gpu`. Отдельного флага нет — слот очищает `JobGuard::drop`.
  - `Runner::start_clone(…, device: CloneDevice, emit)` — новый параметр перед `emit`. Пакет здесь не проверяется. Для `Gpu`: `arg0 = gpu::dir(&root).join("stream-assist")`, `stderr = gpu::dir(&root).join("worker.log")`. В `on_event`: `Error { detail: Some(d), .. }` → `dlog!("vl", "clone {id} gpu failed: {d}")`. Исход `WORKER_CRASHED` при `Gpu` → `GPU_FAILED`, `dlog!` с последними 2 КБ `gpu/worker.log`. В meta профиля — `"device": "cpu"|"gpu"`.
  - `VoiceLab.gpu_gate: std::sync::Mutex<()>` — держится на время «проверить + занять слот» в `vl_clone` (GPU) и в `vl_train_start`/`vl_train_resume`.
  - `commands::clone`/`vl_clone(…, device: Option<CloneDevice>)` (`None` = `Cpu`). Для `Gpu` (под `gpu_gate`): `!gpu::SUPPORTED` → `GPU_UNSUPPORTED`; `gpu::pack(&root) != Ok` → `GPU_PACK_MISSING`; `l.train.active().is_some()` или идёт установка GPU-пакета → `JOB_BUSY`.
  - `vl_train_start`/`vl_train_resume` (под `gpu_gate`): `l.runner.gpu_clone_running()` → `JOB_BUSY`. `vl_gpu_install`/`vl_gpu_uninstall` (под `gpu_gate`): `gpu_clone_running()` → `JOB_BUSY` (удаление во время CPU-клона разрешено — поправить спецификацию).

- [ ] **Step 1: тесты**

```rust
// worker.rs
#[test] fn clone_request_defaults_to_cpu()          // JSON без device → Cpu
#[test] fn error_event_detail_is_optional()          // {"event":"error","code":"X"} парсится; detail None не сериализуется
// runner.rs / tests/voice_lab_runner.rs (fake worker)
#[tokio::test] async fn gpu_clone_sets_arg0()        // fake worker режим "argv0" пишет argv[0] в файл рядом с out_style → == root/gpu/stream-assist
#[tokio::test] async fn gpu_clone_running_tracks_slot() // во время клона true, после — false; при CPU-клоне false
#[tokio::test] async fn gpu_failed_code_kept()       // fake режим "gpu-fail" шлёт Error{GPU_FAILED, detail} → job.error == "GPU_FAILED"
#[tokio::test] async fn gpu_worker_crash_maps_to_gpu_failed() // fake режим "crash" + device Gpu → "GPU_FAILED"; с Cpu → "WORKER_CRASHED"
// commands.rs
#[test] fn gpu_clone_without_pack_fails()            // пустой tempdir root → GPU_PACK_MISSING, слот свободен
#[test] fn train_start_blocked_during_gpu_clone()
```
- [ ] **Step 2:** FAIL → **Step 3:** реализация. CPU-запрос отличается только полем `device` (serde default).
- [ ] **Step 4:** `#[ignore = "needs NVIDIA GPU, CUDA 13, cuDNN 9 and GPU pack"]` тест `real_gpu_clone_fast` в `acceptance_p2.rs`: клон «быстро» на GPU из фикстуры → профиль создан; синтез одной фразы этим стилем на GPU и на CPU → косинус эмбеддингов speaker encoder ≥ 0.9 (проверка совместимости ядра pyke и провайдера MS). Не обязателен для зелёного `cargo test`; оркестратор запускает вручную.
- [ ] **Step 5:** проверки зелёные. **Step 6: Commit** `feat(vl): clone on NVIDIA GPU via provider pack`

---

### Task 7: Панель «Записи», документация, приёмка

**Files:**
- Modify: `src/lib/views/voice/Sources.svelte`, `src/lib/voiceApi.ts`, `src/lib/i18n/ru.ts`, `src/lib/mock.ts`, `docs/voice-lab.md`, `docs/user-guide.md`, `docs/architecture.md`, `README.md`, `docs/backlog.md` (удалить сделанные пункты; пустой — удалить файл), спецификация (статус «реализовано» + отклонения)

**Interfaces:**
- Consumes: `vl_gpu_status/install/install_cancel/uninstall`, `vl-gpu-install`, `vl_clone(..., device)`.
- Produces: `voiceApi.ts`: `type CloneDevice = 'cpu' | 'gpu'`, `GpuStatus`, обёртки, `onGpuInstall(cb)`.

- [ ] **Step 1: UI.** В форме клона — `<select>` «Устройство»: `Процессор` / `Видеокарта (NVIDIA)`; второй `disabled`, пока `pack !== 'ok'`; `!supported` — селекта нет. Рядом: «Установить GPU-пакет (≈230 МБ)» / «Отменить» / «Удалить GPU-пакет», `<progress>`, имя GPU и драйвер, при непустом `missingLibs` — предупреждение `vlGpuMissingLibs: 'Не найдены библиотеки CUDA: {libs}. Установите CUDA 13 и cuDNN 9'`. Выбор в `localStorage` (`vl.cloneDevice`, `try/catch`). Подсказка `vlCloneCpuNote` / `vlCloneGpuNote`. Ошибки: `GPU_FAILED: 'Не удалось запустить на видеокарте. Нужны драйвер NVIDIA 580+, CUDA 13 и cuDNN 9. Подробности — в логе'`, `GPU_PACK_MISSING: 'GPU-пакет не установлен'`, `GPU_PACK_INVALID: 'GPU-пакет повреждён — переустановите'`, `GPU_UNSUPPORTED: 'GPU поддерживается только на Linux x86_64'`, `DISK_FULL` (если ключа нет). `pack === 'broken'` → текст `GPU_PACK_INVALID`.
- [ ] **Step 2:** `npm run check`, `npm test`.
- [ ] **Step 3: приёмка (оркестратор):** `npm run tauri build`; размер AppImage до/после; из AppImage: установить пакет, клон «быстро» и «точно» на GPU, время в `docs/voice-lab.md` и в `vlCloneGpuNote`; `real_gpu_clone_fast` зелёный.
- [ ] **Step 4: docs.** `voice-lab.md`: GPU-клон, требования, пакет, время, `noexec` на `/home` ломает загрузку провайдера. `user-guide.md`: «Клонирование на видеокарте», строки в «Если что-то не так» про `GPU_FAILED` и ссылку на `debugging.md`. `architecture.md`: GPU-пакет, запуск с `arg0` и почему (ORT ищет провайдеры в `dirname(argv[0])`; провайдер pyke без sm_120; ABI ядра pyke и провайдера MS связаны версией 1.28.0).
- [ ] **Step 5: Commit** `feat(vl): GPU clone controls and docs`

---

Порядок выполнения: 1 → 2 → 3 → 4 → 5 (+gate) → 6 → 7.
