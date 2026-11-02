# Widget parts report

Status: DONE
Branch: `feat/widget-parts`
Commit: `HEAD` — `feat(widget): separate now/queue parts and crisp scale` (this report is included in that single commit).

## Changes

- Existing `/widget/music` supports `part=now|queue`; absent/invalid values retain `all`. SDK sets `html[data-part]` before state-driven theme rendering.
- SDK clamps finite scale values to 0.5–4, defaults to 1, and uses Chromium/CEF CSS zoom for text rasterization at the requested size.
- Queue always connects as view, including inside OBS, and never loads YouTube API. Now/all retain OBS player behavior and ordinary browser view behavior.
- All four themes support independent parts at (0,0), with original all layouts preserved. Block's rotated title is straightened only in the isolated now part to avoid clipping at the top edge.
- Optional theme.json parts metadata is exposed by list_themes; legacy themes receive width/height fallback for both parts. Built-in now/queue source sizes: underground 560×120 / 300×260; glass 320×150 / 320×90; crt 330×240 / 330×260; block 330×170 / 330×70.
- OBS creates/updates “Stream Assist — Сейчас играет” and “Stream Assist — Очередь”, attaching missing items to the current scene. Source dimensions are ceil(part size × scale). Only now reroutes audio and sets MonitorAndOutput. Legacy “Stream Assist — Music” is untouched.
- Widget panel has two copyable URLs, scale input, Russian positioning/crispness hint; iframe remains all at scale 1. Scale applies when copying URLs or pressing Add to OBS.
- README and theme author contract updated. Demo supports PART, SCALE and PORT environment variables, and supplies five queue entries.

## Tests and commands

```sh
cd src-tauri && cargo fmt
cd src-tauri && cargo test
npm run check
node src-tauri/tests/widget_sdk.mjs
git diff --check
```

Rust results (final run):

```text
test result: ok. 143 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.09s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Rust: 143 unit + 2 integration + 16 provider tests = 161 passed. Includes metadata for all four built-ins and legacy fallback.

```text
npm notice run stream-assist@0.1.0 check
npm notice run svelte-kit sync && svelte-check --tsconfig ./tsconfig.json
Loading svelte-check in workspace: /mnt/Soft/Work/Projects/stream_assist
Getting Svelte diagnostics...

svelte-check found 0 errors and 0 warnings
PASS: SDK parts, OBS roles, YouTube isolation and scale (7 cases)
```

`git diff --check`: exit 0, no output. SDK VM covers OBS queue/view isolation, now/all player behavior, invalid part, scale defaults/clamps and no YouTube script for views.

## Render verification

```sh
cd src-tauri && PORT=47822 cargo run --example widget_demo
chromium --headless --no-sandbox --disable-gpu --remote-debugging-port=9229 --remote-allow-origins='*' about:blank
node /tmp/sa-render.mjs
node /tmp/sa-measure.mjs
```

Port 47821 was already occupied, so this run used PORT=47822. Demo output:

```text
Finished `dev` profile
Running `target/debug/examples/widget_demo`
http://127.0.0.1:47822/widget/music?part=all&scale=1
```

The Node CDP driver sets viewport dimensions, navigates to part URLs, waits for live state/fonts, measures getBoundingClientRect(), and captures PNG via Page.captureScreenshot. Screenshot-only dark background is set with Emulation.setDefaultBackgroundColorOverride to rgba(32,32,32,1); theme backgrounds remain transparent.

| URL query | Viewport | Measured visible part | Screenshot |
| --- | --- | --- | --- |
| part=now&scale=1 | 560×120 | (0,0), 560×100.30 | /tmp/sa-parts-now.png |
| part=queue&scale=1 | 300×260 | (0,0), 290×258; all 5 rows visible | /tmp/sa-parts-queue.png |
| part=now&scale=2 | 1120×240 | (0,0), 1120×198.59 | /tmp/sa-parts-now-scale2.png |

Screenshots visually inspected. Other-theme measurements using `/widget/theme/<id>/index.html?part=...`: glass now 320×100, queue 320×47; crt now 330×173, queue 330×152; block now 330×147, queue 330×22. These fit the metadata sizes. Glass/block retain their original next-track queue presentation. Raw logs: `/tmp/sa-parts-render.log`, `/tmp/sa-parts-other-themes.log`, `/tmp/sa-parts-cargo.log`, `/tmp/sa-parts-check.log`, `/tmp/sa-parts-sdk.log`, `/tmp/sa-parts-demo.log`.

An initial new test used unavailable tempfile; it was changed to the repository's existing std::env::temp_dir + random suffix approach, with cleanup, before the successful final run.

## Limits

No live OBS instance was used: create/update and audio configuration are verified by code review and Rust compilation, not an OBS end-to-end session. Theme dimensions are reference sizes for demo content; unusually long text or queues beyond five rows can require a larger source or theme-specific truncation. Legacy custom themes receive metadata fallback but must implement the documented CSS contract to visually split their parts.
