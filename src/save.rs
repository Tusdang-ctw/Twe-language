//! v0.2 session 4 — save / load for Twe Values.
//!
//! Bottom layer of the eventual `save` block compiler (see
//! `docs/07-save-system.md`). This module ships the IO + format
//! piece without the schema-block syntax. Twe scripts get
//! `save_to(path, value)` and `load_from(path) -> value`
//! builtins; values must be in the *serializable* subset.
//!
//! What's serializable:
//! - Primitives: Nil, Bool, Int, Float, Str.
//! - Twe-specific scalars: Percent, Range, Quantity (tagged so
//!   they round-trip).
//! - Composites: Tuple, List, Object (when every element is
//!   serializable).
//!
//! What's not (errors at save_to time):
//! - Class, Instance, BcInstance, Function, BcFunction, Builtin.
//!   Saves are data, not code; saving a function reference would
//!   capture a closure over the host env that isn't reconstructible.
//!
//! Format: a single JSON document on disk. Tagged objects
//! (`{ "__twe": "...", ... }`) carry the Twe-specific scalar
//! types so they survive a round trip without lossy heuristics.
//!
//! Atomicity: `save_to` writes to `<path>.tmp` first, then
//! renames to `<path>`. `rename(2)` is atomic on POSIX and on
//! NTFS via `MoveFileEx(MOVEFILE_REPLACE_EXISTING)`. We don't
//! `fsync` in v0.2 — that's a Phase-11 hardening item.
//!
//! Schema enforcement, version migration, CRC, and the Steam
//! Cloud backend all defer to v0.2 session 5+ per
//! `docs/07-save-system.md`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::json;
use crate::value::{Object, RuntimeError, Value};

/// Encode a Twe `Value` into a `json::Value`. Returns an error
/// describing the offending type if the value contains anything
/// outside the serializable subset.
pub fn encode(value: &Value) -> Result<json::Value, String> {
    match value {
        Value::Nil => Ok(json::Value::Null),
        Value::Bool(b) => Ok(json::Value::Bool(*b)),
        Value::Int(n) => Ok(json::Value::Int(*n)),
        Value::Float(f) => Ok(json::Value::Float(*f)),
        Value::Str(s) => Ok(json::Value::Str((**s).clone())),
        Value::Percent(p) => Ok(tagged("percent", &[("v", json::Value::Float(*p))])),
        Value::Range {
            start,
            end,
            exclusive,
        } => Ok(tagged(
            "range",
            &[
                ("start", json::Value::Int(*start)),
                ("end", json::Value::Int(*end)),
                ("exclusive", json::Value::Bool(*exclusive)),
            ],
        )),
        Value::Quantity { value, unit } => Ok(tagged(
            "quantity",
            &[
                ("value", json::Value::Float(*value)),
                ("unit", json::Value::Str((**unit).clone())),
            ],
        )),
        Value::Tuple(elems) => {
            let mut arr = Vec::with_capacity(elems.len());
            for e in elems.iter() {
                arr.push(encode(e)?);
            }
            Ok(tagged("tuple", &[("v", json::Value::Array(arr))]))
        }
        Value::List(rc) => {
            let v = rc.borrow();
            let mut arr = Vec::with_capacity(v.len());
            for e in v.iter() {
                arr.push(encode(e)?);
            }
            Ok(json::Value::Array(arr))
        }
        Value::Object(rc) => {
            let o = rc.borrow();
            // Refuse to save Objects whose `kind` is "class",
            // "input", "module" — these are stdlib ambients
            // (`key`, `mouse`, `math`) that capture host state,
            // not user data. The user almost certainly didn't
            // mean to dump them.
            if matches!(o.kind, "input" | "module" | "screen") {
                return Err(format!(
                    "cannot save the `{}` ambient Object — it carries host state, not game data",
                    o.kind
                ));
            }
            let mut map = std::collections::BTreeMap::new();
            // `__twe` is reserved as the type-tag key. Object
            // keys that collide get rejected.
            for (k, v) in &o.fields {
                if k == "__twe" {
                    return Err(
                        "object field name '__twe' is reserved by the save format".to_string(),
                    );
                }
                map.insert(k.clone(), encode(&v.clone().to_legacy())?);
            }
            Ok(json::Value::Object(map))
        }
        Value::Class(c) => Err(format!(
            "cannot save class '{}' — saves hold data, not declarations",
            c.name
        )),
        Value::Instance(rc) => Err(format!(
            "cannot save instance of `{}` — saves hold data, not live objects (extract the fields you want into a tuple or a plain Object first)",
            rc.borrow().class.name
        )),
        Value::BcInstance(rc) => Err(format!(
            "cannot save bytecode instance of `{}` — same restriction as `Instance`",
            rc.borrow().class.name
        )),
        Value::Function(f) => Err(format!(
            "cannot save function '{}' — saves hold data, not code",
            f.name
        )),
        Value::BcFunction(f) => Err(format!(
            "cannot save bytecode function '{}' — saves hold data, not code",
            f.name
        )),
        Value::BcClass(c) => Err(format!(
            "cannot save bytecode class '{}' — saves hold data, not declarations",
            c.name
        )),
        Value::Builtin { name, .. } => Err(format!(
            "cannot save builtin '{name}' — saves hold data, not code"
        )),
    }
}

/// Decode a `json::Value` into a Twe `Value`. JSON's data model
/// is a strict subset of Twe's serializable surface, so this
/// always succeeds. Tagged objects (`__twe`) reconstruct
/// Tuple / Range / Quantity / Percent. Untagged objects come
/// back as plain `Value::Object` with `kind: "save"` (a sentinel
/// that distinguishes "loaded from disk" from stdlib ambients).
pub fn decode(value: &json::Value) -> Value {
    match value {
        json::Value::Null => Value::Nil,
        json::Value::Bool(b) => Value::Bool(*b),
        json::Value::Int(n) => Value::Int(*n),
        json::Value::Float(f) => Value::Float(*f),
        json::Value::Str(s) => Value::Str(Rc::new(s.clone())),
        json::Value::Array(arr) => {
            let elems: Vec<Value> = arr.iter().map(decode).collect();
            Value::List(Rc::new(RefCell::new(elems)))
        }
        json::Value::Object(map) => {
            // Tagged round-trip for Twe-specific scalars.
            if let Some(json::Value::Str(tag)) = map.get("__twe") {
                match tag.as_str() {
                    "tuple" => {
                        if let Some(json::Value::Array(arr)) = map.get("v") {
                            let elems: Vec<Value> = arr.iter().map(decode).collect();
                            return Value::Tuple(Rc::new(elems));
                        }
                    }
                    "percent" => {
                        if let Some(json::Value::Float(f)) = map.get("v") {
                            return Value::Percent(*f);
                        }
                        if let Some(json::Value::Int(n)) = map.get("v") {
                            return Value::Percent(*n as f64);
                        }
                    }
                    "range" => {
                        if let (
                            Some(json::Value::Int(s)),
                            Some(json::Value::Int(e)),
                            Some(json::Value::Bool(ex)),
                        ) = (map.get("start"), map.get("end"), map.get("exclusive"))
                        {
                            return Value::Range {
                                start: *s,
                                end: *e,
                                exclusive: *ex,
                            };
                        }
                    }
                    "quantity" => {
                        let value = match map.get("value") {
                            Some(json::Value::Float(f)) => Some(*f),
                            Some(json::Value::Int(n)) => Some(*n as f64),
                            _ => None,
                        };
                        if let (Some(value), Some(json::Value::Str(unit))) =
                            (value, map.get("unit"))
                        {
                            return Value::Quantity {
                                value,
                                unit: Rc::new(unit.clone()),
                            };
                        }
                    }
                    _ => {}
                }
                // Unknown tag — fall through to plain Object decoding so
                // the data still loads (the user can inspect `__twe`).
            }
            let mut fields = HashMap::new();
            for (k, v) in map {
                fields.insert(k.clone(), decode(v));
            }
            Value::Object(Rc::new(RefCell::new(Object {
                fields: crate::value::legacy_fields_to_tagged(fields),
                kind: "save",
            })))
        }
    }
}

fn tagged(tag: &str, fields: &[(&str, json::Value)]) -> json::Value {
    let mut map = std::collections::BTreeMap::new();
    map.insert("__twe".to_string(), json::Value::Str(tag.to_string()));
    for (k, v) in fields {
        map.insert((*k).to_string(), v.clone());
    }
    json::Value::Object(map)
}

/// Atomic write: encode, write to `<path>.tmp`, rename to `<path>`.
/// `rename(2)` is atomic on POSIX and on NTFS via
/// `MoveFileEx(MOVEFILE_REPLACE_EXISTING)`. v0.2 session 4 —
/// `fsync` is deferred to Phase 11 hardening.
pub fn save_to_path(path: &Path, value: &Value) -> Result<(), String> {
    let json_value = encode(value)?;
    let serialized = json::to_string(&json_value);
    if let Some(parent) = path.parent() {
        // Skip mkdir for paths with no parent component (e.g. a
        // bare filename in the cwd). Otherwise create_dir_all is
        // a no-op when the dir exists.
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create save directory: {e}"))?;
        }
    }
    let tmp_path: PathBuf = {
        let mut p = path.to_path_buf();
        let mut name = p
            .file_name()
            .ok_or_else(|| format!("invalid save path: {}", path.display()))?
            .to_owned();
        name.push(".tmp");
        p.set_file_name(name);
        p
    };
    std::fs::write(&tmp_path, serialized.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("cannot rename {} -> {}: {e}", tmp_path.display(), path.display()))?;
    Ok(())
}

/// Read + parse + decode. Errors carry the path + the underlying
/// IO / parse failure for debug-ability.
pub fn load_from_path(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| format!("save file at {} is not valid UTF-8: {e}", path.display()))?;
    let json_value = json::parse(text)
        .map_err(|e| format!("save file at {} is not valid JSON: {e}", path.display()))?;
    Ok(decode(&json_value))
}

/// Convert the `Result<(), String>` shape of save_to_path into a
/// `RuntimeError` for the stdlib builtin path. v0.2 session 4.
pub fn to_runtime_error(msg: String, line: u32, col: u32) -> RuntimeError {
    RuntimeError {
        line,
        col,
        message: msg,
        help: Some(
            "saves hold data only — primitives, lists, tuples, plain Objects. Pull values out of class instances before saving."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(v: Value) -> Value {
        let encoded = encode(&v).expect("encode");
        let serialized = json::to_string(&encoded);
        let parsed = json::parse(&serialized).expect("parse");
        decode(&parsed)
    }

    #[test]
    fn primitives_round_trip() {
        match round_trip(Value::Int(42)) {
            Value::Int(42) => {}
            other => panic!("expected Int(42), got {other:?}"),
        }
        match round_trip(Value::Float(3.14)) {
            Value::Float(f) if (f - 3.14).abs() < 1e-9 => {}
            other => panic!("expected ~Float(3.14), got {other:?}"),
        }
        match round_trip(Value::Bool(true)) {
            Value::Bool(true) => {}
            other => panic!("expected Bool(true), got {other:?}"),
        }
        match round_trip(Value::Nil) {
            Value::Nil => {}
            other => panic!("expected Nil, got {other:?}"),
        }
        match round_trip(Value::Str(Rc::new("hello".to_string()))) {
            Value::Str(s) if &**s == "hello" => {}
            other => panic!("expected Str(\"hello\"), got {other:?}"),
        }
    }

    #[test]
    fn tuple_round_trips_as_tuple_not_list() {
        let v = Value::Tuple(Rc::new(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
        let back = round_trip(v);
        match back {
            Value::Tuple(elems) => {
                assert_eq!(elems.len(), 3);
                assert!(matches!(elems[0], Value::Int(1)));
            }
            other => panic!("expected Tuple, got {other:?}"),
        }
    }

    #[test]
    fn list_round_trips_as_list() {
        let v = Value::List(Rc::new(RefCell::new(vec![Value::Int(7), Value::Int(8)])));
        let back = round_trip(v);
        match back {
            Value::List(rc) => assert_eq!(rc.borrow().len(), 2),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn quantity_round_trips_with_unit() {
        let v = Value::Quantity {
            value: 5.0,
            unit: Rc::new("kg".to_string()),
        };
        match round_trip(v) {
            Value::Quantity { value, unit } => {
                assert_eq!(value, 5.0);
                assert_eq!(&**unit, "kg");
            }
            other => panic!("expected Quantity, got {other:?}"),
        }
    }

    #[test]
    fn range_round_trips() {
        let v = Value::Range {
            start: 0,
            end: 10,
            exclusive: true,
        };
        match round_trip(v) {
            Value::Range {
                start,
                end,
                exclusive,
            } => {
                assert_eq!(start, 0);
                assert_eq!(end, 10);
                assert!(exclusive);
            }
            other => panic!("expected Range, got {other:?}"),
        }
    }

    #[test]
    fn percent_round_trips() {
        let v = Value::Percent(0.25);
        match round_trip(v) {
            Value::Percent(p) => assert!((p - 0.25).abs() < 1e-9),
            other => panic!("expected Percent, got {other:?}"),
        }
    }

    #[test]
    fn nested_object_round_trips() {
        let mut inner = HashMap::new();
        inner.insert("hp".to_string(), Value::Int(100));
        inner.insert("name".to_string(), Value::Str(Rc::new("Hero".to_string())));
        let v = Value::Object(Rc::new(RefCell::new(Object {
            fields: crate::value::legacy_fields_to_tagged(inner),
            kind: "save",
        })));
        match round_trip(v) {
            Value::Object(rc) => {
                let o = rc.borrow();
                assert!(matches!(o.get_field("hp"), Some(Value::Int(100))));
                assert!(matches!(o.get_field("name"), Some(Value::Str(_))));
            }
            other => panic!("expected Object, got {other:?}"),
        }
    }

    #[test]
    fn function_value_refuses_to_serialize() {
        // Construct a function value via FunctionDef. Encoding
        // it should error with a clear message.
        let def = crate::value::FunctionDef {
            name: "f".to_string(),
            params: vec![],
            body: vec![],
        };
        let v = Value::Function(Rc::new(def));
        let err = encode(&v).expect_err("functions must not save");
        assert!(err.contains("function") && err.contains("data, not code"));
    }

    #[test]
    fn input_ambient_refuses_to_serialize() {
        let v = Value::Object(Rc::new(RefCell::new(Object {
            fields: HashMap::new(),
            kind: "input",
        })));
        let err = encode(&v).expect_err("input ambient must not save");
        assert!(err.contains("ambient") && err.contains("input"));
    }

    #[test]
    fn save_to_and_load_from_path_round_trip() {
        let dir = std::env::temp_dir();
        let path = dir.join("twec_save_test_round_trip.json");
        let _ = std::fs::remove_file(&path);

        let mut fields = HashMap::new();
        fields.insert("hp".to_string(), Value::Int(75));
        fields.insert(
            "name".to_string(),
            Value::Str(Rc::new("Hero".to_string())),
        );
        let v = Value::Object(Rc::new(RefCell::new(Object {
            fields: crate::value::legacy_fields_to_tagged(fields),
            kind: "save",
        })));

        save_to_path(&path, &v).expect("save");
        let loaded = load_from_path(&path).expect("load");
        let _ = std::fs::remove_file(&path);

        match loaded {
            Value::Object(rc) => {
                let o = rc.borrow();
                assert!(matches!(o.get_field("hp"), Some(Value::Int(75))));
            }
            other => panic!("expected Object, got {other:?}"),
        }
    }

    #[test]
    fn load_from_missing_file_errors_clearly() {
        let err = load_from_path(Path::new(".twec_no_such_save_file.json"))
            .expect_err("missing file should error");
        assert!(err.contains("cannot read"), "got: {err}");
    }

    #[test]
    fn load_from_invalid_json_errors_clearly() {
        let dir = std::env::temp_dir();
        let path = dir.join("twec_save_test_invalid.json");
        std::fs::write(&path, "not json {{").expect("write");
        let err = load_from_path(&path).expect_err("bad json should error");
        let _ = std::fs::remove_file(&path);
        assert!(err.contains("not valid JSON"), "got: {err}");
    }
}
