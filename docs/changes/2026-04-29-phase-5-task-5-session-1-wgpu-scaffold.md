# 2026-04-29 — Phase 5 task 5 session 1: wgpu scaffold + clear-color window

## Status: implementation note. First session of the multi-session 3D backend (task 5 in `docs/changes/2026-04-29-phase-5-status.md`).

## Background

Phase 5 task 5 (3D rendering backend) is the largest single item left in the v0.1 roadmap. The Phase 5 status note enumerated five sub-sessions:

> (a) wgpu scaffold + clear-color
> (b) mesh import + flat shading
> (c) camera system
> (d) integration with `entity` for spawning meshes
> (e) 3D math stdlib

This note records session (a). Subsequent sessions will reference it as the starting point.

## What ships

A new CLI subcommand `twec play3d <file>` opens a wgpu-driven window via winit, configures a swapchain, and runs a render loop that clears the surface to a stable dark teal each frame. The Twe file's top-level code runs once at startup before the window opens so any parse / runtime error is reported before users see a blank window.

Concretely:

- **Cargo.toml** — three new dependencies: `wgpu = "22.0"`, `winit = "0.30"`, `pollster = "0.3"`. Bytemuck stays unintroduced — it only lands when vertex / uniform layout work begins (session b).
- **`src/play3d.rs`** — new module. Mirrors `src/play.rs`'s shape: `launch(path)` parses + runs the script's top-level statements, then hands off to a winit `EventLoop` running a `wgpu::Surface`-driven `ApplicationHandler`. `init_wgpu` blocks on the few async API calls (adapter + device acquisition) via pollster. `render` does a single render-pass clear with `LoadOp::Clear`. No geometry, no shaders, no pipelines.
- **`src/lib.rs`** — `pub mod play3d;`.
- **`src/cli.rs`** — `play3d` subcommand and usage line.
- **`examples/hello_3d.twe`** — stub example that prints a one-liner to stdout at startup. The script does no rendering; the window is driven entirely by the wgpu loop today.

## Architectural decisions

### Two parallel render paths instead of replacing macroquad

`twec play` (macroquad, 2D) and `twec play3d` (wgpu, 3D) coexist as separate CLI subcommands. The macroquad path ships and works for every Phase-2-era 2D game (`survive.twe`, `snake.twe`, `hero.twe`, …); breaking it to introduce wgpu would be a regression on the Phase 2 vertical slice.

Long-term, the two paths may merge — wgpu can do 2D, and macroquad's 2D primitives could be reimplemented over a wgpu pipeline. That's a downstream decision, not a session-1 problem.

### winit 0.30 `ApplicationHandler` API

winit 0.30 split window creation out of the constructor — windows are now created in `ApplicationHandler::resumed`. This handles platforms that suspend / resume the app cleanly (mobile, desktop on display reconfigure). The `RenderState` struct is `Option`al on the App because the window doesn't exist yet when `App::new` runs.

### Synchronous async via pollster

wgpu's `request_adapter` + `request_device` are async. Twe's runtime is single-threaded and the rest of the dispatch loop is sync, so wrapping the two startup calls in `pollster::block_on` is the simplest workable bridge. The blocks are short — one-time setup — and adding a full executor (tokio, async-std) for two calls would be a step backward in dependency weight.

### sRGB surface format preference

`init_wgpu` walks `surface_caps.formats` and picks the first sRGB format if available, falling back to the first available format if not. This makes raw RGB clear colours look correct (no double-gamma).

### No `--vm` flag on `play3d`

`play3d` doesn't accept `--vm tree|bytecode` because session 1 only runs the script's top-level code at startup — there's no per-frame interpreter dispatch yet to choose between. When session (d) wires the bytecode VM into the render loop, the flag lands then.

## What does NOT ship in this session (and why)

- **Mesh loading.** Needs a vertex format, a Buffer abstraction, and a way to import `.glb` / `.obj`. Session b.
- **Shaders.** WGSL shader source + pipeline creation + bind groups. Session b.
- **Camera system.** View / projection matrices, mouse-look, smoothing. Session c.
- **Twe-side rendering API.** `on render():` hooks, `mesh()` / `camera()` builtins, `spawn` for 3D entities. Session d.
- **3D math stdlib.** `vec3`, `mat4`, `quat`, transformation helpers. Session e.
- **Hot reload.** The macroquad path polls mtime; the wgpu path doesn't yet. Cheap to add but not load-bearing for session 1.
- **Tests of the actual render loop.** No CI display; the wgpu init path can't run headless without significant scaffold. Verified by running `twec play3d examples/hello_3d.twe` locally.

## Verification

- `cargo build --release` — clean.
- `cargo clippy -- -D warnings` — clean.
- `cargo test` — 391 tests pass (no regressions).
- `twec play3d` (no args) — surfaces "requires a file path" + the updated USAGE line including `play3d <file>`.
- `twec play3d examples/hello_3d.twe` — opens a 640×480 window with a dark teal clear-color, prints the script's startup message to stdout, exits cleanly when the window closes (manually verified on the implementer's Windows machine).

## Doc edits applied as a result

- `CLAUDE.md` Phase 5 task 5 status updated.
- `docs/05-roadmap.md` Phase 5 §"Status" mentions the wgpu scaffold.
- `notes/future-phases.md` task 5 lists session (a) as shipped, (b)–(e) as the remaining sessions.
- `docs/06-design-document.md` doesn't change in this session — the user-facing 3D surface lands in session (d).
