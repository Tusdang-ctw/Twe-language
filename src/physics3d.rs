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
use std::sync::Mutex;

use rapier3d::control::KinematicCharacterController;
use rapier3d::geometry::ContactPair;
use rapier3d::pipeline::EventHandler;
use rapier3d::prelude::*;

thread_local! {
    static WORLD: RefCell<PhysicsWorld> = RefCell::new(PhysicsWorld::new());
}

/// Phase 18 finish: collision-event scratch buffer. The rapier
/// EventHandler trait requires Send + Sync, which rules out
/// `&mut PhysicsWorld` directly. Instead the handler pushes into
/// this Mutex<Vec<...>>; PhysicsWorld::step drains it after the
/// step returns and translates rapier handles into Twe-side
/// u32 handles via `self.collider_to_twe`.
static EVENT_QUEUE: Mutex<Vec<CollisionEvent>> = Mutex::new(Vec::new());

struct StaticEventHandler;

impl EventHandler for StaticEventHandler {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        if let Ok(mut q) = EVENT_QUEUE.lock() {
            q.push(event);
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: Real,
    ) {
        // We don't surface contact-force events yet — the begin /
        // end contact pair from handle_collision_event is enough
        // for triggers, pickup callbacks, etc.
    }
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
    /// Reverse map collider → Twe handle, used by the collision
    /// event handler to translate rapier collider handles back to
    /// what scripts see.
    collider_to_twe: HashMap<ColliderHandle, u32>,
    next_handle: u32,
    /// Phase 18 finish: characters use a kinematic-position-based
    /// body driven by `KinematicCharacterController::move_shape`.
    /// Cache one controller (cheap to construct, but stateless) and
    /// the per-character capsule shape we built for collision tests.
    character_controller: KinematicCharacterController,
    /// Collision events collected each step. Drained by
    /// `physics.collisions()` from the script side. Each entry is
    /// (a, b, started) where started=true is begin-contact,
    /// started=false is end-contact.
    collision_events: Vec<(u32, u32, bool)>,
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
            collider_to_twe: HashMap::new(),
            next_handle: 1,
            character_controller: KinematicCharacterController::default(),
            collision_events: Vec::new(),
        }
    }

    /// Step the world forward by `dt` seconds. Called once per frame
    /// from `play3d::run_loop` before the Twe `on update(dt)` body.
    /// After integration, drains the static EVENT_QUEUE and
    /// translates rapier collider handles into Twe-side u32 handles
    /// for `physics.collisions()` to read.
    pub fn step(&mut self, dt: f32) {
        self.integration_parameters.dt = dt.max(1e-6);
        let physics_hooks = ();
        let event_handler = StaticEventHandler;
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
        // Drain the global event queue and translate rapier handles
        // into Twe-side u32 handles. Events whose colliders aren't
        // in `collider_to_twe` (despawned bodies, foreign colliders
        // we didn't create) translate to handle 0 — scripts can
        // filter on that.
        if let Ok(mut q) = EVENT_QUEUE.lock() {
            for ev in q.drain(..) {
                let (a, b, started) = match ev {
                    CollisionEvent::Started(a, b, _) => (a, b, true),
                    CollisionEvent::Stopped(a, b, _) => (a, b, false),
                };
                let twe_a = self.collider_to_twe.get(&a).copied().unwrap_or(0);
                let twe_b = self.collider_to_twe.get(&b).copied().unwrap_or(0);
                self.collision_events.push((twe_a, twe_b, started));
            }
        }
    }

    fn allocate_handle(&mut self, h: RigidBodyHandle) -> u32 {
        let id = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(id, h);
        id
    }

    /// Create a dynamic rigid body of the given shape at `at` with
    /// the given mass. Returns the Twe-side handle. ActiveEvents
    /// for collision begin/end are enabled on the collider so
    /// `physics.collisions()` sees events involving this body.
    pub fn body(&mut self, shape: &str, at: [f32; 3], mass: f32) -> Result<u32, String> {
        let body = RigidBodyBuilder::dynamic()
            .translation(vector![at[0], at[1], at[2]])
            .build();
        let body_h = self.bodies.insert(body);
        let builder = match shape {
            "box" => ColliderBuilder::cuboid(0.5, 0.5, 0.5).mass(mass),
            "sphere" => ColliderBuilder::ball(0.5).mass(mass),
            "capsule" => ColliderBuilder::capsule_y(0.5, 0.4).mass(mass),
            other => {
                return Err(format!(
                    "physics.body: unknown shape '{other}' (expected box/sphere/capsule)"
                ));
            }
        };
        let collider = builder
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();
        let coll_h = self
            .colliders
            .insert_with_parent(collider, body_h, &mut self.bodies);
        let twe = self.allocate_handle(body_h);
        self.collider_to_twe.insert(coll_h, twe);
        Ok(twe)
    }

    /// Static box collider at `at` with full extents `size`. Returns
    /// the Twe-side handle (mostly for record-keeping; static bodies
    /// don't move and rarely need lookup).
    pub fn static_box(&mut self, at: [f32; 3], size: [f32; 3]) -> u32 {
        let body = RigidBodyBuilder::fixed()
            .translation(vector![at[0], at[1], at[2]])
            .build();
        let body_h = self.bodies.insert(body);
        let collider = ColliderBuilder::cuboid(size[0] * 0.5, size[1] * 0.5, size[2] * 0.5)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();
        let coll_h = self
            .colliders
            .insert_with_parent(collider, body_h, &mut self.bodies);
        let twe = self.allocate_handle(body_h);
        self.collider_to_twe.insert(coll_h, twe);
        twe
    }

    pub fn static_sphere(&mut self, at: [f32; 3], radius: f32) -> u32 {
        let body = RigidBodyBuilder::fixed()
            .translation(vector![at[0], at[1], at[2]])
            .build();
        let body_h = self.bodies.insert(body);
        let collider = ColliderBuilder::ball(radius)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();
        let coll_h = self
            .colliders
            .insert_with_parent(collider, body_h, &mut self.bodies);
        let twe = self.allocate_handle(body_h);
        self.collider_to_twe.insert(coll_h, twe);
        twe
    }

    /// Phase 18 finish: static triangle-mesh collider built from
    /// a flat list of vertex positions + u32 triangle indices.
    /// Used for level geometry — the player physics capsule slides
    /// along arbitrary mesh surfaces (slopes, stairs, walls).
    /// `at` translates the whole mesh in world space.
    pub fn static_trimesh(
        &mut self,
        at: [f32; 3],
        vertices: &[[f32; 3]],
        indices: &[[u32; 3]],
    ) -> Result<u32, String> {
        if vertices.is_empty() || indices.is_empty() {
            return Err("physics.static_mesh: empty geometry".to_string());
        }
        let pts: Vec<Point<Real>> = vertices
            .iter()
            .map(|v| Point::new(v[0], v[1], v[2]))
            .collect();
        let collider = ColliderBuilder::trimesh(pts, indices.to_vec())
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();
        let body = RigidBodyBuilder::fixed()
            .translation(vector![at[0], at[1], at[2]])
            .build();
        let body_h = self.bodies.insert(body);
        let coll_h = self
            .colliders
            .insert_with_parent(collider, body_h, &mut self.bodies);
        let twe = self.allocate_handle(body_h);
        self.collider_to_twe.insert(coll_h, twe);
        Ok(twe)
    }

    /// Phase 18 finish: create a kinematic-position-based capsule
    /// suitable for `character_move()` with the rapier KCC. Unlike
    /// a dynamic body, a kinematic body's position is moved
    /// directly by us; the integrator never applies gravity or
    /// collision response automatically. The KCC handles slope
    /// climbing, stair stepping, and wall sliding when we use
    /// `move_shape()` to compute the actual translation.
    ///
    /// `height` is the full capsule height (cylinder + 2 hemispheres);
    /// `radius` is the capsule radius. A typical first-person player
    /// is height ≈ 1.8, radius ≈ 0.4.
    pub fn character(&mut self, at: [f32; 3], height: f32, radius: f32) -> u32 {
        let body = RigidBodyBuilder::kinematic_position_based()
            .translation(vector![at[0], at[1], at[2]])
            .build();
        let body_h = self.bodies.insert(body);
        // Capsule's `half_height` is along the cylinder only — the
        // hemispheres add `radius` at each end. Total visible height
        // = 2 * half_height + 2 * radius.
        let half_cyl = ((height - 2.0 * radius).max(0.0)) * 0.5;
        let collider = ColliderBuilder::capsule_y(half_cyl, radius)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();
        let coll_h = self
            .colliders
            .insert_with_parent(collider, body_h, &mut self.bodies);
        let twe = self.allocate_handle(body_h);
        self.collider_to_twe.insert(coll_h, twe);
        twe
    }

    /// Phase 18 finish: raycast against all colliders. Returns
    /// (hit_handle, hit_point, hit_distance) on hit, None on miss.
    /// Used for FPS aim, line-of-sight checks, hover-pick, etc.
    pub fn raycast(
        &mut self,
        origin: [f32; 3],
        direction: [f32; 3],
        max_dist: f32,
    ) -> Option<(u32, [f32; 3], f32)> {
        // Ensure the broad-phase index is current before querying.
        self.query_pipeline.update(&self.colliders);
        let ray = Ray::new(
            Point::new(origin[0], origin[1], origin[2]),
            vector![direction[0], direction[1], direction[2]],
        );
        let filter = QueryFilter::default();
        if let Some((collider_handle, toi)) = self.query_pipeline.cast_ray(
            &self.bodies,
            &self.colliders,
            &ray,
            max_dist,
            true,
            filter,
        ) {
            let hit_point = ray.point_at(toi);
            // Find the body handle that owns this collider, then
            // the Twe-side u32 handle that maps to it.
            let collider = self.colliders.get(collider_handle)?;
            let body_handle = collider.parent()?;
            let twe_handle = self
                .handles
                .iter()
                .find(|(_, h)| **h == body_handle)
                .map(|(k, _)| *k)
                .unwrap_or(0);
            Some((twe_handle, [hit_point.x, hit_point.y, hit_point.z], toi))
        } else {
            None
        }
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

    /// Phase 18 finish: full character move using rapier's
    /// `KinematicCharacterController`. Handles slope climbing
    /// (default 45°), stair stepping (default 0.25 units high),
    /// and wall sliding. Returns the resulting `(translation,
    /// grounded)` pair so scripts can drive jump logic from
    /// ground contact.
    ///
    /// The body must be kinematic-position-based (created by
    /// `physics.character`); calling on a dynamic body created
    /// by `physics.body` returns a polite error rather than
    /// fighting the integrator.
    pub fn character_move(
        &mut self,
        handle: u32,
        desired: [f32; 3],
        dt: f32,
    ) -> Result<([f32; 3], bool), String> {
        let rh = *self
            .handles
            .get(&handle)
            .ok_or_else(|| format!("physics.character_move: unknown handle {handle}"))?;
        // Read the character's current shape + position. Rapier
        // disallows mutable + immutable borrows of the same set,
        // so we extract the data we need first.
        let (collider_handle, isometry, shape_clone) = {
            let body = self.bodies.get(rh).ok_or_else(|| {
                format!("physics.character_move: body for handle {handle} despawned")
            })?;
            if !body.is_kinematic() {
                return Err(format!(
                    "physics.character_move: handle {handle} is not a character body — create with physics.character() instead of physics.body()"
                ));
            }
            let coll = body.colliders().first().copied().ok_or_else(|| {
                format!("physics.character_move: handle {handle} has no collider")
            })?;
            let collider = self.colliders.get(coll).ok_or_else(|| {
                format!("physics.character_move: collider missing for handle {handle}")
            })?;
            (coll, *body.position(), collider.shared_shape().clone())
        };
        let desired_v = vector![desired[0] * dt, desired[1] * dt, desired[2] * dt];
        let filter = QueryFilter::default().exclude_collider(collider_handle);
        // Update the query pipeline so the controller sees the
        // current world state. The pipeline.step() already does
        // this internally on its own pass; doing it again here is
        // cheap and ensures KCC's queries are fresh.
        self.query_pipeline.update(&self.colliders);
        let movement = self.character_controller.move_shape(
            dt,
            &self.bodies,
            &self.colliders,
            &self.query_pipeline,
            shape_clone.as_ref(),
            &isometry,
            desired_v,
            filter,
            |_collision| {
                // Per-collision callback during shape sweep. We
                // don't surface KCC sweep events to Twe yet —
                // begin/end contact events from the main step
                // already fire via the EventHandler path.
            },
        );
        let body = self
            .bodies
            .get_mut(rh)
            .ok_or_else(|| format!("physics.character_move: body vanished for handle {handle}"))?;
        let new_t = isometry.translation.vector + movement.translation;
        body.set_next_kinematic_translation(new_t);
        Ok((
            [
                movement.translation.x,
                movement.translation.y,
                movement.translation.z,
            ],
            movement.grounded,
        ))
    }

    /// Drain the queue of collision events accumulated since the
    /// last call. Each event is `(a, b, started)` — a/b are
    /// Twe-side handles, started=true is begin-contact, false is
    /// end-contact. Either handle can be 0 if the colliding body
    /// was foreign or has been despawned.
    pub fn drain_collisions(&mut self) -> Vec<(u32, u32, bool)> {
        std::mem::take(&mut self.collision_events)
    }

    pub fn despawn(&mut self, handle: u32) -> bool {
        if let Some(rh) = self.handles.remove(&handle) {
            // Clean up the reverse collider→twe map for any
            // colliders this body owned; otherwise stale entries
            // accumulate across long-running games.
            self.collider_to_twe.retain(|_, &mut twe| twe != handle);
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

// ---- glTF geometry loader (positions + triangles only) ----
//
// Physics doesn't need normals, UVs, or materials — just enough
// to build a TriMeshShape. Lighter than play3d::parse_glb_bytes.

/// Vertex positions + triangle indices extracted from a `.glb` for
/// use as a static trimesh collider.
pub type TrimeshGeometry = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// Load a `.glb`'s first-mesh-first-primitive geometry as positions
/// and packed u32 triangles. Used by `physics.static_mesh(path)` to
/// turn a Blender-exported level mesh into a static trimesh
/// collider. Errors are stringified at the boundary.
pub fn load_glb_geometry(path: &str) -> Result<TrimeshGeometry, String> {
    let bytes = crate::bundle::read_asset_bytes(path).map_err(|e| e.to_string())?;
    let (doc, buffers, _images) = gltf::import_slice(&bytes).map_err(|e| e.to_string())?;
    let mesh = doc
        .meshes()
        .next()
        .ok_or_else(|| "glb has no meshes".to_string())?;
    let primitive = mesh
        .primitives()
        .next()
        .ok_or_else(|| "first mesh has no primitives".to_string())?;
    let reader = primitive.reader(|b| Some(&buffers[b.index()]));
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| "primitive has no POSITION accessor".to_string())?
        .collect();
    if positions.is_empty() {
        return Err("primitive has zero vertices".to_string());
    }
    let raw_indices: Vec<u32> = match reader.read_indices() {
        Some(idx) => idx.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    if !raw_indices.len().is_multiple_of(3) {
        return Err(format!(
            "physics.static_mesh: expected triangle list, got {} indices (not divisible by 3)",
            raw_indices.len()
        ));
    }
    let triangles: Vec<[u32; 3]> = raw_indices
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    Ok((positions, triangles))
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

pub fn static_trimesh(
    at: [f32; 3],
    vertices: &[[f32; 3]],
    indices: &[[u32; 3]],
) -> Result<u32, String> {
    WORLD.with(|w| w.borrow_mut().static_trimesh(at, vertices, indices))
}

pub fn raycast(
    origin: [f32; 3],
    direction: [f32; 3],
    max_dist: f32,
) -> Option<(u32, [f32; 3], f32)> {
    WORLD.with(|w| w.borrow_mut().raycast(origin, direction, max_dist))
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

pub fn character(at: [f32; 3], height: f32, radius: f32) -> u32 {
    WORLD.with(|w| w.borrow_mut().character(at, height, radius))
}

pub fn character_move(handle: u32, dir: [f32; 3], dt: f32) -> Result<([f32; 3], bool), String> {
    WORLD.with(|w| w.borrow_mut().character_move(handle, dir, dt))
}

pub fn drain_collisions() -> Vec<(u32, u32, bool)> {
    WORLD.with(|w| w.borrow_mut().drain_collisions())
}

pub fn despawn(handle: u32) -> bool {
    WORLD.with(|w| w.borrow_mut().despawn(handle))
}

pub fn reset() {
    WORLD.with(|w| w.borrow_mut().reset());
}
