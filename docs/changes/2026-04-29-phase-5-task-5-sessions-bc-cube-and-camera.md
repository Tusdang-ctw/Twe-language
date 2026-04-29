# 2026-04-29 — Phase 5 task 5 sessions (b) + (c): cube pipeline + camera

## Status: implementation note. Sessions (b) and (c) of the multi-session 3D backend.

## Background

Session (a) (`docs/changes/2026-04-29-phase-5-task-5-session-1-wgpu-scaffold.md`) opened the wgpu window and cleared it to a single colour. This commit lands two more sessions in one shot:

- **Session (b)**: vertex / index buffers, a WGSL flat-shading pipeline, a depth attachment.
- **Session (c)**: a uniform-buffer camera with hand-rolled column-major matrix math (perspective + look-at + rotate-Y).

Result: `twec play3d examples/hello_3d.twe` shows a colored cube rotating in 3D space. Front-face red, back green, right blue, left yellow, top cyan, bottom magenta — six per-face flat-shaded colours so the geometry is unambiguous on first look.

## What ships

### Session (b) — pipeline

- **`bytemuck = { version = "1", features = ["derive"] }`** — required as soon as the GPU sees anything other than a clear color. Used for the `Vertex` and `CameraUniform` structs (both `Pod + Zeroable`).
- **`Vertex { position: [f32; 3], color: [f32; 3] }`** with a `wgpu::vertex_attr_array!` layout. Right-handed model space, +x right, +y up, +z toward the camera.
- **`CUBE_VERTICES`** — 24 vertices (4 per face) so each face can carry a single solid colour without per-vertex blending across face boundaries.
- **`CUBE_INDICES`** — 36 indices (12 triangles, 2 per face), counter-clockwise winding when viewed from outside. Pairs with `FrontFace::Ccw` + `Face::Back` cull mode in the pipeline so the back-facing triangles drop.
- **WGSL shader** as a `const &str`. Two entry points: `vs_main` multiplies the per-vertex position by `camera.view_proj` and forwards the colour; `fs_main` writes the colour as RGBA. No lighting, no textures.
- **Render pipeline** with a depth-stencil attachment (`Depth32Float`, `Less` compare). Without it, faces drawn later would overwrite faces drawn earlier regardless of distance from the camera. The depth view is recreated on resize so its size always matches the swapchain.
- **Vertex / index buffers** uploaded once via `wgpu::util::DeviceExt::create_buffer_init` — GPU-resident for the program's lifetime.

### Session (c) — camera

- **`CameraUniform { view_proj: [[f32; 4]; 4] }`** — single 4×4 matrix uploaded each frame. Column-major to match WGSL `mat4x4<f32>`'s memory layout, so `bytemuck::bytes_of` is a direct upload — no transpose.
- **Bind group layout + bind group** for the camera uniform at `@group(0) @binding(0)`, visible to the vertex stage only.
- **Hand-rolled matrix math** (`perspective`, `look_at`, `rotate_y`, `mul`, plus `dot` / `cross` / `normalize`). Right-handed view space, depth into [0, 1] (wgpu / Vulkan / D3D clip-space convention). No `glam` dependency — Twe's own `vec3` / `mat4` types ship in session (e), and pinning a math crate before that decision would be premature.
- **Per-frame uniform write** via `queue.write_buffer`. The rotation angle is `Instant::elapsed().as_secs_f32() * 0.7`, so the cube turns ~0.6 turns per second — slow enough to read each face's colour, fast enough that the 3D is unambiguous.

### 5 new unit tests in `src/play3d.rs::tests`

The matrix math is the kind of code that's easy to typo (transpose-confusion, wrong sign, etc.). Five tests pin the contract:

- `mul_identity_is_identity` — `I·M = M·I = M` for a perspective matrix.
- `rotate_y_zero_is_identity` — sanity floor for the rotation helper.
- `rotate_y_quarter_turn_swaps_x_and_z` — pins the rotation direction (right-handed: +x → -z under +90°).
- `look_at_eye_at_origin_facing_minus_z_is_identity_axes` — pins the view-matrix orientation.
- `perspective_maps_near_plane_to_zero_depth` — pins the clip-space depth convention (wgpu/Vulkan/D3D, not OpenGL's [-1, 1]).

These run on every `cargo test` and don't need a display.

## Architectural notes

### Right-handed, +y up, depth ∈ [0, 1]

This is the wgpu-native convention. Picking it now (rather than mirroring an OpenGL-style depth ∈ [-1, 1] perspective and calling glam's `..._infinite_lh_zo` later) means the matrix code stays simple and the WGSL doesn't need any flips.

### Column-major matrices stored as `[[f32; 4]; 4]` of *columns*

`m[c][r]` reads "column c, row r." WGSL `mat4x4<f32>` reads memory the same way, so a `bytemuck::cast` of the array uploads a matrix that behaves identically on the GPU. The alternative — row-major in Rust + transpose-on-upload — is more error-prone for one bytes-saved per matrix.

### No glam (yet)

Twe's roadmap has a 3D math stdlib in session (e). The user-facing `vec3` / `mat4` types should drive the GPU side, not the other way around. Adding glam now would either pollute the public surface (if reused for the stdlib) or duplicate effort (if the stdlib reaches for its own types). Hand-rolled in session (c); the math may stay forever — three `f32` elements and a 4×4 matrix don't justify a dependency.

### Depth attachment recreated on resize

The depth texture's size has to match the swapchain. Without recreating it, resizing the window past the initial size triggers a wgpu validation error. `WindowEvent::Resized` rebuilds it (alongside reconfiguring the surface).

### `RenderState::started_at: Instant`

Drives the rotation angle. When session (d) lands the Twe-side `on render():` hook, this becomes a per-frame `dt` flowing from the script's frame loop instead of an internal `Instant`.

## What does NOT ship in this session (and why)

- **`.glb` / `.obj` mesh import**. The cube is hardcoded in Rust. File-loading meshes is its own dependency conversation (which crate? `gltf`? `easy-gltf`? hand-rolled OBJ?). Better folded into session (d) when the Twe-side `mesh()` API also lands — that pins the file-loading API by the user's needs, not by a Rust-side default.
- **Lighting**. Per-face flat colours give the cube enough visual structure to be unambiguous as 3D. Per-fragment lighting (Lambertian / Blinn-Phong) is a follow-on; needs a normal attribute on `Vertex`.
- **Texture mapping**. Same — needs `tex_coord` on `Vertex`, a sampler binding, and an image loader.
- **Multiple meshes / instances**. One cube, one draw call, no instancing. Session (d)'s `spawn`-with-mesh introduces the multi-instance path.
- **Mouse-look / WASD camera**. The camera is fixed at `(0, 1.5, 3)` looking at the origin. Input wiring rides session (d) too — the existing `key.*` infrastructure is macroquad-backed; bringing winit input into the runtime is its own work.
- **Hot reload**. The macroquad `play` path polls mtime; the wgpu path doesn't yet. Cheap to add but not load-bearing.

## Verification

- `cargo build --release` — clean (16s rebuild after the bytemuck add).
- `cargo clippy -- -D warnings` — clean.
- `cargo test` — 396 tests pass (5 new matrix-math tests; no regressions).
- `twec play3d examples/hello_3d.twe` — opens a 640×480 window with a rotating coloured cube on a dark teal background. Six per-face colours, back-face culled, depth-tested. Closes cleanly. (Manually verified by the implementer; CI can't run the wgpu loop.)

## Doc edits applied as a result

- `notes/future-phases.md` task 5 lists sessions (a), (b), (c) as shipped; (d) and (e) remain.
- `CLAUDE.md` Phase 5 task 5 status updated.
- `docs/05-roadmap.md` Phase 5 §"Status" mentions cube + camera.
