# Phantom and Moonlight implementation report

STATUS=DONE

Branch: `feat/persona-themes`.
Commit: `HEAD`, `feat(widget): phantom and moonlight themes` (the single commit containing this report; obtain its immutable hash with `git log -1 --format=%H -- .superpowers/sdd/persona/report.md`).

## Delivered

- Phantom: red/black/white angular panels, original geometric burst and star decorations, tilted white cover frame, deterministic ransom lettering using three bundled font families, slanted metadata strips, staggered command queue with red first row, red progress and white slash.
- Moonlight: blue gradient cut-corner panel, outlined MUSIC lettering, cyan skewed cover frame, condensed italic typography, red title slash, indexed queue with white selected row, cyan progress and glowing head.
- Both: SDK state handling, offscreen `sa-youtube`, transparent page, `now` / `queue` / `all`, SDK zoom, two-line title clamp, five visible queue rows, empty queue hiding, missing/broken-cover note, clamped progress, reduced-motion rule. Track data uses textContent/createElement/replaceChildren only. Phantom title spans are rebuilt only when the full title changes; titles longer than 40 characters are shortened to 39 plus ellipsis.
- No game images, logos, characters, or copied artwork are included. Reference images were viewed only as style references. Decorations are original CSS geometry and text symbols.

## Files

- `src-tauri/assets/widget/themes/phantom/`: `index.html`, `style.css`, `render.js`, `theme.json`, `barlow-700-italic.ttf`, `anton.ttf`, `bangers.ttf`, `OFL.txt`, `OFL-Anton.txt`, `OFL-Bangers.txt`.
- `src-tauri/assets/widget/themes/moonlight/`: `index.html`, `style.css`, `render.js`, `theme.json`, `barlow-700-italic.ttf`, `OFL.txt`.
- `src-tauri/src/server/tests.rs`: six built-in IDs and count.
- `docs/themes.md`, `README.md`: built-in theme lists; documentation of local OFL fonts.
- This report.

Barlow Condensed 700 italic and its OFL license were copied from Underground. Anton and Bangers and their licenses were downloaded from `https://raw.githubusercontent.com/google/fonts/main/ofl/{anton,bangers}/` (respectively `Anton-Regular.ttf`, `Bangers-Regular.ttf`, and `OFL.txt`). No external runtime font requests.

## Measured dimensions

Both themes declare all = 900×390, now = 570×225, queue = 310×380 at scale 1. Scale-2 now source size = 1140×450.

Chromium DOM measurements after fonts and animation settled confirmed every measured panel, heading, cover, details block and row fits the declared source bounds in all three parts. Phantom's rotated last queue row ends at y=378.87 within 380; Moonlight's last row ends at y=370. Rotations, burst tips and cover shadows were also checked visually in the large-window screenshots. Browser measurement details: `/tmp/sa-persona-browser-phantom.json` and `/tmp/sa-persona-browser-moonlight.json`.

## Commands and output

From `src-tauri`:

```sh
cargo test
```

```text
test result: ok. 143 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
Doc-tests stream_assist_lib: 0 passed; 0 failed
```

Full output: `/tmp/sa-persona-cargo.log`.

From repository root:

```sh
npm run check
```

```text
svelte-check found 0 errors and 0 warnings
```

Full output: `/tmp/sa-persona-check.log`.

```sh
cd src-tauri
cargo fmt
cargo fmt --check
```

Final formatting check: exit 0, no output. `git diff --check`: exit 0, no output.

Preview commands, run sequentially from `src-tauri`, stopping the previous demo before switching themes:

```sh
THEME=phantom PART=all PORT=47899 cargo run --example widget_demo
THEME=moonlight PART=all PORT=47899 cargo run --example widget_demo
```

```text
Running `target/debug/examples/widget_demo`
http://127.0.0.1:47899/widget/music?part=all&scale=1
```

The existing demo already supports THEME/PART/SCALE/PORT; no demo changes were needed. The user's port 47821 was not used.

For each active theme, final screenshots used this command for each part (`THEME` in the filename denotes the active demo theme):

```sh
for part in all now queue; do
  chromium --headless --no-sandbox --user-data-dir=/tmp/sa-chr \
    --default-background-color=202020ff --virtual-time-budget=8000 \
    --run-all-compositor-stages-before-draw --window-size=1000,500 \
    --screenshot=/tmp/sa-persona-THEME-$part.png \
    "http://127.0.0.1:47899/widget/music?part=$part"
done
chromium --headless --no-sandbox --user-data-dir=/tmp/sa-chr \
  --default-background-color=202020ff --virtual-time-budget=8000 \
  --run-all-compositor-stages-before-draw --window-size=1300,600 \
  --screenshot=/tmp/sa-persona-THEME-now-scale2.png \
  'http://127.0.0.1:47899/widget/music?part=now&scale=2'
```

Output: all eight PNG files written successfully. Chromium diagnostics are in `/tmp/sa-persona-chromium-*.log`.

Additional real-browser checks via Chromium DevTools, using `/tmp/sa-persona-browser.mjs`:

```sh
node /tmp/sa-persona-browser.mjs phantom
node /tmp/sa-persona-browser.mjs moonlight
```

For both themes: `fit=true` for all/now/queue; `stable=true`, `safe=true`, `zero=true`, `placeholder=true`, `rows=5`, `rowFit=true`, `brokenCover=true`, `clamped=true`, `empty=true`, `hidden=true`. These exercise repeated state updates, markup-like long title input, zero duration, absent/broken covers, a six-item queue capped to five rows, two-line long queue content, progress beyond duration, empty queue, and hidden state. The temporary test harness captures the theme callback through a test-only StreamAssist.on replacement; production uses the unchanged SDK.

## Screenshots inspected

- `/tmp/sa-persona-phantom-all.png`
- `/tmp/sa-persona-phantom-now.png`
- `/tmp/sa-persona-phantom-queue.png`
- `/tmp/sa-persona-phantom-now-scale2.png`
- `/tmp/sa-persona-moonlight-all.png`
- `/tmp/sa-persona-moonlight-now.png`
- `/tmp/sa-persona-moonlight-queue.png`
- `/tmp/sa-persona-moonlight-now-scale2.png`

All final images were opened and visually inspected. The sample data intentionally has no cover, so these screenshots show the theme-styled note placeholders.

## Iterations and deviations

- No scope deviations. Existing demo support was reused.
- Initial Rust run exposed the second old built-in ID assertion; both list/count assertions were updated and the full suite rerun successfully.
- Initial fade animation could be captured mid-fade by Chromium's virtual clock after WebSocket state arrival. Opacity was removed; the requested quick slide/skew remains, with reduced-motion support. Final screenshots were recaptured.
- Queue typography/padding was adjusted so two title lines plus requester fit within each fixed-height row, verified with long injected content.
- Screenshot windows were 1000×500 and 1300×600 to avoid Chromium minimum-window cropping. Chromium required `--no-sandbox` in this environment.
