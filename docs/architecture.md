# Stream Assist — архитектура

Документ для разработчиков. Исходные требования — в
[спецификации](superpowers/specs/2026-11-02-stream-assist-design.md),
пошаговый план v1 — в [плане](superpowers/plans/2026-11-02-stream-assist-v1.md).

## Общая схема

```
Twitch EventSub ─┐                     ┌─► Spotify Web API
Панель (Tauri) ──┼─► driver (tokio) ───┼─► Twitch Helix (статусы, чат, награда)
Виджет (WS) ─────┘      │    ▲         ├─► YouTube oEmbed / Data API
                        ▼    │         └─► OBS WebSocket
                   Player::step (чистый автомат)
                        │
                       SQLite (очередь, история, настройки)
```

- **Tauri v2**, ядро на Rust (tokio), панель на SvelteKit (SPA, adapter-static).
- Локальный **axum**-сервер на `127.0.0.1:<port>` отдаёт виджет, `sdk.js`, темы,
  WebSocket и OAuth-callback Spotify.
- Вся логика очереди и сторожа — **чистый автомат** `Player::step(input, now) -> Vec<Action>`
  без async и сети. Им единолично владеет драйвер.

## Модули (`src-tauri/src`)

| Файл | Ответственность |
|---|---|
| `model.rs` | Общие типы: `Track`, `Source`, `QueueItem`, `Redemption`, `PlaybackSnapshot`, `Tokens`, `PlayError` |
| `player.rs` | Автомат очереди и сторожа. Входы `Input`, выходы `Action` |
| `driver.rs` | Async-цикл: опрос Spotify, исполнение `Action`, приём выкупов, настройки, reconcile |
| `music.rs` | Фича «Музыка»: `resolve` (заказ → кандидаты) и `decide` (лимиты → очередь/отказ) |
| `router.rs` | Точка расширения: `reward_id` → `FeatureKind` |
| `request.rs` | Разбор текста заказа: ссылка Spotify / YouTube / поиск |
| `limits.rs` | Лимиты зрителей |
| `db.rs` | SQLite: настройки, текущий трек, очередь, история, отменённые выкупы |
| `spotify.rs` | PKCE, токены, Web API (поиск, плеер, устройства) |
| `twitch/` | Device Code Flow, Helix (награды, статусы выкупов, чат), EventSub WebSocket |
| `youtube.rs` | oEmbed и поиск Data API |
| `obs.rs` | obs-websocket v5: проверка, создание источников виджета |
| `server/` | axum: виджет, темы (встроенные через `include_dir` + импорт zip), WS, callback |
| `secrets.rs` | Keyring (сервис `stream-assist`), одна очередь записи |
| `http.rs` | Общий reqwest-клиент: таймаут 15 с, подключение 5 с |
| `i18n.rs` | Тексты сообщений в чат |
| `app.rs`, `lib.rs` | Bootstrap Tauri, команды панели |

## Ключевые инварианты

1. **Баллы возвращаются только до старта трека.** Все отмены идут через
   `Player::cancel`, который ничего не делает для `item.started == true`.
2. **FULFILLED — только после подтверждённого старта**: успешный `PlayResult`
   (Spotify) или `Yt Started` (YouTube).
3. **На каждый `SpotifyPlay` драйвер возвращает ровно один `PlayResult`,
   на каждый `ResolveFallback` — ровно один `FallbackResolved`**, даже при ошибке
   или таймауте. У автомата нет своих таймаутов на эти ответы.
4. **Порядок исполнения пачки действий**: локальные отметки в БД
   (`mark_cancelled`, `history_push`) → `Persist` (снимок очереди) → сетевые вызовы.
5. **Новый выкуп**: `insert_item` (уникальность по `redemption_id`) →
   `Player::step(Enqueue)` → действия. Нельзя сохранять снимок до того, как элемент
   попал в автомат, — `save_state` удалит строку.
6. Опрос Spotify — не больше одного в полёте; ответ, отправленный до последнего
   `SpotifyPlay`, отбрасывается.
7. Правила по снимку Spotify (конец трека, ручная пауза, чужой трек) работают только
   для Spotify-трека. Пока активен фолбэк, драйвер не пытается запускать Spotify.
8. Лок `Db` (`std::sync::Mutex`) никогда не держится через `.await`.

## Сторож

| Наблюдение | Решение |
|---|---|
| Наш URI, пауза, прогресс у конца (или сброс в 0 после конца) | Конец трека → следующий |
| Наш URI, пауза, прогресс далеко от конца | Ручная пауза → ничего не делать |
| Чужой URI вне 4 с после старта, не у конца | Стример взял управление → `Suspended` |
| Чужой URI / нет плеера у конца трека | Автоплей/конец → следующий |
| Устройство пропало | Повтор play с позиции, бэкофф 2–30 с |
| API недоступен ≥ 30 с и прошёл ожидаемый конец (или трек ещё не стартовал) | Фолбэк на YouTube |
| YouTube не стартовал за 20 с | Отмена (до старта — с возвратом) |
| YouTube не прислал конец к `duration + 10 с` (без пауз) | Считаем концом |

## Выкупы и сверка (reconcile)

- EventSub WebSocket: подписка только на свежей сессии; `session_reconnect` — без
  переподписки; жёсткий обрыв — переподключение с бэкоффом, подписка и `Resync`.
- При старте и на `Resync` драйвер читает все `UNFULFILLED` выкупы награды:
  `Queued` → пропустить, `Started` → FULFILLED, `Cancelled` → отменить повторно,
  `Unknown` → обработать как новый.
- Дубли отсекаются по `redemption_id` (in-flight множество + уникальный индекс в БД).

## Виджет

- `GET /widget/music?part=now|queue&scale=N` — HTML активной темы.
- `GET /widget/sdk.js` — клиент: WebSocket `/ws`, события для темы, YouTube IFrame Player.
- WebSocket принимает только `Origin`/`Host` `127.0.0.1|localhost:<port>`.
- YouTube-команды получает одно соединение `role=player` — его запрашивает только
  страница внутри OBS (`window.obsstudio`) и только для `part=now|all`.
  Новое player-соединение вытесняет старое (код `4001`) и получает повтор текущего трека.
- Контракт тем — [themes.md](themes.md).

## Как добавить новую награду

1. Вариант в `router::FeatureKind` и ветка в `route`.
2. Модуль фичи (по образцу `music.rs`): разбор ввода зрителя и решение.
3. Ветка `match` в обработке выкупа в `driver.rs`. Отмена/выполнение выкупа — через
   те же действия, чтобы сохранить инварианты 1–5.
4. Если фиче нужна своя награда — хранить её id в `Settings` (как `music_reward_id`)
   и создавать/находить её при старте так же, как музыкальную.
5. Тексты для чата — в `i18n.rs`, тексты панели — в `src/lib/i18n/ru.ts`.

## Сборка и тесты

```sh
npm ci
cd src-tauri && cargo test        # ~160 тестов: автомат, БД, клиенты (wiremock), сервер
npm run check                     # svelte-check
TWITCH_CLIENT_ID=… npm run tauri dev
TWITCH_CLIENT_ID=… npm run tauri build   # deb / rpm / AppImage на Linux
```

Демо виджета без аккаунтов: `THEME=phantom PART=now PORT=47899 cargo run --example widget_demo`
(в `src-tauri`), затем открыть `http://127.0.0.1:47899/widget/music`.

## Известные ограничения v1

- Spotify в режиме разработчика: каждый стример создаёт своё приложение.
- Отдельного бот-аккаунта нет — бот пишет от аккаунта стримера.
- Смена порта применяется после перезапуска.
- Интерфейс только на русском (строки вынесены в `ru.ts` и `i18n.rs`).
