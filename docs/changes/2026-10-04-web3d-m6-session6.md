# web3d-M6 session 6: delete macroquad; a 3D desktop `.exe`

**Date:** 2026-10-04
**Plan:** [2026-10-01-web3d-m6-plan.md](2026-10-01-web3d-m6-plan.md), session 6 (the last of M6)

## macroquad is gone

The `macroquad` dependency is removed from `Cargo.toml`; `cargo tree -i macroquad` finds nothing. What it did, the kernel and the shells now do:

- **The four macroquad play loops** (`run_loop`, `run_loop_embedded`, and the wasm loop deleted in session 4) are gone. `src/play.rs` shrank from ~1,260 lines to ~240: the gamepad helpers (gilrs), the hot-reload debounce gate, and `launch` / `launch_embedded`, which now dispatch to the kernel shells.
- **Audio** no longer has a macroquad branch. `sound.*` and `music.*` always queue through `audio_host`; the shell plays them (`native_audio` through quad-snd natively, WebAudio in the browser). `SOUND_CACHE` and the macroquad decode are gone, as is `block_on` (its only use was the macroquad async load).
- **Textures and fonts** load in the kernel canvas (`AssetKind::Raw`), so `SPRITE_CACHE`, `FONT_CACHE`, `with_cached_texture`, `with_decoded_font`, `ensure_font_decoded` and `draw2d::replay_macroquad` are deleted. The sprite builtins keep only `image_size` (the `image` crate).
- **Colours** are a plain `crate::color::Color { r, g, b, a }` (replacing `macroquad::color::Color`), used by `color_of`, the UI widgets, `fx` and `light2d`. The canvas decodes them when it draws.
- **`macroquad_live()` / `set_macroquad_live` / `TWE_PLAYER=macroquad`** are gone; there is one 2D runtime now, the kernel.
- **The browser runtime imports nothing from `env`** any more (macroquad/miniquad left those). `web/env.js`, its import map in `web/index.html`, and the `env` file in `write_web_build` are removed. `tests/web_runtime.rs` now fails if an `env` import ever comes back. The shipped wasm is a touch smaller.

## `twec build` makes a 3D desktop `.exe`

`twec build` embedded only 2D games before. Now `launch` and `launch_embedded` detect the game's dimension (`frame2d::detect_dimension`) and dispatch:

- a 2D game runs on the kernel's 2D canvas (`play2d`);
- a 3D game runs on the kernel's 3D renderer — `play3d::launch_embedded` runs the embedded source with no hot reload, reading meshes / textures / sounds from the mounted bundle.

So `twec play hello_3d.twe` and a built 3D `.exe` both open the 3D renderer; `twec play3d` is still the explicit 3D command. `frame2d` gained `load_any` / `load_source_any` (load without refusing `look:` blocks) for the dispatcher.

## Verified

- **No macroquad:** `cargo tree -i macroquad` finds nothing; every target compiles without it.
- **A 3D `.exe`:** `twec build` of `hello_3d` runs from an empty folder, into the 3D renderer, with no errors over a 5-second run (a 2D misroute would print a `cube()`/`look:` error). The 3D renderer itself is covered by `tests/kernel_render.rs`; a pixel capture of the windowed 3D `.exe` is deferred (the 3D shell has no capture hook; the 2D shell does).
- **A 2D `.exe` still works:** a built `survive_demo` (sprites + sound) runs from an empty folder on the kernel shell, loading its image from the bundle and its sound through quad-snd — the paths that replaced macroquad.
- **The picture didn't move:** seven examples with sprites, atlases, fonts and fx (`sprite_demo`, `hero`, `atlas_demo` exact; `walk_demo`, `survive_beta`, `pong`, `button_demo` within the known text-edge tolerance) still match their references on the kernel shell.
- **The browser build has no `env.js`:** `twec build --target web` on `flappy` writes `index.html` (no import map), the hashed runtime and the bundle — four files, not five.
- **Checks:** `cargo test` 1,086 passed, none failed; clippy `-D warnings` clean for native, `wasm32-unknown-unknown` and `-p twe-web`; `cargo build --release` zero warnings.

## Deferred

- **A pixel capture of the windowed 3D `.exe`** (the 3D shell has no `TWE_CAPTURE` hook; adding one needs a surface readback). The 3D renderer's output is tested headless.
- **Stale comments** mentioning macroquad remain in a few stdlib docstrings; harmless, cleaned opportunistically.
