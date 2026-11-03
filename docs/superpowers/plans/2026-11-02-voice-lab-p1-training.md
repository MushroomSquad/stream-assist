# Voice Lab P1 — Training Studio Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Второй способ получить профиль голоса — GPU-обучение `style_ttl` по признакам WavLM-Large в управляемом Python-окружении, с прогрессом, графиком, остановкой/продолжением и чекпоинтами-профилями.

**Architecture:** Код тренажёра (MIT, ~400 строк) скопирован в `src-tauri/resources/trainer/upstream/`, адаптер `train_job.py` — свой цикл обучения. Все Python-файлы и `pyproject.toml`/`uv.lock` встроены в бинарник (`include_str!`) и пишутся в `<trainer>/adapter/`. Rust строит venv через `uv sync --frozen` (Python и кэш внутри каталога пакета) и качает веса WavLM через `install_files` P0. Обучение — дочерний Python-процесс с JSONL в stdout; остановка = закрытие stdin. Прогоны — таблица `training_runs`.

**Tech Stack:** как P0 + `uv` (внешний), Python 3.12 (управляемый uv), torch/torchaudio/torchvision cu128, transformers, onnx, onnx2torch, onnxslim, onnxruntime, soundfile, numpy.

**Spec:** `docs/superpowers/specs/2026-11-02-voice-lab-p1-training.md` (+ P0 spec)

**Предусловие:** P0 выполнен.

## Global Constraints

- Ограничения P0 действуют (музыкальный код не трогать, без shell, `CREATE_NO_WINDOW`, коды → `ru.ts`, лок `Store` не через `.await`, без транскриптов/тензоров в логах).
- `TRAINER_REV = "84a6f4c220cd7cc57dce14f638d43e13fa777f42"` — ревизия скопированного кода (строка в заголовке каждого файла копии).
- `WAVLM_REV = "c1423ed94bb01d80a3f5ce5bc39f6026a0f4828c"`; файлы `config.json`, `preprocessor_config.json`, `pytorch_model.bin` из `https://huggingface.co/microsoft/wavlm-large/resolve/<rev>/<file>` → `<trainer>/weights/wavlm-large/`; SHA-256 и размеры — константы `WAVLM_FILES: &[ModelFile]` (Task 1).
- Каталог пакета `<root>/trainer/` (`adapter/`, `.venv/`, `python/`, `uv-cache/`, `weights/`, `installed.ok`); прогоны `<root>/training/<run_id>/`.
- `uv sync --frozen --no-config --project <adapter>` с `UV_PYTHON_DOWNLOADS=automatic`, `UV_PROJECT_ENVIRONMENT=<trainer>/.venv`, `UV_PYTHON_INSTALL_DIR=<trainer>/python`, `UV_PYTHON_PREFERENCE=only-managed`, `UV_CACHE_DIR=<trainer>/uv-cache`; прочее окружение пользователя сохраняется.
- Python-прогон: `<trainer>/.venv/bin/python -I <adapter>/train_job.py <request.json>`; окружение очищено до `PATH`, `HOME`, `CUDA_VISIBLE_DEVICES` (если задана), `HF_HUB_OFFLINE=1`, `TRANSFORMERS_OFFLINE=1`; `cwd` = каталог прогона; stderr → `File(<run>/worker.log)`; Unix — `process_group(0)`; без `kill_on_drop`.
- Один прогон одновременно (`JOB_BUSY`); установка/удаление пакета во время прогона → `JOB_BUSY`; P0-клон и синтез не блокируются.
- Регион 3000–30000 мс, согласие обязательно (`NO_CONSENT`). Пресеты: fast 1000 / standard 3000 / long 6000 шагов; lr 2e-4; vocoder_steps 6; save_every 500; seed 42; порог 0.24.
- Коды из воркера: белый список `GPU_UNAVAILABLE, GPU_OUT_OF_MEMORY, CUDA_DRIVER_MISMATCH, TRAINING_STATE_CORRUPTED, JOB_BUSY`, иначе `WORKER_FAILED`. Новые коды в `ru.ts`: `UV_MISSING`, `UV_TOO_OLD`, `TRAINER_NOT_INSTALLED`, `TRAINER_OUTDATED`, `TRAINER_INSTALL_FAILED` + пять из белого списка.

## Review Focus

1. **Закрыли приложение посреди обучения** → Python получает EOF, сохраняет `state.pt`, выходит; на GPU не остаётся процессов; после старта прогон `interrupted`, продолжается с того же шага. Тесты `eof_triggers_stop` (Task 2), `startup_marks_running_interrupted` (Task 3), `resume_continues_step` (Task 5).
2. **Нет `uv`/CUDA/драйвера, OOM, torch без sm_120** → понятный код, P0 работает. Тесты `find_uv_fallbacks`, `gpu_check_before_import` (Task 2/4), `oom_event_fails_run` (Task 5).
3. **Второй процесс на тот же прогон** (старый Python ещё дописывает после EOF, пользователь жмёт «Продолжить») → `JOB_BUSY` через `flock`. Тест `lock_rejects_second_process` (Task 2).
4. **Остановили, а процесс не выходит** → через 30 с kill; после «Остановить» любой выход (и kill по таймауту) даёт `stopped`; «Прервать» без остановки → `failed` (продолжаемый). Тесты `stop_timeout_kills`, `kill_without_stop_is_failed` (Task 5).
5. **Обновили приложение** → пакет помечен `outdated`, обучение не стартует со старым venv. Тест `outdated_when_lock_hash_differs` (Task 4).

---

## File Structure

```
src-tauri/resources/trainer/
  pyproject.toml, uv.lock
  train_job.py                 # адаптер: протокол, проверки GPU, цикл, find_closest_style, RU-фразы
  test_train_job.py            # unittest без torch
  upstream/{LICENSE, helper.py, utils_model.py, utils_dataloader.py, utils_style.py, wavlm_loss.py}
src-tauri/src/voice_lab/
  trainer.rs                   # константы, встроенные файлы, установка, диагностика
  train_runner.rs              # прогоны: процесс, события, статусы
  runner.rs                    # вынести pub(super) next_line()
  store.rs                     # MIGRATIONS, training_runs
  commands.rs, mod.rs
src-tauri/examples/vl_fake_trainer.rs
src-tauri/tests/voice_lab_train.rs        # тесты TrainRunner (ищут фейки в target/<profile>/examples)
src-tauri/tests/voice_lab_trainer_gpu.rs   # #[ignore]
src/lib/views/voice/Train.svelte, src/lib/voiceApi.ts, src/lib/i18n/ru.ts
docs/voice-lab.md
```

---

### Task 1: Спайк — копия кода, окружение, веса, замер

**Files:** Create `src-tauri/resources/trainer/{pyproject.toml,uv.lock,upstream/*}`, `src-tauri/src/voice_lab/trainer.rs` (константы); Modify `voice_lab/mod.rs` (`pub mod trainer;`)

**Interfaces — Produces:** `pub const TRAINER_REV: &str`; `pub const WAVLM_REV: &str`; `pub const WAVLM_FILES: &[ModelFile]` (`path` — имя файла); `pub const MIN_UV: (u32, u32, u32)` (версия uv, которой сгенерирован lock).

- [x] **Step 1:** Скопировать из апстрима `TRAINER_REV` в `upstream/`: `pipeline/helper.py` → `helper.py`; `pipeline/utils/model.py` → `utils_model.py` **без** блока `httpx`/`_NoVerifyClient`/удаления `SSL_*` переменных; `utils/dataloader.py` → `utils_dataloader.py`; `utils/style.py` → `utils_style.py`; из `utils/loss.py` только `WAVLM_LAYER`, `extract_wavlm_targets`, `wavlm_loss` → `wavlm_loss.py`; `LICENSE`. В каждом файле первая строка `# Copied from Saganaki22/supertonic_embeddings_trainer@<rev> (MIT). Changes: <что изменено>`. Внутренние импорты поправить на плоские имена.
- [x] **Step 2:** `pyproject.toml`: `[project] requires-python = "==3.12.*"`, зависимости с точными версиями `torch`, `torchaudio`, `torchvision` (тройка из одного релиза cu128), `transformers`, `onnx`, `onnx2torch`, `onnxslim`, `onnxruntime`, `soundfile`, `numpy`; `[tool.uv] package = false`, `environments = ["sys_platform == 'linux' and platform_machine == 'x86_64'"]`; `[[tool.uv.index]] name = "pytorch-cu128"`, `url = "https://download.pytorch.org/whl/cu128"`, `explicit = true`; `[tool.uv.sources]` для трёх torch-пакетов → этот индекс. `uv lock` → `uv.lock`; `MIN_UV` = `uv --version`.
- [x] **Step 3:** Скачать три файла WavLM ревизии `WAVLM_REV`, снять SHA-256/размеры → `WAVLM_FILES`.
- [x] **Step 4:** Во временном каталоге с переменными из Global Constraints: `uv sync --frozen`; `python -c` → `torch.cuda.is_available()==True`, `torch.version.cuda` 12.8, пробное ядро на GPU выполняется. Иначе остановиться и доложить.
- [x] **Step 5:** (предусловие: модель P0 установлена; одноразовый скрипт в scratchpad, не в репо) **Офлайн** (`HF_HUB_OFFLINE=1 TRANSFORMERS_OFFLINE=1`): `WavLMModel.from_pretrained(<weights>)`, `SupertonicModel(<model-cache>/onnx)` из копии, 50 шагов цикла на WAV 5–10 с. Записать: размер `.venv`+`python`+`uv-cache`, время 50 шагов, пик VRAM (`torch.cuda.max_memory_allocated`), время конвертации onnx2torch.
- [x] **Step 6:** Commit `chore(voice-lab): vendored trainer code and pinned environment`.

**Task 1 spike (2026-11-03):** upstream проверен на `TRAINER_REV`; uv 0.10.4,
Python 3.12.12, torch/torchaudio 2.7.1+cu128 и torchvision 0.22.1+cu128.
`uv sync --frozen --no-config` выполнен в scratchpad/p1-spike с управляемыми
Python/venv/cache; CUDA 12.8, `sm_120` и пробное GPU-ядро — PASS
(RTX 5070 Ti 16 GB, драйвер 615.78.08).
Офлайн WavLM + Supertonic: 50 шагов Adam/style_ttl (lr 2e-4, vocoder_steps 6,
seed 42, 16 английских фраз апстрима, начальный стиль M1), регион реальной
`luna_reference.wav` 7.534 с после trim/peak, без ECAPA/ранней остановки:
**17.650 с**, пик `torch.cuda.max_memory_allocated` **4 859 623 424 байт
(4.526 ГиБ)**; конвертация четырёх ONNX через onnx2torch **10.273 с**.
Loss/градиенты конечны; best loss 1.06504. После выхода Python на GPU не остался.
Размер `.venv` + `python` + `uv-cache`: **7 120 588 800 байт (6.632 ГиБ)**
(`du -sB1 -c`, общие hardlink-файлы учитываются один раз; веса WavLM отдельно).
Тест контракта констант сначала FAIL (модуля нет), затем PASS; полный `cargo test`:
238 passed, 0 failed, 7 ignored. `cargo clippy --all-targets`: успешно,
предупреждений в voice_lab нет (существующие предупреждения в других модулях).
Отклонения: Task 1 не перечисляет тесты — добавлен один тест контракта констант
для требуемого TDD; упомянутый workflow superpowers недоступен, шаги выполнены напрямую.
Одноразовый скрипт, логи, веса и окружение оставлены только в scratchpad/p1-spike.

### Task 2: `train_job.py`

**Files:** Create `src-tauri/resources/trainer/{train_job.py,test_train_job.py}`

**Interfaces — Produces:**
- Запрос: `{"protocolVersion":1,"upstreamDir","modelDir","wavlmDir","runDir","lang","totalSteps","lr","saveEvery","seed","vocoderSteps":6,"threshold":0.24,"resume":bool}`; референс — `runDir/reference.wav`.
- События (stdout-протокол): `started{resumedFrom}`, `progress{step,total,loss,bestLoss,lr}` (каждые 8 шагов), `checkpoint{step,stylePath,bestLoss}`, `done{step,stylePath,bestLoss}`, `stopped{step,stylePath|null}`, `error{code}`. Нефинитные числа → `null`. Exit 0 для `done`/`stopped`/ранний выход, 1 для `error`.

Устройство `main(argv) -> int`:
1. Первым делом: `proto = os.fdopen(os.dup(1), "w", buffering=1); os.dup2(2, 1)`; `emit` пишет только в `proto`.
2. `sys.path[:0] = [dirname(abspath(__file__)), req["upstreamDir"]]`. Unix: `os.nice(10)`.
3. `flock(runDir/lock, LOCK_EX|LOCK_NB)` → занято → `error JOB_BUSY` (на не-Unix — `msvcrt.locking` или пропуск блокировки; код не должен падать на импорте `fcntl`).
4. Поток-сторож stdin: EOF → `stop_event.set()` и daemon-таймер `t = threading.Timer(60, os._exit, (1,)); t.daemon = True; t.start()` (страховка: зависшее сохранение не держит GPU после выхода приложения); если цикл ещё не начался — `emit stopped{step: resumedFrom или 0, stylePath: null}`, flush, `os._exit(0)`. `emit` и флаг `loop_started` — под одним `threading.Lock`: сторож проверяет флаг под локом, главный поток выставляет его под тем же локом.
5. `import torch`; нет CUDA → `GPU_UNAVAILABLE`; пробное ядро `(torch.ones(1, device="cuda") + 1).item()`, `RuntimeError` (в т.ч. «no kernel image») → `CUDA_DRIVER_MISMATCH`; `torch.cuda.set_per_process_memory_fraction(min(0.75, max(0.1, (free − 2 ГиБ)/total)))`; `torch.set_num_threads(4)`; `np.random.seed(seed)`, `torch.manual_seed(seed)`.
6. Референс: `soundfile.read` → моно → trim по порогу −20 дБ от пика (`abs > peak·10^(−20/20)`, первая/последняя выборка) → пик 0.95 → `runDir/reference_prepared.wav`.
7. Тексты: `RU_TEXTS` (16 строк в этом файле) при `lang=="ru"`, иначе 16 английских фраз апстрима (скопировать в этот же файл как `EN_TEXTS`).
8. Модели: `SupertonicModel(modelDir/onnx)`, `WavLMModel.from_pretrained(wavlmDir).to(device).eval()` (параметры `requires_grad_(False)`).
9. Старт: `resume` и есть `state.pt` → `torch.load(..., map_location=device, weights_only=True)`; ошибка → `TRAINING_STATE_CORRUPTED`; ключи `step, best_loss, best_ttl, best_dp, style_ttl, style_dp, noisy_fixed, lmask, optimizer_state, scheduler_state, total_steps, seed`; `total_steps`/`seed` ≠ запроса → `TRAINING_STATE_CORRUPTED`; пропустить `step % len(texts)` элементов даталоадера. Иначе `find_closest_style(wavlm, …)` (своя версия, принимает загруженный WavLM, стили из `modelDir/voice_styles/[FM]*.json`), `noisy_fixed/lmask` из `sample_noisy_latent`, Adam(lr), `ReduceLROnPlateau(patience=200, factor=0.5, min_lr=lr·0.01)`.
10. Цикл (как `app.py::train_voice`, без ECAPA): `clip_grad_norm_(1.0)`, best по шагу, `scheduler.step(best_loss)`; каждые 8 — `progress`; каждые `saveEvery` — `ckpt_<step>.json` + `state.pt` (оба через `*.tmp` + `os.replace`) + `checkpoint`; `stop_event` → если `best_ttl is None` → `stopped{stylePath:null}` без записи, иначе state + `final.json` + `stopped`; `best_loss ≤ threshold` или конец → `final.json` + `done`.
11. Исключения: `torch.cuda.OutOfMemoryError` → `GPU_OUT_OF_MEMORY`; прочее → `WORKER_FAILED` (трейсбек в stderr).

- [x] **Step 1: Тесты** (`python3 -m unittest discover -s src-tauri/resources/trainer -p 'test_*.py'`, без torch: торч-зависимые части импортируются внутри функций): `parse_request_rejects_bad_version`, `eof_triggers_stop` (поток-сторож на pipe, цикл уже начался → event, не выход; после `stopped` процесс завершается < 5 с — daemon-таймер не держит), `eof_before_loop_emits_stopped` (в подпроцессе с заглушкой `torch` в `sys.modules`, импорт которой спит 5 с: EOF до цикла → строка `stopped` с `step:0`, exit 0), `emit_sanitizes_nonfinite`, `write_atomic_replaces`, `trim_and_peak` (синтетический массив numpy: тишина–сигнал–тишина → обрезка, пик 0.95), `lock_rejects_second_process` (два `flock` на один файл в подпроцессах), `gpu_check_before_import` (подменённый `torch` в `sys.modules`: `cuda.is_available()=False` → `GPU_UNAVAILABLE`; пробное ядро бросает `RuntimeError` → `CUDA_DRIVER_MISMATCH`), `texts_16_each`, `error_code_mapping`.
- [x] **Step 2–4:** FAIL → реализовать → PASS.
- [x] **Step 5:** GPU вручную (venv из Task 1, офлайн): 60 шагов, `saveEvery=50` → `checkpoint` на 50; закрыть stdin на ~30 → `stopped`, `state.pt` есть; повтор с `resume:true` → `started.resumedFrom≈30`, `done`; подменить `state.pt` мусором → `TRAINING_STATE_CORRUPTED`. Записать вывод.
- [x] **Step 6:** Commit `feat(voice-lab): trainer adapter`.

**Task 2 verification (2026-11-03):** TDD: первый запуск unittest — FAIL
(`train_job.py` отсутствует), после реализации — все 10 тестов PASS через
`python3 -I -m unittest discover -s src-tauri/resources/trainer -p 'test_*.py'`,
без torch в системном Python. Офлайн GPU smoke в окружении Task 1, русский
референс 7.534 с, 60 шагов, saveEvery=50, threshold=0:
- Полный прогон: started(0), checkpoint(50), done(60), bestLoss=0.455925,
  exit 0, 57.47 с вместе с загрузкой моделей и поиском ближайшего стиля.
- EOF после progress(32): stopped(33), state.pt и final.json сохранены,
  exit 0; затем resume: started(resumedFrom=33), checkpoint(50), done(60),
  bestLoss=0.482922, exit 0 (26.41 с).
- Мусор в state.pt: error(TRAINING_STATE_CORRUPTED), exit 1 (3.44 с).
- В stdout только JSONL, стили имеют ожидаемые формы и конечные значения;
  после завершения процессов адаптера на GPU нет.
- Полный cargo test: 238 passed, 0 failed, 7 ignored;
  cargo clippy --all-targets: успешно, предупреждений в voice_lab нет.
Артефакты GPU-проверки: scratchpad/task2-smoke (вне репозитория).
Отклонение: workflow superpowers недоступен, шаги выполнены напрямую.

### Task 3: store — миграции и `training_runs`

**Files:** Modify `src-tauri/src/voice_lab/store.rs`

**Interfaces — Produces:**
- `const MIGRATIONS: &[&str]` — индекс + 1 = версия схемы. Миграция 1 = текущая DDL P0 (с `IF NOT EXISTS`, без `PRAGMA user_version`); миграция 2 = `CREATE TABLE IF NOT EXISTS training_runs(...)`. `open`: прочитать `user_version`; `> MIGRATIONS.len()` → `IO_FAILED`; `PRAGMA foreign_keys=ON` до `BEGIN`; применить недостающие в одной транзакции (миграции применяются строго по версии, поэтому `ALTER TABLE` в будущих шагах не обязан быть идемпотентным), выставить `user_version = MIGRATIONS.len()`; затем `UPDATE training_runs SET status='interrupted' WHERE status='running'`; `create_dir_all("training")`. Будущие фазы дописывают свои миграции в конец.
- Схема: `training_runs(id INTEGER PRIMARY KEY, source_id INTEGER REFERENCES sources(id) ON DELETE SET NULL, start_ms INTEGER, end_ms INTEGER, lang TEXT, params_json TEXT NOT NULL, status TEXT NOT NULL, step INTEGER DEFAULT 0, total INTEGER, best_loss REAL, history_json TEXT DEFAULT '[]', error TEXT, created_at INTEGER, updated_at INTEGER)`.
- `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] #[serde(rename_all="camelCase")] pub struct TrainParams { pub preset: String /*fast|standard|long*/, pub total_steps: u32, pub lr: f64, pub save_every: u32, pub seed: u64, pub threshold: f64 /*0.24*/ }`
- `#[derive(Serialize, Clone, Debug)] #[serde(rename_all="camelCase")] pub struct TrainRun { pub id: i64, pub source_id: Option<i64>, pub start_ms: i64, pub end_ms: i64, pub lang: String, pub params: TrainParams, pub status: String, pub step: i64, pub total: i64, pub best_loss: Option<f64>, pub history: Vec<[f64; 3]>, pub error: Option<String>, #[serde(skip_deserializing)] pub lr: Option<f64> /*последний lr, только в памяти, не в БД*/, pub created_at: i64, pub updated_at: i64 }`
- `create_run(source_id, start_ms, end_ms, lang, params) -> TrainRun` (`running`); `run(id)`; `runs()`; `set_run_status(id, status, error: Option<&str>)`; `set_run_step(id, step)`; `add_run_point(id, step, loss: f64, best: f64)` (точки только с конечными числами); `truncate_run_history(id, max_step)`; `set_run_best(id, best: f64)`; `delete_run(id)` (+ `remove_dir_all(run_dir)`); `run_dir(id) -> PathBuf`.
- [ ] **Step 1: Тесты:** `migration_from_v1_keeps_data` (БД с `user_version=1` и профилем → open → профиль цел, `training_runs` есть, версия 2), `fresh_db_gets_latest_version`, `future_version_rejected`, `startup_marks_running_interrupted`, `history_points_and_truncate`, `delete_run_keeps_profiles`; обновить ассерт версии в `open_clears_tmp` на `MIGRATIONS.len()`.
- [ ] **Step 2–4:** `cd src-tauri && cargo test voice_lab::store` → сначала FAIL, после реализации все тесты store PASS; затем весь `cargo test` зелёный. **Step 5:** Commit `feat(voice-lab): schema migrations and training runs`.

### Task 4: trainer.rs — установка и диагностика

**Files:** Modify `src-tauri/src/voice_lab/trainer.rs`

**Interfaces — Consumes:** константы Task 1; `models::install_files` (P0) и `ModelFile`.
**Produces:**
- `pub const EMBEDDED: &[(&str, &str)]` — `("pyproject.toml", include_str!(…))`, `("uv.lock", …)`, `("train_job.py", …)`, `("upstream/LICENSE", …)`, `("upstream/helper.py", …)`, … (тестовый файл не встраивается).
- `pub fn trainer_dir(root) -> PathBuf`, `pub fn venv_python(root) -> PathBuf`, `pub fn write_adapter(root) -> Result<(), VlError>` (пишет `EMBEDDED` в `adapter/`, через tmp + rename).
- `pub fn lock_hash() -> String` (sha256 встроенного `uv.lock`); `pub fn pack_state(root) -> &'static str` — `"none"` (нет `installed.ok`), `"ok"` (содержимое = `lock_hash()` и все `WAVLM_FILES` на месте), `"outdated"`.
- `pub fn find_uv(path_var: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf>` — `PATH`, затем `home/.local/bin/uv`, `home/.cargo/bin/uv`.
- `pub fn parse_uv_version(s: &str) -> Option<(u32,u32,u32)>`; `pub fn parse_nvidia_smi(csv: &str) -> Option<GpuInfo>`.
- `#[derive(Serialize)] #[serde(rename_all="camelCase")] pub struct TrainStatus { pub uv: Option<String>, pub uv_ok: bool, pub pack: String, pub gpu: Option<GpuInfo>, pub torch_cuda: Option<String>, pub disk_free_mb: Option<u64> }`; `GpuInfo { pub name, pub driver, pub vram_total_mb: u64, pub vram_free_mb: u64 }`.
- `pub async fn status(root, cache: &std::sync::Mutex<Option<Option<String>>>, refresh: bool) -> TrainStatus` (лок только до/после `await`; `refresh` сбрасывает кэш) — внешние вызовы `tokio::process` с таймаутом 10 с (`uv --version`, `nvidia-smi …`); `torch_cuda` — `python -I -c "import torch;print(torch.version.cuda if torch.cuda.is_available() else '')"` (таймаут 60 с) только при `pack == "ok"`, результат кэшируется в `cache` до install/uninstall; `disk_free_mb` — `libc::statvfs` (unix), иначе `None`.
- `pub async fn install(root, progress: &(dyn Fn(&str, u64, u64) + Send + Sync), cancel: oneshot::Receiver<()>) -> Result<(), VlError>` — нет uv → `UV_MISSING`, версия < `MIN_UV` → `UV_TOO_OLD`; удалить `installed.ok`; `write_adapter`; свободно < 9000 МБ (если известно) → `TRAINER_INSTALL_FAILED` до начала; стадия `venv` — `uv sync --frozen --no-config …` (переменные из Global Constraints + `UV_PYTHON_DOWNLOADS=automatic`, `kill_on_drop`, отмена → kill → `JOB_CANCELLED`); стадия `weights` — `select!` по `install_files(WAVLM_FILES → weights/wavlm-large/, tmp = <trainer>/tmp, прогресс-адаптер |d,t| progress("weights", d, t))` и `cancel` (→ `JOB_CANCELLED`, `<trainer>/tmp` удаляется; ошибки → `TRAINER_INSTALL_FAILED`); стадия `venv` сообщает `progress("venv", 0, 1)` в начале и `(1, 1)` в конце; записать `installed.ok` = `lock_hash()`. Любая ошибка стадии → `TRAINER_INSTALL_FAILED`.
- `pub fn uninstall(root) -> Result<(), VlError>` — `remove_dir_all(trainer_dir)`.
- [ ] **Step 1: Тесты:** `parse_nvidia_smi_line` (`"NVIDIA GeForce RTX 5070 Ti, 580.95, 16303, 15800"`), `parse_nvidia_smi_empty`, `parse_uv_version_line` (`"uv 0.10.4 (…)"`), `find_uv_fallbacks` (временный HOME с `.local/bin/uv`, пустой PATH), `outdated_when_lock_hash_differs`, `pack_none_without_marker`, `write_adapter_writes_embedded`.
- [ ] **Step 2–4:** `cd src-tauri && cargo test voice_lab::trainer` → FAIL, затем PASS; весь `cargo test` зелёный. **Step 5:** `#[ignore]` `install_real_pack` (сеть, временный root): `pack == "ok"`, `torch_cuda` = `"12.8"`. **Step 6:** Commit `feat(voice-lab): trainer pack install and diagnostics`.

### Task 5: train_runner.rs — прогоны

**Files:** Create `src-tauri/src/voice_lab/train_runner.rs`, `src-tauri/examples/vl_fake_trainer.rs`, `src-tauri/tests/voice_lab_train.rs`; Modify `runner.rs` (вынести чтение строк), `mod.rs`

**Interfaces — Consumes:** `Store::{create_run, run, set_run_status, set_run_step, add_run_point, truncate_run_history, set_run_best, run_dir, source, source_work_path, put_profile}`, `read_style_file`, `audio::{validate_range, read_segment, wav_bytes}`, `trainer::{venv_python, trainer_dir, write_adapter}`, `models::model_dir`.
**Produces:**
- В `runner.rs`: `pub(super) async fn next_line<R: AsyncBufRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>, VlError>` (лимит 65536 → `WORKER_CRASHED`; на EOF возвращает непустой хвост без `\n`; буфер по `&mut` переживает отмену в `select!`); `run_worker` переходит на неё без изменения поведения; тест `next_line_returns_trailing_partial`.
- `#[derive(Deserialize, Debug)] #[serde(tag="event", rename_all="lowercase", rename_all_fields="camelCase")] pub enum TrainEvent { Started { resumed_from: u32 }, Progress { step: u32, total: u32, loss: Option<f64>, best_loss: Option<f64>, lr: Option<f64> }, Checkpoint { step: u32, style_path: PathBuf, best_loss: Option<f64> }, Done { step: u32, style_path: PathBuf, best_loss: Option<f64> }, Stopped { step: u32, style_path: Option<PathBuf> }, Error { code: String } }`
- `pub fn train_code(code: &str) -> &'static str` — белый список из Global Constraints, иначе `WORKER_FAILED`.
- `pub struct TrainRunner { python: PathBuf, script: PathBuf, extra_env: Vec<(String,String)>, stop_grace: Duration, active: Mutex<Option<Active { run_id: i64, stdin: Option<ChildStdin>, kill: Arc<tokio::sync::Notify>, stop_requested: Arc<AtomicBool> }>> }`; `TrainRunner::new(python, script, extra_env /*в проде пусто, добавляется после env_clear*/, stop_grace /*30 с*/)`.
- `pub fn start(self: &Arc<Self>, store: Arc<StdMutex<Store>>, root: PathBuf, source_id: i64, start_ms: u64, end_ms: u64, lang: String, params: TrainParams, emit: Arc<dyn Fn(&TrainRun) + Send + Sync>) -> Result<i64, VlError>` — `JOB_BUSY`, `NO_CONSENT`, `validate_range(.., 3000, 30000)`; `create_run`; `read_segment` → `run_dir/reference.wav` (ошибка → `set_run_status(failed, code)`, слот свободен, ошибка наружу); `request.json` по схеме Task 2 (`protocolVersion:1`, `upstreamDir = <trainer>/adapter/upstream`, `modelDir = models::model_dir(root)`, `wavlmDir = <trainer>/weights/wavlm-large`, `runDir`, `lang`, `totalSteps/lr/saveEvery/seed/threshold` из `TrainParams`, `vocoderSteps: 6`, `resume`); spawn.
- `pub fn resume(self: &Arc<Self>, store: Arc<StdMutex<Store>>, root: PathBuf, run_id: i64, emit: Arc<dyn Fn(&TrainRun) + Send + Sync>) -> Result<(), VlError>` — для `stopped|interrupted|failed`; статус → `running`, `error` очищается; `request.json` пересобирается; `resume = state.pt существует` (битый state снова даст `TRAINING_STATE_CORRUPTED` — безвредно).
- `pub fn stop(&self, run_id) -> Result<(), VlError>` — `stop_requested=true`, `stdin.take()` (EOF), `tauri::async_runtime::spawn(sleep(stop_grace) → kill.notify_one())`; `pub fn kill(&self, run_id)` — только `kill.notify_one()` (флаг ставит лишь `stop()`; «Прервать» без остановки → `failed`, его можно продолжить); `pub fn active(&self) -> Option<i64>`.
- Задача прогона: `write_adapter`; `Command(python).arg("-I").arg(script).arg(request)` с окружением из Global Constraints; stdin piped (хранится в `Active`), stdout piped, stderr → `File`; Unix `process_group(0)`, kill = `libc::killpg(pid, SIGKILL)`; Windows `start_kill`, `CREATE_NO_WINDOW|BELOW_NORMAL`. События: `Started` → `set_run_step(resumedFrom)` + `truncate_run_history`; `Progress` → `add_run_point` + `set_run_best` + запомнить `lr` в эмитируемом `TrainRun` + emit (≤ 1/с); `Checkpoint`/`Done` → `read_style_file` → `put_profile(origin="train", name="<source.name> · шаг N", meta={runId, step, bestLoss, trainerRev})`; `Done` → `completed`; `Stopped` → `set_run_step` только если `step > 0` + `stopped` (стиль остановки профилем не становится — профили только из чекпоинтов и итога; осознанно); `Error{JOB_BUSY}` → статус возвращается к прежнему (`interrupted|stopped|failed`), `error = JOB_BUSY`; прочие `Error` → `failed` + `train_code`. Задача ждёт в `select!` строку stdout или `kill.notified()` → `killpg` (unix) / `start_kill`, затем `wait()`; после `wait()` kill больше не вызывается (pgid мог переиспользоваться). Выход процесса без финального события: `stop_requested` → `stopped`; kill по «Прервать» → `failed JOB_CANCELLED`; иначе `failed WORKER_CRASHED`. Слот освобождается RAII-guard'ом (как `JobGuard` P0); финальный emit всегда.
- `vl_fake_trainer.rs` (вызывается как `<fake> -I <script> <request>` — берёт последний аргумент): режимы `VL_FAKE_MODE` (через `extra_env`): `busy` (`error JOB_BUSY`), `slowstart` (5 с без событий, EOF → `stopped{step:0}`), `ok` (started, 3 progress, checkpoint с валидным стилем, done), `stoppable` (progress каждые 100 мс; EOF stdin → пишет `state.pt`, stopped), `hang` (игнорирует EOF), `oom` (`error GPU_OUT_OF_MEMORY`), `weird` (`error FOO`), `crash` (exit 3), `resume` (печатает `started{resumedFrom: N}`, где N из `state.pt` если `resume:true`).
- [ ] **Step 1: Тесты:** `run_ok_creates_checkpoint_profiles`, `stop_saves_and_marks_stopped`, `stop_timeout_kills` (`stop_grace` 1 с, `hang`), `oom_event_fails_run`, `unknown_code_is_worker_failed`, `crash_fails_run`, `second_run_busy`, `resume_continues_step`, `resume_without_state_restarts`, `resume_after_oom`, `busy_lock_keeps_resumable`, `stop_before_loop_is_stopped` (`slowstart`), `kill_without_stop_is_failed`, `no_consent_rejected`, `region_bounds_3_30s`, `clone_and_train_run_together` (P0 `Runner` с фейком и `TrainRunner` с фейком одновременно).
- [ ] **Step 2–4:** `cd src-tauri && cargo build --example vl_fake_worker --example vl_fake_trainer && cargo test --test voice_lab_train` → FAIL, затем все PASS; `cargo test voice_lab::runner` (next_line) PASS; весь `cargo test` зелёный. **Step 5:** Commit `feat(voice-lab): training runner`.

### Task 6: Команды и UI «Обучение»

**Files:** Modify `voice_lab/mod.rs` (`VoiceLab` + `train: Arc<TrainRunner>`, `train_install: tokio::sync::Mutex<Option<oneshot::Sender<()>>>`, `torch_cache`), `commands.rs`, `lib.rs`, `src/lib/voiceApi.ts`, `src/lib/i18n/ru.ts`, `src/lib/views/voice/VoiceLab.svelte`; Create `src/lib/views/voice/Train.svelte`

**Interfaces — Produces:** `vl_train_status(refresh: bool) -> TrainStatus`; `vl_train_install()` (прогресс `vl-train-install {stage,done,total}` ≤ 1/200 мс + финальное; повтор → `JOB_BUSY`; активный прогон → `JOB_BUSY`); `vl_train_install_cancel()`; `vl_train_uninstall()` (активный прогон/установка → `JOB_BUSY`); `vl_train_start(sourceId, startMs, endMs, lang, preset, seed) -> i64` (`pack != ok` → `TRAINER_NOT_INSTALLED`/`TRAINER_OUTDATED`; модель P0 обязательна → `MODEL_MISSING`); `vl_train_resume(runId)` (те же проверки пакета и модели, что у start); `vl_train_stop(runId)`; `vl_train_kill(runId)`; `vl_train_runs() -> Vec<TrainRun>`; `vl_train_delete(runId)` (активный → `JOB_BUSY`). Событие `vl-train` = `TrainRun`. Пресет → `TrainParams` на стороне Rust.

- [ ] **Step 1:** Команды + тесты логики: `start_requires_pack`, `resume_requires_pack`, `delete_active_run_busy`, `uninstall_during_run_busy`.
- [ ] **Step 2:** `Train.svelte`: диагностика (uv и версия, состояние пакета, GPU, драйвер, VRAM, CUDA torch, диск; «Установить пакет обучения (≈ 7 ГБ)» со стадиями и «Отмена», «Обновить пакет» при `outdated`, «Удалить пакет»); постоянное предупреждение «Обучение загружает GPU полностью — не запускайте во время стрима», жёлтое при свободной VRAM < 6 ГБ; форма (источник/регион из «Записей», 3–30 с, язык, пресет, seed); активный прогон: шаг/всего, loss, best, lr, ETA по скорости последних точек, SVG-polyline loss и best по `history`, «Остановить», «Прервать» (с подтверждением); список прогонов, «Продолжить» для `stopped|interrupted|failed`, «Удалить».
- [ ] **Step 3:** `ru.ts`: строки и коды. `npm run check` — 0 ошибок.
- [ ] **Step 4:** Commit `feat(panel): voice lab training tab`.

### Task 7: Документация и приёмка на GPU

**Files:** Modify `docs/voice-lab.md`, `docs/architecture.md`, `README.md`; Create `src-tauri/tests/voice_lab_trainer_gpu.rs`

- [ ] **Step 1:** `#[ignore]` `gpu_train_stop_resume`: реальный пакет и модель, регион 10 с, 1000 шагов, `saveEvery=100`, `threshold=0` (без ранней остановки); стоп на ~300; новый `TrainRunner` (имитация перезапуска); resume; `done`; ≥ 2 профиля `train`; во время прогона `nvidia-smi` — пик VRAM, `ps -o rss` Python — пик RAM, общее время; после — нет процессов Python на GPU.
- [ ] **Step 2:** Приёмка из спеки P1 в `npm run tauri dev`, включая закрытие приложения посреди обучения и `nvidia-smi` после.
- [ ] **Step 3:** Документация: `uv` (установка, путь), размер пакета, что обучается только `style_ttl`, советы по региону, лицензии (код тренажёра MIT — копия, WavLM — MIT), предупреждение про стрим.
- [ ] **Step 4:** `cargo test`, `npm run check`, unittest адаптера — зелёные. Commit `docs: voice lab training`.
