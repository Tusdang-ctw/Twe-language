use std::cell::RefCell;
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
