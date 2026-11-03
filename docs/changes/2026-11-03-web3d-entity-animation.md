# Per-entity skeletal animation (Lanternfall E4)

**Date:** 2026-11-03
**Line:** Lanternfall engine enablers, E4 (fourth of E1–E6)
**Status:** shipped.

## Why

Animation state was keyed by mesh path (`stdlib/mesh_anim.rs`), and the
renderer had one joint buffer per mesh. So every entity drawing
`bot.glb` played the same clip at the same time. Lanternfall has 40 bots
running, shooting, reloading and dying independently, and lockstep
animation is what makes a crowd look fake.

## Decisions

- **One way to animate (Principle 2).** The existing `mesh_anim.*`
  functions accept an entity where they accepted a mesh path; nothing new
  to learn. The mesh-path form keeps its meaning (shared by every entity
  without its own), so `survive3d`, `action3d` and their tests are
  unchanged.
  - Rejected: stateless `anim:` / `anim_time:` look keys. Every script
    would then re-implement crossfades and blend-space bookkeeping, which
    `mesh_anim` already does.
- **The state lives on the entity.** `value::Instance.anim:
  Option<Box<AnimEntry>>` dies with the entity, so there is no map keyed
  by pointer to prune and nothing to leak.
  - `AnimEntry` (moved to `render3d_types`) gains `advance(dt)` and
    `snapshot()`.
  - `mesh_anim.advance` steps the shared entries and every live entity's.
- **One joint palette per frame, not a joint buffer per pose.**
  - Every pose's joint matrices are packed back to back into one
    read-only storage buffer bound at the joints slot for every draw.
  - A skinned instance's `glow.w` (free since E3 moved the flags) is
    `1 + its first matrix`.
  - So 40 bots in 40 poses stay one instanced draw per pass. The
    alternative, splitting draws per pose, would have multiplied draw
    calls by the number of poses in every pass (main, three shadow
    cascades, prepass, point shadows).
  - The per-mesh 8 KB uniform buffers and the identity buffer are gone.

## What changed

- **Script side.**
  - `mesh_anim::AnimTarget` is `Mesh(id) | Entity(instance)`; `with_entry`
    creates an entity's animation on first use and bumps the look epoch.
  - `stop(entity)` drops it, returning the entity to the mesh's shared
    animation.
  - `gather_looks` sends animated entities down the per-entity path, and
    `emit_looks` pushes their snapshots to `Env::instance_anims`. Each
    such draw carries `DrawCall3d.anim = n` (0 = the mesh's shared pose).
  - `RenderSnapshot.instance_anims` hands the snapshots to the kernel.
- **Kernel.**
  - **Building the palette.** `JointPalette::base(skin, mesh, anim)` poses
    each `(mesh, anim)` once (`compute_skinned_joint_matrices`), keeps only
    the skin's joints, and returns the pose's base. Opaque mesh groups and
    translucent mesh draws assign bases while the instances are built.
  - **Retained frames** (only possible with no per-entity looks, so only
    shared poses) re-lay the shared poses in build order, so the bases the
    kept instances hold stay right.
  - **Upload.** The palette uploads once per frame; the buffer grows to the
    next power of two and the old one is destroyed explicitly.
  - **Shaders.** The main, shadow / prepass and mask shaders read
    `joints_u[jb + joint]` from `var<storage, read> joints_u:
    array<mat4x4<f32>>`.

## Limits

- Morph targets still follow the mesh's shared animation. CPU morphing
  rewrites the mesh's one vertex buffer, so per-entity morphs would need
  per-instance vertex data.
- Poses aren't deduplicated across entities. Two bots in the same clip at
  the same time are posed twice, which is cheap at this scale.

## Measured

`tests/render_bench.rs::render_animated_crowd` (release, native, RTX
laptop): 40 skinned heroes cost 0.25 ms of kernel CPU per frame with an
animation each, and 0.37 ms sharing one. The difference is noise; posing
40 rigs is well within budget. The wasm build will be slower: re-measure
in Chrome at G3.

## Verified in the browser

A multi-file test game built with `twec build --target web` and captured
in headless Chrome (WebGPU, `bench/graphics/browser-capture.mjs`):
- two heroes share `hero.glb`, one mid-attack and one idle;
- a glowing tracer beam is tilted by `quat.look_along` (E3);
- name tags are placed by `camera.project` (E1);
- both entity classes come from an imported module (E5).

So the joint palette, a storage buffer read in the vertex stage, works in
the browser as natively.

## Open

`tests/kernel_render.rs` crashed twice during a full `cargo test
--workspace` run (once in the E3 session, once here): the binary exits
with no failing test reported. It hasn't reproduced in 13 isolated runs
of the binary or in the next full run. It may be a GPU-driver or
allocator flake under the long suite, like the pre-M6
`STATUS_HEAP_CORRUPTION` one, or something the 80-byte instances / joint
palette exposed. The next step is to capture the exit status when it
recurs.

## Tests

- **`tests/kernel_render.rs::entities_sharing_a_mesh_animate_apart`
  (GPU):** two heroes sharing `hero.glb`. Changing the left one's clip
  changes over 150 pixels of the left half and fewer than 20 of the right.
- **The 47 other renderer tests**, including the skinned
  `hero_renders_mid_stride` and GPU culling, still pass on the palette
  path.
- **`tests/entity_anim.rs`:**
  - own clip and clock per entity;
  - per-draw snapshot indices;
  - a crossfade only on the entity that changed clip;
  - `stop` returns an entity to the shared animation;
  - help text for a bad handle.
