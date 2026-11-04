//! web3d (2026-10-08): the `physics.*` builtin implementations, split
//! out of the former 12.9k-line `stdlib.rs` god-module. Each `fn` here
//! is a builtin registered by `super::install`; they forward to the
//! thread-local `PhysicsWorld` in `crate::physics3d`. Shared helpers
//! (`handle_int`, `xyz_of`, `arity`, …) stay in the parent and reach
//! here through `use super::*`.

use super::*;

pub(super) fn physics_body_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "physics.body")?;
    let shape = string_arg(&args[0], "physics.body", "shape")?;
    let at = xyz_of(&args[1], "physics.body.at")?;
    let mass = number(&args[2], "physics.body.mass")? as f32;
    let id = crate::physics3d::body(&shape, at, mass).map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: e,
        help: Some("expected shape: \"box\", \"sphere\", or \"capsule\"".to_string()),
    })?;
    Ok(Value::from_int(id as i64))
}

// web3d-M11 session 2: physics joints + CCD. The rapier joint + CCD sets
// were already stepped; these expose them to scripts.
pub(super) fn joint_result(h: Option<u32>, what: &str) -> Result<Value, RuntimeError> {
    match h {
        Some(id) => Ok(Value::from_int(id as i64)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what}: unknown body handle (pass handles from physics.body / static_*)"
            ),
            help: None,
        }),
    }
}

pub(super) fn physics_fixed_joint_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 4, "physics.fixed_joint")?;
    let a = handle_int(&args[0], "physics.fixed_joint.a")?;
    let b = handle_int(&args[1], "physics.fixed_joint.b")?;
    let aa = xyz_of(&args[2], "physics.fixed_joint.anchor_a")?;
    let ab = xyz_of(&args[3], "physics.fixed_joint.anchor_b")?;
    joint_result(
        crate::physics3d::fixed_joint(a, b, aa, ab),
        "physics.fixed_joint",
    )
}

pub(super) fn physics_spherical_joint_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 4, "physics.spherical_joint")?;
    let a = handle_int(&args[0], "physics.spherical_joint.a")?;
    let b = handle_int(&args[1], "physics.spherical_joint.b")?;
    let aa = xyz_of(&args[2], "physics.spherical_joint.anchor_a")?;
    let ab = xyz_of(&args[3], "physics.spherical_joint.anchor_b")?;
    joint_result(
        crate::physics3d::spherical_joint(a, b, aa, ab),
        "physics.spherical_joint",
    )
}

pub(super) fn physics_revolute_joint_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 5, "physics.revolute_joint")?;
    let a = handle_int(&args[0], "physics.revolute_joint.a")?;
    let b = handle_int(&args[1], "physics.revolute_joint.b")?;
    let aa = xyz_of(&args[2], "physics.revolute_joint.anchor_a")?;
    let ab = xyz_of(&args[3], "physics.revolute_joint.anchor_b")?;
    let axis = xyz_of(&args[4], "physics.revolute_joint.axis")?;
    joint_result(
        crate::physics3d::revolute_joint(a, b, aa, ab, axis),
        "physics.revolute_joint",
    )
}

pub(super) fn physics_prismatic_joint_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 5, "physics.prismatic_joint")?;
    let a = handle_int(&args[0], "physics.prismatic_joint.a")?;
    let b = handle_int(&args[1], "physics.prismatic_joint.b")?;
    let aa = xyz_of(&args[2], "physics.prismatic_joint.anchor_a")?;
    let ab = xyz_of(&args[3], "physics.prismatic_joint.anchor_b")?;
    let axis = xyz_of(&args[4], "physics.prismatic_joint.axis")?;
    joint_result(
        crate::physics3d::prismatic_joint(a, b, aa, ab, axis),
        "physics.prismatic_joint",
    )
}

pub(super) fn physics_remove_joint_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 1, "physics.remove_joint")?;
    let h = handle_int(&args[0], "physics.remove_joint.handle")?;
    Ok(Value::from_bool(crate::physics3d::remove_joint(h)))
}

pub(super) fn physics_set_ccd_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.set_ccd")?;
    let h = handle_int(&args[0], "physics.set_ccd.handle")?;
    let on = args[1].is_bool() && args[1].as_bool();
    Ok(Value::from_bool(crate::physics3d::set_ccd(h, on)))
}

pub(super) fn physics_static_box_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.static_box")?;
    let at = xyz_of(&args[0], "physics.static_box.at")?;
    let size = xyz_of(&args[1], "physics.static_box.size")?;
    Ok(Value::from_int(
        crate::physics3d::static_box(at, size) as i64
    ))
}

pub(super) fn physics_static_sphere_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.static_sphere")?;
    let at = xyz_of(&args[0], "physics.static_sphere.at")?;
    let radius = number(&args[1], "physics.static_sphere.radius")? as f32;
    Ok(Value::from_int(
        crate::physics3d::static_sphere(at, radius) as i64
    ))
}

pub(super) fn physics_position_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "physics.position")?;
    let handle = handle_int(&args[0], "physics.position")?;
    let pos = crate::physics3d::position(handle).ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!("physics.position: unknown handle {handle}"),
        help: None,
    })?;
    Ok(Value::from_tuple(vec![
        Value::from_float(pos[0] as f64),
        Value::from_float(pos[1] as f64),
        Value::from_float(pos[2] as f64),
    ]))
}

pub(super) fn physics_velocity_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.velocity")?;
    let handle = handle_int(&args[0], "physics.velocity")?;
    let v = xyz_of(&args[1], "physics.velocity.v")?;
    crate::physics3d::set_velocity(handle, v).map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: e,
        help: None,
    })?;
    Ok(Value::NIL)
}

pub(super) fn physics_impulse_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.impulse")?;
    let handle = handle_int(&args[0], "physics.impulse")?;
    let v = xyz_of(&args[1], "physics.impulse.v")?;
    crate::physics3d::apply_impulse(handle, v).map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: e,
        help: None,
    })?;
    Ok(Value::NIL)
}

pub(super) fn physics_gravity_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "physics.gravity")?;
    let v = xyz_of(&args[0], "physics.gravity.v")?;
    crate::physics3d::set_gravity(v);
    Ok(Value::NIL)
}

pub(super) fn physics_character_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 3, "physics.character")?;
    let at = xyz_of(&args[0], "physics.character.at")?;
    let height = number(&args[1], "physics.character.height")? as f32;
    let radius = number(&args[2], "physics.character.radius")? as f32;
    Ok(Value::from_int(
        crate::physics3d::character(at, height, radius) as i64,
    ))
}

pub(super) fn physics_character_move_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 3, "physics.character_move")?;
    let handle = handle_int(&args[0], "physics.character_move")?;
    let dir = xyz_of(&args[1], "physics.character_move.dir")?;
    let dt = number(&args[2], "physics.character_move.dt")? as f32;
    let (translation, grounded) =
        crate::physics3d::character_move(handle, dir, dt).map_err(|e| RuntimeError {
            line: 0,
            col: 0,
            message: e,
            help: None,
        })?;
    let mut fields = HashMap::new();
    fields.insert(
        "translation".to_string(),
        Value::from_tuple(vec![
            Value::from_float(translation[0] as f64),
            Value::from_float(translation[1] as f64),
            Value::from_float(translation[2] as f64),
        ]),
    );
    fields.insert("grounded".to_string(), Value::from_bool(grounded));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "character_move",
    }))))
}

pub(super) fn physics_collisions_impl(
    _env: &mut Env,
    _args: &[Value],
) -> Result<Value, RuntimeError> {
    let events = crate::physics3d::drain_collisions();
    let list: Vec<Value> = events
        .into_iter()
        .map(|(a, b, started)| {
            let mut fields = HashMap::new();
            fields.insert("a".to_string(), Value::from_int(a as i64));
            fields.insert("b".to_string(), Value::from_int(b as i64));
            fields.insert("started".to_string(), Value::from_bool(started));
            Value::from_object(Rc::new(RefCell::new(Object {
                fields,
                kind: "collision_event",
            })))
        })
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(list))))
}

pub(super) fn physics_despawn_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "physics.despawn")?;
    let handle = handle_int(&args[0], "physics.despawn")?;
    Ok(Value::from_bool(crate::physics3d::despawn(handle)))
}

pub(super) fn physics_reset_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    crate::physics3d::reset();
    Ok(Value::NIL)
}

/// Phase 18 finish: load a `.glb` and add its first primitive as a
/// static trimesh collider. The translation `at` positions the
/// whole mesh in world space; the mesh's own internal node
/// transforms are flattened into the positions at load time.
/// Returns the body handle (mostly for record-keeping; static
/// bodies rarely need post-creation lookup).
pub(super) fn physics_static_mesh_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.static_mesh")?;
    let path = string_arg(&args[0], "physics.static_mesh", "path")?;
    let at = xyz_of(&args[1], "physics.static_mesh.at")?;
    let (verts, tris) = crate::physics3d::load_glb_geometry(&path).map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: format!("physics.static_mesh: {e}"),
        help: Some("expected a .glb file with a positions accessor".to_string()),
    })?;
    let id = crate::physics3d::static_trimesh(at, &verts, &tris).map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: e,
        help: None,
    })?;
    Ok(Value::from_int(id as i64))
}

/// Phase 18 finish: raycast against all colliders. Returns
/// nil on miss, or an Object `{ handle, point, distance }` on
/// hit. The handle field is the same u32 id `physics.body()`
/// returns, so callers can look up the body that was struck.
pub(super) fn physics_raycast_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "physics.raycast")?;
    let origin = xyz_of(&args[0], "physics.raycast.origin")?;
    let direction = xyz_of(&args[1], "physics.raycast.direction")?;
    let max_dist = number(&args[2], "physics.raycast.max_dist")? as f32;
    match crate::physics3d::raycast(origin, direction, max_dist) {
        Some((handle, point, distance)) => {
            let mut fields = HashMap::new();
            fields.insert("handle".to_string(), Value::from_int(handle as i64));
            fields.insert(
                "point".to_string(),
                Value::from_tuple(vec![
                    Value::from_float(point[0] as f64),
                    Value::from_float(point[1] as f64),
                    Value::from_float(point[2] as f64),
                ]),
            );
            fields.insert("distance".to_string(), Value::from_float(distance as f64));
            Ok(Value::from_object(Rc::new(RefCell::new(Object {
                fields,
                kind: "raycast_hit",
            }))))
        }
        None => Ok(Value::NIL),
    }
}
