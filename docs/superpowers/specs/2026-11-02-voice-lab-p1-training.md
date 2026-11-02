# Voice Lab — P1: Training Studio (GPU-тренажёр профилей)

Дата: 2026-11-02
Источник: SVL-SPEC-001, §2 P1, §5 Backend B, §13–14, фаза 4. Опирается на Voice Lab v1 (P0):
`docs/superpowers/specs/2026-11-02-voice-lab-design.md` — модуль `voice_lab`, store, runner, вкладка «Голоса».

## Зачем

CPU-клон (P0) ищет стиль перебором и занимает минуты–десятки минут. Тренажёр
`Saganaki22/supertonic_embeddings_trainer` оптимизирует `style_ttl` градиентным спуском через
PyTorch-копию модели на GPU по признакам WavLM-Large — по данным автора 5–6 мин на v3 при ~8 ГБ VRAM.
Нужен как второй способ получить профиль, с прогрессом, остановкой и продолжением.

## Объём P1

1. «Пакет обучения»: управляемое Python-окружение (через `uv`, Python и кэш внутри каталога пакета)
   + веса WavLM-Large. Ставится по явному согласию, удаляется одной кнопкой целиком.
2. Диагностика: есть ли `uv`, NVIDIA-драйвер, CUDA в torch, свободная VRAM, место на диске.
3. Запуск обучения из региона записи: пресет шагов, seed.
4. Прогресс: шаг, loss, best loss, lr, ETA; график loss.
5. Остановка с сохранением состояния; продолжение с того же шага; принудительное завершение.
6. Чекпоинты каждые N шагов → отдельные неизменяемые профили (`origin=train`) в библиотеке.
7. Перезапуск приложения посреди обучения → прогон `interrupted`, можно продолжить.
8. Без GPU/пакета всё P0 работает; вкладка обучения показывает, чего не хватает.

## Не цели P1

- Совместное обучение на нескольких записях, обучение `style_dp` (тренажёр держит DP замороженным).
- Распространение torch-окружения внутри установщика (PyInstaller) — только управляемый venv.
- Пакетный прогон «10 вариантов разом» — прогоны идут по одному; сравнение — существующим A/B.
- Режим ECAPA-guided: ECAPA не даёт градиента, только раннюю остановку, а тянет `speechbrain` и
  второй набор весов. Вернуть, если ранней остановки по WavLM 0.24 окажется мало.
- Автоматические метрики качества сверх loss.
- Windows/macOS как целевые платформы P1 (код не должен ломаться, но проверяется только Linux + NVIDIA).

## Проверено по исходникам тренажёра (коммит `84a6f4c220cd7cc57dce14f638d43e13fa777f42`, MIT)

- Обучаемый параметр — только `style_ttl [1,50,256]`; `style_dp` берётся у ближайшего пресета
  (`find_closest_style` по WavLM Layer 3) и заморожен. Оптимизатор Adam, `ReduceLROnPlateau(patience=200,
  factor=0.5, min_lr=lr·0.01)`, `clip_grad_norm 1.0`. Defaults v3: 3000 шагов, lr 2e-4,
  vocoder_steps 6, порог WavLM 0.24, порог ECAPA 0.15.
- Loss — MSE средних и std признаков WavLM-Large слоя 3 (`transformers.WavLMModel`,
  `microsoft/wavlm-large`). ECAPA (`speechbrain/spkrec-ecapa-voxceleb`) считается под `no_grad` —
  это только монитор и ранняя остановка, градиента не даёт.
- Цикл обучения со stop/resume живёт в Gradio-`app.py`; `pipeline/train_style.py::train` resume
  не умеет. Состояние resume — `torch.save` словаря (style, noisy_latent, optimizer, scheduler, step,
  best). Значит, адаптер — свой цикл (~100 строк) на блоках апстрима: `SupertonicModel`,
  `utils.loss`, `helper.load_text_to_speech`, `sample_noisy_latent`, `get_train_dataloader`.
- `pipeline/utils/model.py` при импорте **глобально отключает проверку TLS** (`httpx.Client` с
  `verify=False`, удаляет `SSL_CERT_FILE`). В нашей копии этот блок вырезан (см. «Решения»).
- `requirements.txt` не содержит `transformers`, но код его импортирует. Свой `pyproject.toml` с
  закреплёнными версиями и `uv.lock`.
- Тренировочные фразы — 16 английских (`configs/utterances.py`). Для `ru` адаптер подставляет свои
  16 русских фраз; язык прогона = язык региона.
- Препроцессинг апстрима: `librosa.effects.trim(top_db=20)` + пик-нормализация 0.95; длина 3–30 с.
  У нас — приближение на numpy (порог −20 дБ от пика по выборкам) без librosa.
- Всё выше про ECAPA, librosa и httpx — факты об апстриме; в P1 они не используются.

## Решения

### Код тренажёра — копия в репозитории

Нужные ~400 строк апстрима (`pipeline/helper.py`, `pipeline/utils/{model,dataloader,style}.py`,
функции WavLM-loss из `utils/loss.py`) лежат в `src-tauri/resources/trainer/upstream/` с `LICENSE`
(MIT) и строкой ревизии в заголовке каждого файла. TLS-хак и `httpx` из `model.py` удалены в копии.
`find_closest_style` и цикл обучения — в нашем `train_job.py` (принимают уже загруженный WavLM).
Скачивания исходника нет. Все Python-файлы и `pyproject.toml`/`uv.lock` встроены в бинарник
(`include_str!`) и пишутся в `<trainer>/adapter/` перед каждой установкой и каждым запуском.

### Окружение

- `<app_data>/voice-lab/trainer/`: `adapter/` (наши файлы), `.venv/`, `python/`
  (`UV_PYTHON_INSTALL_DIR`), `uv-cache/` (`UV_CACHE_DIR`), `weights/wavlm-large/`, `installed.ok`.
  `uv sync --frozen` с `UV_PYTHON_PREFERENCE=only-managed`, `UV_PROJECT_ENVIRONMENT=<…>/.venv`.
  Удаление пакета = `remove_dir_all(trainer)`, ничего не остаётся в `~/.cache` и `~/.local`.
- `pyproject.toml`: Python 3.12, `package = false`, `environments = linux x86_64`, torch/torchaudio/
  torchvision из индекса `pytorch-cu128` (нужен для Blackwell), `transformers`, `onnx`, `onnx2torch`,
  `onnxslim`, `onnxruntime`, `soundfile`, `numpy`. Без gradio, librosa, speechbrain, huggingface_hub.
- `installed.ok` содержит sha256 встроенного `uv.lock`; не совпало (обновили приложение) →
  статус «нужно обновить пакет».
- Веса WavLM-Large (`microsoft/wavlm-large@c1423ed94bb01d80a3f5ce5bc39f6026a0f4828c`: `config.json`,
  `preprocessor_config.json`, `pytorch_model.bin`) качает Rust (`install_files` P0, SHA-256 и размер
  — константы); Python грузит `WavLMModel.from_pretrained(<weights>/wavlm-large)`. Обучение офлайн.
- `uv` ищется в `PATH`, затем `~/.local/bin`, `~/.cargo/bin`. Нет → `UV_MISSING`. Минимальная
  версия `uv` — константа (`UV_TOO_OLD`). `uv` получает обычное окружение пользователя (прокси,
  сертификаты); белый список переменных — только для Python-прогона.

### Протокол воркера (JSONL, как P0)

- Запуск: `<venv>/bin/python -I <adapter>/train_job.py <request.json>` без shell, `cwd` = каталог
  прогона, окружение — белый список (`PATH`, `HOME`, `CUDA_VISIBLE_DEVICES`, `HF_HUB_OFFLINE=1`,
  `TRANSFORMERS_OFFLINE=1`). Unix: отдельная группа процессов, `nice 10`. stderr → `worker.log`.
- Остановка = закрытие stdin (EOF): адаптер дообучает текущий шаг, сохраняет `state.pt` и стиль,
  событие `stopped`, выход 0. Так же срабатывает при выходе приложения. До начала цикла EOF →
  немедленный выход. Если после EOF нет выхода за 30 с или нажато «Прервать» — kill группы процессов. Python сам
  выходит через 60 с после EOF, если сохранение зависло. «Остановить» до начала цикла → `stopped`.
- stdout: протокол пишется в дубликат fd 1, а fd 1 перенаправлен в stderr — `print` апстрима и
  нативных библиотек не попадают в протокол. События: `started{resumedFrom}`,
  `progress{step,total,loss,bestLoss,lr}` каждые 8 шагов, `checkpoint{step,stylePath,bestLoss}`,
  `done{step,stylePath,bestLoss}`, `stopped{step,stylePath|null}`, `error{code}`.
- Проверки до работы: CUDA доступна (`GPU_UNAVAILABLE`, без тихого отката на CPU), пробное ядро на GPU выполняется (`CUDA_DRIVER_MISMATCH`), `flock` на `runDir/lock`
  (`JOB_BUSY` — второй процесс на тот же прогон). Лимит памяти процесса
  `min(0.75, (free − 2 ГБ)/total)` — OOM случится в Python, а не в OBS.
- Коды из воркера пропускаются только из списка `GPU_UNAVAILABLE, GPU_OUT_OF_MEMORY,
  CUDA_DRIVER_MISMATCH, TRAINING_STATE_CORRUPTED, JOB_BUSY`, остальное → `WORKER_FAILED`.

### Прогоны

- Таблица `training_runs(id, source_id, start_ms, end_ms, lang, params_json, status, step, total,
  best_loss, history_json, error, created_at, updated_at)`. Статусы
  `running|stopped|completed|failed|interrupted`; на старте `running` → `interrupted`.
  `history_json` — точки `[step, loss, bestLoss]` каждые 8 шагов (≤ 750); при `started{resumedFrom}`
  точки с шагом > resumedFrom отбрасываются.
- Каталог прогона `training/<run_id>/`: `reference.wav` (вырез региона, trim тишины по −20 дБ и пик
  0.95 — в адаптере на numpy), `state.pt`, `ckpt_<step>.json`, `final.json`, `worker.log`, `lock`.
- `state.pt` (`torch.save`, загрузка `weights_only=True`): `step, best_loss, best_ttl, best_dp,
  style_ttl, style_dp, noisy_fixed, lmask, optimizer_state, scheduler_state, total_steps, seed`.
- Чекпоинт и итог → `put_profile(origin="train", name="<имя источника> · шаг N", meta={runId,
  step, bestLoss, trainerRev})`.
- Продолжение: `stopped|interrupted|failed` → тот же каталог; `JOB_BUSY` от воркера (старый процесс
  ещё дописывает) статус не меняет; есть `state.pt` → `resume:true`, иначе
  старт заново (`resume:false`). Повреждённый `state.pt` → `TRAINING_STATE_CORRUPTED`.
- Удаление прогона удаляет каталог, но не профили. Один прогон одновременно; установка/удаление
  пакета во время прогона → `JOB_BUSY`. CPU-клон и синтез P0 не блокируются.

### Параметры (UI)

Пресеты: «быстро» 1000 шагов, «стандарт» 3000, «долго» 6000; lr 2e-4; vocoder_steps 6; порог ранней
остановки WavLM 0.24; чекпоинт каждые 500; seed 42; регион 3–30 с; язык фраз = язык региона
(для `ru` — 16 своих фраз, иначе английские апстрима); согласие на запись обязательно.
Предупреждения: «Обучение загружает GPU полностью — не запускайте во время стрима»; свободной
VRAM < 6 ГБ — жёлтое предупреждение.

### Диагностика

`vl_train_status`: путь и версия `uv`, состояние пакета (`none|ok|outdated`), GPU из
`nvidia-smi --query-gpu=name,driver_version,memory.total,memory.free --format=csv,noheader,nounits`,
CUDA torch (проверка в venv, кэшируется до установки/удаления), свободное место на диске.

## Анти-цели

- Сломать P0 или музыку: Python/torch не обязательны, ничего не грузится без согласия.
- Оставить процесс на GPU после выхода приложения (EOF → stop → выход; страховка — самовыход через 60 с).
- Потерять прогресс при остановке/падении: `state.pt` и чекпоинты пишутся через временный файл + rename.
- Выполнить чужой код: только наша копия кода тренажёра, `torch.load(..., weights_only=True)` только
  своих `state.pt` из каталога прогона, веса по SHA-256.

## Критерий приёмки

На RTX 5070 Ti: установить пакет → обучить профиль из 10-секундного русского региона пресетом «быстро» → видеть прогресс и график → остановить на середине → закрыть и открыть
приложение (прогон `stopped`) → продолжить → получить итоговый профиль и 2+ чекпоинта в библиотеке
→ синтезировать им фразу на CPU. (Ранняя остановка может дать один чекпоинт: приёмочный прогон —
с `saveEvery=100` и порогом 0.) Во время обучения CPU-синтез P0 работает. Замерить пик VRAM/RAM и время.
