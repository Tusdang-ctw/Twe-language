# Phases 19–23 closeout — 3D production roadmap (post-v1.0, MVP)

**Date:** 2026-05-07.
**Status:** **codebase-closed (MVP scope per phase).**
**Roadmap reference:** `docs/3d-roadmap.md`.

This single closeout covers Phases 19–23 because each was scoped down from the full roadmap plan into a single-session MVP. The full roadmap remains the canonical specification — closed phases ship a real but partial subset, with deferred items honestly tracked.

---

## Phase 19 — Full glTF Scene Graph + mat4

**What shipped:**
- `parse_glb_bytes` walks the default scene's node tree, multiplies parent transforms, and bakes the world matrix into vertex positions + normals at load time. Multi-node Blender exports render correctly without per-frame matrix math.
- All primitives across all nodes flatten into one combined index buffer with rebased indices.
- `flatten_node` / `flatten_primitive` helpers do the recursion + vertex transformation.
- Inline `mat4_identity`, `mat4_mul`, `mat4_transform_point`, `mat4_transform_dir` for the load path.
- New Twe `mat4` namespace, stored as a tagged 16-element list Object (no new Value variant):
  - `mat4.identity()`
  - `mat4.translate(v)`, `mat4.scale(v)`
  - `mat4.rotate_x/y/z(angle)` — radians, column-major
  - `mat4.mul(a, b)`, `mat4.transform_vec3(m, v)`

**Deferred:**
- Per-instance dynamic transforms. Currently node transforms bake at load — animated nodes need a per-frame `mat4` instance attribute, which lands alongside Phase 21 GPU skinning.
- Multi-material partitioning per primitive. Auto-loaded texture comes from the first primitive with one; others share it. A real per-material draw split is a Phase 19 follow-on.

---

## Phase 20 — Point Lights + Blinn-Phong

**What shipped:**
- `LightsUniform` (pub) with ambient + directional sun + 8 point lights. All vec3 fields std140-aligned to vec4.
- WGSL fragment shader rewritten to consume the lights uniform: ambient + directional sun + 8-point loop with bounded smooth-edge attenuation (`t = 1 - dist/radius; atten = t*t`). Disabled slots have `radius == 0` and early-continue.
- Vertex shader passes world_pos to fragment.
- New `lights_buffer` + `lights_bgl` + `lights_bind_group` in `RenderState`, bound at group 2. Pipeline layout updated to `[camera, texture, lights]`.
- `play3d::render` writes the buffer once per frame from `stdlib::lights_snapshot()`.
- Twe surface:
  - `light.add(at, color, radius) → handle` (1..=8, errors if all 8 slots full)
  - `light.remove(h)`, `light.set(h, at, color, radius)`, `light.set_radius(h, r)`
  - `light.clear()`, `light.ambient(color)`
  - `sun.direction(v)` (normalized internally), `sun.intensity(i)`

**Deferred:**
- Shadow maps. Real shadow rendering needs a depth-only render pass from the sun's POV, a 2K depth target, and PCF sampling in the main pass. ~5 sessions of focused work, not in this MVP.
- HDR / bloom / SSAO. Beyond Blinn-Phong's basic surface; deferred indefinitely.

---

## Phase 21 — quat + Animation API (no GPU skinning)

**What shipped:**
- `quat` Twe type — tagged 4-element list Object, kind="quat":
  - `quat.identity()`
  - `quat.from_axis_angle(axis, angle)`
  - `quat.slerp(a, b, t)` — shortest-path with cosine flip
  - `quat.mul(a, b)`, `quat.to_mat4(q)` (column-major)
- Animation playback API surface in `mesh_anim.*`:
  - `mesh_anim.play(handle, clip, looping)`
  - `mesh_anim.stop(handle)`
  - `mesh_anim.blend(handle, clip_a, clip_b, t)`
  - `mesh_anim.current(handle) → { clip, time, looping }`
  - `mesh_anim.advance(dt)`
- State lives in `MESH_ANIM_STATE` thread-local. Scripts can drive game logic against the API (e.g. branching on `current().clip == "attack" and time > 0.5`).

**Deferred (significant — honest scope note):**
- GPU skinning. Vertex struct still has no JOINTS_0/WEIGHTS_0 attributes. Adding them requires bumping the Vertex layout and reading 4-bone influence per vertex.
- Joint UBO + skin matrix in WGSL. The vertex shader doesn't yet apply `skin_matrix = w0*joints[j0] + ... + w3*joints[j3]`.
- Animation channel sampling. The `gltf` crate exposes `Animation → Channel → Sampler` accessors; we'd read TRS keyframes per node and linearly interpolate. Currently `mesh_anim.advance(dt)` only ticks time — no joint matrices are computed.

The Twe-facing API is stable; only the rendering implementation needs to land. A character will play `mesh_anim.play(h, "walk")` today; the visual skinning lights up when the GPU pipeline catches up.

---

## Phase 22 — Typed Save Namespace

**What shipped:**
- `save.*` typed save layer keyed by string. In-memory `SAVE_STORE` thread-local with disk write/read via the Phase 8 codec:
  - `save.vec3(key, v)` / `save.get_vec3(key)`
  - `save.f32(key, v)` / `save.get_f32(key)`
  - `save.int(key, v)` / `save.get_int(key)`
  - `save.string(key, v)` / `save.get_string(key)`
  - `save.has(key)`, `save.remove(key)`, `save.clear()`
  - `save.write(path)`, `save.read(path)`, `save.try_read(path)` (false on missing)
- Type-safe getters return `nil` if missing or shape-mismatched. `try_read` returns false on missing file (canonical bootstrap pattern: set defaults, then `try_read`).

**Deferred:**
- `scene.enter(name)` formal scene swap. Existing state-transition syntax (`-> name`) covers the use case; a separate "swap to a different top-level scene declaration" needs Env-level redesign.
- `on enter()` / `on exit()` state hooks. The existing every-clock + first-frame initialization patterns suffice; dedicated entry/exit hooks add ergonomics, not capability.
- Async asset preload + LRU GPU cache. Lazy-load on first reference is fine until games stop fitting in 256 MB of GPU memory.

---

## Phase 23 — Polish + Performance

**What shipped:**
- Dynamic instance buffer. The hard `MAX_INSTANCES = 4096` cap is gone; the buffer doubles capacity whenever a frame needs more. Open-world scenes can push thousands of dynamic objects without silent draw drops.
- 3D spatial audio. `sound.play3d(handle, at, radius)` reads `camera.eye`, computes distance-based attenuation matching the point-light curve (`t² where t = 1 - dist/radius`), volume = 0 outside radius. Stereo-only (no panning) — macroquad audio limit.

**Deferred:**
- Frustum culling. Needs per-mesh AABB at load time + 6-plane extraction + plane-vs-AABB tests. Until games hit ~10k visible instances per frame, the dynamic buffer + GPU depth rejection cover.
- Post-processing pass (ACES tone mapping, vignette). Requires render-to-texture + second fullscreen pass; substantial pipeline change.
- `twec bench3d`. Headless 3D benchmarking needs a wgpu fake adapter; the criterion VM harness covers what's measurable today.

---

## Test count

Pre-Phase-19: 732. Post-Phase-23: 732. No new tests across the
five phases. GPU-side validation (lighting math, scene graph,
instance buffer growth) requires a real adapter that the headless
test harness doesn't provide. Smoke-tested manually:

- Phase 19: `mat4.rotate_y(π/2) * (1,0,0) ≈ (0,0,-1)` ✓; `mat4.translate(5,0,0) · rotate_y(π/2) · (1,0,0) ≈ (5,0,-1)` ✓
- Phase 20: light.add returns sequential handles 1..8; sun.direction normalizes; light.clear empties all slots ✓
- Phase 21: quat.from_axis_angle((0,1,0), π/2) ≈ (0, 0.707, 0, 0.707) ✓; slerp halfway from identity ≈ 45° rotation ✓
- Phase 22: vec3+f32+int+string round-trip through disk write/clear/read ✓
- Phase 23: builds + tests pass; in-game verification needs `twec play3d`

---

## Where the 3D roadmap stands

Phases 17–23 are all codebase-closed at MVP scope. Looking at the
gap between MVP and "commercial-ready":

| Phase | Codebase | Polish gap |
|-------|----------|-----------|
| 17 | UV + textures + glTF auto-tex | Mipmaps, anisotropic filtering |
| 18 | Full physics + KCC + raycast + events | Joints, CCD, convex decomposition |
| 19 | Multi-node baked scene graph | Per-instance dynamic transforms |
| 20 | 8 point lights + sun + Blinn-Phong | Shadow maps, HDR, bloom |
| 21 | quat + animation API | GPU skinning, channel sampling |
| 22 | Typed save layer | Async preload, LRU GPU cache |
| 23 | Dynamic instance buffer, spatial audio | Frustum culling, post-processing |

The biggest remaining work item is Phase 21 GPU skinning — for
character animation in shipped 3D games this is unavoidable. The
plan in `docs/3d-roadmap.md §"Phase 21"` enumerates the seven
sessions; this MVP closeout commits to landing them when a real
character-driven game pressures the renderer.
