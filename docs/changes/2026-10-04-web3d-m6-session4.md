# web3d-M6 session 4: 2D games in the browser

**Date:** 2026-10-04
**Plan:** [2026-10-01-web3d-m6-plan.md](2026-10-01-web3d-m6-plan.md), session 4

## What shipped

**`twec build --target web` builds 2D games.** The one browser runtime (`crates/twe-web`, WebGPU) now runs both kinds of game: a 2D game draws through the kernel canvas, a 3D game through the scene path, chosen automatically at startup.

- **`frame2d::detect_dimension(env)`** decides 2D vs 3D once, after the top level runs. A game is 3D if any entity class has a `look:` block, or its first rendered frame enqueues a 3D draw (`cube`, `mesh`, `sphere`, …). It runs the top-level `on render():` once through the 3D path and checks `render_queue3d`; render code is pure drawing, so the first real frame running it again is harmless. It leaves `env` as it found it. Shells with separate 2D/3D entry points (native `twec play` / `play3d`) don't need it; the one browser shell does.
- **`Renderer::render_2d(layers, assets)`** clears the surface to black and draws the canvas layers (the game under its camera, then the shell's overlay) over it. It reuses the renderer's surface, device, queue and canvas. The browser 2D shell calls it; the native 2D shell (`play2d`) keeps its own window.
- **The `twe-web` shell** detects the dimension after the top level and branches each frame: 2D runs `frame2d::tick` (with the DOM `InputState`) + `audio_polish` + `frame2d::render` + `render_2d`; 3D keeps the existing `host3d` path. `sound.*` and `music.*` (including layers and crossfades) play through the existing WebAudio shell. `save.*` / `settings.*` stay in `localStorage`. The frame-stats export covers both paths.

**The macroquad wasm build is gone.** Before this, `--target wasm32` recompiled the whole `twec` binary for `wasm32-unknown-unknown` as a macroquad game and shipped `game.wasm` + `mq_js_bundle.js`. That path is deleted:
- `build_wasm_target`, `wasm_html` and `find_mq_js_bundle` are removed from `src/build.rs`;
- `BuildTarget::Wasm32` is removed; `wasm32` / `wasm32-unknown-unknown` now parse to `BuildTarget::Web`, so old invocations build on `twe-web`;
- `play::launch_wasm` and `run_loop_wasm` are removed from `src/play.rs`;
- the `twec` binary's wasm `main()` is now empty — the browser runtime is `twe-web`, not the `twec` binary. The binary still type-checks for `wasm32-unknown-unknown` (a CI gate).

The macroquad *dependency* stays linked into the `twe-web` wasm (the `env.js` shim is still needed) until session 6 removes it. The browser shell never initialises macroquad, so `macroquad_live()` is false there and every 2D draw goes through the command list — exactly the kernel-canvas path.

**CI** (`.github/workflows/wasm-demo.yml`) now builds the runtime once (`twe-web` wasm + `wasm-bindgen` into `target/web-runtime/`) and ships `examples/flappy.twe` through `twec build --target web`, instead of recompiling a macroquad game per project.

## Verified

- **End to end:** built the runtime (`cargo build -p twe-web --target wasm32-unknown-unknown --release`, then `wasm-bindgen --target web`), then `twec build --target web` on a copy of `examples/pong.twe`. The output `dist/web/` holds the hashed runtime (`twe_web.<hash>.js`, `twe_web_bg.<hash>.wasm`), `env.js`, `index.html` and the game bundle.
- **`Renderer::render_2d` headless** (`tests/kernel_render.rs`): a 2D frame (blue field, yellow circle, a red HUD square on top) draws into the offscreen target and the pixels land where the canvas put them.
- **`detect_dimension`** (`tests/frame2d_input.rs`): `pong`, `snake`, `tetris`, `button_demo`, `particles_demo` are 2D; `hello_3d`, `hello_glb` are 3D.
- **Checks:** `cargo test` all green; clippy `-D warnings` clean for native, `wasm32-unknown-unknown` (whole workspace, including the `twec` binary) and `-p twe-web`; `cargo build --release` zero warnings.

## Not verified / honest deferrals

- **No live browser run this session** — the browser tooling was unavailable. The build produces the files and the 2D render path is the same `canvas.rs` that `tests/canvas_2d.rs` compares to the references natively, plus the headless `render_2d` test. A real-browser pass (and a Chrome FPS check) is a follow-on, and the native shell already proves the 2D runtime.
- **Alpha blends composite in linear space on web.** The browser draws the canvas onto the renderer's sRGB target (as the 3D HUD already does), so layer alpha blends are a shade off the native 2D shell, which blends in gamma space. Solid draws match. A non-sRGB surface view would close the gap and is a small follow-on.
- **Typed text isn't wired on web** (`text_input`): the DOM key listeners feed keys, not characters. Native has it via `set_typed_text`.
- **Still open from session 1:** what a screen shake does with no camera, and a screen-space HUD under a moving `camera2d`.

## Next

Session 5: the rest of the 2D surface compared on every example (tilemaps, nine-slice, `play_visual`, touch, clipboard) — most of which already draw through the command list; then session 6 deletes the macroquad dependency and the four play loops, and gives `twec build` a desktop `.exe` for 3D games.
