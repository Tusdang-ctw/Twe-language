# web3d-M6 session 1: the draw-command list, and references

**Date:** 2026-10-01
**Plan:** [2026-10-01-web3d-m6-plan.md](2026-10-01-web3d-m6-plan.md), session 1

## What shipped

**One draw-command list.** `src/draw2d.rs` defines `Draw2d`:
- `Rect`, `RectLines`, `Circle`, `CircleLines`, `Line`;
- `Text`: the baseline position, plus an optional font asset path;
- `Texture`: an asset path, plus an optional destination size, source rectangle and tint.

Commands hold plain data (asset paths, sRGB colours), never renderer handles, so any renderer can draw them.

Every 2D draw path now pushes into `Env::draw2d` instead of calling macroquad:
- the drawing builtins, sprites and atlas frames, nine-slice panels and the UI widgets (`src/stdlib.rs`);
- the built-in particle-emitter dots (`src/eval.rs`);
- the `fx` overlay (`src/fx.rs`);
- the `light2d` overlay (`src/light2d.rs`).

The 3D HUD's old `HudItem` enum is gone, and `render3d_types::HudItem` is now an alias of `Draw2d`. In 3D the kernel HUD still draws only `Rect` and `Text`. The other 2D calls keep refusing in 3D with their existing message rather than queueing commands nothing draws; session 2 lifts that.

**macroquad replays the list.** In each of the three play loops (file, embedded, wasm) the frame does this:
1. clear the list;
2. run the render code;
3. push the `light2d` and `fx` overlays;
4. call `draw2d::replay_macroquad` under the frame's `camera2d` transform.

**The F3 frame-time panel still draws with macroquad directly.** It belongs to the shell, and session 3 replaces it.

**Pixel-identical.** Before the fixes below, frame 60 of all 41 2D examples was captured with this build and with the committed tree, which got only the capture hook added. Every image was identical.

**`TWE_CAPTURE=N:path`.**
- `twec play` writes frame `N` to a PNG and exits. There is no input, and every frame advances exactly one `PHYSICS_DT`. The RNG's default seed is fixed, so a capture is the same picture on any machine.
- `TWE_CAPTURE_DUMP=1` also prints the frame's commands.
- The references are in `tests/references/m6/`: frames 60 and 300 of 37 examples, 74 PNGs (about 0.9 MB). `capture.sh` regenerates them and `capture.log` lists each one.
- The four console examples print and draw nothing, so they have no reference: `dialogue_demo`, `save_demo`, `modular_audio_demo`, `modular_math_demo`.

## Bugs the references found

The first capture found 17 examples blank under `twec play`. Four of them are the console examples. The other thirteen were bugs older than M6.

1. **The top-level `on render():` never ran in 2D.**
   - `eval::render_frame` ran state and entity renders only. The top-level body was run by `render_frame3d` and, before June, by the bytecode VM.
   - Twelve examples drew nothing: `atlas_demo`, `audio_demo`, `button_demo`, `dungeon_demo`, `font_demo`, `gamepad_demo`, `keybind_demo`, `mouse_demo`, `pause_menu_demo`, `tilemap_demo`, `walk_demo` and `widgets_demo`.
   - `render_frame` now runs it first, in the same order as 3D.
   - Test: `render_frame_2d_runs_top_level_render_into_draw_commands`.
2. **`survive_beta` drew everything off screen.**
   - Its v1.0.1 rewrite (aaf53b3) added `camera2d.follow` and `camera2d.bounds` but kept drawing in screen space (`pos - camera_x`), so the view moved twice.
   - `camera2d.bounds` alone moves `camera.pos` off the origin, which turns the transform on.
   - The example keeps its own clamped camera again, as before the rewrite. A 2D camera has no screen-space escape for a HUD, so this example can't use `camera2d.*` as written.
3. **`rhythm_demo` and `survive_beta_mobile` stopped on their first frame:** `text` without `size:`, and `circle_outline` without `thickness:`. Fixed in the scripts, and in `examples/experimental/openworld_demo.twe`.

Unrelated to M6, the full test run showed that **`kernel_render::procedural_surface_outputs` had been failing since M7.** `r + 80` on `u8` channels overflowed and hid the real result: the "glows blue" pixel is `[179, 186, 250]`. Very bright HDR emission is pushed toward white. The colours are now compared as `i32`, and the margin requires blue to lead by 50.

## Open, for session 2

- **Screen shake with no camera.** When `camera.pos` is unset, a shake builds the camera around `(shake_x, shake_y)`. That puts the world origin at screen centre for the duration of the shake. The kernel's camera transform should decide whether a shake offsets the default view instead. The references don't cover a shake.
- **A HUD under a 2D camera.** There is no way to draw in screen space while `camera2d` moves the view, which is why `survive_beta` can't use it. Adding one would be new surface; flagged, not decided.

## Verification

- `cargo test`: 1,083 passed, none failed.
- Clippy `-D warnings` is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- `cargo build --release` produces zero warnings.
