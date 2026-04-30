use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::{Env, Object, RuntimeError, Value};

// Texture cache: macroquad's `Texture2D` can only be constructed once
// the GL context exists, so loading is lazy — the first `sprite(spr, at)`
// call inside `on render():` decodes the file. Cleared by `clear_sprite_cache`
// when `twec play` hot-reloads the script. Single-threaded (macroquad
// runs on the main thread), so a thread_local is the right shape.
thread_local! {
    static SPRITE_CACHE: RefCell<HashMap<String, macroquad::texture::Texture2D>>
        = RefCell::new(HashMap::new());
    static SOUND_CACHE: RefCell<HashMap<String, macroquad::audio::Sound>>
        = RefCell::new(HashMap::new());
}

/// Drop every cached `Texture2D` and `Sound`. The play loop calls this
/// on hot reload so swapped asset paths pick up.
pub fn clear_asset_caches() {
    SPRITE_CACHE.with(|c| c.borrow_mut().clear());
    SOUND_CACHE.with(|c| c.borrow_mut().clear());
}

/// Drive a future to completion synchronously. Used for macroquad's
/// async asset APIs (e.g. `audio::load_sound_from_bytes`) whose
/// underlying work is sync CPU; the futures only exist for browser
/// compatibility and never actually pend on native. Uses
/// `Waker::noop()` (stable since Rust 1.85) so no unsafe is needed —
/// `unsafe_code = "forbid"` stays intact.
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let waker = Waker::noop();
    let mut ctx = Context::from_waker(waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(out) = f.as_mut().poll(&mut ctx) {
            return out;
        }
    }
}

pub fn install(env: &mut Env) {
    env.set(
        "print".to_string(),
        Value::Builtin {
            name: "print",
            params: &[],
            func: print_impl,
        },
    );
    env.set(
        "load".to_string(),
        Value::Builtin {
            name: "load",
            params: &["path"],
            func: load_impl,
        },
    );
    // v0.2 session 4: save / load for Twe Values. Bottom layer
    // of the eventual `save` block compiler — see
    // `docs/07-save-system.md`. Schema declarations come in
    // session 5+; for now `save_to` / `load_from` round-trip
    // the serializable Value subset directly.
    env.set(
        "save_to".to_string(),
        Value::Builtin {
            name: "save_to",
            params: &["path", "value"],
            func: save_to_impl,
        },
    );
    env.set(
        "load_from".to_string(),
        Value::Builtin {
            name: "load_from",
            params: &["path"],
            func: load_from_impl,
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

    // v0.2 session 3: mouse surface. Mirrors the key / key_press
    // pair for symmetry. `mouse` carries cursor position + wheel
    // delta; `mouse_held` is continuous (true while held);
    // `mouse_press` is edge-triggered (true only on the frame the
    // button transitions to down). Both backends (macroquad
    // `play` and winit `play3d`) update these each frame.
    let mut mouse_fields = HashMap::new();
    mouse_fields.insert("x".to_string(), Value::Float(0.0));
    mouse_fields.insert("y".to_string(), Value::Float(0.0));
    mouse_fields.insert(
        "pos".to_string(),
        Value::Tuple(Rc::new(vec![Value::Float(0.0), Value::Float(0.0)])),
    );
    mouse_fields.insert("wheel".to_string(), Value::Float(0.0));
    env.set(
        "mouse".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: mouse_fields,
            kind: "input",
        }))),
    );
    let buttons = ["left", "middle", "right"];
    let mut held = HashMap::new();
    let mut pressed = HashMap::new();
    for b in buttons {
        held.insert(b.to_string(), Value::Bool(false));
        pressed.insert(b.to_string(), Value::Bool(false));
    }
    env.set(
        "mouse_held".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: held,
            kind: "input",
        }))),
    );
    env.set(
        "mouse_press".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: pressed,
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
    install_time(env);
    install_sound(env);
    install_3d(env);
}

fn install_sound(env: &mut Env) {
    let mut sound = HashMap::new();
    sound.insert(
        "load".to_string(),
        Value::Builtin {
            name: "sound.load",
            params: &["path"],
            func: sound_load,
        },
    );
    sound.insert(
        "play".to_string(),
        Value::Builtin {
            name: "sound.play",
            params: &["handle"],
            func: sound_play,
        },
    );
    env.set(
        "sound".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields: sound,
            kind: "module",
        }))),
    );
}

fn sound_load(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sound.load")?;
    let path = match &args[0] {
        Value::Str(s) => s.as_ref().clone(),
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "sound.load expected a string path, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `sound.load(\"shot.wav\")`".to_string()),
            });
        }
    };
    if std::fs::metadata(&path).is_err() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("sound.load: cannot find asset '{path}'"),
            help: Some(
                "the path is relative to the working directory; check spelling and case"
                    .to_string(),
            ),
        });
    }
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::Str(Rc::new(path)));
    Ok(Value::Object(Rc::new(RefCell::new(Object {
        fields,
        kind: "sound",
    }))))
}

fn sound_play(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sound.play")?;
    let path = match &args[0] {
        Value::Object(rc) => {
            let o = rc.borrow();
            if o.kind != "sound" {
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!(
                        "sound.play expects a sound handle from `sound.load(...)`, got {}",
                        o.kind
                    ),
                    help: None,
                });
            }
            match o.fields.get("path") {
                Some(Value::Str(s)) => s.as_ref().clone(),
                _ => {
                    return Err(RuntimeError {
                        line: 0,
                        col: 0,
                        message: "sound handle is missing a `path` field".to_string(),
                        help: None,
                    });
                }
            }
        }
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "sound.play expects a sound handle from `sound.load(...)`, got {}",
                    other.type_name()
                ),
                help: None,
            });
        }
    };
    SOUND_CACHE.with(|cache| -> Result<(), RuntimeError> {
        let mut c = cache.borrow_mut();
        if !c.contains_key(&path) {
            let bytes = std::fs::read(&path).map_err(|e| RuntimeError {
                line: 0,
                col: 0,
                message: format!("sound.play: cannot read '{path}': {e}"),
                help: None,
            })?;
            let snd = block_on(macroquad::audio::load_sound_from_bytes(&bytes))
                .map_err(|e| RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!("sound.play: failed to decode '{path}': {e}"),
                    help: Some("supported formats: WAV, Ogg Vorbis".to_string()),
                })?;
            c.insert(path.clone(), snd);
        }
        macroquad::audio::play_sound_once(&c[&path]);
        Ok(())
    })?;
    Ok(Value::Nil)
}

fn install_time(env: &mut Env) {
    // `time.dt` is rewritten by `eval::tick_frame` on every frame, so
    // `every` clocks (which receive no implicit dt) and other code can
    // read the live frame delta instead of hardcoding `0.016`. Closes
    // Phase-2 frustration F8.
    let mut fields = HashMap::new();
    fields.insert("dt".to_string(), Value::Float(0.0));
    env.set(
        "time".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields,
            kind: "module",
        }))),
    );
}

fn install_math(env: &mut Env) {
    let mut math = HashMap::new();
    math.insert(
        "abs".to_string(),
        Value::Builtin {
            name: "math.abs",
            params: &["x"],
            func: math_abs,
        },
    );
    math.insert(
        "sqrt".to_string(),
        Value::Builtin {
            name: "math.sqrt",
            params: &["x"],
            func: math_sqrt,
        },
    );
    math.insert(
        "floor".to_string(),
        Value::Builtin {
            name: "math.floor",
            params: &["x"],
            func: math_floor,
        },
    );
    math.insert(
        "ceil".to_string(),
        Value::Builtin {
            name: "math.ceil",
            params: &["x"],
            func: math_ceil,
        },
    );
    math.insert(
        "min".to_string(),
        Value::Builtin {
            name: "math.min",
            params: &["a", "b"],
            func: math_min,
        },
    );
    math.insert(
        "max".to_string(),
        Value::Builtin {
            name: "math.max",
            params: &["a", "b"],
            func: math_max,
        },
    );
    math.insert(
        "sin".to_string(),
        Value::Builtin {
            name: "math.sin",
            params: &["x"],
            func: math_sin,
        },
    );
    math.insert(
        "cos".to_string(),
        Value::Builtin {
            name: "math.cos",
            params: &["x"],
            func: math_cos,
        },
    );
    math.insert(
        "pi".to_string(),
        Value::Float(std::f64::consts::PI),
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

fn load_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    // Returns a sprite handle: { path, x = 0, y = 0 }. The texture
    // is decoded lazily on the first `sprite(spr, at)` call inside
    // `on render():` because macroquad's `Texture2D` can only be
    // constructed after the GL context exists. Path existence is
    // checked here so typos fail fast.
    arity(args, 1, "load")?;
    let path = match &args[0] {
        Value::Str(s) => s.as_ref().clone(),
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("load expected a string path, got {}", other.type_name()),
                help: Some("e.g. `load(\"hero.png\")`".to_string()),
            });
        }
    };
    if std::fs::metadata(&path).is_err() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("load: cannot find asset '{path}'"),
            help: Some(
                "the path is relative to the working directory; check spelling and case"
                    .to_string(),
            ),
        });
    }
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::Str(Rc::new(path)));
    fields.insert("x".to_string(), Value::Int(0));
    fields.insert("y".to_string(), Value::Int(0));
    Ok(Value::Object(Rc::new(RefCell::new(Object {
        fields,
        kind: "sprite",
    }))))
}

/// `save_to(path, value)` — serialize `value` to JSON and write
/// atomically to `path`. Errors when `value` includes a non-
/// serializable type (functions, instances, builtins). v0.2
/// session 4.
fn save_to_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save_to")?;
    let path = match &args[0] {
        Value::Str(s) => s.as_ref().clone(),
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "save_to expects a string path, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `save_to(\"slot1.save\", { hp: 100 })`".to_string()),
            });
        }
    };
    crate::save::save_to_path(std::path::Path::new(&path), &args[1])
        .map_err(|m| crate::save::to_runtime_error(m, 0, 0))?;
    Ok(Value::Nil)
}

/// `load_from(path)` — read + JSON-parse + decode a saved value.
/// Returns the value the saver passed to `save_to`. v0.2 session
/// 4 — schema enforcement deferred.
fn load_from_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "load_from")?;
    let path = match &args[0] {
        Value::Str(s) => s.as_ref().clone(),
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "load_from expects a string path, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `let state = load_from(\"slot1.save\")`".to_string()),
            });
        }
    };
    let v = crate::save::load_from_path(std::path::Path::new(&path))
        .map_err(|m| crate::save::to_runtime_error(m, 0, 0))?;
    Ok(v)
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

fn math_sin(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.sin")?;
    Ok(Value::Float(as_f64(&args[0], "math.sin")?.sin()))
}

fn math_cos(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.cos")?;
    Ok(Value::Float(as_f64(&args[0], "math.cos")?.cos()))
}

fn install_random(env: &mut Env) {
    let mut random = HashMap::new();
    random.insert(
        "int".to_string(),
        Value::Builtin {
            name: "random.int",
            params: &["range"],
            func: random_int,
        },
    );
    random.insert(
        "float".to_string(),
        Value::Builtin {
            name: "random.float",
            params: &[],
            func: random_float,
        },
    );
    random.insert(
        "choice".to_string(),
        Value::Builtin {
            name: "random.choice",
            params: &["list"],
            func: random_choice,
        },
    );
    random.insert(
        "seed".to_string(),
        Value::Builtin {
            name: "random.seed",
            params: &["seed"],
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
            params: &["at", "size", "color"],
            func: draw_rect,
        },
    );
    env.set(
        "circle".to_string(),
        Value::Builtin {
            name: "circle",
            params: &["at", "radius", "color"],
            func: draw_circle,
        },
    );
    env.set(
        "line".to_string(),
        Value::Builtin {
            name: "line",
            params: &["from", "to", "width", "color"],
            func: draw_line,
        },
    );
    env.set(
        "text".to_string(),
        Value::Builtin {
            name: "text",
            params: &["content", "at", "size", "color"],
            func: draw_text,
        },
    );
    // sprite() is variadic-style — 2 or 3 positional args, no kwargs in v0.1.
    // Add named-param support when the optional `size` slot has a clean
    // representation in bind_kwargs.
    env.set(
        "sprite".to_string(),
        Value::Builtin {
            name: "sprite",
            params: &[],
            func: draw_sprite,
        },
    );
}

fn draw_sprite(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "sprite")?;
    if args.len() != 2 && args.len() != 3 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "sprite expected 2 or 3 arguments (handle, at, [size]), got {}",
                args.len()
            ),
            help: None,
        });
    }
    let path = match &args[0] {
        Value::Object(rc) => {
            let o = rc.borrow();
            if o.kind != "sprite" {
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!(
                        "sprite expects a sprite handle from `load(...)`, got {}",
                        o.kind
                    ),
                    help: None,
                });
            }
            match o.fields.get("path") {
                Some(Value::Str(s)) => s.as_ref().clone(),
                _ => {
                    return Err(RuntimeError {
                        line: 0,
                        col: 0,
                        message: "sprite handle is missing a `path` field".to_string(),
                        help: Some(
                            "build the handle with `load(\"file.png\")` rather than \
                             constructing one by hand"
                                .to_string(),
                        ),
                    });
                }
            }
        }
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "sprite expects a sprite handle from `load(...)`, got {}",
                    other.type_name()
                ),
                help: None,
            });
        }
    };
    let (x, y) = xy_of(&args[1], "sprite.at")?;
    let size = if args.len() == 3 {
        Some(xy_of(&args[2], "sprite.size")?)
    } else {
        None
    };

    SPRITE_CACHE.with(|cache| -> Result<(), RuntimeError> {
        let mut c = cache.borrow_mut();
        if !c.contains_key(&path) {
            let bytes = std::fs::read(&path).map_err(|e| RuntimeError {
                line: 0,
                col: 0,
                message: format!("sprite: cannot read '{path}': {e}"),
                help: None,
            })?;
            let tex = macroquad::texture::Texture2D::from_file_with_format(&bytes, None);
            c.insert(path.clone(), tex);
        }
        let tex = &c[&path];
        match size {
            None => macroquad::texture::draw_texture(
                tex,
                x as f32,
                y as f32,
                macroquad::color::WHITE,
            ),
            Some((w, h)) => macroquad::texture::draw_texture_ex(
                tex,
                x as f32,
                y as f32,
                macroquad::color::WHITE,
                macroquad::texture::DrawTextureParams {
                    dest_size: Some(macroquad::math::vec2(w as f32, h as f32)),
                    ..Default::default()
                },
            ),
        }
        Ok(())
    })?;
    Ok(Value::Nil)
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
            params: &["class"],
            func: entities_of,
        },
    );
    entities.insert(
        "count".to_string(),
        Value::Builtin {
            name: "entities.count",
            params: &["class"],
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

// --- Phase 5 task 5 sessions (d) + (e): 3D rendering surface ---

fn install_3d(env: &mut Env) {
    // `vec3(x, y, z)` constructor — returns a 3-tuple. Twe tuples
    // already expose `.x`/`.y`/`.z` and component-wise +/-/* with
    // scalars (see `eval::field_get` and tuple arithmetic), so a
    // 3-tuple already behaves as a vec3. The constructor is just
    // sugar that reads better at call sites:
    //   `cube(at: vec3(0, 1, 0), …)`
    env.set(
        "vec3".to_string(),
        Value::Builtin {
            name: "vec3",
            params: &["x", "y", "z"],
            func: vec3_impl,
        },
    );

    // `cube(at: vec3, color: (r, g, b, a), size: float)` — queues a
    // unit-cube draw at `at`, scaled by `size`, tinted by `color`.
    // Only valid inside `on render():` (require_render guards).
    env.set(
        "cube".to_string(),
        Value::Builtin {
            name: "cube",
            params: &["at", "color", "size"],
            func: cube_impl,
        },
    );

    // `sphere(at: vec3, color: (r, g, b, a), size: float)` — same
    // shape as `cube`, different mesh. Phase 6 session 7 (the
    // first v0.2 carry-over to actually ship in v0.1).
    env.set(
        "sphere".to_string(),
        Value::Builtin {
            name: "sphere",
            params: &["at", "color", "size"],
            func: sphere_impl,
        },
    );

    // `mesh(path: string, at: vec3, color: (r, g, b, a), size: float)`
    // — load a glTF 2.0 binary (`.glb`) and queue an instanced draw.
    // Path is resolved relative to the working directory. The first
    // primitive of the first mesh is used; multi-primitive scenes
    // and node transforms are a follow-on. v0.2 session 1.
    env.set(
        "mesh".to_string(),
        Value::Builtin {
            name: "mesh",
            params: &["path", "at", "color", "size"],
            func: mesh_impl,
        },
    );

    // `camera` ambient — eye / target / up are mutable Tuple fields
    // the script writes via `camera.eye = vec3(...)`. The `play3d`
    // render loop reads them each frame to build the view matrix.
    let mut fields = HashMap::new();
    fields.insert(
        "eye".to_string(),
        Value::Tuple(Rc::new(vec![
            Value::Float(0.0),
            Value::Float(1.5),
            Value::Float(3.0),
        ])),
    );
    fields.insert(
        "target".to_string(),
        Value::Tuple(Rc::new(vec![
            Value::Float(0.0),
            Value::Float(0.0),
            Value::Float(0.0),
        ])),
    );
    fields.insert(
        "up".to_string(),
        Value::Tuple(Rc::new(vec![
            Value::Float(0.0),
            Value::Float(1.0),
            Value::Float(0.0),
        ])),
    );
    env.set(
        "camera".to_string(),
        Value::Object(Rc::new(RefCell::new(Object {
            fields,
            kind: "camera",
        }))),
    );
}

fn vec3_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "vec3")?;
    let x = number(&args[0], "vec3.x")?;
    let y = number(&args[1], "vec3.y")?;
    let z = number(&args[2], "vec3.z")?;
    Ok(Value::Tuple(Rc::new(vec![
        Value::Float(x),
        Value::Float(y),
        Value::Float(z),
    ])))
}

fn cube_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "cube")?;
    arity(args, 3, "cube")?;
    let at = xyz_of(&args[0], "cube.at")?;
    let color = rgba_of(&args[1], "cube.color")?;
    let size = number(&args[2], "cube.size")? as f32;
    env.render_queue3d.push(crate::value::DrawCall3d {
        primitive: crate::value::Primitive::Cube,
        at,
        color,
        size,
    });
    Ok(Value::Nil)
}

fn sphere_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "sphere")?;
    arity(args, 3, "sphere")?;
    let at = xyz_of(&args[0], "sphere.at")?;
    let color = rgba_of(&args[1], "sphere.color")?;
    let size = number(&args[2], "sphere.size")? as f32;
    env.render_queue3d.push(crate::value::DrawCall3d {
        primitive: crate::value::Primitive::Sphere,
        at,
        color,
        size,
    });
    Ok(Value::Nil)
}

fn mesh_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "mesh")?;
    arity(args, 4, "mesh")?;
    let path = match &args[0] {
        Value::Str(s) => s.clone(),
        other => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "mesh.path expects a string, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `mesh(\"models/box.glb\", at: ...)`".to_string()),
            });
        }
    };
    let at = xyz_of(&args[1], "mesh.at")?;
    let color = rgba_of(&args[2], "mesh.color")?;
    let size = number(&args[3], "mesh.size")? as f32;
    let id = env.intern_mesh_path(&path);
    env.render_queue3d.push(crate::value::DrawCall3d {
        primitive: crate::value::Primitive::Mesh(id),
        at,
        color,
        size,
    });
    Ok(Value::Nil)
}

/// Pull a 3-component float vector out of a Twe tuple. Used by the
/// 3D builtins. Mirrors `xy_of` but for the third axis.
fn xyz_of(v: &Value, what: &str) -> Result<[f32; 3], RuntimeError> {
    match v {
        Value::Tuple(elems) if elems.len() == 3 => Ok([
            number(&elems[0], what)? as f32,
            number(&elems[1], what)? as f32,
            number(&elems[2], what)? as f32,
        ]),
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects a 3-component tuple (vec3), got {}",
                other.type_name()
            ),
            help: Some("e.g. `vec3(0, 1, 0)` or `(0, 1, 0)`".to_string()),
        }),
    }
}

/// Pull an RGBA float quartet out of a Twe tuple.
fn rgba_of(v: &Value, what: &str) -> Result<[f32; 4], RuntimeError> {
    match v {
        Value::Tuple(elems) if elems.len() == 4 => Ok([
            number(&elems[0], what)? as f32,
            number(&elems[1], what)? as f32,
            number(&elems[2], what)? as f32,
            number(&elems[3], what)? as f32,
        ]),
        other => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects a 4-component color tuple, got {}",
                other.type_name()
            ),
            help: Some(
                "use `color.red` etc. or build with `(r, g, b, a)` floats".to_string(),
            ),
        }),
    }
}
