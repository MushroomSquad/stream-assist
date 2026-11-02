# Task 9 report

Status: DONE

## Commits

- `7d67323` — `feat(music): redemption pipeline and driver`

## Files changed

- `src-tauri/src/music.rs` — request resolution, title normalization, current-snapshot limit decisions.
- `src-tauri/src/driver.rs` — serialized redemption decisions, persistence-first action execution, bounded background service calls, OAuth, reconciliation, reward lifecycle, settings.
- `src-tauri/src/app.rs` — startup, keyring-backed clients, managed sender/watch receiver, bounded exit cleanup.
- `src-tauri/src/lib.rs` — module exports, asynchronous setup, exit handling.
- `src-tauri/src/limits.rs` — Clone/Serialize/Deserialize derives required by Settings.
- `src-tauri/src/spotify.rs` — nullable device IDs, device selection, single-use PKCE state helper.
- `src-tauri/src/spotify/tests.rs` — existing device fixtures adapted to optional IDs.
- `src-tauri/src/twitch/eventsub.rs` — ten-second connection/subscription timeouts.
- `src-tauri/src/obs.rs` — five-second connection timeout.
- `src-tauri/tests/task9.rs` — all thirteen named brief tests plus `pick_device_skips_null_id`.
- `.superpowers/sdd/2026-11-02-stream-assist-v1/task-9-report.md` — this final report, written after the implementation commit.

## Tests

Commands are relative to the repository root unless a working directory is specified.

- `cargo test --manifest-path src-tauri/Cargo.toml --test task9`
  - Before implementation: expected FAIL, exit 101, missing `music`/`driver` imports; no tests executed.
  - After implementation: PASS, 14 passed, 0 failed.
- `cargo test` in `src-tauri` (three runs, including the final implementation before committing): PASS each time.
  - Library unit tests: 117 passed, 0 failed.
  - Existing integration tests: 2 passed, 0 failed.
  - Task 9 integration tests: 14 passed, 0 failed.
  - Binary and doc tests: 0 tests, 0 failures.
  - Total: 133 passed, 0 failed.
- `cargo fmt --manifest-path src-tauri/Cargo.toml`: PASS from the repository root.
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`: PASS, no formatting differences.
- `git diff --check`: PASS, no whitespace errors.

Two verification invocations initially used an incorrect working directory: `cargo fmt --manifest-path src-tauri/Cargo.toml` from `src-tauri`, and `cargo test` from the repository root. Both failed to locate the manifest and were rerun successfully with the correct directory. An editing script also initially used repository-relative paths from `src-tauri`; it made no changes and was rerun from the root.

## Deviations

- Added derives to Limits so the exact Settings type can be cloned and serialized.
- Adapted existing Spotify device fixtures and added `pick_device_skips_null_id`; the additional test name was missing from the supplied integration rule, so this descriptive name was chosen.
- Updated EventSub and OBS connection boundaries outside the brief's initial four-file list to satisfy the mandatory timeout rules.
- Used a private internal result channel alongside the public DriverMsg receiver so the exact public `run(ctx, rx)` signature does not require adding a sender field to DriverCtx. Background results re-enter the serialized loop as Input, DriverMsg, or private startup/reconciliation result messages.
- Reversed `history_recent` output before the limits decision because Db returns newest-first while limits documents oldest-first input.
- No Tauri commands were implemented; AppState exposes only the driver sender and WidgetState watch receiver for Task 11.

## Concerns

- Live Spotify/Twitch/OBS credentials and the desktop GUI were not exercised. Verification used the full Rust suite and wiremock fixtures; account permissions, real keyring availability, and desktop exit behavior still need live integration validation.
- The final report is intentionally written after the implementation commit, following the requested final-step ordering.

## Fix round 1

Implementation commit: `2c34e60` — `fix(driver): single in-flight spotify poll, keep reward id, non-blocking keyring, hardening`.

Addressed all nine review findings:

- Spotify polling has one outstanding request at a time, including while Idle. Each result carries its issue time; results issued before the latest dispatched SpotifyPlay are discarded before reaching Player.
- SettingsChanged retains the current driver-owned music_reward_id before persisting the incoming settings.
- Keyring token saves (including refresh callbacks), startup/exit loads, and logout deletion run through tokio::task::spawn_blocking. Awaited loads have a ten-second timeout; exit also retains its overall two-second timeout. Saves/deletion are detached so an unlock prompt cannot block the select loop.
- Non-Duplicate insert failures attempt mark_cancelled, refund the redemption, and send TEXT_SERVICE_DOWN rejection text.
- Every OBS request has a five-second timeout, in addition to connect.
- Title normalization removes only the listed complete trailing or parenthesised/bracketed noise phrases, retaining words inside real titles such as HD Lover and annotations such as (Live).
- TwitchLogout retains the client in a bounded background operation to best-effort pause the reward before releasing it.
- WidgetTest schedules a reset after ten seconds to a fresh Player WidgetState; a generation number prevents an earlier test's timer from resetting a newer test.
- All Db mutex locks in driver.rs and app.rs recover poisoned guards with unwrap_or_else(|e| e.into_inner()).

Tests and commands (run from repository root unless noted):

- `cd src-tauri && cargo test` — PASS, exit 0. Output: library `119 passed; 0 failed`; core integration `2 passed; 0 failed`; Task 9 integration `14 passed; 0 failed`; binary/doc tests `0 passed; 0 failed`. Total: **135 passed, 0 failed**.
- New tests: `driver::tests::stale_poll_after_play_is_dropped` (also verifies a second poll cannot start while outstanding, and a fresh result is accepted), `driver::tests::settings_change_keeps_music_reward_id` (missing and stale incoming IDs, other settings still updated).
- Extended `normalize_title_strips_noise` with official audio/music video, lyric video, lyrics/MV/HD suffixes, HD Lover, Official Audio Lover, (Live), and MyHD.
- `cargo fmt --manifest-path src-tauri/Cargo.toml` — PASS.
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check` — PASS, no output.
- `git diff --check` — PASS, no output.

Live account, keyring unlock dialogs, OBS, and desktop UI behavior were not exercised. A timed-out blocking keyring operation can continue on its blocking worker; the awaiting task/driver stays responsive.
