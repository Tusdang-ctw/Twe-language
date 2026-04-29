# 2026-04-29 — Phase 5 task 5 sessions (d) + (e): Twe-driven 3D rendering

## Status: implementation note. Sessions (d) and (e) of the multi-session 3D backend.

## Background

Sessions (a) (`docs/changes/2026-04-29-phase-5-task-5-session-1-wgpu-scaffold.md`) and (b)+(c) (`docs/changes/2026-04-29-phase-5-task-5-sessions-bc-cube-and-camera.md`) opened the wgpu window and made it render a hardcoded rotating cube. This commit hands the scene over to the Twe script: a top-level `on render():` body queues cubes via a `cube(at:, color:, size:)` builtin, and `camera.eye`/`.target`/`.up` are mutable ambient fields the script writes from `on update(dt):`.

The 3D math stdlib (session e) collapses to a single `vec3(x, y, z)` constructor — Twe's tuples already expose `.x`/`.y`/`.z` and component-wise arithmetic, so a 3-tuple already behaves as a vec3. `mat4` and `quat` only ship when something actually consumes them.

## What ships

### Session (e) — `vec3` constructor + math primitives

- **`vec3(x, y, z)`** builtin — returns a `Value::Tuple` of three floats. Tuples already support `+` / `-` (component-wise), scalar `*` / `/`, and `.x` / `.y` / `.z` field access (`src/eval.rs::field_get`). The constructor is sugar that reads better than `(x, y, z)` at call sites.
- **`math.sin(x)`** and **`math.cos(x)`** builtins, plus the constant **`math.pi`**. Required by any nontrivial camera math; the demo orbits the camera with `cam_distance * math.sin(t * 0.6)`.
- **No `mat4` / `quat`**. Adding them now would lock the API before anything consumes them — Phase 5 task 5's GPU side carries its own column-major matrices in `src/play3d.rs`. When session (d) wants `mat4` for `entity.transform` or session (5+) wants `quat` for orientation interpolation, those types ship driven by the consumer.

### Session (d) — Twe-driven 3D scene

- **AST**: new `Stmt::OnRender { body, line, col }` — distinct from the state-scoped `StateMember::OnRender` (which is the 2D macroquad path). Top-level form is the 3D entry point.
- **Parser**: `parse_on` accepts `update` and now `render` at the top level. Other event names (`click`, `tap`, …) error with a helpful message pointing at `on update(dt):` / `on render():`.
- **Eval**: `Stmt::OnRender` arm registers the body on `Env::top_on_render: Option<Vec<Stmt>>`. New `pub fn eval::render_frame3d(env)` clears `env.render_queue3d`, sets `in_render = true`, runs the body, and returns. The caller drains the queue.
- **Bytecode VM**: rejects top-level `on render():` at compile time with a pointer at `--vm tree`. The 3D path runs only against the tree-walker for v0.1; bytecode 3D wires in alongside the rest of the per-frame tick integration in a future session.
- **Stdlib `cube(at:, color:, size:)`** — guarded by `require_render`, validates argument shapes, pushes a `DrawCall3d { at: [f32; 3], color: [f32; 4], size: f32 }` onto `env.render_queue3d`. Helpers `xyz_of` and `rgba_of` extract typed vec3/RGBA from Twe tuples with descriptive errors on mismatch.
- **Stdlib `camera`** — registered as a `Value::Object` with default `eye = vec3(0, 1.5, 3)`, `target = vec3(0, 0, 0)`, `up = vec3(0, 1, 0)`. The script mutates fields via `camera.eye = vec3(...)`. The `play3d` render loop reads them each frame.
- **`Env`** gains `top_on_render: Option<Vec<Stmt>>` and `render_queue3d: Vec<DrawCall3d>`. Both initialised empty/None.
- **`DrawCall3d`** is a public struct on `value::*` so both `stdlib::cube` and `play3d::render` can construct/consume it.

### `play3d` integration

- `App` holds the live `Env` across the event loop (previously `initialize` ran the script and dropped env). `RenderState` no longer has `started_at` — the rotation now comes from the script's own `on update(dt):` handler.
- Per-frame flow:
  1. `eval::render_frame3d(&mut env)` runs the script's `on render():` body. It pushes cubes onto `env.render_queue3d`.
  2. `read_camera(&env)` snapshots `camera.eye/.target/.up` — falls back to defaults for missing/malformed fields so a script that never touches the camera still gets a viewable scene.
  3. The queued cubes are packed into a per-instance vertex buffer (preallocated to `MAX_INSTANCES = 4096`).
  4. One **instanced draw call** renders all cubes: `draw_indexed(0..36, 0, 0..N)` where N is the queue length (capped at `MAX_INSTANCES`).
- **Vertex layout simplification**: dropped per-vertex RGB color from session (b)/(c). Kept per-vertex `brightness` (one f32 per vertex, six values across the cube — one per face) so faces stay visually distinct even when every cube shares one user-supplied color. Fragment shader: `instance_color.rgb * brightness`.
- **Per-instance attributes** at locations 2 and 3: `inst_pos_size` (vec4: xyz + uniform scale packed) and `inst_color` (vec4 RGBA).
- **Time-based rotation removed** — the rotation now lives in Twe (the demo's `on update(dt):` writes `camera.eye`). The `started_at: Instant` field is gone.

### Demo program

`examples/hello_3d.twe` now drives a real scene:

```twe
let radius = 2.0
let cam_distance = 4.0
var t = 0.0

on update(dt):
    t = t + dt
    camera.eye = vec3(cam_distance * math.sin(t * 0.6), 1.5,
                      cam_distance * math.cos(t * 0.6))
    camera.target = vec3(0, 0, 0)

on render():
    cube(at: vec3(0, 0, 0),         color: (0.95, 0.95, 0.95, 1.0), size: 0.8)
    cube(at: vec3( radius, 0, 0),   color: color.red,    size: 0.5)
    cube(at: vec3(-radius, 0, 0),   color: color.green,  size: 0.5)
    cube(at: vec3(0, 0,  radius),   color: color.blue,   size: 0.5)
    cube(at: vec3(0, 0, -radius),   color: color.yellow, size: 0.5)
    cube(at: vec3(0,  radius, 0),   color: color.purple, size: 0.5)
    cube(at: vec3(0, -radius, 0),   color: color.orange, size: 0.5)
```

A central white cube with a ring of six smaller colored cubes. The camera orbits the origin once every ~10 seconds. Type-checks cleanly (`twec types` reports `cam_distance: float`, `radius: float`, `t: float`).

## Architectural notes

### Top-level `on render():` ≠ state-scoped `on render():`

These are **distinct constructs**. The state-scoped form (`StateMember::OnRender`, exists since the 2D macroquad work) fires inside `eval::render_frame` and is for sprite drawing. The top-level form (`Stmt::OnRender`, this commit) fires inside `eval::render_frame3d` and is for cube/mesh drawing. They share a name because both compose "the next frame," but they have different scopes and different builtins. A `cube()` call at top level only makes sense in 3D; a `sprite()` call only makes sense in 2D. The runtime distinguishes them by which event loop is running (`twec play` vs `twec play3d`).

### Vec3 = 3-tuple, not a new value variant

The simplest design that ships. Tuples already do everything a vec3 needs. Adding `Value::Vec3` would mean updating every `match` over `Value` — there are dozens — for zero new behaviour. The `vec3(x, y, z)` constructor exists purely as a readability shim at call sites. If profiling later shows tuple boxing as a hot path, the optimisation can land transparently.

### Instanced rendering, not per-cube draws

The per-instance buffer is preallocated to `MAX_INSTANCES = 4096` cubes. Each frame, the script's queue is packed into the buffer (zero-allocation on the GPU side) and a single `draw_indexed` issues N instances. This scales linearly in CPU work (the queue copy) and constant-cost on the GPU. A "1 draw per cube" approach would have hit GPU command-stream limits at maybe a few hundred cubes; the instanced path runs comfortably at 4096+.

### Camera as an Object, not a uniform-only construct

The script reads/writes `camera.eye` like any other Twe object field. There's no special "camera type" — the GPU side reads three vec3 fields out of a stdlib-installed Object each frame. This keeps the user-visible surface small (no new type, no new syntax) while letting future sessions add fields (`fov`, `near`, `far`, `mode: orthographic | perspective`) without breaking anything.

### Fallbacks for missing camera fields

`read_camera` returns sensible defaults when `camera.eye`/`.target`/`.up` are missing or malformed (eye 3 units back + 1.5 up, looking at origin, +y up). A script that never touches the camera still gets a viewable scene. A script that sets `camera.eye = "broken"` gets the default (string isn't a 3-tuple) rather than a crash. Errors are silent — the right place to surface them is the type system once strict mode lands.

### `print` output drained per frame

`on render():` may call `print(...)` for debugging. The play3d loop drains `env.out` at the end of each frame to stdout. Without this, debug output would accumulate forever. Same pattern as session (a)'s startup-output drain.

## What does NOT ship in this session (and why)

- **`.glb` / `.obj` mesh import**. Choosing a glTF crate (`gltf` is the standard but pulls a lot in; `easy-gltf` is lighter; hand-rolled OBJ first is even lighter) is its own dependency conversation. For now, scenes are made of cubes. When the user's actual game needs more, the crate choice can be informed by the actual file shapes coming through.
- **winit input → `key.*`**. The current `key.*` infrastructure is macroquad-backed; bringing winit input into the same Object surface is its own refactor (probably refactor `key.*` into a backend-agnostic structure that both macroquad and winit can write to). For session (d), the demo gets by with internal time-based animation.
- **Bytecode VM 3D path**. The compiler errors on top-level `on render():` with a pointer at `--vm tree`. Bytecode VM dialogue / 3D / function-body wait all share a "wires per-frame interpreter calls into the host loop" pattern; doing them together makes more sense than piecemeal. Likely a single bytecode-driven session.
- **Hot reload**. The macroquad `play` loop polls mtime and rebuilds the env on file change. The wgpu `play3d` loop doesn't yet — a follow-on once everything else stabilises.
- **`mat4` / `quat`**. Twe types for matrix/quaternion math. Not consumed by anything in v0.1; lands when the user-facing surface (`entity.transform`, `mesh.scale_to(...)`) needs them.
- **Lighting beyond per-face brightness**. A real light source (directional / point / ambient) would make scenes more readable. The brightness-per-face shading is a sufficient floor for "is the cube three-dimensional?" — anything more is polish.
- **Multiple primitives**. Just `cube()` for now. A `sphere()` / `cylinder()` / generic `mesh()` ride later sessions, the latter wanting `.glb` import first.

## Verification

- `cargo build --release` — clean.
- `cargo clippy -- -D warnings` — clean.
- `cargo test` — 397 tests pass (no regressions; one existing test updated to match the new `on <event>` error message).
- `twec types examples/hello_3d.twe` — clean; reports `cam_distance: float`, `radius: float`, `t: float`.
- `twec parse examples/hello_3d.twe` — produces `OnUpdate` and `OnRender` AST nodes; `cube()` calls round-trip with proper kwargs.
- `twec run --frames 3 examples/hello_3d.twe` — runs headlessly without crashing (the `on render():` handler registers but never fires outside `twec play3d`).
- `twec play3d examples/hello_3d.twe` — opens the wgpu window, the demo's central + ring of cubes appears, the camera orbits over time. Manually verified by the implementer.

## Doc edits applied as a result

- `notes/future-phases.md` task 5 lists sessions (a)–(e) as shipped; the carried list captures the explicit follow-ons (`.glb` import, winit input, bytecode 3D, hot reload, `mat4` / `quat`).
- `CLAUDE.md` Phase 5 task 5 status updated.
- `docs/05-roadmap.md` Phase 5 §"Status" reflects the round-trip Twe→wgpu shipping.
- `docs/06-design-document.md` §4.6 / §4.10a unchanged this commit; the 3D rendering surface gets its own §4.10b in a follow-on once `.glb` import + lighting decisions stabilise (the surface today is the minimum viable; documenting it more thoroughly before it grows would just create rework).
