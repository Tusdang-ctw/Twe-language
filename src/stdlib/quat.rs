//! web3d (2026-10-08): the `quat.*` quaternion helpers, split out of
//! the stdlib god-module.

use super::*;

pub(super) fn quat_to_value(q: [f32; 4]) -> Value {
    let elems: Vec<Value> = q.iter().map(|f| Value::from_float(*f as f64)).collect();
    let mut fields = HashMap::new();
    fields.insert(
        "data".to_string(),
        Value::from_list(Rc::new(RefCell::new(elems))),
    );
    Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "quat",
    })))
}

pub(super) fn quat_from_value(v: &Value, what: &str) -> Result<[f32; 4], RuntimeError> {
    if !v.is_object() {
        let other = *v;
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: expected a quat, got {}", other.type_name()),
            help: Some(
                "create one with quat.identity() / quat.from_axis_angle(axis, angle)".to_string(),
            ),
        });
    }
    let rc = v.as_object();
    let o = rc.borrow();
    if o.kind != "quat" {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: expected a quat Object, got kind '{}'", o.kind),
            help: None,
        });
    }
    let data = o.get_field("data").ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!("{what}: quat missing data field"),
        help: None,
    })?;
    if !data.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: quat.data is not a list"),
            help: None,
        });
    }
    let list = data.as_list();
    let list = list.borrow();
    if list.len() != 4 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: quat.data has {} elements, expected 4", list.len()),
            help: None,
        });
    }
    let mut out = [0.0f32; 4];
    for (i, e) in list.iter().enumerate() {
        out[i] = number(e, what)? as f32;
    }
    Ok(out)
}

pub(super) fn quat_identity_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    Ok(quat_to_value([0.0, 0.0, 0.0, 1.0]))
}

pub(super) fn quat_from_axis_angle_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "quat.from_axis_angle")?;
    let axis = xyz_of(&args[0], "quat.from_axis_angle.axis")?;
    let angle = number(&args[1], "quat.from_axis_angle.angle")? as f32;
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if len < 1e-6 {
        return Ok(quat_to_value([0.0, 0.0, 0.0, 1.0]));
    }
    let half = angle * 0.5;
    let s = half.sin() / len;
    Ok(quat_to_value([
        axis[0] * s,
        axis[1] * s,
        axis[2] * s,
        half.cos(),
    ]))
}

pub(super) fn quat_slerp_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "quat.slerp")?;
    let a = quat_from_value(&args[0], "quat.slerp.a")?;
    let b = quat_from_value(&args[1], "quat.slerp.b")?;
    let t = number(&args[2], "quat.slerp.t")? as f32;
    // Standard slerp with shortest-path cosine flip.
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let mut bn = b;
    if dot < 0.0 {
        dot = -dot;
        bn = [-b[0], -b[1], -b[2], -b[3]];
    }
    if dot > 0.9995 {
        // Near-parallel — fall back to linear interp + normalize.
        let mut r = [
            a[0] + t * (bn[0] - a[0]),
            a[1] + t * (bn[1] - a[1]),
            a[2] + t * (bn[2] - a[2]),
            a[3] + t * (bn[3] - a[3]),
        ];
        let n = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2] + r[3] * r[3]).sqrt();
        if n > 1e-6 {
            r[0] /= n;
            r[1] /= n;
            r[2] /= n;
            r[3] /= n;
        }
        return Ok(quat_to_value(r));
    }
    let theta_0 = dot.acos();
    let theta = theta_0 * t;
    let sin_theta_0 = theta_0.sin();
    let s1 = ((theta_0 - theta).sin()) / sin_theta_0;
    let s2 = theta.sin() / sin_theta_0;
    Ok(quat_to_value([
        a[0] * s1 + bn[0] * s2,
        a[1] * s1 + bn[1] * s2,
        a[2] * s1 + bn[2] * s2,
        a[3] * s1 + bn[3] * s2,
    ]))
}

pub(super) fn quat_mul_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "quat.mul")?;
    let a = quat_from_value(&args[0], "quat.mul.a")?;
    let b = quat_from_value(&args[1], "quat.mul.b")?;
    Ok(quat_to_value([
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]))
}

pub(super) fn quat_to_mat4_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "quat.to_mat4")?;
    let q = quat_from_value(&args[0], "quat.to_mat4.q")?;
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    // Column-major to match the rest of the mat4 surface.
    let m: [f32; 16] = [
        1.0 - 2.0 * (yy + zz),
        2.0 * (xy + wz),
        2.0 * (xz - wy),
        0.0,
        2.0 * (xy - wz),
        1.0 - 2.0 * (xx + zz),
        2.0 * (yz + wx),
        0.0,
        2.0 * (xz + wy),
        2.0 * (yz - wx),
        1.0 - 2.0 * (xx + yy),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    Ok(mat4_to_value(&m))
}

