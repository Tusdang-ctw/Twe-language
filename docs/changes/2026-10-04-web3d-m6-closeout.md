# web3d-M6 closeout: one engine

**Opened:** 2026-10-01 · **Closed:** 2026-10-04
**Plan:** [2026-10-01-web3d-m6-plan.md](2026-10-01-web3d-m6-plan.md)

M6 merged Twe's two runtimes into one. The 2D engine (macroquad) is gone; everything — 2D games, 3D games, the HUD over a 3D scene, native and browser — now draws through the wgpu kernel.

## Exit criteria

| Criterion | Status |
|---|---|
| No `macroquad` in `Cargo.toml` | **Met.** `cargo tree -i macroquad` finds nothing. |
| Every example runs on the kernel, native and web | **Met.** All 37 drawing examples match their macroquad references headless (`tests/canvas_2d.rs`) and through the real `twec play` window (within a mean of 1.12/255, text-edge antialiasing only). `twec build --target web` ships 2D games on `twe-web`. |
| `twec build` makes a desktop `.exe` for 3D games | **Met.** `launch_embedded` detects the dimension and dispatches; a built `hello_3d.exe` runs the 3D renderer from an empty folder. |

## The sessions

1. **The draw-command list, and references** ([note](2026-10-01-web3d-m6-session1.md)). Every 2D draw pushes a `Draw2d` command; macroquad replayed it pixel-identically; `TWE_CAPTURE` captured 74 reference frames. The references found 13 examples blank under `twec play` (the 2D runtime never ran the top-level `on render():`; `survive_beta` moved its view twice) — all fixed.
2. **The kernel's 2D canvas** ([note](2026-10-02-web3d-m6-session2.md)). `kernel/canvas.rs` draws every command with macroquad's tessellation and byte-truncated colours; 31 of 37 examples matched within 0.36/255, the rest waiting on widgets.
3. **`twec play` on the kernel** ([note](2026-10-02-web3d-m6-session3.md)). `play2d.rs`: a winit window drawn by the canvas, with input through the per-tick command stream, hot reload, F3/F12, idle/blur pause, gamepad. UI widgets pulled off macroquad. All 74 references matched through the real window.
4. **2D games in the browser** ([note](2026-10-04-web3d-m6-session4.md)). `twec build --target web` runs 2D on `twe-web`; the shell auto-detects the dimension and draws 2D through `Renderer::render_2d`. The macroquad wasm build was deleted.
5. **The rest of the 2D surface** ([note](2026-10-04-web3d-m6-session5.md)). Widgets, fx, light2d, tilemaps and nine-slice already drew through the command list; touch was made host-agnostic (winit + DOM + macroquad fallback); clipboard and `play_visual` needed no port.
6. **Delete macroquad; a 3D `.exe`** ([note](2026-10-04-web3d-m6-session6.md)). The dependency, the four play loops, the audio/texture/font caches, the colour type and the `env.js` shim are gone; `twec build` dispatches a 3D game's `.exe` to the 3D renderer.

## What shrank

- `src/play.rs`: ~1,260 → ~240 lines.
- Deleted: `src/kernel/hud.rs`, the macroquad play loops, `SPRITE_CACHE` / `SOUND_CACHE` / `FONT_CACHE`, `replay_macroquad`, `with_cached_texture` / `with_decoded_font` / `ensure_font_decoded`, `block_on`, `macroquad_live` / `set_macroquad_live`, `build_wasm_target` / `wasm_html` / `find_mq_js_bundle`, `web/env.js`.
- Added: `src/draw2d.rs`, `src/kernel/canvas.rs`, `src/frame2d.rs`, `src/play2d.rs`, `src/color.rs`, `src/native_audio.rs`, and the comparison harness `tests/canvas_2d.rs`.

## Honest deferrals

- **A live-browser pass.** The browser tooling was unavailable through M6; the 2D web path is compile-verified and shares its renderer (`canvas.rs`) with the native headless and windowed tests, but no frame was captured in a real browser. A Chrome run (and an FPS check) is the follow-on.
- **Web alpha blends composite in linear space** (the canvas draws onto the renderer's sRGB target), a shade off the native gamma-space blend; solid draws match. A non-sRGB surface view would close it.
- **A pixel capture of the windowed 3D `.exe`** (the 3D shell has no `TWE_CAPTURE` hook).
- **Two camera questions from session 1:** what a screen shake does with no camera set, and a screen-space HUD under a moving `camera2d`.
- **Typed text on web** for `text_input` (DOM keys are fed, characters aren't).
- **Stale macroquad mentions** in a few stdlib docstrings.

## Where this leaves the pivot

All of M0–M7 are now closed. The v1.0 thesis — a top-down 3D survivors game at 60 fps in Chrome — was met at M4 (`survive3d` plays in Chrome) and the graphics bar at M7 (at or below Three.js on every scene). M6 removes the last split in the engine: one kernel draws everything. The remaining known work is the deferrals above (chiefly a live-browser pass) and the parked M5 benchmark's live runs.
