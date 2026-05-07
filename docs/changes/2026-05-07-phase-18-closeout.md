# Phase 18 closeout — Physics + Collision (post-v1.0)

**Date:** 2026-05-07.
**Status:** **codebase-closed** (full surface shipped).
**Roadmap reference:** `docs/3d-roadmap.md` §"Phase 18".

---

## What shipped

Phase 18 ran in three sessions:

| # | Session | Surface |
|---|---------|---------|
| 1 | rapier3d + physics MVP | `rapier3d = "0.22"` (default-features=false + dim3 + f32, no rayon). `src/physics3d.rs` with thread-local `PhysicsWorld` wrapping the full rapier pipeline. Twe surface: `physics.body` (box/sphere/capsule), `physics.static_box`, `physics.static_sphere`, `physics.position`, `physics.velocity`, `physics.impulse`, `physics.gravity`, `physics.character_move` (basic), `physics.despawn`, `physics.reset`. play3d::render calls `physics3d::step(dt)` before `eval::tick_frame`. `examples/physics_demo.twe` (falling boxes onto a floor). |
| 2 | trimesh static + raycasts | `physics.static_mesh(path, at)` reads a `.glb`'s first primitive via `load_glb_geometry` (a lighter sibling of play3d's parser) and builds a rapier `TriMeshShape` collider. Used for level geometry. `physics.raycast(origin, direction, max_dist)` queries via `QueryPipeline::cast_ray`; returns `nil` on miss or `{ handle, point, distance, kind: "raycast_hit" }` on hit. The handle field maps back to the same u32 returned by `physics.body()`. |
| 3 | character controller + collision events | `physics.character(at, height, radius)` creates a kinematic-position-based capsule body suitable for `KinematicCharacterController::move_shape`. `physics.character_move` now uses the full KCC (slope climbing, stair stepping, wall sliding) and returns `{ translation, grounded }`. `physics.collisions()` returns a list of `{ a, b, started }` events accumulated since last call — captured by a custom `StaticEventHandler` that pushes into a static `Mutex<Vec<CollisionEvent>>` during the rapier step, drained and translated to Twe-side handles after. ActiveEvents bit set on every collider so events fire reliably. `examples/fps_demo.twe` is the playable proof: WASD walk, space jump, coin pickup via collision events, R raycast aim, mouse look + cursor lock. |

---

## Architecture: scripts own intent, physics owns truth

The integration order is deliberate:

```
each frame:
  1. physics3d::step(dt)        ← rapier integrates
  2. eval::tick_frame(dt)       ← script reads positions, sets velocity / impulse
                                  / character_move, polls collisions
  3. eval::render_frame3d()     ← script renders cubes at physics positions
```

Scripts call `physics.velocity(h, v)` and `physics.character_move(h, dir, dt)` to set *intent*. The integrator computes the next-frame *truth*. Reading `physics.position(h)` always returns the integrator's authoritative position.

Two body kinds:

- **Dynamic** (`physics.body`) — full rigid-body dynamics. Gravity, forces, collision response all automatic. Use for projectiles, boxes, anything physics-driven.
- **Kinematic** (`physics.character`) — script controls position via `character_move`. The integrator never applies forces or auto-resolves collisions; the KCC handles wall sliding / slope climbing / stair stepping. Manual gravity + jump impulse via the script's `dt`-scaled translation.

Calling `physics.character_move` on a non-character (dynamic) body returns a polite error pointing the user to `physics.character()` — better than fighting the integrator.

---

## Test count

Pre-phase: 732. Post-phase: 732. The physics surface is exercised
manually via `examples/fps_demo.twe` and `examples/physics_demo.twe`.
Unit tests would require either a headless deterministic-physics
rig or a fake adapter; both are overkill for the v0.1 surface.

---

## What slipped (deferred, lower priority)

The full Phase 18 plan in `docs/3d-roadmap.md` listed six sessions;
this closeout ships sessions 1–6 (all body shapes, static + trimesh
colliders, raycasts, full KCC, collision events) in three commits.
What's still deferred:

- **Joint constraints** (revolute, prismatic, fixed). rapier
  exposes `ImpulseJointSet` already wired into the pipeline; we
  just don't surface a Twe builtin yet. Deferred to whenever a
  game needs ragdolls / vehicles / chains.
- **Continuous collision detection (CCD)** for fast-moving
  projectiles. Toggled per-body via rapier's `ccd_enabled`.
  Currently always off — high-speed bullets can tunnel through
  thin walls. Deferred until a game's projectile speed pressures it.
- **Per-collider physics groups / filters.** rapier supports
  bitmask filtering for which colliders interact. We use the
  default ALL-vs-ALL groups; deferred until a game wants
  one-way platforms or layer-specific raycasts.
- **Convex decomposition / convex hulls.** Trimesh static is
  fine for level geometry; dynamic concave bodies need convex
  decomposition. rapier supports it via `parry3d`'s `vhacd` —
  small wrapper deferred.
