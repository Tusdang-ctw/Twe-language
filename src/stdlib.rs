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

    let key_names = [
        "right", "left", "up", "down", "space", "escape", "enter", "r", "w", "a", "s", "d",
    ];
    let mut key_fields = HashMap::new();
    let mut press_fields = HashMap::new();
    for k in key_names {
        key_fields.insert(k.to_string(), Value::Bool(false));
        press_fields.insert(k.to_string(), Value::Bool(false));
    }
    env.set(
        "key".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: key_fields,
            kind: "input",
        }))),
    );
    env.set(
        "key_press".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: press_fields,
            kind: "input",
        }))),
    );

    // Rarity tier symbols. Stay as strings until v0.2 introduces enums.
    for r in ["common", "uncommon", "rare", "epic", "legendary"] {
        env.set(r.to_string(), Value::Str(Rc::new(r.to_string())));
    }

    install_math(env);
    install_random(env);
    install_color(env);
    install_screen(env);
    install_draw(env);
    install_entities(env);
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

fn install_random(env: &mut Env) {
    let mut random = HashMap::new();
    random.insert(
        "int".to_string(),
        Value::Builtin {
            name: "random.int",
            func: random_int,
        },
    );
    random.insert(
        "float".to_string(),
        Value::Builtin {
            name: "random.float",
            func: random_float,
        },
    );
    random.insert(
        "choice".to_string(),
        Value::Builtin {
            name: "random.choice",
            func: random_choice,
        },
    );
    random.insert(
        "seed".to_string(),
        Value::Builtin {
            name: "random.seed",
            func: random_seed,
        },
    );
    env.set(
        "random".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: random,
            kind: "module",
        }))),
    );
}

fn random_int(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "random.int")?;
    let (start, end, exclusive) = match &args[0] {
        Value::Range { start, end, exclusive } => (*start, *end, *exclusive),
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "random.int expected a range, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `random.int(1..6)` rolls a six-sided die".to_string()),
            });
        }
    };
    let upper = if exclusive { end } else { end + 1 };
    if upper <= start {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "random.int on an empty range".to_string(),
            help: None,
        });
    }
    let n = env.next_random_u64();
    let span = (upper - start) as u64;
    Ok(Value::Int(start + (n % span) as i64))
}

fn random_float(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "random.float")?;
    // 53 bits of randomness mapped to [0.0, 1.0).
    let n = env.next_random_u64() >> 11;
    let f = n as f64 * (1.0 / ((1u64 << 53) as f64));
    Ok(Value::Float(f))
}

fn random_choice(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "random.choice")?;
    match &args[0] {
        Value::List(rc) => {
            let v = rc.borrow();
            if v.is_empty() {
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: "random.choice on an empty list".to_string(),
                    help: None,
                });
            }
            let idx = (env.next_random_u64() as usize) % v.len();
            Ok(v[idx].clone())
        }
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "random.choice expected a list, got {}",
                other.type_name()
            ),
            help: None,
        }),
    }
}

fn random_seed(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "random.seed")?;
    match &args[0] {
        Value::Int(n) => {
            env.seed_rng(*n as u64);
            Ok(Value::Nil)
        }
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "random.seed expected an int, got {}",
                other.type_name()
            ),
            help: None,
        }),
    }
}

fn install_color(env: &mut Env) {
    let palette: &[(&str, f64, f64, f64, f64)] = &[
        ("red", 1.0, 0.0, 0.0, 1.0),
        ("green", 0.0, 1.0, 0.0, 1.0),
        ("blue", 0.0, 0.0, 1.0, 1.0),
        ("yellow", 1.0, 1.0, 0.0, 1.0),
        ("orange", 1.0, 0.5, 0.0, 1.0),
        ("purple", 0.5, 0.0, 0.5, 1.0),
        ("white", 1.0, 1.0, 1.0, 1.0),
        ("black", 0.0, 0.0, 0.0, 1.0),
        ("gray", 0.5, 0.5, 0.5, 1.0),
        ("transparent", 0.0, 0.0, 0.0, 0.0),
    ];
    let mut fields = HashMap::new();
    for (name, r, g, b, a) in palette {
        fields.insert(
            (*name).to_string(),
            Value::Tuple(Rc::new(vec![
                Value::Float(*r),
                Value::Float(*g),
                Value::Float(*b),
                Value::Float(*a),
            ])),
        );
    }
    env.set(
        "color".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields,
            kind: "module",
        }))),
    );
}

fn install_screen(env: &mut Env) {
    // Default values; the play loop overwrites them each frame.
    let mut fields = HashMap::new();
    fields.insert(
        "size".to_string(),
        Value::Tuple(Rc::new(vec![Value::Float(640.0), Value::Float(480.0)])),
    );
    fields.insert(
        "center".to_string(),
        Value::Tuple(Rc::new(vec![Value::Float(320.0), Value::Float(240.0)])),
    );
    env.set(
        "screen".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields,
            kind: "module",
        }))),
    );
}

fn install_draw(env: &mut Env) {
    env.set(
        "rect".to_string(),
        Value::Builtin {
            name: "rect",
            func: draw_rect,
        },
    );
    env.set(
        "circle".to_string(),
        Value::Builtin {
            name: "circle",
            func: draw_circle,
        },
    );
    env.set(
        "line".to_string(),
        Value::Builtin {
            name: "line",
            func: draw_line,
        },
    );
    env.set(
        "text".to_string(),
        Value::Builtin {
            name: "text",
            func: draw_text,
        },
    );
}

fn require_render(env: &Env, name: &str) -> Result<(), RuntimeError> {
    if !env.in_render {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{name}() must be called from inside `on render():`"),
            help: Some(
                "drawing primitives are only valid in a render handler; \
                 do mutation in `every` / `on update(dt)` instead"
                    .to_string(),
            ),
        });
    }
    Ok(())
}

fn xy_of(v: &Value, what: &str) -> Result<(f64, f64), RuntimeError> {
    match v {
        Value::Tuple(elems) if elems.len() >= 2 => {
            Ok((number(&elems[0], what)?, number(&elems[1], what)?))
        }
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects a tuple of 2 numbers, got {}",
                other.type_name()
            ),
            help: None,
        }),
    }
}

fn color_of(v: &Value, what: &str) -> Result<macroquad::color::Color, RuntimeError> {
    match v {
        Value::Tuple(elems) if elems.len() >= 3 => {
            let r = number(&elems[0], what)? as f32;
            let g = number(&elems[1], what)? as f32;
            let b = number(&elems[2], what)? as f32;
            let a = if elems.len() >= 4 {
                number(&elems[3], what)? as f32
            } else {
                1.0
            };
            Ok(macroquad::color::Color::new(r, g, b, a))
        }
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what} expects an (r, g, b[, a]) tuple, got {}", other.type_name()),
            help: Some("use color.red, color.green, … or build with `(0.5, 0.0, 0.0, 1.0)`".to_string()),
        }),
    }
}

fn number(v: &Value, what: &str) -> Result<f64, RuntimeError> {
    match v {
        Value::Int(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        Value::Quantity { value, .. } => Ok(*value),
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what} expects a number, got {}", other.type_name()),
            help: None,
        }),
    }
}

fn draw_rect(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "rect")?;
    arity(args, 3, "rect")?;
    let (x, y) = xy_of(&args[0], "rect.at")?;
    let (w, h) = xy_of(&args[1], "rect.size")?;
    let color = color_of(&args[2], "rect.color")?;
    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, color);
    Ok(Value::Nil)
}

fn draw_circle(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "circle")?;
    arity(args, 3, "circle")?;
    let (x, y) = xy_of(&args[0], "circle.at")?;
    let radius = number(&args[1], "circle.radius")? as f32;
    let color = color_of(&args[2], "circle.color")?;
    macroquad::shapes::draw_circle(x as f32, y as f32, radius, color);
    Ok(Value::Nil)
}

fn draw_line(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "line")?;
    arity(args, 4, "line")?;
    let (x1, y1) = xy_of(&args[0], "line.from")?;
    let (x2, y2) = xy_of(&args[1], "line.to")?;
    let thickness = number(&args[2], "line.width")? as f32;
    let color = color_of(&args[3], "line.color")?;
    macroquad::shapes::draw_line(
        x1 as f32, y1 as f32, x2 as f32, y2 as f32, thickness, color,
    );
    Ok(Value::Nil)
}

fn install_entities(env: &mut Env) {
    let mut entities = HashMap::new();
    entities.insert(
        "of".to_string(),
        Value::Builtin {
            name: "entities.of",
            func: entities_of,
        },
    );
    entities.insert(
        "count".to_string(),
        Value::Builtin {
            name: "entities.count",
            func: entities_count,
        },
    );
    env.set(
        "entities".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: entities,
            kind: "module",
        }))),
    );
}

fn entities_of(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "entities.of")?;
    let class = match &args[0] {
        Value::Class(c) => c.clone(),
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "entities.of expects a class (e.g. `entities.of(Monster)`), got {}",
                    other.type_name()
                ),
                help: Some(
                    "pass the entity class itself, not an instance".to_string(),
                ),
            });
        }
    };
    let mut result = Vec::new();
    for inst in &env.active_entities {
        let borrowed = inst.borrow();
        if borrowed.despawned {
            continue;
        }
        if Rc::ptr_eq(&borrowed.class, &class) {
            drop(borrowed);
            result.push(Value::Instance(inst.clone()));
        }
    }
    Ok(Value::List(Rc::new(RefCell::new(result))))
}

fn entities_count(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "entities.count")?;
    let class = match &args[0] {
        Value::Class(c) => c.clone(),
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "entities.count expects a class (e.g. `entities.count(Monster)`), got {}",
                    other.type_name()
                ),
                help: None,
            });
        }
    };
    let mut n: i64 = 0;
    for inst in &env.active_entities {
        let borrowed = inst.borrow();
        if borrowed.despawned {
            continue;
        }
        if Rc::ptr_eq(&borrowed.class, &class) {
            n += 1;
        }
    }
    Ok(Value::Int(n))
}

fn draw_text(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "text")?;
    arity(args, 4, "text")?;
    let content = match &args[0] {
        Value::Str(s) => s.as_ref().clone(),
        other => other.display(),
    };
    let (x, y) = xy_of(&args[1], "text.at")?;
    let size = number(&args[2], "text.size")? as f32;
    let color = color_of(&args[3], "text.color")?;
    macroquad::text::draw_text(&content, x as f32, y as f32, size, color);
    Ok(Value::Nil)
}
