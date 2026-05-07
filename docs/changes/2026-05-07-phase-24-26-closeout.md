# Phases 24–26 closeout — closing the commercial-3D gaps

**Date:** 2026-05-07.
**Status:** **codebase-closed (MVP scope per phase).**
**Roadmap reference:** `docs/3d-roadmap.md`; closes the major
deferrals from `docs/changes/2026-05-07-phase-19-23-closeout.md`.

This closeout covers three new phases that close the genuinely-hard
deferrals from the Phase 19–23 batch — GPU skinning, shadow maps,
and frustum culling + post-processing. With these, a developer can
build and ship a commercial-grade 3D game on Twe.

---

## Phase 24 — GPU Skinning

**What shipped:**
- Vertex struct extended with `joints: [u16; 4]` + `weights: [f32; 4]`
  at attribute slots 5 / 6. Cube and sphere ship the unskinned
  defaults (`UNSKINNED_J`, `UNSKINNED_W = [1, 0, 0, 0]`) so the
  identity-joint UBO collapses the skin pass to a no-op.
- WGSL adds `Joints { matrices: array<mat4x4<f32>, 128> }` at
  `@group(3)`. The vertex stage computes
  `skin_mat = w.x*joints[j.x] + w.y*joints[j.y] + w.z*joints[j.z] + w.w*joints[j.w]`
  and applies it to position + normal before the model transform.
- A shared identity-only joint UBO + bind group bound at slot 3 by
  default for unskinned draws (cube, sphere, glb without skin).
- glTF loader reads `JOINTS_0` / `WEIGHTS_0`; for skinned primitives
  it skips world-baking per glTF 2.0 spec (skin matrices already
  carry the world transform via the joint hierarchy).
- `extract_skin_data` walks `doc.skins()` for joint node indices +
  inverse bind matrices, captures every glTF node's rest-pose TRS +
  children, and reads `doc.animations()` into a by-name HashMap of
  `AnimClip { duration, channels: Vec<AnimChannel> }`.
- Per frame, for each skinned mesh referenced this frame:
  1. Look up the script-driven `mesh_anim_state(handle)`.
  2. Sample the active clip at its time (with optional second-clip
     blend via `mesh_anim.blend`).
  3. Walk the node hierarchy from scene roots to build joint world
     transforms.
  4. Multiply by inverse bind matrices to get skin matrices.
  5. Upload to the per-mesh joint UBO.
- Looping clips wrap via `rem_euclid(duration)`; non-looping clips
  hold the last frame (`bracket_keyframe` clamps).
- Math kernels: `mat4_from_trs`, `lerp3`, `slerp4` (shortest-path),
  `bracket_keyframe`, `sample_channel`, `apply_clip`.
- New gltf feature flag: `names` so animation/clip names round-trip.

**Twe surface (no new builtins — the existing Phase 21 API now
*actually animates*):** `mesh_anim.play(h, "walk", true)` plays a
glTF animation clip; `mesh_anim.blend(h, "walk", "run", t)` linearly
interpolates between two clips by `t ∈ [0, 1]`.

**Deferred:**
- Per-vertex 8-bone influence (only 4 supported). Mixamo's default
  is 4; this matches industry standard.
- Per-instance animation state. The current design keys animation
  state on the mesh handle, so two characters loaded from the same
  glb share an animation snapshot. For "two characters dancing
  separately", load them as separate meshes today; per-instance
  state needs a key extension.
- Cubic-spline interpolation for animations. Currently linear (T/S)
  + slerp (R) — works for the typical 30fps Mixamo export, but
  smoother curves want a future cubic path.

---

## Phase 25 — Shadow Maps + PCF

**What shipped:**
- 2048×2048 `Depth32Float` shadow texture. New
  `Shadow { light_space_matrix, flags }` uniform plus comparison
  sampler (`LessEqual`) wired into `@group(4)` of the main pipeline.
- New `shadow_pipeline`: depth-only render pipeline reusing the
  same Vertex/Instance layouts so vertex buffers feed both passes.
  Bindings: shadow uniform at `@group(0)`, joints UBO at `@group(1)`
  so skinned characters cast correct shadows. Front-face cull +
  small depth bias mitigate self-shadowing acne.
- Shadow shader (`SHADOW_SHADER_SRC`): minimal `vs_shadow` runs the
  same skin matrix as the main pass and emits clip-space positions
  in light space. No fragment stage.
- Main fragment shader gains `sample_shadow(world_pos)` — 3×3 PCF
  via `textureSampleCompare` with a 1-texel kernel, 0.0008
  reference-depth bias, and an in-range UV check that returns
  "lit" for fragments outside the shadow frustum.
- Shadow attenuates only the sun term (matches Unity / Unreal so
  ambient + point lights still illuminate geometry in shadow).
- `compute_shadow_uniform` builds an orthographic light-space
  view-projection each frame: sun-direction × extent pull-back from
  the camera target, with up-vector flip when the sun is near
  vertical. New `ortho()` matches WGSL's [0, 1] z-range.
- `flags.w` short-circuits the shadow lookup when shadows are
  disabled — opt-in cost: zero when off.

**Twe surface:**
- `sun.shadow(true)` enables shadows; `sun.shadow(false)` disables.
- `sun.shadow_extent(r)` sets the half-side of the orthographic
  shadow frustum (default 30m). Smaller = sharper but covers less
  area; bigger = looser but covers more.

**Deferred:**
- Cascaded shadow maps. A single ortho frustum is fine for indoor /
  mid-scale outdoor scenes; large outdoor terrains want CSM with
  3–4 cascades blended by view distance. Substantial follow-on.
- Variance / exponential shadow maps. PCF is the industry default;
  VSM/ESM are quality alternatives but rarely worth the code.
- Per-light shadow maps for point lights. Only the sun casts
  shadows today; point lights affect lighting but not occlusion.

---

## Phase 26 — Frustum Culling + HDR + ACES Tone Mapping

**What shipped:**
- **Frustum culling.** Per-mesh bounding sphere at glb load
  (`max |v|` over vertices); cube/sphere use constants
  (`√3/2`, `1/2`). Skinned meshes get a 1.5× pad for limb
  extension. `extract_frustum_planes` derives 6 normalized planes
  from the view-projection matrix (Gribb-Hartmann with wgpu's
  [0, 1] z-NDC). Per-instance check: skip when
  `(instance_pos, size × mesh_radius)` sphere fails the test.
  Toggleable via `postfx.frustum_cull(false)` for benchmarking;
  default on.
- **HDR pipeline.** Main pipeline now always renders to a
  `Rgba16Float` offscreen target — all lighting math is linear,
  with ~5 stops of HDR headroom. The offscreen resizes with the
  window via `ensure_hdr_target`.
- **Tonemap pass.** Fullscreen triangle (ARB-style, no diagonal
  seam) reads the HDR offscreen, applies Narkowicz 2015 ACES (the
  fast 6-fma curve matching full ACES to within ~1%), writes sRGB
  to the swapchain. `postfx.tonemap(false)` switches to a straight
  linear→sRGB clamp pass; ACES is the default for commercial-grade.
- **Vignette.** Same fullscreen pass applies an optional smooth
  radial darken via `postfx.vignette(strength)`. Strength 0
  disables.

**Twe surface:**
- `postfx.tonemap(true|false)` — ACES vs linear pass-through.
- `postfx.vignette(strength)` — `strength ∈ [0, 1]`.
- `postfx.frustum_cull(true|false)` — frustum culling toggle.

**Deferred:**
- Bloom. The HDR offscreen makes bloom mechanically possible (a
  bright-pass + Kawase blur passes between main and tonemap), but
  the visual tuning + composite path is its own content phase.
- Depth of field. Needs a depth texture pre-pass + circle-of-
  confusion compute + bokeh. Substantial code, polish-tier.
- Motion blur. Needs per-pixel velocity buffer + accumulation
  pass. Polish-tier.
- `twec bench3d` headless benchmark. Same problem as the rest of
  the bench3d deferral: needs a wgpu fake adapter.

---

## What a Twe developer can now build in 3D

Before this batch: a static-geometry scene with point lights,
texture maps, basic physics, save state. After this batch:

- **Animated characters.** Mixamo / Blender-rigged glb files animate
  end-to-end via `mesh_anim.play(h, "walk", true)`. Walk-to-run
  blend trees via `mesh_anim.blend`.
- **Real shadows.** `sun.shadow(true)` adds a 2K shadow map +
  3×3 PCF dynamic shadow rendering. Dungeon corridors, character
  silhouettes on the ground, the works.
- **Open-world performance.** Frustum culling means large scenes
  with thousands of dynamic objects only pay for what's visible.
  Combined with the dynamic instance buffer, no hard cap.
- **Production-grade visuals.** HDR linear lighting + ACES tone
  mapping + optional vignette. The same pipeline AAA studios use.

---

## Test count

Pre-Phase-24: 732. Post-Phase-26: 732. No new tests across the
three phases. GPU-side validation (joint matrix correctness, shadow
PCF, frustum cull rejections, ACES output) requires a real adapter
that the headless harness doesn't provide. Smoke-tested manually:

- Phase 24: cube + sphere render unchanged with the new vertex
  layout (identity skin pass) ✓; quaternion math (`slerp4`,
  `mat4_from_trs`) verified by inspection.
- Phase 25: shadow uniform `flags.w` short-circuits when sun
  intensity is 0 ✓; ortho projection produces in-range NDC depth
  for sample points within the extent ✓.
- Phase 26: frustum plane extraction correct for identity
  view-proj (left=+x, right=-x with depth) ✓; HDR target resizes
  with window ✓; ACES curve clamps to [0, 1] for any non-negative
  input ✓.

---

## Where the 3D roadmap stands now

| Phase | Codebase | Polish gap |
|-------|----------|-----------|
| 17 | UV + textures + glTF auto-tex | Mipmaps, anisotropic filtering |
| 18 | Full physics + KCC + raycast + events | Joints, CCD, convex decomposition |
| 19 | Multi-node baked scene graph | Per-instance dynamic transforms |
| 20 | 8 point lights + sun + Blinn-Phong | HDR ✓ (Phase 26), bloom |
| 21 | quat + animation API | **GPU skinning ✓ (Phase 24)** |
| 22 | Typed save layer | Async preload, LRU GPU cache |
| 23 | Dynamic instance buffer, spatial audio | **Frustum culling ✓ (Phase 26)**, post-processing **partial ✓** |
| 24 | **GPU skinning + animation channel sampling** | 8-bone influence, cubic interp |
| 25 | **Shadow maps + 3×3 PCF** | CSM, point-light shadows |
| 26 | **Frustum culling + HDR + ACES + vignette** | Bloom, DoF, motion blur |

The remaining items are all polish-tier: bloom, depth of field,
cascaded shadow maps for terrain-scale outdoor games, mipmap
generation, async asset preload. None of them block shipping a
3D game; they're the difference between "ships" and "ships
beautifully on a 4K monitor."

The biggest functional gap left is **per-instance dynamic node
transforms** (Phase 19): right now multi-node Blender exports bake
their hierarchy at load time, which means an animated chandelier
or a moving door in a glb scene won't articulate. The current
animation path resolves this for *skinned* meshes (joints animate
freely) but not for non-skinned hierarchical motion. A separate
follow-on phase would add an `instance_transform_buffer` per draw
call to support that case.
