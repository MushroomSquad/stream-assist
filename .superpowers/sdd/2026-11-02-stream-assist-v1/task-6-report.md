# Task 6 report

Status: DONE

## Commits

- `8cd065441977275d9121ec2ae7fdd7131bdf058a` — `feat(server): widget server, sdk, ws, themes`
- Implementation committed on existing branch `feat/v1-music`; no push or branch creation. This report was written after the implementation commit.

## Files changed

- `src-tauri/src/server/mod.rs` — loopback server, HTTP routes, channels and WsOut.
- `src-tauri/src/server/ws.rs` — Origin/Host checks, state snapshot, player-only commands and input events, player eviction with close code 4001.
- `src-tauri/src/server/themes.rs` — embedded/custom assets, theme metadata, ZIP import and path validation.
- `src-tauri/src/server/tests.rs` — all 11 required test names; real ephemeral-port HTTP/WS connections; additional Host/missing-Origin assertions.
- `src-tauri/src/lib.rs` — exports server.
- `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` — futures-util dev dependency for WebSocket stream/sink test helpers.
- `src-tauri/assets/widget/sdk.js` — theme events, OBS-only player, YouTube playback acknowledgements and commands, reconnect and eviction downgrade.
- `src-tauri/assets/widget/themes/underground/theme.json`
- `src-tauri/assets/widget/themes/underground/index.html`
- `src-tauri/assets/widget/themes/underground/style.css`
- `src-tauri/assets/widget/themes/underground/barlow-500-italic.ttf`
- `src-tauri/assets/widget/themes/underground/barlow-600-italic.ttf`
- `src-tauri/assets/widget/themes/underground/barlow-700-italic.ttf`
- `src-tauri/assets/widget/themes/underground/OFL.txt` — font license.
- `src-tauri/examples/widget_demo.rs` — fake current track, three queued tracks, progressing state; port 47821.
- `.superpowers/sdd/2026-11-02-stream-assist-v1/task-6-report.md` — this report.

## Tests

Commands below ran from the repository root unless a working directory is specified.

- RED: `cargo test --manifest-path src-tauri/Cargo.toml server:: > /tmp/sa-task6-red.log 2>&1` — expected failure before implementation: 23 compilation errors, no tests executed (missing server interfaces and stream helpers).
- GREEN: `cargo test --manifest-path src-tauri/Cargo.toml server:: > /tmp/sa-task6-green.log 2>&1` — 11 passed, 0 failed, 82 library tests and 2 integration tests filtered out.
- Final required targeted verification, working directory `src-tauri`: `cargo test server::` — 11 passed, 0 failed; 82 library tests and 2 integration tests filtered out. Includes foreign Origin, foreign Host and absent Origin rejection.
- Full verification, working directory `src-tauri`: `cargo test` — run twice; final result 93 library unit tests and 2 integration tests passed, 0 failed (95 total); binary and doc-test suites each 0 tests.
- `cargo fmt --manifest-path src-tauri/Cargo.toml` — succeeded.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` — exit 0.
- `node --check src-tauri/assets/widget/sdk.js` — exit 0.
- `node --input-type=module` with an inline Node VM harness — passed SDK assertions for viewer/OBS role selection, fallback container, state dispatch, deferred playback, mute-before-load and start offset, one started event with duration, explicit unmute, ended item ID, pause/resume/reload, 2-second reconnect and downgrade to viewer on 4001. The harness printed “SDK behavior checks: 19 assertions passed”; the actual assertion count is 18.
- `git diff --check` — exit 0, no whitespace errors.
- An intermediate `cargo test server::` accidentally ran from the repository root: exit 101, no Cargo.toml there; immediately rerun successfully from `src-tauri` as listed above.

Browser verification:

- Working directory `src-tauri`: `cargo run --example widget_demo` — built and served `http://127.0.0.1:47821/widget/music`; stopped cleanly using Ctrl-C after verification.
- `chromium --headless --no-sandbox --disable-gpu --hide-scrollbars --window-size=800,420 --timeout=10000 --screenshot=/tmp/sa-task6-widget.png --dump-dom http://127.0.0.1:47821/widget/music > /tmp/sa-task6-dom.html 2>/tmp/sa-task6-chromium.log` — exit 0; initial screenshot and populated DOM inspected.
- After bundling fonts and rebuilding the demo: `chromium --headless --no-sandbox --disable-gpu --hide-scrollbars --window-size=800,420 --timeout=10000 --screenshot=/tmp/sa-task6-widget-final.png --dump-dom http://127.0.0.1:47821/widget/music > /tmp/sa-task6-dom-final.html 2>/tmp/sa-task6-chromium-final.log` — exit 0; final screenshot visually inspected: condensed italic text, dark translucent panels, lime borders, circular note placeholder, artist/title/requester lines, MUSIC tab, progress bar, three queue rows and lime gradient header.

## Deviations

- No functional deviations from the Task 6 brief.
- ZIP layout was unspecified: import expects theme.json at archive root and extracts into themes_dir/theme.id. Existing custom theme directories are rejected rather than overwritten; an imported custom theme can still override the embedded theme of the same ID.
- Only Underground was created, matching the task's explicit scope; the global list also names glass/crt/block, whose implementation is outside this brief.
- Browser inspection used headless Chromium and its screenshot instead of an interactive browser window.
- Barlow Condensed italic fonts and their OFL license are bundled locally for reliable rendering without an external font request.

## Concerns

- No blocking concerns found.
- Live OBS/YouTube audio was not exercised; SDK command behavior was checked using a mocked IFrame Player. The demo is a viewer and does not request playback control.
- OAuth callback forwards code/state to the driver as required by the brief; pending-login validation and exact one-time state matching remain the driver's responsibility.

## Fix round 1

- Commit: `cdaa9146d784b690372ea2bd839eea2a0be881c7` — `fix(server): non-blocking oauth callback, forward oauth errors, ws limits`.
- OAuth callbacks now use `try_send`; full or closed channels return HTTP 503 with a short plain-text body without waiting for the driver.
- OAuth channel items are `(Result<String, String>, String)` (code/error, state). Spotify denial redirects forward the error and state and display “Вход отменён, вернитесь в программу”. Requests missing both code and error still return 400.
- WebSocket upgrades enforce a 64 KiB maximum message size. `YtPlayerConnected` uses `try_send` while player ownership is serialized; failures are logged without awaiting a send under the player lock.
- Tests: added `callback_full_channel_returns_503` (fills the channel without draining; next request must complete within one second), `callback_forwards_error` (forwarded denial/state and cancellation text), and `callback_closed_channel_returns_503`. Adapted `callback_forwards_code_and_state` to assert `Ok(code)`; it passes.

Commands and output:

- `cd src-tauri && cargo test server::` — exit 0; `14 passed; 0 failed; 82 filtered out`; integration suite: 2 filtered out.
- `cd src-tauri && cargo test` — exit 0; library: `96 passed; 0 failed`; integration: `2 passed; 0 failed`; binary and doc-test suites: 0 tests. Total: 98 passed.
- `cargo fmt --manifest-path src-tauri/Cargo.toml` — exit 0.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` — exit 0, no output.
- `git diff --check` — exit 0, no whitespace errors.
