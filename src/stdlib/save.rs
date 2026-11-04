//! web3d (2026-10-08): the `save.*` typed-field helpers, split out of
//! the stdlib god-module. See docs/changes/2026-10-08-web3d-stdlib-split.md.

use super::*;

pub(super) fn save_vec3_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save.vec3")?;
    let key = string_arg(&args[0], "save.vec3", "key")?;
    let v = xyz_of(&args[1], "save.vec3.v")?;
    let tup = Value::from_tuple(vec![
        Value::from_float(v[0] as f64),
        Value::from_float(v[1] as f64),
        Value::from_float(v[2] as f64),
    ]);
    SAVE_STORE.with(|s| s.borrow_mut().insert(key, tup));
    Ok(Value::NIL)
}

pub(super) fn save_f32_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save.f32")?;
    let key = string_arg(&args[0], "save.f32", "key")?;
    let f = number(&args[1], "save.f32.v")?;
    SAVE_STORE.with(|s| s.borrow_mut().insert(key, Value::from_float(f)));
    Ok(Value::NIL)
}

pub(super) fn save_int_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save.int")?;
    let key = string_arg(&args[0], "save.int", "key")?;
    if !args[1].is_int() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "save.int: value must be an integer".to_string(),
            help: None,
        });
    }
    SAVE_STORE.with(|s| s.borrow_mut().insert(key, args[1]));
    Ok(Value::NIL)
}

pub(super) fn save_string_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save.string")?;
    let key = string_arg(&args[0], "save.string", "key")?;
    let v = string_arg(&args[1], "save.string", "v")?;
    SAVE_STORE.with(|s| s.borrow_mut().insert(key, Value::from_string(v)));
    Ok(Value::NIL)
}

pub(super) fn save_get_vec3_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.get_vec3")?;
    let key = string_arg(&args[0], "save.get_vec3", "key")?;
    let result = SAVE_STORE.with(|s| s.borrow().get(&key).copied());
    match result {
        Some(v) if v.is_tuple() && v.as_tuple().len() == 3 => Ok(v),
        _ => Ok(Value::NIL),
    }
}

pub(super) fn save_get_f32_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.get_f32")?;
    let key = string_arg(&args[0], "save.get_f32", "key")?;
    let result = SAVE_STORE.with(|s| s.borrow().get(&key).copied());
    match result {
        Some(v) if v.is_float() => Ok(v),
        Some(v) if v.is_int() => Ok(Value::from_float(v.as_int() as f64)),
        _ => Ok(Value::NIL),
    }
}

pub(super) fn save_get_int_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.get_int")?;
    let key = string_arg(&args[0], "save.get_int", "key")?;
    let result = SAVE_STORE.with(|s| s.borrow().get(&key).copied());
    match result {
        Some(v) if v.is_int() => Ok(v),
        _ => Ok(Value::NIL),
    }
}

pub(super) fn save_get_string_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.get_string")?;
    let key = string_arg(&args[0], "save.get_string", "key")?;
    let result = SAVE_STORE.with(|s| s.borrow().get(&key).copied());
    match result {
        Some(v) if v.is_str() => Ok(v),
        _ => Ok(Value::NIL),
    }
}

pub(super) fn save_has_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.has")?;
    let key = string_arg(&args[0], "save.has", "key")?;
    let has = SAVE_STORE.with(|s| s.borrow().contains_key(&key));
    Ok(Value::from_bool(has))
}

pub(super) fn save_remove_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.remove")?;
    let key = string_arg(&args[0], "save.remove", "key")?;
    let removed = SAVE_STORE.with(|s| s.borrow_mut().remove(&key).is_some());
    Ok(Value::from_bool(removed))
}

pub(super) fn save_clear_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    SAVE_STORE.with(|s| s.borrow_mut().clear());
    Ok(Value::NIL)
}

pub(super) fn save_write_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.write")?;
    let path = string_arg(&args[0], "save.write", "path")?;
    // v1.0.1 session 5: stamp the running game's schema version
    // into the save under the reserved key `_schema_version`. On
    // load, scripts read this back via `save.loaded_version()` and
    // run their own migration steps for v < current.
    let schema_version = SAVE_SCHEMA_VERSION.with(|v| *v.borrow());
    let value = SAVE_STORE.with(|s| {
        let mut obj_fields = HashMap::new();
        for (k, v) in s.borrow().iter() {
            obj_fields.insert(k.clone(), *v);
        }
        obj_fields.insert(
            "_schema_version".to_string(),
            Value::from_int(schema_version),
        );
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: obj_fields,
            kind: "save_store",
        })))
    });
    crate::save::save_to_path(std::path::Path::new(&path), &value)
        .map_err(|e| crate::save::to_runtime_error(e, 0, 0))?;
    Ok(Value::NIL)
}

pub(super) fn save_read_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.read")?;
    let path = string_arg(&args[0], "save.read", "path")?;
    let value = crate::save::load_from_path(std::path::Path::new(&path))
        .map_err(|e| crate::save::to_runtime_error(e, 0, 0))?;
    apply_save_from_value(value);
    Ok(Value::NIL)
}

pub(super) fn save_try_read_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.try_read")?;
    let key = string_arg(&args[0], "save.try_read", "path")?;
    if !crate::save::exists(std::path::Path::new(&key)) {
        return Ok(Value::from_bool(false));
    }
    match crate::save::load_from_path(std::path::Path::new(&key)) {
        Ok(value) => {
            apply_save_from_value(value);
            Ok(Value::from_bool(true))
        }
        Err(_) => Ok(Value::from_bool(false)),
    }
}

pub(super) fn apply_save_from_value(value: Value) {
    if !value.is_object() {
        return;
    }
    let rc = value.as_object();
    let o = rc.borrow();
    // v1.0.1 session 5: extract the loaded schema version. Files
    // written before this session lack `_schema_version`; those
    // saves load as v1 so legacy data is detected and migrated
    // forward cleanly.
    let loaded_version = o
        .fields
        .get("_schema_version")
        .and_then(|v| if v.is_int() { Some(v.as_int()) } else { None })
        .unwrap_or(1);
    SAVE_LOADED_VERSION.with(|v| *v.borrow_mut() = Some(loaded_version));
    SAVE_STORE.with(|s| {
        let mut store = s.borrow_mut();
        store.clear();
        for (k, v) in o.fields.iter() {
            if k == "_schema_version" {
                // Don't expose the reserved key through user-visible
                // typed getters. Scripts read it via `save.loaded_version()`.
                continue;
            }
            store.insert(k.clone(), *v);
        }
    });
}

pub(super) fn save_set_schema_version_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.set_schema_version")?;
    let v = number(&args[0], "save.set_schema_version.version")? as i64;
    if v < 1 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("save.set_schema_version: version must be >= 1, got {v}"),
            help: Some(
                "schema versions are monotonically increasing integers; \
                 the unstamped historical layout is v1"
                    .to_string(),
            ),
        });
    }
    SAVE_SCHEMA_VERSION.with(|cell| *cell.borrow_mut() = v);
    Ok(Value::NIL)
}

pub(super) fn save_schema_version_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 0, "save.schema_version")?;
    let v = SAVE_SCHEMA_VERSION.with(|cell| *cell.borrow());
    Ok(Value::from_int(v))
}

pub(super) fn save_loaded_version_impl(
    _env: &mut Env,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    arity(args, 0, "save.loaded_version")?;
    let v = SAVE_LOADED_VERSION.with(|cell| *cell.borrow());
    Ok(match v {
        Some(n) => Value::from_int(n),
        None => Value::NIL,
    })
}
