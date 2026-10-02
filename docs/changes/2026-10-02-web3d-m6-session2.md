# web3d-M6 session 2: the kernel's 2D canvas

**Date:** 2026-10-02
**Plan:** [2026-10-01-web3d-m6-plan.md](2026-10-01-web3d-m6-plan.md), session 2

## What shipped

**`src/kernel/canvas.rs` draws a frame's `Draw2d` list.** It replaces `kernel/hud.rs`, which drew only rectangles and built-in-font text. One canvas serves both kinds of picture: a 2D game's whole frame, and the HUD over a 3D scene.
- **Shapes** are tessellated the way macroquad drew them, so the two renderers produce the same pixels:
  - filled circles are 20-sided fans;
  - circle outlines are 30 quads from the radius outward;
  - rectangle outlines are half the requested thickness, inside the rectangle;
  - lines are quads centred on the segment.
- **Text** is rasterised by fontdue at the requested size, truncated to whole pixels as macroquad did. The glyph's top-left is at `(pen + xmin, y − height − ymin)`, so `y` is the baseline. Any TTF loaded with `load_font` works.
  - Glyphs are cached per font, size and character in a 1024² RGBA atlas, which starts over if a frame overflows it.
  - When the canvas is scaled up, glyphs are rasterised at the target's scale so they stay sharp.
- **Sprites** cover native size, scaled size, atlas source rectangles and tint. Sampling is linear, as in macroquad, including its blending across atlas-cell edges.
- **Colours** are truncated to bytes per vertex, as macroquad stored them: `0.05` is 12, not 13. That one detail was a uniform 0.68 of the mean difference on three examples.
- **Blending** happens in the target's space:
  - on a non-sRGB target (the 2D shell's), in gamma space, as macroquad blended;
  - on the 3D renderer's sRGB target, colours are decoded first, as the M3 HUD did.
- **`camera2d`.** An optional `Camera2d { center, zoom }` maps world to canvas the way the 2D camera did.

**Images and fonts load through the host's `AssetSource`**, as a new `AssetKind::Raw` / `AssetReady::Raw`. The native host, the web host and the new `canvas::SyncAssets` all serve it.
- The 3D renderer hands raw loads to `Canvas::accept`.
- A host that owns its source alone calls `prepare_loaded`, so an image appears on the first frame that draws it.

**The 3D HUD gains every 2D primitive.** `circle`, `circle_outline`, `rect_outline`, `line`, `sprite`, `sprite_frame`, `sprite_frame_at`, `text_with_font` and `tilemap_render` used to refuse in 3D; they now draw.
- The stdlib builtins no longer touch macroquad.
- Sprite, atlas and nine-slice builtins read only an image's size, decoded once per path. A missing file is still an error at the call.
- `text_with_font` checks that the font was loaded.
- macroquad's replay decodes its own textures and fonts on first use.
- UI widgets and panels still refuse in 3D (session 5).

**`src/frame2d.rs` is one 2D frame without a renderer.** It has three entry points:
- `load` / `load_source` load the script: module-aware, refusing `look:` blocks, with the same messages as before;
- `tick` runs the fixed-step accumulator, with pause, persistent states and hit-stop;
- `render` advances the visual clocks, runs the render code, adds the `light2d` and `fx` overlays, and returns the camera.

The three macroquad play loops now call it; 211 lines of triplicated loop body are gone. The headless renderer calls it too, so the kernel and `twec play` advance a frame the same way. After the refactor, `twec play` still matches all 74 reference frames pixel for pixel.

**`canvas::Offscreen`** is the canvas on its own device, drawing into a non-sRGB `Rgba8Unorm` texture like macroquad's framebuffer, with pixel readback.

## The comparison

`tests/canvas_2d.rs::kernel_frames_match_the_macroquad_references` runs each example headless through `frame2d`:
- no input, one fixed tick per frame;
- a fresh thread per example, so the runtime's thread-locals start clean;
- the script's directory as the asset root, as `twec play` sets it.

The kernel draws the reference frame, and the result is scored against the macroquad capture. Kernel frames and the report go to `target/m6-compare/`.

| | examples (frames) |
|---|---|
| render on the kernel | 31 (62) |
| mean difference per channel | 0.00 – 0.36 of 255 |
| pixels off by more than 32 | 0.00 – 0.40 % |
| exact (0.00 / 0.00 %) | `atlas_demo`, `font_demo`, `hero`, `particles_demo`, `sprite_demo`, `survive_demo` |
| wait for UI widgets / panels | 6: `button_demo`, `keybind_demo`, `layout_demo`, `pause_demo`, `pause_menu_demo`, `widgets_demo` |

The remaining differences are glyph antialiasing at fractional pen positions. Text is the same text: the worst frame, `pong_net_internet`, was inspected crop by crop.

The test requires every example except the six to be within a mean of 2 and 1 % of pixels. A new example is held to its reference by default.

Other new tests:
- exact pixels for each command, gamma-space alpha, the text baseline, a generated sprite image (native, scaled, atlas cell, tint) and the camera;
- `kernel_render::hud_draws_every_2d_primitive`: a circle, a line and a sprite over a 3D scene.

`examples/assets/hero.png` turned out to be a single half-transparent pixel, so the first sprite test checked nothing. It now generates its image.

## Not in this session

- The 2D window. `twec play` still runs on macroquad (session 3).
- The browser (session 4).
- Widgets, panels, `play_visual` (session 5).
- Still open from session 1: what a screen shake does with no camera set, and whether a HUD can be drawn in screen space under `camera2d`.

## Verification

- `cargo test`: 1,090 passed, none failed.
- Clippy `-D warnings` is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- `cargo build --release` produces zero warnings.
- `twec play` captures are identical to the 74 references.
