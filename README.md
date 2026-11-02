# Stream Assist

Настольное приложение для стримеров: зрители заказывают музыку за баллы канала Twitch,
треки встают в очередь и играют через Spotify стримера (YouTube — запасной источник),
а виджет в OBS показывает текущий трек и очередь. Tauri v2, Rust, SvelteKit.

## Документация

- [Руководство стримера](docs/user-guide.md) — настройка Twitch, Spotify, OBS, работа во время стрима, проблемы.
- [Темы виджета](docs/themes.md) — встроенные темы и как сделать свою.
- [Архитектура](docs/architecture.md) — устройство, инварианты, как добавить новую награду.

## Сборка

Нужны Node.js с npm, Rust и системные зависимости Tauri v2
(на Linux — WebKitGTK 4.1 и инструменты сборки).

1. Зарегистрируйте приложение в [Twitch Developer Console](https://dev.twitch.tv/console/apps):
   Client Type **Public**, включён **Device Code Flow**, OAuth Redirect URL — любой
   (например `http://localhost`, программа его не использует).
2. Соберите, подставив Client ID — он встраивается в бинарник при компиляции:

   ```sh
   npm ci
   TWITCH_CLIENT_ID=… npm run tauri build
   ```

3. Пакеты — в `src-tauri/target/release/bundle/` (на Linux: `.deb`, `.rpm`, `.AppImage`).

Разработка: `TWITCH_CLIENT_ID=… npm run tauri dev`. Тесты: `cd src-tauri && cargo test`, `npm run check`.
