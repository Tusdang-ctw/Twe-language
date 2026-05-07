# Phase 18 closeout — Physics + Collision (post-v1.0, MVP)

**Date:** 2026-05-07.
**Status:** codebase-closed (MVP).
**Roadmap reference:** `docs/3d-roadmap.md` §"Phase 18".

---

## What shipped

Phase 18 ran in one commit (`phase-18: session 1`):

- **Crate:** `rapier3d = "0.22"`, default-features=false +
  `dim3` + `f32`. Pure Rust, no native deps. `parallel` (rayon)
  feature dropped because the play loop is single-threaded.
- **`src/physics3d.rs`:** thread-local `PhysicsWorld` wrapping a
  full rapier pipeline (PhysicsPipeline + RigidBodySet +
  ColliderSet + IslandManager + DefaultBroadPhase + NarrowPhase
  + ImpulseJointSet + MultibodyJointSet + CCDSolver +
  QueryPipeline). Twe-side handles are `u32` keys into a
  `HashMap<u32, RigidBodyHandle>` so scripts get flat numbers.
- **Twe surface** (registered as `physics.*`):
  - `physics.body(shape, at, mass)` — box/sphere/capsule
  - `physics.static_box(at, size)`, `physics.static_sphere(at, r)`
  - `physics.position(h)`, `physics.velocity(h, v)`,
    `physics.impulse(h, v)`
  - `physics.gravity(v)`, `physics.character_move(h, dir, dt)`
  - `physics.despawn(h)`, `physics.reset()`
- **Render integration:** `play3d::render` calls
  `physics3d::step(dt)` before `eval::tick_frame` so script logic
  reads authoritative positions. Hot reload calls `reset()` so
  stale handles don't leak across edits.
- **Demo:** `examples/physics_demo.twe` — three boxes fall onto
  a static floor, R drops more.

---

## Architecture: scripts own intent, physics owns truth

The integration order is deliberate:

```
each frame:
  1. physics3d::step(dt)        ← rapier integrates
  2. eval::tick_frame(dt)       ← script reads positions, sets velocity/impulse
  3. eval::render_frame3d()     ← script renders cubes at physics positions
```

Scripts call `physics.velocity(h, v)` to set *intent*. The
integrator then computes the next-frame *truth*. Reading
`physics.position(h)` always returns the integrator's authoritative
position — never a stale value from before the step.

This is the key design decision: scripts cannot fight the
integrator. If a script tries to teleport a body via direct
position-set, rapier will resolve any resulting penetration on
the next step. The right way to move a kinematic body is
`physics.character_move(h, dir, dt)`, which translates the body
in a way the integrator handles cleanly.

---

## Test count

Pre-phase: 732. Post-phase: 732. The physics surface is exercised
via `physics_demo.twe` and the `test_physics.twe` smoke test;
unit tests for rapier integration would require either a headless
fake adapter or a deterministic-physics rig that's overkill for
the MVP surface.

---

## What slipped (deferred to follow-on)

The full Phase 18 roadmap calls for six sessions; this MVP ships
session 1's surface (body/position/velocity/gravity/character_move)
plus session 4's shape coverage (box/sphere/capsule + static box).
Deferred:

- **Trimesh static colliders** (`physics.static_mesh(path)`).
  Reads the same `parse_glb_bytes` positions/indices as the
  renderer and builds a rapier `TriMeshShape`. Needed for level
  geometry exported from Blender. ~50 lines.
- **Collision callbacks** (`physics.on_collision(a, b, fn)`).
  Requires plumbing rapier's `EventHandler` → a Twe callback
  dispatch path. Drives item pickup / damage triggers.
  ~150 lines + Twe-side closure storage.
- **Full `KinematicCharacterController`**. The current
  `physics.character_move` directly translates the body and lets
  the solver handle penetration; the proper rapier
  `KinematicCharacterController` adds stair stepping, slope
  climbing, and ground contact detection. ~80 lines.
- **Raycasts** (`physics.raycast(origin, dir, max_dist)`).
  rapier's `QueryPipeline` already supports it; just needs a Twe
  builtin returning `(handle, hit_point)` or nil. ~40 lines.
- **Continuous collision detection (CCD).** Useful for fast-moving
  projectiles. Toggled per-body via rapier's `ccd_enabled`.

Each follow-on is a focused session. Together they're maybe one
focused week of additional work. Nothing in the MVP forecloses
any of them.
