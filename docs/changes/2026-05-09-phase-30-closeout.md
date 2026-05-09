# Phase 30 closeout — WASM / web target

**Status:** codebase-closed 2026-05-09. All six sessions shipped. Browser playtest of `examples/flappy.twe` via the CI-published GitHub Pages demo is the remaining manual verification step.

The second phase of the post-v1.0 plan from `docs/05-roadmap.md` after Phase 29 (determinism layer). Unlocks browser-playable 2D distribution: itch.io HTML5 pages, embeddable demos, Show-HN-grade reach — without requiring any engine change.

## Sessions shipped

| # | Surface | Commit |
|---|---------|--------|
| 1 | `BuildTarget::Wasm32` + `build_wasm_target()` + WASM play loop | `35fb3ad` |
| 2 | `save_to_path` / `load_from_path` → localStorage via `quad-url` | `35fb3ad` |
| 3 | click-to-start overlay in HTML template (AudioContext unlock) | `35fb3ad` |
| 4 | CSS `aspect-ratio` letterbox — fills viewport, preserves 640×480 | this commit |
| 5 | `.github/workflows/wasm-demo.yml` — auto-publishes flappy to Pages | this commit |
| 6 | This closeout note | this commit |

## Exit criteria

Per the Phase 30 entry in `docs/05-roadmap.md`:

- **`examples/flappy.twe` runs in Chrome + Firefox at 60fps with sound + keyboard, served from a static host.** Infrastructure ships: `twec build --target wasm32` produces a runnable web output; `.github/workflows/wasm-demo.yml` auto-deploys to GitHub Pages on every push to `main`. The actual headed-browser test is the user's manual step — the CI job itself can't open a browser. flappy.twe has no sound load calls, so the "with sound" criterion is vacuously met for that specific example. A game that uses `sound.load` / `sound.play` needs a real browser test to confirm macroquad's WASM audio path works end-to-end.
- **`survive_beta` is deferred.** Asset size + Twe script complexity make it a heavier WASM target; deferred per the roadmap note.

## Stdlib delta

**No new stdlib functions.** Phase 30 is build-pipeline + HTML infrastructure. The only user-visible surface change is that `twec build --target wasm32` now works.

## Code-side audit

**Session 1 — WASM build target** (commit `35fb3ad`):
- `BuildTarget::Wasm32` added to `src/build.rs`; `parse()` accepts `"wasm32"` and `"wasm32-unknown-unknown"`. `label()` returns `"wasm32"`. `binary_extension()` returns `""` (output is a directory).
- `build_wasm_target()`: creates `dist/web/`, copies `main.twe` + assets flat, writes `index.html` (sessions 1+3+4 combined), finds `mq_js_bundle.js` from `~/.cargo/registry`, invokes `cargo build --target wasm32-unknown-unknown --release` (with `TWEC_WASM_GAME_NAME` env), copies the resulting `twec.wasm` to `dist/web/game.wasm`.
- `wasm_html()`: generates the HTML template with CSS letterbox and click-to-start overlay. `find_mq_js_bundle()` walks `$CARGO_HOME/registry/src/macroquad-*/js/mq_js_bundle.js`.
- `src/lib.rs`: `build`, `cli`, `play3d`, `play_visual`, `physics3d` modules excluded from `wasm32` via `#[cfg(not(target_arch = "wasm32"))]`. `bundle` stays compiled on all targets (stdlib.rs uses `bundle::read_asset_bytes`).
- `Cargo.toml`: `wgpu`, `winit`, `pollster`, `bytemuck`, `gltf`, `gilrs`, `arboard`, `rapier3d`, `zstd`, `image` moved to `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`. `quad-url = "0.1"` added to `[target.'cfg(target_arch = "wasm32")'.dependencies]`.
- `src/main.rs`: `#[cfg(not(wasm32))]` for `cli::run()`; `#[cfg(wasm32)]` calls `play::launch_wasm()`.
- `src/play.rs`: all gilrs items (`GAMEPAD_BUTTONS`, `GAMEPAD_AXES`, `GILRS`, `PREV_GAMEPAD`, `GilrsState`, `clear_gamepad_state`, `shutdown_gilrs`, `poll_gamepad`) wrapped in `#[cfg(not(target_arch = "wasm32"))]`. WASM stubs for the three public functions. `launch_wasm()` and `run_loop_wasm()` added under `#[cfg(target_arch = "wasm32")]`. The WASM loop fetches `main.twe` via `macroquad::file::load_string` (uses `fetch` internally), then runs the identical fixed-timestep loop as `run_loop_embedded`.
- `src/stdlib.rs`: `clipboard_read` and `clipboard_write` guarded; `#[cfg(not(target_arch = "wasm32"))]` around the arboard calls and the Ctrl+V paste handler. WASM reads return `""`, writes silently drop.
- `src/bundle.rs`: `zstd::encode_all` guarded (returns `Unsupported` error on WASM — compression is never requested on the web target since build.rs is native-only); `zstd::decode_all` guarded similarly; `std::fs::read` fallback in `read_asset_bytes` guarded (returns `Unsupported` on WASM — assets on web are served over HTTP and loaded by macroquad's own fetch-based API, not via this path).

**Session 2 — File I/O reroute** (commit `35fb3ad`):
- `src/save.rs`: two `#[cfg(target_arch = "wasm32")]` implementations of `save_to_path` and `load_from_path` using `quad_url::set_program_parameter` / `quad_url::get_program_parameter` (localStorage). Native implementations unchanged, wrapped with `#[cfg(not(target_arch = "wasm32"))]`. No schema change.

**Session 3 — Audio context unlock** (commit `35fb3ad`):
- `wasm_html()` in `build.rs`: the HTML template includes a centered click-to-start overlay. The `onclick` removes the overlay and focuses the canvas. macroquad's `mq_js_bundle.js` independently resumes the Web AudioContext on any user gesture; the overlay makes that gesture explicit and visible.

**Session 4 — Variable canvas sizing** (this commit):
- Updated `wasm_html()` CSS: `canvas` now has `max-width:100vw; max-height:100vh; aspect-ratio:640/480; image-rendering:pixelated`. Paired with `body { display:flex; justify-content:center; align-items:center; width:100vw; height:100vh }`, the canvas fills the viewport letterboxed at 640:480 at any window size. The internal resolution stays at 640×480 (macroquad sets the `width`/`height` attributes); CSS scales the display size only. `image-rendering:pixelated` keeps pixel art crisp at non-integer zoom levels.

**Session 5 — CI pipeline** (this commit):
- `.github/workflows/wasm-demo.yml`: triggers on push to `main`, on GitHub Release publication, and on manual `workflow_dispatch`. Installs `wasm32-unknown-unknown` target via `dtolnay/rust-toolchain@stable`. Builds twec for the Linux host, wraps `examples/flappy.twe` in a minimal project, runs `twec build --target wasm32`, deploys `dist/web/` to the `gh-pages` branch via `peaceiris/actions-gh-pages@v4`. Cargo cache is keyed on `Cargo.lock` to avoid WASM recompilation when only .twe files change.

## Honest deferrals

- **End-to-end browser test in CI.** There is no headless browser in the CI job — the workflow builds and deploys but cannot verify 60fps playback in Chrome/Firefox. The manual step is: open the GitHub Pages URL, play flappy, confirm keyboard + frame rate. A follow-on could use Playwright on `@playwright/test` to load the URL and assert the canvas exists + tick count advances, but that adds a significant CI dependency and is not gated on Phase 30.
- **`survive_beta` WASM.** Explicitly deferred. The ~1300-line script parses fine but the asset directory includes PNGs that increase bundle size, and the WASM binary size itself (~20MB unstripped) may be slow to load without `wasm-opt` post-processing. A "compress WASM" follow-on session (add `wasm-opt` to the CI step) is the entry point.
- **Installed-twec WASM build.** `build_wasm_target` uses `env!("CARGO_MANIFEST_DIR")` embedded at compile time to locate the twec workspace for the inner `cargo build --target wasm32-unknown-unknown`. This path is correct when twec is built from source but stale when installed via `cargo install` on another machine. Resolution: ship a pre-built `game.wasm` stub as a binary asset with twec releases (so the inner cargo build is skipped), or provide `--workspace-path <dir>` as a CLI flag. Deferred to a follow-on.
- **3D WASM.** `play3d` / `play_visual` / `physics3d` are excluded from the `wasm32` target. Enabling them would need the wgpu `webgpu` or `webgl` feature and browser-specific event loop setup. Deferred.
- **IndexedDB.** The roadmap listed IndexedDB; we shipped `localStorage` via `quad-url`. localStorage is synchronous (fits the blocking builtin model), has a 5 MB per-origin limit (sufficient for game saves), and is supported on all target browsers. IndexedDB is async and would require either blocking on a JS promise (not straightforward from Rust) or a redesigned async-save API. Documented in §7 as "saves use localStorage on wasm32."

## Doc updates

- `docs/05-roadmap.md` — Phase 30 status note + size-table row updated.
- `CLAUDE.md` — Phase 30 marked codebase-closed.
- `README.md` — test count stays 755 (no new tests this phase).

## Test delta

`cargo test --release` reports **755 passing** (unchanged from Phase 29 close — Phase 30 is infrastructure, no new Twe-program tests). `cargo clippy --release --all-targets -- -D warnings` clean.

## What this enables

- Any Twe 2D game can now be shipped as a browser-playable demo via `twec build --target wasm32`.
- First-party examples with no external assets (flappy, pong, snake, dialogue) are immediately WASM-ready.
- The GitHub Pages CI pipeline gives a free, always-current live demo link that can go in README + itch.io description.
- Phase 31 (multiplayer) and Phase 32 (open-world 3D) proceed from here; neither gates on WASM.

## What does not change

- No grammar change. No new keyword. No type-system change.
- The 2D macroquad native path (`twec play`) is identical — the WASM changes are purely in `cfg(target_arch = "wasm32")` branches.
- Existing examples continue to parse + run on native; the only behavior change for already-shipped games is that `os.clipboard.read()` returns `""` on WASM (documented honest deferral).
