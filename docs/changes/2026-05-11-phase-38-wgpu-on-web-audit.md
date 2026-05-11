# Phase 38 session 2 — wgpu-on-web audit

**Status:** audit document, not a code change. Catalogues the
specific cfg gates that must relax (and the new wasm32 branches that
must land) for browser 3D to work. Lives here as a checklist for the
follow-on session that performs the actual port, once Firefox-stable
+ Safari-stable browser wgpu support arrives.

As of 2026-05, Chrome ships browser wgpu; Safari Tech Preview ships;
Firefox lags. Production browser 3D requires all three.

---

## Native-only modules

These modules are excluded from wasm builds today via
`#[cfg(not(target_arch = "wasm32"))]` at module declaration in
`src/lib.rs`. The audit walks each one + names the per-file barrier.

### `src/play3d.rs`

Drives `twec play3d`. Owns the wgpu device + queue + surface, the
window event loop, the render passes (forward + shadow + post),
glTF mesh loading.

**Barriers to wasm:**

1. **winit window creation.** winit's wasm32 backend uses a
   `<canvas>` element rather than an OS window. The wgpu surface
   is built from the canvas, not a raw window handle. Code that
   reaches `EventLoop::new().expect("winit")` and `WindowBuilder::new().build()`
   needs `#[cfg(target_arch = "wasm32")]` branches that:
   - Look up the canvas by id (`document.getElementById("glcanvas")`)
     and pass it to `WindowAttributes::with_canvas`.
   - Skip native flags like `with_inner_size` in pixels — browser
     canvas sizing is CSS-driven.

2. **Async wgpu device acquisition.** `wgpu::Instance::request_adapter` +
   `Adapter::request_device` are async. The native path uses
   `pollster::block_on` (blocking). On wasm32 there is no thread to
   block, so the same calls must run inside `wasm_bindgen_futures::spawn_local`
   (or the equivalent macroquad wasm-future executor).

3. **`request_animation_frame` integration.** Native uses winit's
   event loop. Browser drives frames via `requestAnimationFrame`;
   we either wire winit's wasm event loop (newer winit versions
   support this) or call the frame stepper directly from a JS
   `requestAnimationFrame` callback registered through `wasm_bindgen`.

4. **`pollster` dep.** Pure-Rust async-executor; compiles for wasm32
   but pollster::block_on doesn't work in a non-blocking environment.
   The wasm branch needs `wasm_bindgen_futures` (a JS-aware executor).

5. **glTF mesh loading.** `gltf` crate is pure-Rust and compiles
   for wasm32. The barrier is the *file I/O* — `std::fs::read` won't
   work in a browser. The Phase 38 session 3 `assets.fetch(url)`
   builtin closes this: scripts call `assets.fetch("models/hero.glb")`
   and get bytes, which the wasm-side `play3d` plumbs into
   `gltf::Gltf::from_slice`.

### `src/physics3d.rs`

Drives rapier3d. The crate already compiles to wasm32 — Phase 28
verified this transitively. The module is gated out of wasm only
because it's referenced by `play3d` which is wasm-gated. **No new
barriers**; once `play3d` lands on wasm, the `cfg` can drop from
both modules in the same commit.

### `src/play_visual.rs`

Drives the procedural shader pipeline. Same wgpu/winit barriers as
`play3d`. Lower priority — Phase 9's `visual` blocks are a v0.3+
feature, not core to v1.0 thesis. Port together with `play3d` in the
follow-on session.

---

## Stdlib branches that need wasm-aware paths

A handful of stdlib functions in `src/stdlib.rs` are wasm-gated
because they touch native-only crates. The audit lists the ones that
need browser-side replacements before browser 3D ships:

| Builtin | Native impl | Wasm32 replacement |
|---------|-------------|---------------------|
| `mesh(path)` / `mesh_textured` | `play3d` (wgpu) | wgpu-on-web canvas |
| `texture(path)` | `play3d` | `fetch` → `ImageBitmap` → `wgpu::Texture` |
| `sound.play(path)` | macroquad audio | Phase 30 already wires this (WebAudio) |
| `save_to_path(file, ...)` | `std::fs::write` | Phase 30 already wires this (localStorage) |
| `physics.body(...)` | rapier3d | rapier3d compiles for wasm; no change needed once play3d gates relax |

The biggest unblock is **#1 (mesh + texture loading via fetch)** — the
Phase 38 session 3 `assets.fetch` builtin is the primitive.

---

## Dep-side audit

`Cargo.toml` puts the following in
`[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`:

- `wgpu`, `winit`, `pollster`, `bytemuck` — needed by play3d on
  wasm too. Move to **unconditional `[dependencies]`** during the
  wgpu-on-web port. `pollster` is replaced with
  `wasm_bindgen_futures` on wasm — gate the `block_on` call site,
  not the dep.
- `gltf` — pure Rust; move to unconditional.
- `rapier3d` — pure Rust; move to unconditional.
- `image` — pure Rust; move to unconditional.
- `gilrs` — has no wasm backend. Stays native-only. Browser gamepad
  routes through the GamepadAPI (a different stdlib branch in
  `src/play.rs`'s wasm-friendly Phase 30 path).
- `arboard` — has no browser-sandbox backend. Stays native-only.

Move-to-unconditional deps don't *block* the audit; they're a
mechanical step during the follow-on session.

---

## What ships in Phase 38 today (codebase-scaffolding-closed)

| Session | Deliverable | Status |
|---------|-------------|--------|
| 1 | `BuildTarget::Wasm32_3D` variant + parser + directory layout | ✓ shipped |
| 2 | This audit document | ✓ shipped |
| 3 | `assets.fetch(url)` builtin (browser asset streaming primitive) | ✓ shipped |
| 4 | WebAudio scaffold | reuses Phase 30's AudioContext-unlock path; no new work |
| 5 | `physics.body` wasm verification | rapier3d compiles for wasm; verified in session 5 |
| 6 | `examples/crystal_hunter_web.twe` | ✓ shipped |
| 7 | Closeout | ✓ shipped |

## What's deferred to the follow-on

The actual wgpu-on-web pipeline — porting `src/play3d.rs` so all the
`#[cfg(not(target_arch = "wasm32"))]` gates in that file relax to
wasm-friendly equivalents. This is a phase-sized chunk of work
because:

- Every winit call needs a wasm branch.
- Every async wgpu call needs a wasm executor.
- Every `std::fs` call needs to route through `assets.fetch`.
- Browser wgpu must be production-ready (Firefox-stable + Safari-
  stable) before the work is worth landing.

Until then, scripts targeting `--target wasm32-3d` get the directory
layout + asset bundle + an informative placeholder HTML telling the
user that browser 3D is gated on the runner-side port.
