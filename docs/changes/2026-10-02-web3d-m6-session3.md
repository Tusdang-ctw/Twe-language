# web3d-M6 session 3: `twec play` on the kernel

**Date:** 2026-10-02
**Plan:** [2026-10-01-web3d-m6-plan.md](2026-10-01-web3d-m6-plan.md), session 3, plus the UI widgets from session 5

## What shipped

**`src/play2d.rs` is the native 2D shell:** a winit window drawn by the kernel canvas. `twec play` and shipped builds (`play::launch_embedded`) run on it. The macroquad loops in `play.rs` are still reachable with `TWE_PLAYER=macroquad`, for comparison, until session 6 deletes macroquad.

Everything the macroquad loop did, the shell does:
- **Rendering.** A non-sRGB view of the surface, so blending is in gamma space, as macroquad blended. The 640×480 canvas scales to fit the window with black bars (letterboxed). The window is now resizable, and on high-DPI screens text is rasterised at the screen's scale.
- **Input** goes through `host3d::InputState`, as the 3D shell's does:
  - one command per tick, recorded and replayed by `replay::step`;
  - keys, mouse buttons and the wheel, with the cursor mapped through the letterbox into canvas units;
  - raw mouse motion, the first gamepad (gilrs), typed text, cursor lock.
- **Shell features:**
  - pause on idle and on focus loss;
  - Escape quits (unless the script took it over); `quit()`;
  - debounced hot reload; Steam init; touch-tap tracking;
  - the F3 frame-time panel, drawn as a second canvas layer outside the game's camera;
  - F12 and `screenshot()`.
- **Sound.** Through `native_audio` (`quad-snd`, moved out of `play3d.rs` so both shells share it). `music.layer` volume changes and crossfades used to call macroquad directly; they now go through `audio_host` too.
- **Images at canvas size.** Screenshots and `TWE_CAPTURE` render at the canvas's own 640×480 whatever the window size, and a capture ignores the OS's input.

**Input semantics, one frame step.** `frame2d::tick` takes the shell's `InputState`. Each tick takes its own command. The render code then sees the frame's summary:
- the latest held keys and cursor;
- every press of every tick;
- summed motion and wheel.

Two cases need care:
- **A frame that runs no tick** (paused, or between ticks on a fast display) takes a fresh command for the render code. Pause-menu buttons need that.
- **A paused frame's input is recorded and replayed** (one line per frame). Otherwise a pause menu's Resume click would be missing from a replay. A running frame that falls between ticks isn't recorded, because how many there are depends on the display. So at more than 60 Hz, a click that lands on such a frame reaches render-time widgets but not the replay.

**The UI widgets no longer need macroquad**, pulled forward from session 5 so that `twec play` didn't lose six examples in between:
- `measure_text` is `draw2d::measure_text` (the same fontdue arithmetic);
- `text_input` reads Ctrl, V and Backspace from the input ambients, its characters from the shell (`stdlib::set_typed_text`, which the macroquad loops also feed), and its caret blink from `clock::now_secs`;
- every widget and panel draws in 3D too, over the scene.

## Verification

- **The real shell against the references.** All 74 reference frames were captured through the `twec play` window on the kernel and scored against the macroquad references. The worst frame has a mean difference of 1.12 of 255 (`keybind_demo`) with 1.33% of pixels off; 12 frames are exact.
  - The remaining differences are glyph edges at fractional positions. Centred widget labels put text there, and OpenGL and wgpu filter those edges slightly differently. The comparison tests now allow up to 2% of pixels off.
- **The headless comparison** (`tests/canvas_2d.rs`) now covers all 37 examples, widgets included; nothing is pending.
- **Input through replays** (`tests/frame2d_input.rs`, also run through the shell with `twec replay`):
  - three clicks reach `button_demo`'s render-time button (`Count: 3`);
  - in `pause_menu_demo`, Escape pauses from `on update`, the menu's Resume click unpauses, and the paused frames don't advance the game.
- **By hand:**
  - hot reload: a running script edited from red to blue redraws blue;
  - a `twec build` of `survive_demo` runs from an empty folder on the kernel shell, its sprite loading from the embedded bundle;
  - `audio_demo` and `rhythm_demo` start without errors.
- **Checks:** `cargo test`, 1,092 passed, none failed. Clippy `-D warnings` is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.

A capture once disagreed because the OS cursor sat over the window (`dungeon_demo`'s light follows the mouse). Captures now ignore real input, as documented.

## Not done

- **Driving the window by hand.** Resizing, F3, F12 and gamepads weren't exercised interactively in this session; the paths are written but only replay-tested.
- **Typed text isn't part of the replay command**, so a replay of `text_input` typing won't retype it.
- **Still open** from session 1: what a screen shake does with no camera, and whether a HUD can be drawn in screen space under `camera2d`.
- **Next:** session 4, the browser. `twec build --target web` for 2D on `twe-web`.
