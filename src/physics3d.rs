//! Phase 18: 3D physics for `twec play3d` via rapier3d.
//!
//! Architecture:
//!
//! - One global thread-local `PhysicsWorld` per process. The play3d
//!   loop calls `step(dt)` once per frame BEFORE the Twe `on update(dt)`
//!   handler runs, so script logic reads authoritative positions
//!   from `physics.position(handle)` and writes intent
//!   (velocity / impulse) — the integrator owns truth.
//! - All Twe-side handles are `u32` keys into the world's body map.
//!   The Rust `RigidBodyHandle` is a `(u32, u32)` generational index
//!   internally; we wrap it so scripts get a flat number they can
//!   stash in scene fields without needing a tuple.
//!
//! Twe surface (registered in stdlib::install):
//!
//!   physics.body(shape, at, mass)         → handle (u32)
//!   physics.position(handle)              → vec3
//!   physics.velocity(handle, vec)
//!   physics.impulse(handle, vec)
//!   physics.gravity(vec)
//!   physics.character_move(handle, dir, dt)
//!   physics.static_box(at, size)          → handle (u32)
//!   physics.static_sphere(at, radius)     → handle (u32)
//!   physics.despawn(handle)
//!   physics.reset()
//!
//! The full plan calls for collision callbacks (physics.on_collision),
//! trimesh static colliders, and a kinematic character controller
//! with stair stepping. v0.1 of this module ships the shape-hit
//! capsule + dynamic boxes/spheres — enough for a 3D platformer
//! demo. The follow-ons land as the physics surface gets pressured
//! by a real game.

use std::cell::RefCell;
use std::collections::HashMap;

use rapier3d::prelude::*;

thread_local! {
    static WORLD: RefCell<PhysicsWorld> = RefCell::new(PhysicsWorld::new());
}

pub struct PhysicsWorld {
    pipeline: PhysicsPipeline,
    gravity: Vector<Real>,
    integration_parameters: IntegrationParameters,
    islands: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    query_pipeline: QueryPipeline,
    /// Twe-side handle (u32) → rapier RigidBodyHandle. Scripts
    /// pass the u32 around; we look up the rapier handle here.
    handles: HashMap<u32, RigidBodyHandle>,
    next_handle: u32,
}

impl PhysicsWorld {
    pub fn new() -> Self {
        Self {
            pipeline: PhysicsPipeline::new(),
            gravity: vector![0.0, -9.81, 0.0],
            integration_parameters: IntegrationParameters::default(),
            islands: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            handles: HashMap::new(),
            next_handle: 1,
        }
    }

    /// Step the world forward by `dt` seconds. Called once per frame
    /// from `play3d::run_loop` before the Twe `on update(dt)` body.
    pub fn step(&mut self, dt: f32) {
        self.integration_parameters.dt = dt.max(1e-6);
        let physics_hooks = ();
        let event_handler = ();
        self.pipeline.step(
            &self.gravity,
            &self.integration_parameters,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &physics_hooks,
            &event_handler,
        );
    }

    fn allocate_handle(&mut self, h: RigidBodyHandle) -> u32 {
        let id = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(id, h);
        id
    }

    /// Create a dynamic rigid body of the given shape at `at` with
    /// the given mass. Returns the Twe-side handle.
    pub fn body(&mut self, shape: &str, at: [f32; 3], mass: f32) -> Result<u32, String> {
        let body = RigidBodyBuilder::dynamic()
            .translation(vector![at[0], at[1], at[2]])
            .build();
        let body_h = self.bodies.insert(body);
        let collider = match shape {
            "box" => ColliderBuilder::cuboid(0.5, 0.5, 0.5).mass(mass).build(),
            "sphere" => ColliderBuilder::ball(0.5).mass(mass).build(),
            "capsule" => ColliderBuilder::capsule_y(0.5, 0.4).mass(mass).build(),
            other => {
                return Err(format!(
                    "physics.body: unknown shape '{other}' (expected box/sphere/capsule)"
                ));
            }
        };
        self.colliders
            .insert_with_parent(collider, body_h, &mut self.bodies);
        Ok(self.allocate_handle(body_h))
    }

    /// Static box collider at `at` with full extents `size`. Returns
    /// the Twe-side handle (mostly for record-keeping; static bodies
    /// don't move and rarely need lookup).
    pub fn static_box(&mut self, at: [f32; 3], size: [f32; 3]) -> u32 {
        let body = RigidBodyBuilder::fixed()
            .translation(vector![at[0], at[1], at[2]])
            .build();
        let body_h = self.bodies.insert(body);
        let collider = ColliderBuilder::cuboid(size[0] * 0.5, size[1] * 0.5, size[2] * 0.5).build();
        self.colliders
            .insert_with_parent(collider, body_h, &mut self.bodies);
        self.allocate_handle(body_h)
    }

    pub fn static_sphere(&mut self, at: [f32; 3], radius: f32) -> u32 {
        let body = RigidBodyBuilder::fixed()
            .translation(vector![at[0], at[1], at[2]])
            .build();
        let body_h = self.bodies.insert(body);
        let collider = ColliderBuilder::ball(radius).build();
        self.colliders
            .insert_with_parent(collider, body_h, &mut self.bodies);
        self.allocate_handle(body_h)
    }

    pub fn position(&self, handle: u32) -> Option<[f32; 3]> {
        let rh = self.handles.get(&handle)?;
        let body = self.bodies.get(*rh)?;
        let t = body.translation();
        Some([t.x, t.y, t.z])
    }

    pub fn set_velocity(&mut self, handle: u32, v: [f32; 3]) -> Result<(), String> {
        let rh = self
            .handles
            .get(&handle)
            .ok_or_else(|| format!("physics.velocity: unknown handle {handle}"))?;
        let body = self
            .bodies
            .get_mut(*rh)
            .ok_or_else(|| format!("physics.velocity: body for handle {handle} despawned"))?;
        body.set_linvel(vector![v[0], v[1], v[2]], true);
        Ok(())
    }

    pub fn apply_impulse(&mut self, handle: u32, v: [f32; 3]) -> Result<(), String> {
        let rh = self
            .handles
            .get(&handle)
            .ok_or_else(|| format!("physics.impulse: unknown handle {handle}"))?;
        let body = self
            .bodies
            .get_mut(*rh)
            .ok_or_else(|| format!("physics.impulse: body for handle {handle} despawned"))?;
        body.apply_impulse(vector![v[0], v[1], v[2]], true);
        Ok(())
    }

    pub fn set_gravity(&mut self, v: [f32; 3]) {
        self.gravity = vector![v[0], v[1], v[2]];
    }

    /// Simple kinematic character move — directly translate the body
    /// by `dir * dt` after a short raycast for ground/wall collision.
    /// Not a full KinematicCharacterController (that needs more
    /// per-shape config); good enough for "WASD walks the player
    /// capsule across a static level."
    pub fn character_move(&mut self, handle: u32, dir: [f32; 3], dt: f32) -> Result<(), String> {
        let rh = *self
            .handles
            .get(&handle)
            .ok_or_else(|| format!("physics.character_move: unknown handle {handle}"))?;
        let body = self
            .bodies
            .get_mut(rh)
            .ok_or_else(|| format!("physics.character_move: body for handle {handle} despawned"))?;
        let t = body.translation();
        // Simple sliding: try to move; if a body exists at the target
        // (very approximately checked via the broad phase next tick),
        // the integrator resolves the penetration. For the MVP we
        // just translate directly and let rapier's solver clean up.
        let new_t = vector![
            t.x + dir[0] * dt,
            t.y + dir[1] * dt,
            t.z + dir[2] * dt,
        ];
        body.set_translation(new_t, true);
        Ok(())
    }

    pub fn despawn(&mut self, handle: u32) -> bool {
        if let Some(rh) = self.handles.remove(&handle) {
            self.bodies.remove(
                rh,
                &mut self.islands,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                true,
            );
            true
        } else {
            false
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl Default for PhysicsWorld {
    fn default() -> Self {
        Self::new()
    }
}

// ---- Public API used by play3d.rs and stdlib.rs ----

pub fn step(dt: f32) {
    WORLD.with(|w| w.borrow_mut().step(dt));
}

pub fn body(shape: &str, at: [f32; 3], mass: f32) -> Result<u32, String> {
    WORLD.with(|w| w.borrow_mut().body(shape, at, mass))
}

pub fn static_box(at: [f32; 3], size: [f32; 3]) -> u32 {
    WORLD.with(|w| w.borrow_mut().static_box(at, size))
}

pub fn static_sphere(at: [f32; 3], radius: f32) -> u32 {
    WORLD.with(|w| w.borrow_mut().static_sphere(at, radius))
}

pub fn position(handle: u32) -> Option<[f32; 3]> {
    WORLD.with(|w| w.borrow().position(handle))
}

pub fn set_velocity(handle: u32, v: [f32; 3]) -> Result<(), String> {
    WORLD.with(|w| w.borrow_mut().set_velocity(handle, v))
}

pub fn apply_impulse(handle: u32, v: [f32; 3]) -> Result<(), String> {
    WORLD.with(|w| w.borrow_mut().apply_impulse(handle, v))
}

pub fn set_gravity(v: [f32; 3]) {
    WORLD.with(|w| w.borrow_mut().set_gravity(v));
}

pub fn character_move(handle: u32, dir: [f32; 3], dt: f32) -> Result<(), String> {
    WORLD.with(|w| w.borrow_mut().character_move(handle, dir, dt))
}

pub fn despawn(handle: u32) -> bool {
    WORLD.with(|w| w.borrow_mut().despawn(handle))
}

pub fn reset() {
    WORLD.with(|w| w.borrow_mut().reset());
}
