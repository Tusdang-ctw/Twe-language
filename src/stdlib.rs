use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::{Env, Object, RuntimeError, Value};

pub fn install(env: &mut Env) {
    env.set(
        "print".to_string(),
        Value::Builtin {
            name: "print",
            func: print_impl,
        },
    );
    env.set(
        "load".to_string(),
        Value::Builtin {
            name: "load",
            func: load_impl,
        },
    );

    let mut key_fields = HashMap::new();
    for k in ["right", "left", "up", "down", "space"] {
        key_fields.insert(k.to_string(), Value::Bool(false));
    }
    env.set(
        "key".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: key_fields,
            kind: "input",
        }))),
    );

    // Rarity tier symbols. Stay as strings until v0.2 introduces enums.
    for r in ["common", "uncommon", "rare", "epic", "legendary"] {
        env.set(r.to_string(), Value::Str(Rc::new(r.to_string())));
    }

    install_math(env);
}

fn install_math(env: &mut Env) {
    let mut math = HashMap::new();
    math.insert(
        "abs".to_string(),
        Value::Builtin {
            name: "math.abs",
            func: math_abs,
        },
    );
    math.insert(
        "sqrt".to_string(),
        Value::Builtin {
            name: "math.sqrt",
            func: math_sqrt,
        },
    );
    math.insert(
        "floor".to_string(),
        Value::Builtin {
            name: "math.floor",
            func: math_floor,
        },
    );
    math.insert(
        "ceil".to_string(),
        Value::Builtin {
            name: "math.ceil",
            func: math_ceil,
        },
    );
    math.insert(
        "min".to_string(),
        Value::Builtin {
            name: "math.min",
            func: math_min,
        },
    );
    math.insert(
        "max".to_string(),
        Value::Builtin {
            name: "math.max",
            func: math_max,
        },
    );
    env.set(
        "math".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: math,
            kind: "module",
        }))),
    );
}

fn print_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    let parts: Vec<String> = args.iter().map(Value::display).collect();
    env.out.push_str(&parts.join(" "));
    env.out.push('\n');
    Ok(Value::Nil)
}

fn load_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    // Phase-1 stub: returns a fresh sprite-shaped object with x = 0, y = 0.
    // Real asset loading lands in Phase 2 alongside the macroquad backend.
    let mut fields = std::collections::HashMap::new();
    fields.insert("x".to_string(), Value::Int(0));
    fields.insert("y".to_string(), Value::Int(0));
    Ok(Value::Object(Rc::new(RefCell::new(Object {
        fields,
        kind: "sprite",
    }))))
}

fn arity(args: &[Value], expected: usize, name: &str) -> Result<(), RuntimeError> {
    if args.len() != expected {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{name} expected {expected} argument{}, got {}",
                if expected == 1 { "" } else { "s" },
                args.len()
            ),
            help: None,
        });
    }
    Ok(())
}

fn as_f64(v: &Value, op: &str) -> Result<f64, RuntimeError> {
    match v {
        Value::Int(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op} expected a numeric argument, got {}", other.type_name()),
            help: None,
        }),
    }
}

fn math_abs(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.abs")?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.abs())),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("math.abs expected int or float, got {}", other.type_name()),
            help: None,
        }),
    }
}

fn math_sqrt(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.sqrt")?;
    let x = as_f64(&args[0], "math.sqrt")?;
    if x < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("math.sqrt of negative number {x}"),
            help: Some("complex numbers ship later; check your input".to_string()),
        });
    }
    Ok(Value::Float(x.sqrt()))
}

fn math_floor(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.floor")?;
    let x = as_f64(&args[0], "math.floor")?;
    Ok(Value::Int(x.floor() as i64))
}

fn math_ceil(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.ceil")?;
    let x = as_f64(&args[0], "math.ceil")?;
    Ok(Value::Int(x.ceil() as i64))
}

fn math_min(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "math.min")?;
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int((*a).min(*b))),
        (a, b) => {
            let af = as_f64(a, "math.min")?;
            let bf = as_f64(b, "math.min")?;
            Ok(Value::Float(af.min(bf)))
        }
    }
}

fn math_max(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "math.max")?;
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int((*a).max(*b))),
        (a, b) => {
            let af = as_f64(a, "math.max")?;
            let bf = as_f64(b, "math.max")?;
            Ok(Value::Float(af.max(bf)))
        }
    }
}
