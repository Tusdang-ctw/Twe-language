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
    // Phase 9 session 2: 2D camera shake state. The Twe-facing API is
    // `camera.shake(amplitude, duration)`; that builtin writes here.
    // Each frame the play loop calls `camera_tick` to decay the timer
    // and `camera_shake_offset` to read a fresh random offset that
    // feeds the macroquad `Camera2D` transform.
    static CAMERA_SHAKE: RefCell<CameraShake> = const { RefCell::new(CameraShake {
        amplitude: 0.0,
        remaining: 0.0,
    }) };
}

#[derive(Clone, Copy)]
struct CameraShake {
    amplitude: f64,
    remaining: f64,
}

/// Drop every cached `Texture2D` and `Sound`. The play loop calls this
/// on hot reload so swapped asset paths pick up. Also resets camera
/// shake — a hot-reloaded script shouldn't inherit jitter from the
/// previous session.
pub fn clear_asset_caches() {
    SPRITE_CACHE.with(|c| c.borrow_mut().clear());
    SOUND_CACHE.with(|c| c.borrow_mut().clear());
    CAMERA_SHAKE.with(|c| {
        let mut s = c.borrow_mut();
        s.amplitude = 0.0;
        s.remaining = 0.0;
    });
}

/// Decay the camera-shake timer by `dt` seconds. The play loop calls
/// this once per frame between `tick_frame` and `render_frame`.
pub fn camera_tick(dt: f64) {
    CAMERA_SHAKE.with(|c| {
        let mut s = c.borrow_mut();
        if s.remaining > 0.0 {
            s.remaining = (s.remaining - dt).max(0.0);
            if s.remaining == 0.0 {
                s.amplitude = 0.0;
            }
        }
    });
}

/// Sample a per-frame screen-shake offset in pixels. Returns `(0, 0)`
/// when no shake is active. Uses the env's PRNG so the offset stream
/// is reproducible across replays of the same seed.
pub fn camera_shake_offset(env: &mut Env) -> (f64, f64) {
    let amp = CAMERA_SHAKE.with(|c| {
        let s = c.borrow();
        if s.remaining > 0.0 { s.amplitude } else { 0.0 }
    });
    if amp == 0.0 {
        return (0.0, 0.0);
    }
    // Two 53-bit floats in [0, 1), remapped to [-amp, amp].
    let a = (env.next_random_u64() >> 11) as f64 * (1.0 / ((1u64 << 53) as f64));
    let b = (env.next_random_u64() >> 11) as f64 * (1.0 / ((1u64 << 53) as f64));
    let dx = (a * 2.0 - 1.0) * amp;
    let dy = (b * 2.0 - 1.0) * amp;
    (dx, dy)
}

/// Read the current `camera.pos` and `camera.zoom` off the Twe ambient.
/// Returns `((px, py), zoom)`. Falls back to `((0, 0), 1.0)` if the
/// camera ambient is missing or has been overwritten — the render loop
/// must not crash on a script with `camera = nil`.
pub fn camera_view(env: &Env) -> ((f64, f64), f64) {
    let Some(cam) = env.get("camera") else {
        return ((0.0, 0.0), 1.0);
    };
    if !cam.is_object() {
        return ((0.0, 0.0), 1.0);
    }
    let rc = cam.as_object();
    let o = rc.borrow();
    let pos = match o.get_field("pos") {
        Some(v) if v.is_tuple() => {
            let elems = v.as_tuple();
            if elems.len() >= 2 {
                let x = number(&elems[0], "camera.pos.x").unwrap_or(0.0);
                let y = number(&elems[1], "camera.pos.y").unwrap_or(0.0);
                (x, y)
            } else {
                (0.0, 0.0)
            }
        }
        _ => (0.0, 0.0),
    };
    let zoom = match o.get_field("zoom") {
        Some(v) => number(&v, "camera.zoom").unwrap_or(1.0),
        None => 1.0,
    };
    ((pos.0, pos.1), zoom)
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
        Value::from_builtin("print", &[], print_impl),
    );
    env.set(
        "load".to_string(),
        Value::from_builtin("load", &["path"], load_impl),
    );
    // Phase 9 session 3: spritesheet loader. `load_atlas(path, grid)`
    // returns an atlas handle `{ path, grid, kind: "atlas" }` where
    // `grid = (cols, rows)`. Combine with `sprite_frame(handle, at,
    // frame)` to draw one cell. Keeping `sprite(handle, at, [size])`
    // unchanged on plain sprites — atlases get their own draw call
    // because Twe's calling convention requires every kwarg to be
    // supplied (no defaults yet), so an optional `frame:` on sprite
    // isn't expressible without breaking existing call sites.
    env.set(
        "load_atlas".to_string(),
        Value::from_builtin("load_atlas", &["path", "grid"], load_atlas_impl),
    );
    // v0.2 session 4: save / load for Twe Values. Bottom layer
    // of the eventual `save` block compiler — see
    // `docs/07-save-system.md`. Schema declarations come in
    // session 5+; for now `save_to` / `load_from` round-trip
    // the serializable Value subset directly.
    env.set(
        "save_to".to_string(),
        Value::from_builtin("save_to", &["path", "value"], save_to_impl),
    );
    env.set(
        "load_from".to_string(),
        Value::from_builtin("load_from", &["path"], load_from_impl),
    );

    let key_names = [
        "right", "left", "up", "down", "space", "escape", "enter", "r", "w", "a", "s", "d",
    ];
    let mut key_fields = HashMap::new();
    let mut press_fields = HashMap::new();
    for k in key_names {
        key_fields.insert(k.to_string(), Value::FALSE);
        press_fields.insert(k.to_string(), Value::FALSE);
    }
    env.set(
        "key".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: key_fields,
            kind: "input",
        }))),
    );
    env.set(
        "key_press".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
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
    mouse_fields.insert("x".to_string(), Value::from_float(0.0));
    mouse_fields.insert("y".to_string(), Value::from_float(0.0));
    mouse_fields.insert(
        "pos".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(0.0),
            Value::from_float(0.0),
        ])),
    );
    mouse_fields.insert("wheel".to_string(), Value::from_float(0.0));
    env.set(
        "mouse".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: mouse_fields,
            kind: "input",
        }))),
    );
    let buttons = ["left", "middle", "right"];
    let mut held = HashMap::new();
    let mut pressed = HashMap::new();
    for b in buttons {
        held.insert(b.to_string(), Value::FALSE);
        pressed.insert(b.to_string(), Value::FALSE);
    }
    env.set(
        "mouse_held".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: held,
            kind: "input",
        }))),
    );
    env.set(
        "mouse_press".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: pressed,
            kind: "input",
        }))),
    );

    // Rarity tier symbols. Stay as strings until v0.2 introduces enums.
    for r in ["common", "uncommon", "rare", "epic", "legendary"] {
        env.set(r.to_string(), Value::from_string(r.to_string()));
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
    install_tilemap(env);
}

fn install_sound(env: &mut Env) {
    let mut sound = HashMap::new();
    sound.insert(
        "load".to_string(),
        Value::from_builtin("sound.load", &["path"], sound_load),
    );
    sound.insert(
        "play".to_string(),
        Value::from_builtin("sound.play", &["handle"], sound_play),
    );
    // v0.2 session 5: audio v2. Twe's calling convention requires
    // every kwarg to be supplied (no defaults yet), so configurable
    // plays get their own fixed-arity builtins rather than overloads
    // on `sound.play`. Pitch is omitted — macroquad's quad-snd
    // backend doesn't support it.
    sound.insert(
        "play_at".to_string(),
        Value::from_builtin("sound.play_at", &["handle", "volume"], sound_play_at),
    );
    sound.insert(
        "stop".to_string(),
        Value::from_builtin("sound.stop", &["handle"], sound_stop),
    );
    sound.insert(
        "set_volume".to_string(),
        Value::from_builtin("sound.set_volume", &["handle", "volume"], sound_set_volume),
    );
    env.set(
        "sound".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: sound,
            kind: "module",
        }))),
    );

    // `music` namespace: same underlying handles, but the play
    // primitives default to `looped = true`. Useful for BGM
    // tracks that should restart at end-of-file.
    let mut music = HashMap::new();
    music.insert(
        "play".to_string(),
        Value::from_builtin("music.play", &["handle"], music_play),
    );
    music.insert(
        "play_at".to_string(),
        Value::from_builtin("music.play_at", &["handle", "volume"], music_play_at),
    );
    music.insert(
        "stop".to_string(),
        Value::from_builtin("music.stop", &["handle"], sound_stop),
    );
    env.set(
        "music".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: music,
            kind: "module",
        }))),
    );
}

fn sound_load(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sound.load")?;
    let path = {
        let __t = &args[0];
        if __t.is_str() {
            let s = __t.as_string();
            s.clone()
        } else {
            let other = *__t;
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
    fields.insert("path".to_string(), Value::from_string(path));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "sound",
    }))))
}

fn sound_play(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sound.play")?;
    let path = sound_handle_path(&args[0], "sound.play")?;
    play_sound_path(&path, "sound.play", 1.0, false)?;
    Ok(Value::NIL)
}

/// `sound.play_at(handle, volume)` — one-shot at the given volume.
/// v0.2 session 5.
fn sound_play_at(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "sound.play_at")?;
    let path = sound_handle_path(&args[0], "sound.play_at")?;
    let volume = number(&args[1], "sound.play_at.volume")? as f32;
    play_sound_path(&path, "sound.play_at", volume.clamp(0.0, 1.0), false)?;
    Ok(Value::NIL)
}

/// `sound.stop(handle)` — stop all playing instances of this
/// sound. Used as the underlying op for `music.stop` too.
/// v0.2 session 5.
fn sound_stop(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sound.stop")?;
    let path = sound_handle_path(&args[0], "sound.stop")?;
    SOUND_CACHE.with(|cache| {
        if let Some(snd) = cache.borrow().get(&path) {
            macroquad::audio::stop_sound(snd);
        }
    });
    Ok(Value::NIL)
}

/// `sound.set_volume(handle, volume)` — adjust volume of any
/// currently-playing instances. v0.2 session 5.
fn sound_set_volume(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "sound.set_volume")?;
    let path = sound_handle_path(&args[0], "sound.set_volume")?;
    let volume = number(&args[1], "sound.set_volume.volume")? as f32;
    SOUND_CACHE.with(|cache| {
        if let Some(snd) = cache.borrow().get(&path) {
            macroquad::audio::set_sound_volume(snd, volume.clamp(0.0, 1.0));
        }
    });
    Ok(Value::NIL)
}

/// `music.play(handle)` — looped play at default volume. Same
/// codepath as `sound.play` but with `looped = true`.
/// v0.2 session 5.
fn music_play(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "music.play")?;
    let path = sound_handle_path(&args[0], "music.play")?;
    play_sound_path(&path, "music.play", 1.0, true)?;
    Ok(Value::NIL)
}

/// `music.play_at(handle, volume)` — looped play at the given
/// volume. v0.2 session 5.
fn music_play_at(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "music.play_at")?;
    let path = sound_handle_path(&args[0], "music.play_at")?;
    let volume = number(&args[1], "music.play_at.volume")? as f32;
    play_sound_path(&path, "music.play_at", volume.clamp(0.0, 1.0), true)?;
    Ok(Value::NIL)
}

/// Pull the on-disk path out of a sound handle, validating
/// `kind` and the `path` field. Shared by every audio builtin.
fn sound_handle_path(v: &Value, callee: &str) -> Result<String, RuntimeError> {
    if v.is_object() {
        let rc = v.as_object();
        let o = rc.borrow();
        if o.kind != "sound" {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "{callee} expects a sound handle from `sound.load(...)`, got {}",
                    o.kind
                ),
                help: None,
            });
        }
        {
            let __opt = o.get_field("path");
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_str() {
                    let s = __t.as_string();
                    Ok(s.clone())
                } else {
                    Err(RuntimeError {
                        line: 0,
                        col: 0,
                        message: "sound handle is missing a `path` field".to_string(),
                        help: None,
                    })
                }
            } else {
                Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: "sound handle is missing a `path` field".to_string(),
                    help: None,
                })
            }
        }
    } else {
        let other = *v;
        Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{callee} expects a sound handle from `sound.load(...)`, got {}",
                other.type_name()
            ),
            help: None,
        })
    }
}

/// Decode-then-play helper. Caches decoded `Sound` values per
/// path; subsequent plays of the same file skip the decode.
/// v0.2 session 5 generalizes the old `sound_play` body.
fn play_sound_path(
    path: &str,
    callee: &str,
    volume: f32,
    looped: bool,
) -> Result<(), RuntimeError> {
    SOUND_CACHE.with(|cache| -> Result<(), RuntimeError> {
        let mut c = cache.borrow_mut();
        if !c.contains_key(path) {
            let bytes = std::fs::read(path).map_err(|e| RuntimeError {
                line: 0,
                col: 0,
                message: format!("{callee}: cannot read '{path}': {e}"),
                help: None,
            })?;
            let snd = block_on(macroquad::audio::load_sound_from_bytes(&bytes)).map_err(|e| {
                RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!("{callee}: failed to decode '{path}': {e}"),
                    help: Some("supported formats: WAV, Ogg Vorbis".to_string()),
                }
            })?;
            c.insert(path.to_string(), snd);
        }
        let snd = &c[path];
        macroquad::audio::play_sound(snd, macroquad::audio::PlaySoundParams { looped, volume });
        Ok(())
    })
}

fn install_time(env: &mut Env) {
    // `time.dt` is rewritten by `eval::tick_frame` on every frame, so
    // `every` clocks (which receive no implicit dt) and other code can
    // read the live frame delta instead of hardcoding `0.016`. Closes
    // Phase-2 frustration F8.
    let mut fields = HashMap::new();
    fields.insert("dt".to_string(), Value::from_float(0.0));
    env.set(
        "time".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "module",
        }))),
    );
}

fn install_math(env: &mut Env) {
    let mut math = HashMap::new();
    math.insert(
        "abs".to_string(),
        Value::from_builtin("math.abs", &["x"], math_abs),
    );
    math.insert(
        "sqrt".to_string(),
        Value::from_builtin("math.sqrt", &["x"], math_sqrt),
    );
    math.insert(
        "floor".to_string(),
        Value::from_builtin("math.floor", &["x"], math_floor),
    );
    math.insert(
        "ceil".to_string(),
        Value::from_builtin("math.ceil", &["x"], math_ceil),
    );
    math.insert(
        "min".to_string(),
        Value::from_builtin("math.min", &["a", "b"], math_min),
    );
    math.insert(
        "max".to_string(),
        Value::from_builtin("math.max", &["a", "b"], math_max),
    );
    math.insert(
        "sin".to_string(),
        Value::from_builtin("math.sin", &["x"], math_sin),
    );
    math.insert(
        "cos".to_string(),
        Value::from_builtin("math.cos", &["x"], math_cos),
    );
    math.insert(
        "smoothstep".to_string(),
        Value::from_builtin("math.smoothstep", &["low", "high", "x"], math_smoothstep),
    );
    math.insert(
        "mix".to_string(),
        Value::from_builtin("math.mix", &["a", "b", "t"], math_mix),
    );
    math.insert(
        "noise".to_string(),
        Value::from_builtin("math.noise", &["point"], math_noise),
    );
    math.insert("pi".to_string(), Value::from_float(std::f64::consts::PI));
    // Top-level aliases so `noise(uv)`, `smoothstep(a, b, x)`, `mix(a, b, t)` work
    // without the `math.` prefix — Example 5 in docs/01-examples.md uses the
    // bare names because the same surface must compile inside `visual` blocks
    // (Phase 9 sessions 8+) where module access isn't available.
    env.set(
        "smoothstep".to_string(),
        Value::from_builtin("smoothstep", &["low", "high", "x"], math_smoothstep),
    );
    env.set(
        "mix".to_string(),
        Value::from_builtin("mix", &["a", "b", "t"], math_mix),
    );
    env.set(
        "noise".to_string(),
        Value::from_builtin("noise", &["point"], math_noise),
    );
    env.set(
        "math".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: math,
            kind: "module",
        }))),
    );
}

fn print_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    let parts: Vec<String> = args.iter().map(Value::display).collect();
    env.out.push_str(&parts.join(" "));
    env.out.push('\n');
    Ok(Value::NIL)
}

fn load_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    // Returns a sprite handle: { path, x = 0, y = 0 }. The texture
    // is decoded lazily on the first `sprite(spr, at)` call inside
    // `on render():` because macroquad's `Texture2D` can only be
    // constructed after the GL context exists. Path existence is
    // checked here so typos fail fast.
    arity(args, 1, "load")?;
    let path = {
        let __t = &args[0];
        if __t.is_str() {
            let s = __t.as_string();
            s.clone()
        } else {
            let other = *__t;
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
    fields.insert("path".to_string(), Value::from_string(path));
    fields.insert("x".to_string(), Value::from_int(0));
    fields.insert("y".to_string(), Value::from_int(0));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "sprite",
    }))))
}

/// `load_atlas(path, grid)` — register a spritesheet. Returns an
/// atlas handle `{ path, grid, kind: "atlas" }`. `grid` is
/// `(cols, rows)` — number of cells horizontally and vertically.
/// Texture decoding is lazy on first `sprite_frame` (same reason as
/// `load`: GL context). Path existence is checked at load time.
/// Phase 9 session 3.
fn load_atlas_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "load_atlas")?;
    let path = {
        let t = &args[0];
        if t.is_str() {
            t.as_string().clone()
        } else {
            let other = *t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "load_atlas expected a string path, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `load_atlas(\"walk.png\", (8, 4))`".to_string()),
            });
        }
    };
    if std::fs::metadata(&path).is_err() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("load_atlas: cannot find asset '{path}'"),
            help: Some(
                "the path is relative to the working directory; check spelling and case"
                    .to_string(),
            ),
        });
    }
    let (cols, rows) = xy_of(&args[1], "load_atlas.grid")?;
    let cols = cols as i64;
    let rows = rows as i64;
    if cols <= 0 || rows <= 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "load_atlas: grid must be positive integers, got ({cols}, {rows})"
            ),
            help: Some("e.g. `load_atlas(\"walk.png\", (8, 4))`".to_string()),
        });
    }
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::from_string(path));
    fields.insert(
        "grid".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_int(cols),
            Value::from_int(rows),
        ])),
    );
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "atlas",
    }))))
}

/// `save_to(path, value)` — serialize `value` to JSON and write
/// atomically to `path`. Errors when `value` includes a non-
/// serializable type (functions, instances, builtins). v0.2
/// session 4.
fn save_to_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save_to")?;
    let path = {
        let __t = &args[0];
        if __t.is_str() {
            let s = __t.as_string();
            s.clone()
        } else {
            let other = *__t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("save_to expects a string path, got {}", other.type_name()),
                help: Some("e.g. `save_to(\"slot1.save\", { hp: 100 })`".to_string()),
            });
        }
    };
    crate::save::save_to_path(std::path::Path::new(&path), &args[1])
        .map_err(|m| crate::save::to_runtime_error(m, 0, 0))?;
    Ok(Value::NIL)
}

/// `load_from(path)` — read + JSON-parse + decode a saved value.
/// Returns the value the saver passed to `save_to`. v0.2 session
/// 4 — schema enforcement deferred.
fn load_from_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "load_from")?;
    let path = {
        let __t = &args[0];
        if __t.is_str() {
            let s = __t.as_string();
            s.clone()
        } else {
            let other = *__t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("load_from expects a string path, got {}", other.type_name()),
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
    if v.is_int_or_boxed_int() {
        let n = v.as_int();
        Ok(n as f64)
    } else if v.is_float() {
        let f = v.as_float();
        Ok(f)
    } else {
        let other = *v;
        Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{op} expected a numeric argument, got {}",
                other.type_name()
            ),
            help: None,
        })
    }
}

fn math_abs(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.abs")?;
    {
        let __t = &args[0];
        if __t.is_int_or_boxed_int() {
            let n = __t.as_int();
            Ok(Value::from_int(n.abs()))
        } else if __t.is_float() {
            let f = __t.as_float();
            Ok(Value::from_float(f.abs()))
        } else {
            let other = *__t;
            Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("math.abs expected int or float, got {}", other.type_name()),
                help: None,
            })
        }
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
    Ok(Value::from_float(x.sqrt()))
}

fn math_floor(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.floor")?;
    let x = as_f64(&args[0], "math.floor")?;
    Ok(Value::from_int(x.floor() as i64))
}

fn math_ceil(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.ceil")?;
    let x = as_f64(&args[0], "math.ceil")?;
    Ok(Value::from_int(x.ceil() as i64))
}

fn math_min(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "math.min")?;
    if args[0].is_int_or_boxed_int() && args[1].is_int_or_boxed_int() {
        return Ok(Value::from_int(args[0].as_int().min(args[1].as_int())));
    }
    let af = as_f64(&args[0], "math.min")?;
    let bf = as_f64(&args[1], "math.min")?;
    Ok(Value::from_float(af.min(bf)))
}

fn math_max(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "math.max")?;
    if args[0].is_int_or_boxed_int() && args[1].is_int_or_boxed_int() {
        return Ok(Value::from_int(args[0].as_int().max(args[1].as_int())));
    }
    let af = as_f64(&args[0], "math.max")?;
    let bf = as_f64(&args[1], "math.max")?;
    Ok(Value::from_float(af.max(bf)))
}

fn math_sin(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.sin")?;
    Ok(Value::from_float(as_f64(&args[0], "math.sin")?.sin()))
}

fn math_cos(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.cos")?;
    Ok(Value::from_float(as_f64(&args[0], "math.cos")?.cos()))
}

// WGSL-spec smoothstep: t = clamp((x - low) / (high - low), 0, 1);
// return t * t * (3 - 2t). Degenerate-interval (low == high) returns
// 0 if x < low else 1 — WGSL spec calls it undefined; this matches the
// step-function intuition.
fn math_smoothstep(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "smoothstep")?;
    let low = as_f64(&args[0], "smoothstep")?;
    let high = as_f64(&args[1], "smoothstep")?;
    let x = as_f64(&args[2], "smoothstep")?;
    if (high - low).abs() < f64::EPSILON {
        return Ok(Value::from_float(if x < low { 0.0 } else { 1.0 }));
    }
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    Ok(Value::from_float(t * t * (3.0 - 2.0 * t)))
}

// Linear interpolation. Works on numbers, or on same-shape tuples
// (so colors `(r, g, b, a)` and 2D vectors `(x, y)` mix elementwise).
fn math_mix(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "mix")?;
    let t = as_f64(&args[2], "mix")?;
    if args[0].is_tuple() && args[1].is_tuple() {
        let a = args[0].as_tuple();
        let b = args[1].as_tuple();
        if a.len() != b.len() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "mix: tuple lengths differ ({} vs {})",
                    a.len(),
                    b.len()
                ),
                help: Some(
                    "mix two same-shape tuples — e.g. two (r, g, b, a) colors or two (x, y) vectors"
                        .to_string(),
                ),
            });
        }
        let mut out = Vec::with_capacity(a.len());
        for i in 0..a.len() {
            let av = as_f64(&a[i], "mix")?;
            let bv = as_f64(&b[i], "mix")?;
            out.push(Value::from_float(av * (1.0 - t) + bv * t));
        }
        return Ok(Value::from_tuple(Rc::new(out)));
    }
    let a = as_f64(&args[0], "mix")?;
    let b = as_f64(&args[1], "mix")?;
    Ok(Value::from_float(a * (1.0 - t) + b * t))
}

// Wang-style 2D integer hash → float in [-1, 1]. Deterministic so
// the CPU `noise` and the future WGSL `noise` (Phase 9 session 10)
// produce bit-identical output for the same point. The 0x9e3779b9
// offset is the 32-bit golden-ratio constant; without it,
// `noise_hash2(0, 0)` would collapse to 0 → output -1.0, which makes
// the origin a strong negative point in the noise field.
fn noise_hash2(x: i64, y: i64) -> f64 {
    let mut h = (x as u32)
        .wrapping_mul(0x27d4_eb2du32)
        .wrapping_add((y as u32).wrapping_mul(0x1656_67b1u32))
        .wrapping_add(0x9e37_79b9u32);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6bu32);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35u32);
    h ^= h >> 16;
    (h as f64 / u32::MAX as f64) * 2.0 - 1.0
}

// 2D value noise: hash four lattice corners, smoothstep-weighted
// bilinear interp on the fractional. Output is in [-1, 1].
fn value_noise_2d(x: f64, y: f64) -> f64 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let u = xf * xf * (3.0 - 2.0 * xf);
    let v = yf * yf * (3.0 - 2.0 * yf);
    let xi = xi as i64;
    let yi = yi as i64;
    let n00 = noise_hash2(xi, yi);
    let n10 = noise_hash2(xi + 1, yi);
    let n01 = noise_hash2(xi, yi + 1);
    let n11 = noise_hash2(xi + 1, yi + 1);
    let nx0 = n00 * (1.0 - u) + n10 * u;
    let nx1 = n01 * (1.0 - u) + n11 * u;
    nx0 * (1.0 - v) + nx1 * v
}

fn math_noise(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "noise")?;
    let (x, y) = xy_of(&args[0], "noise")?;
    Ok(Value::from_float(value_noise_2d(x, y)))
}

fn install_random(env: &mut Env) {
    let mut random = HashMap::new();
    random.insert(
        "int".to_string(),
        Value::from_builtin("random.int", &["range"], random_int),
    );
    random.insert(
        "float".to_string(),
        Value::from_builtin("random.float", &[], random_float),
    );
    random.insert(
        "choice".to_string(),
        Value::from_builtin("random.choice", &["list"], random_choice),
    );
    random.insert(
        "seed".to_string(),
        Value::from_builtin("random.seed", &["seed"], random_seed),
    );
    env.set(
        "random".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: random,
            kind: "module",
        }))),
    );
}

fn random_int(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "random.int")?;
    let (start, end, exclusive) = if args[0].is_range() {
        args[0].as_range()
    } else {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("random.int expected a range, got {}", args[0].type_name()),
            help: Some("e.g. `random.int(1..6)` rolls a six-sided die".to_string()),
        });
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
    Ok(Value::from_int(start + (n % span) as i64))
}

fn random_float(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "random.float")?;
    // 53 bits of randomness mapped to [0.0, 1.0).
    let n = env.next_random_u64() >> 11;
    let f = n as f64 * (1.0 / ((1u64 << 53) as f64));
    Ok(Value::from_float(f))
}

fn random_choice(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "random.choice")?;
    {
        let __t = &args[0];
        if __t.is_list() {
            let rc = __t.as_list();
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
            Ok(v[idx])
        } else {
            let other = *__t;
            Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("random.choice expected a list, got {}", other.type_name()),
                help: None,
            })
        }
    }
}

fn random_seed(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "random.seed")?;
    {
        let __t = &args[0];
        if __t.is_int_or_boxed_int() {
            let n = __t.as_int();
            env.seed_rng(n as u64);
            Ok(Value::NIL)
        } else {
            let other = *__t;
            Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("random.seed expected an int, got {}", other.type_name()),
                help: None,
            })
        }
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
            Value::from_tuple(Rc::new(vec![
                Value::from_float(*r),
                Value::from_float(*g),
                Value::from_float(*b),
                Value::from_float(*a),
            ])),
        );
    }
    env.set(
        "color".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
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
        Value::from_tuple(Rc::new(vec![
            Value::from_float(640.0),
            Value::from_float(480.0),
        ])),
    );
    fields.insert(
        "center".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(320.0),
            Value::from_float(240.0),
        ])),
    );
    env.set(
        "screen".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "module",
        }))),
    );
}

fn install_tilemap(env: &mut Env) {
    // v0.2 session 6: tilemap surface as stdlib builtins. The
    // `tilemap Name:` block syntax from Example 9 needs lexer +
    // parser hooks; that lands in a follow-on session. The
    // builtin form is enough to ship rendering + collision
    // queries today.
    env.set(
        "tilemap".to_string(),
        Value::from_builtin("tilemap", &["layout", "tile_size", "tiles"], tilemap_build),
    );
    env.set(
        "tilemap_render".to_string(),
        Value::from_builtin("tilemap_render", &["map", "at"], tilemap_render),
    );
    env.set(
        "tilemap_at".to_string(),
        Value::from_builtin("tilemap_at", &["map", "x", "y"], tilemap_at),
    );
    env.set(
        "tilemap_solid_at".to_string(),
        Value::from_builtin("tilemap_solid_at", &["map", "x", "y"], tilemap_solid_at),
    );
}

/// `tilemap(layout, tile_size, tiles)` — build a tilemap value
/// from a multi-line layout string + tile spec list. Returns an
/// `Object { kind: "tilemap" }` with fields:
///   - `layout` (string): the original layout for inspection.
///   - `tile_size` (int): pixels per tile, square.
///   - `width`, `height` (int): grid dimensions in tiles.
///   - `cells` (List of List of String): per-row, per-column
///     tile name. Empty string for chars not in the spec.
///   - `tiles` (Object): name → spec Object{name, traits: List<String>}.
///
/// `tiles` argument is a list of tuples `(char, name, traits)`
/// where `traits` is a list of trait-name strings. Example:
///
/// ```twe
/// let map = tilemap(
///     layout: "...\n#.#",
///     tile_size: 16,
///     tiles: [
///         (".", "floor", ["walkable"]),
///         ("#", "wall", ["solid"]),
///     ]
/// )
/// ```
///
/// v0.2 session 6.
fn tilemap_build(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "tilemap")?;
    let layout = {
        let __t = &args[0];
        if __t.is_str() {
            __t.as_string()
        } else {
            let other = *__t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("tilemap.layout expects a string, got {}", other.type_name()),
                help: Some("use a triple-quoted multi-line string for the grid".to_string()),
            });
        }
    };
    let tile_size = number(&args[1], "tilemap.tile_size")? as i64;
    if tile_size <= 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("tilemap.tile_size must be positive, got {tile_size}"),
            help: None,
        });
    }

    // Parse the `tiles` list into a char → spec map. Each entry
    // is a tuple `(char_str, name_str, traits_list)`.
    let tiles_arg = {
        let __t = &args[2];
        if __t.is_list() {
            let rc = __t.as_list();
            rc.clone()
        } else {
            let other = *__t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "tilemap.tiles expects a list of (char, name, traits) tuples, got {}",
                    other.type_name()
                ),
                help: None,
            });
        }
    };
    let mut by_char: HashMap<char, (String, Vec<String>)> = HashMap::new();
    let mut tile_specs_field: HashMap<String, Value> = HashMap::new();
    for (i, entry) in tiles_arg.borrow().iter().enumerate() {
        let elems = if entry.is_tuple() {
            let elems = entry.as_tuple();
            elems.clone()
        } else {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("tilemap.tiles[{i}] must be a tuple of (char, name, traits)"),
                help: Some("e.g. `(\".\", \"floor\", [\"walkable\"])`".to_string()),
            });
        };
        if elems.len() != 3 {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "tilemap.tiles[{i}] tuple needs exactly 3 fields (char, name, traits), got {}",
                    elems.len()
                ),
                help: None,
            });
        }
        let ch_str = {
            let __t = &elems[0];
            if __t.is_str() && {
                let s = __t.as_string();
                s.chars().count() == 1
            } {
                let s = __t.as_string();
                s.chars().next().unwrap()
            } else if __t.is_str() {
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!("tilemap.tiles[{i}].char must be a single character"),
                    help: None,
                });
            } else {
                let other = *__t;
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!(
                        "tilemap.tiles[{i}].char must be a string, got {}",
                        other.type_name()
                    ),
                    help: None,
                });
            }
        };
        let name: String = {
            let __t = &elems[1];
            if __t.is_str() {
                __t.as_string()
            } else {
                let other = *__t;
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!(
                        "tilemap.tiles[{i}].name must be a string, got {}",
                        other.type_name()
                    ),
                    help: None,
                });
            }
        };
        let traits: Vec<String> = {
            let __t = &elems[2];
            if __t.is_list() {
                let rc = __t.as_list();
                let v = rc.borrow();
                let mut out: Vec<String> = Vec::with_capacity(v.len());
                for (j, t) in v.iter().enumerate() {
                    if t.is_str() {
                        let s = t.as_string();
                        out.push(s)
                    } else {
                        let other = *t;
                        return Err(RuntimeError {
                            line: 0,
                            col: 0,
                            message: format!(
                                "tilemap.tiles[{i}].traits[{j}] must be a string, got {}",
                                other.type_name()
                            ),
                            help: None,
                        });
                    }
                }
                out
            } else if __t.is_nil() {
                Vec::new()
            } else {
                let other = *__t;
                return Err(RuntimeError {
                    line: 0,
                    col: 0,
                    message: format!(
                        "tilemap.tiles[{i}].traits must be a list of strings or nil, got {}",
                        other.type_name()
                    ),
                    help: None,
                });
            }
        };
        by_char.insert(ch_str, (name.clone(), traits.clone()));

        // Also expose the spec as a Twe-readable Object on the
        // tilemap's `tiles` field, keyed by name.
        let mut spec_fields = HashMap::new();
        spec_fields.insert("name".to_string(), Value::from_string(name.clone()));
        let trait_values: Vec<Value> = traits
            .iter()
            .map(|t| Value::from_string(t.clone()))
            .collect();
        spec_fields.insert(
            "traits".to_string(),
            Value::from_list(Rc::new(RefCell::new(trait_values))),
        );
        tile_specs_field.insert(
            name,
            Value::from_object(Rc::new(RefCell::new(Object {
                fields: spec_fields,
                kind: "tile_spec",
            }))),
        );
    }

    // Walk the layout into a 2D grid. Lines are split on '\n';
    // leading + trailing whitespace per line is stripped so that
    // triple-quoted layouts can use indentation freely.
    let raw_lines: Vec<&str> = layout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let height = raw_lines.len();
    let width = raw_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    let mut cells: Vec<Value> = Vec::with_capacity(height);
    for line in &raw_lines {
        let mut row: Vec<Value> = Vec::with_capacity(width);
        let mut chars: Vec<char> = line.chars().collect();
        // Pad short rows so width is uniform.
        while chars.len() < width {
            chars.push(' ');
        }
        for ch in chars {
            let name = by_char.get(&ch).map(|(n, _)| n.clone()).unwrap_or_default();
            row.push(Value::from_string(name));
        }
        cells.push(Value::from_list(Rc::new(RefCell::new(row))));
    }

    let mut fields = HashMap::new();
    fields.insert("layout".to_string(), Value::from_string(layout));
    fields.insert("tile_size".to_string(), Value::from_int(tile_size));
    fields.insert("width".to_string(), Value::from_int(width as i64));
    fields.insert("height".to_string(), Value::from_int(height as i64));
    fields.insert(
        "cells".to_string(),
        Value::from_list(Rc::new(RefCell::new(cells))),
    );
    fields.insert(
        "tiles".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: tile_specs_field,
            kind: "tile_specs",
        }))),
    );
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "tilemap",
    }))))
}

/// `tilemap_render(map, at)` — draw the tilemap as colored
/// per-tile rects keyed by trait. v0.2 session 6: a rectangle-
/// based renderer is the v0.2 minimum; sprite-based renderer
/// (with per-tile texture handles) rides Phase 9's atlas
/// + `sprite(handle, frame:)` work.
fn tilemap_render(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "tilemap_render")?;
    arity(args, 2, "tilemap_render")?;
    let map = expect_tilemap(&args[0], "tilemap_render.map")?;
    let (origin_x, origin_y) = {
        let __t = &args[1];
        if __t.is_tuple() && {
            let elems = __t.as_tuple();
            elems.len() == 2
        } {
            let elems = __t.as_tuple();
            let x = number(&elems[0], "tilemap_render.at.x")? as f32;
            let y = number(&elems[1], "tilemap_render.at.y")? as f32;
            (x, y)
        } else {
            let other = *__t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "tilemap_render.at expects a 2-tuple (x, y), got {}",
                    other.type_name()
                ),
                help: None,
            });
        }
    };
    let m = map.borrow();
    let tile_size = {
        let __opt = m.get_field("tile_size");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_int_or_boxed_int() {
                let n = __t.as_int();
                n as f32
            } else {
                return Err(tilemap_internal_error("tile_size"));
            }
        } else {
            return Err(tilemap_internal_error("tile_size"));
        }
    };
    let cells_value = m.get_field("cells");
    let tiles_value = m.get_field("tiles");
    drop(m);

    let cells_rc = if let Some(__t) = (cells_value).as_ref() {
        if __t.is_list() {
            __t.as_list()
        } else {
            return Err(tilemap_internal_error("cells"));
        }
    } else {
        return Err(tilemap_internal_error("cells"));
    };
    let tile_specs_rc = if let Some(__t) = (tiles_value).as_ref() {
        if __t.is_object() {
            __t.as_object()
        } else {
            return Err(tilemap_internal_error("tiles"));
        }
    } else {
        return Err(tilemap_internal_error("tiles"));
    };

    let cells = cells_rc.borrow();
    let tile_specs = tile_specs_rc.borrow();
    for (row_idx, row_value) in cells.iter().enumerate() {
        let row_rc = if row_value.is_list() {
            row_value.as_list()
        } else {
            continue;
        };
        let row = row_rc.borrow();
        for (col_idx, cell) in row.iter().enumerate() {
            let name_string: String = if cell.is_str() {
                cell.as_string()
            } else {
                continue;
            };
            let name = name_string.as_str();
            if name.is_empty() {
                continue;
            }
            let color = trait_color(&tile_specs.fields, name);
            let tx = origin_x + col_idx as f32 * tile_size;
            let ty = origin_y + row_idx as f32 * tile_size;
            push_rect(env, tx, ty, tile_size, color);
        }
    }
    Ok(Value::NIL)
}

/// Fixed palette per trait. Keeps v0.2 dependency-free; Phase 9
/// will let each tile carry an explicit color or sprite handle.
fn trait_color(
    tile_specs: &HashMap<String, crate::tagged_value::TaggedValue>,
    tile_name: &str,
) -> [f32; 4] {
    let traits = tile_specs
        .get(tile_name)
        .and_then(|v| {
            let __t = *v;
            if __t.is_object() {
                let rc = __t.as_object();
                Some(rc)
            } else {
                None
            }
        })
        .and_then(|rc| {
            let __opt = rc.borrow().get_field("traits");
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_list() {
                    let list_rc = __t.as_list();
                    Some(list_rc.clone())
                } else {
                    None
                }
            } else {
                None
            }
        });
    let traits_vec: Vec<String> = match traits {
        Some(rc) => rc
            .borrow()
            .iter()
            .filter_map(|v| {
                if v.is_str() {
                    let s = v.as_string();
                    Some(s)
                } else {
                    None
                }
            })
            .collect(),
        None => Vec::new(),
    };
    if traits_vec.iter().any(|t| t == "solid") {
        return [0.45, 0.45, 0.50, 1.0];
    }
    if traits_vec.iter().any(|t| t == "trigger") {
        return [0.95, 0.85, 0.30, 1.0];
    }
    if traits_vec.iter().any(|t| t == "slow") {
        return [0.20, 0.40, 0.75, 1.0];
    }
    if traits_vec.iter().any(|t| t == "walkable") {
        return [0.18, 0.18, 0.20, 1.0];
    }
    [0.85, 0.20, 0.85, 1.0] // missing — magenta "missing-tile" indicator
}

/// Push a single tile rect into the active draw queue. Mirrors
/// the body of `draw_rect` to avoid going through the kwargs-binding
/// path for an inner-loop call.
fn push_rect(env: &mut Env, x: f32, y: f32, size: f32, color: [f32; 4]) {
    // The 2D draw queue lives on the active scene-or-equivalent;
    // for v0.2 minimum we go through the same `rect` builtin to
    // reuse its existing pipe. Build the args inline.
    let args = vec![
        Value::from_tuple(Rc::new(vec![
            Value::from_float(x as f64),
            Value::from_float(y as f64),
        ])),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(size as f64),
            Value::from_float(size as f64),
        ])),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(color[0] as f64),
            Value::from_float(color[1] as f64),
            Value::from_float(color[2] as f64),
            Value::from_float(color[3] as f64),
        ])),
    ];
    let _ = draw_rect(env, &args);
}

/// `tilemap_at(map, x, y)` — return the tile name at world
/// pixel coords `(x, y)`. Out-of-bounds reads return the empty
/// string. v0.2 session 6.
fn tilemap_at(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "tilemap_at")?;
    let map = expect_tilemap(&args[0], "tilemap_at.map")?;
    let x = number(&args[1], "tilemap_at.x")? as f32;
    let y = number(&args[2], "tilemap_at.y")? as f32;
    let name = tilemap_name_at(&map, x, y);
    Ok(Value::from_string(name))
}

/// `tilemap_solid_at(map, x, y)` — true if the tile at pixel
/// coords `(x, y)` carries the `solid` trait. Convenience for
/// the most common collision query. v0.2 session 6.
fn tilemap_solid_at(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "tilemap_solid_at")?;
    let map = expect_tilemap(&args[0], "tilemap_solid_at.map")?;
    let x = number(&args[1], "tilemap_solid_at.x")? as f32;
    let y = number(&args[2], "tilemap_solid_at.y")? as f32;
    let name = tilemap_name_at(&map, x, y);
    if name.is_empty() {
        return Ok(Value::FALSE);
    }
    let tile_specs = {
        let __opt = map.borrow().get_field("tiles");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_object() {
                __t.as_object()
            } else {
                return Ok(Value::FALSE);
            }
        } else {
            return Ok(Value::FALSE);
        }
    };
    let specs = tile_specs.borrow();
    let solid = specs
        .get_field(&name)
        .and_then(|v| {
            if v.is_object() {
                let rc = v.as_object();
                Some(rc)
            } else {
                None
            }
        })
        .and_then(|rc| {
            let __opt = rc.borrow().get_field("traits");
            if let Some(__t) = (__opt).as_ref() {
                if __t.is_list() {
                    let list_rc = __t.as_list();
                    Some(list_rc.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .map(|rc| {
            rc.borrow().iter().any(|v| {
                if v.is_str() {
                    let s = v.as_string();
                    s == "solid"
                } else {
                    false
                }
            })
        })
        .unwrap_or(false);
    Ok(Value::from_bool(solid))
}

fn tilemap_name_at(map: &Rc<RefCell<Object>>, x: f32, y: f32) -> String {
    let m = map.borrow();
    let tile_size = {
        let __opt = m.get_field("tile_size");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_int_or_boxed_int() {
                let n = __t.as_int();
                n as f32
            } else {
                return String::new();
            }
        } else {
            return String::new();
        }
    };
    if tile_size <= 0.0 {
        return String::new();
    }
    let col = (x / tile_size).floor() as i64;
    let row = (y / tile_size).floor() as i64;
    if col < 0 || row < 0 {
        return String::new();
    }
    let cells = {
        let __opt = m.get_field("cells");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_list() {
                let rc = __t.as_list();
                rc.clone()
            } else {
                return String::new();
            }
        } else {
            return String::new();
        }
    };
    let cells = cells.borrow();
    let row_idx = row as usize;
    if row_idx >= cells.len() {
        return String::new();
    }
    let row_rc = {
        let __t = &cells[row_idx];
        if __t.is_list() {
            let rc = __t.as_list();
            rc.clone()
        } else {
            return String::new();
        }
    };
    let row = row_rc.borrow();
    let col_idx = col as usize;
    if col_idx >= row.len() {
        return String::new();
    }
    {
        let __t = &row[col_idx];
        if __t.is_str() {
            __t.as_string()
        } else {
            String::new()
        }
    }
}

fn expect_tilemap(v: &Value, what: &str) -> Result<Rc<RefCell<Object>>, RuntimeError> {
    if v.is_object() {
        let rc = v.as_object();
        let is_tilemap = rc.borrow().kind == "tilemap";
        if is_tilemap {
            return Ok(rc);
        }
    }
    let other = *v;
    Err(RuntimeError {
        line: 0,
        col: 0,
        message: format!(
            "{what} expects a tilemap value from `tilemap(...)`, got {}",
            other.type_name()
        ),
        help: None,
    })
}

fn tilemap_internal_error(field: &str) -> RuntimeError {
    RuntimeError {
        line: 0,
        col: 0,
        message: format!("internal: tilemap missing or malformed `{field}` field"),
        help: Some(
            "tilemap values are constructed by `tilemap(...)` — don't hand-build them".to_string(),
        ),
    }
}

fn install_draw(env: &mut Env) {
    env.set(
        "rect".to_string(),
        Value::from_builtin("rect", &["at", "size", "color"], draw_rect),
    );
    env.set(
        "circle".to_string(),
        Value::from_builtin("circle", &["at", "radius", "color"], draw_circle),
    );
    env.set(
        "line".to_string(),
        Value::from_builtin("line", &["from", "to", "width", "color"], draw_line),
    );
    env.set(
        "text".to_string(),
        Value::from_builtin("text", &["content", "at", "size", "color"], draw_text),
    );
    // sprite() is variadic-style — 2 or 3 positional args, no kwargs in v0.1.
    // Add named-param support when the optional `size` slot has a clean
    // representation in bind_kwargs.
    env.set(
        "sprite".to_string(),
        Value::from_builtin("sprite", &[], draw_sprite),
    );
    // Phase 9 session 3: atlas-frame draw calls. Two builtins instead
    // of optional kwargs on `sprite()` because Twe's calling convention
    // requires every kwarg to be supplied (audio v2 took the same
    // shape per the comment in install_sound).
    //   - sprite_frame(atlas, at, frame): draws one cell at native size
    //   - sprite_frame_at(atlas, at, size, frame): cell scaled to size
    env.set(
        "sprite_frame".to_string(),
        Value::from_builtin(
            "sprite_frame",
            &["handle", "at", "frame"],
            draw_sprite_frame,
        ),
    );
    env.set(
        "sprite_frame_at".to_string(),
        Value::from_builtin(
            "sprite_frame_at",
            &["handle", "at", "size", "frame"],
            draw_sprite_frame_at,
        ),
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
    let path = {
        let __t = &args[0];
        if __t.is_object() {
            let rc = __t.as_object();
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
            {
                let __opt = o.get_field("path");
                if let Some(__t) = (__opt).as_ref() {
                    if __t.is_str() {
                        let s = __t.as_string();
                        s.clone()
                    } else {
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
                } else {
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
        } else {
            let other = *__t;
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
            None => {
                macroquad::texture::draw_texture(tex, x as f32, y as f32, macroquad::color::WHITE)
            }
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
    Ok(Value::NIL)
}

/// Pull `(path, cols, rows)` off an atlas handle. Errors when the
/// argument isn't an `atlas`-kind object built by `load_atlas`.
fn atlas_handle(v: &Value, callee: &str) -> Result<(String, i64, i64), RuntimeError> {
    if !v.is_object() {
        let other = *v;
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{callee} expects an atlas handle from `load_atlas(...)`, got {}",
                other.type_name()
            ),
            help: None,
        });
    }
    let rc = v.as_object();
    let o = rc.borrow();
    if o.kind != "atlas" {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{callee} expects an atlas handle from `load_atlas(...)`, got a `{}` handle",
                o.kind
            ),
            help: Some("plain sprite handles draw via `sprite(handle, at, ...)`".to_string()),
        });
    }
    let path = match o.get_field("path") {
        Some(v) if v.is_str() => v.as_string().clone(),
        _ => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{callee}: atlas handle is missing a `path` field"),
                help: None,
            });
        }
    };
    let (cols, rows) = match o.get_field("grid") {
        Some(v) => xy_of(&v, &format!("{callee}.grid"))?,
        None => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{callee}: atlas handle is missing a `grid` field"),
                help: None,
            });
        }
    };
    Ok((path, cols as i64, rows as i64))
}

/// Compute the source rectangle in the atlas texture for a frame
/// index. Frame 0 is the top-left cell; frames advance left-to-right,
/// then top-to-bottom (row-major).
fn atlas_source_rect(
    tex: &macroquad::texture::Texture2D,
    cols: i64,
    rows: i64,
    frame: i64,
    callee: &str,
) -> Result<macroquad::math::Rect, RuntimeError> {
    let total = cols * rows;
    if frame < 0 || frame >= total {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{callee}: frame {frame} out of range for a {cols}x{rows} atlas (0..{total})"
            ),
            help: None,
        });
    }
    let col = (frame % cols) as f32;
    let row = (frame / cols) as f32;
    let cell_w = tex.width() / cols as f32;
    let cell_h = tex.height() / rows as f32;
    Ok(macroquad::math::Rect {
        x: col * cell_w,
        y: row * cell_h,
        w: cell_w,
        h: cell_h,
    })
}

/// Decode + cache a texture by path, then call `draw` with the live
/// `Texture2D`. Mirrors the pattern in `draw_sprite` so atlases share
/// the same cache and hot-reload behavior.
fn with_texture<F>(path: &str, callee: &str, draw: F) -> Result<(), RuntimeError>
where
    F: FnOnce(&macroquad::texture::Texture2D) -> Result<(), RuntimeError>,
{
    SPRITE_CACHE.with(|cache| -> Result<(), RuntimeError> {
        let mut c = cache.borrow_mut();
        if !c.contains_key(path) {
            let bytes = std::fs::read(path).map_err(|e| RuntimeError {
                line: 0,
                col: 0,
                message: format!("{callee}: cannot read '{path}': {e}"),
                help: None,
            })?;
            let tex = macroquad::texture::Texture2D::from_file_with_format(&bytes, None);
            c.insert(path.to_string(), tex);
        }
        let tex = &c[path];
        draw(tex)
    })
}

/// `sprite_frame(atlas, at, frame)` — draw cell `frame` of `atlas`
/// at world position `at` at the cell's native pixel size.
fn draw_sprite_frame(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "sprite_frame")?;
    arity(args, 3, "sprite_frame")?;
    let (path, cols, rows) = atlas_handle(&args[0], "sprite_frame")?;
    let (x, y) = xy_of(&args[1], "sprite_frame.at")?;
    let frame = as_i64(&args[2], "sprite_frame.frame")?;
    with_texture(&path, "sprite_frame", |tex| {
        let src = atlas_source_rect(tex, cols, rows, frame, "sprite_frame")?;
        macroquad::texture::draw_texture_ex(
            tex,
            x as f32,
            y as f32,
            macroquad::color::WHITE,
            macroquad::texture::DrawTextureParams {
                source: Some(src),
                ..Default::default()
            },
        );
        Ok(())
    })?;
    Ok(Value::NIL)
}

/// `sprite_frame_at(atlas, at, size, frame)` — draw cell `frame` of
/// `atlas` at world position `at`, scaled to `size`.
fn draw_sprite_frame_at(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "sprite_frame_at")?;
    arity(args, 4, "sprite_frame_at")?;
    let (path, cols, rows) = atlas_handle(&args[0], "sprite_frame_at")?;
    let (x, y) = xy_of(&args[1], "sprite_frame_at.at")?;
    let (w, h) = xy_of(&args[2], "sprite_frame_at.size")?;
    let frame = as_i64(&args[3], "sprite_frame_at.frame")?;
    with_texture(&path, "sprite_frame_at", |tex| {
        let src = atlas_source_rect(tex, cols, rows, frame, "sprite_frame_at")?;
        macroquad::texture::draw_texture_ex(
            tex,
            x as f32,
            y as f32,
            macroquad::color::WHITE,
            macroquad::texture::DrawTextureParams {
                dest_size: Some(macroquad::math::vec2(w as f32, h as f32)),
                source: Some(src),
                ..Default::default()
            },
        );
        Ok(())
    })?;
    Ok(Value::NIL)
}

/// Coerce a Twe Value to i64 for index/frame arguments. Floats with
/// fractional components error so a typo `frame: 3.5` doesn't silently
/// truncate.
fn as_i64(v: &Value, op: &str) -> Result<i64, RuntimeError> {
    if v.is_int_or_boxed_int() {
        return Ok(v.as_int());
    }
    if v.is_float() {
        let f = v.as_float();
        if f.fract() != 0.0 {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{op} expected an integer, got {f}"),
                help: None,
            });
        }
        return Ok(f as i64);
    }
    let other = *v;
    Err(RuntimeError {
        line: 0,
        col: 0,
        message: format!("{op} expected an integer, got {}", other.type_name()),
        help: None,
    })
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
    if v.is_tuple() && {
        let elems = v.as_tuple();
        elems.len() >= 2
    } {
        let elems = v.as_tuple();
        Ok((number(&elems[0], what)?, number(&elems[1], what)?))
    } else {
        let other = *v;
        Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects a tuple of 2 numbers, got {}",
                other.type_name()
            ),
            help: None,
        })
    }
}

fn color_of(v: &Value, what: &str) -> Result<macroquad::color::Color, RuntimeError> {
    if v.is_tuple() && {
        let elems = v.as_tuple();
        elems.len() >= 3
    } {
        let elems = v.as_tuple();
        let r = number(&elems[0], what)? as f32;
        let g = number(&elems[1], what)? as f32;
        let b = number(&elems[2], what)? as f32;
        let a = if elems.len() >= 4 {
            number(&elems[3], what)? as f32
        } else {
            1.0
        };
        Ok(macroquad::color::Color::new(r, g, b, a))
    } else {
        let other = *v;
        Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects an (r, g, b[, a]) tuple, got {}",
                other.type_name()
            ),
            help: Some(
                "use color.red, color.green, … or build with `(0.5, 0.0, 0.0, 1.0)`".to_string(),
            ),
        })
    }
}

fn number(v: &Value, what: &str) -> Result<f64, RuntimeError> {
    if v.is_int_or_boxed_int() {
        let n = v.as_int();
        Ok(n as f64)
    } else if v.is_float() {
        let f = v.as_float();
        Ok(f)
    } else if v.is_quantity() {
        let (value, _) = v.as_quantity();
        Ok(value)
    } else {
        let other = *v;
        Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what} expects a number, got {}", other.type_name()),
            help: None,
        })
    }
}

fn draw_rect(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "rect")?;
    arity(args, 3, "rect")?;
    let (x, y) = xy_of(&args[0], "rect.at")?;
    let (w, h) = xy_of(&args[1], "rect.size")?;
    let color = color_of(&args[2], "rect.color")?;
    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, color);
    Ok(Value::NIL)
}

fn draw_circle(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "circle")?;
    arity(args, 3, "circle")?;
    let (x, y) = xy_of(&args[0], "circle.at")?;
    let radius = number(&args[1], "circle.radius")? as f32;
    let color = color_of(&args[2], "circle.color")?;
    macroquad::shapes::draw_circle(x as f32, y as f32, radius, color);
    Ok(Value::NIL)
}

fn draw_line(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "line")?;
    arity(args, 4, "line")?;
    let (x1, y1) = xy_of(&args[0], "line.from")?;
    let (x2, y2) = xy_of(&args[1], "line.to")?;
    let thickness = number(&args[2], "line.width")? as f32;
    let color = color_of(&args[3], "line.color")?;
    macroquad::shapes::draw_line(x1 as f32, y1 as f32, x2 as f32, y2 as f32, thickness, color);
    Ok(Value::NIL)
}

fn install_entities(env: &mut Env) {
    let mut entities = HashMap::new();
    entities.insert(
        "of".to_string(),
        Value::from_builtin("entities.of", &["class"], entities_of),
    );
    entities.insert(
        "count".to_string(),
        Value::from_builtin("entities.count", &["class"], entities_count),
    );
    env.set(
        "entities".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: entities,
            kind: "module",
        }))),
    );
}

fn entities_of(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "entities.of")?;
    let class = {
        let __t = &args[0];
        if __t.is_class() {
            let c = __t.as_class();
            c.clone()
        } else {
            let other = *__t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "entities.of expects a class (e.g. `entities.of(Monster)`), got {}",
                    other.type_name()
                ),
                help: Some("pass the entity class itself, not an instance".to_string()),
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
            result.push(Value::from_instance(inst.clone()));
        }
    }
    Ok(Value::from_list(Rc::new(RefCell::new(result))))
}

fn entities_count(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "entities.count")?;
    let class = {
        let __t = &args[0];
        if __t.is_class() {
            let c = __t.as_class();
            c.clone()
        } else {
            let other = *__t;
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
    Ok(Value::from_int(n))
}

fn draw_text(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "text")?;
    arity(args, 4, "text")?;
    let content = {
        let __t = &args[0];
        if __t.is_str() {
            let s = __t.as_string();
            s.clone()
        } else {
            let other = *__t;
            other.display()
        }
    };
    let (x, y) = xy_of(&args[1], "text.at")?;
    let size = number(&args[2], "text.size")? as f32;
    let color = color_of(&args[3], "text.color")?;
    macroquad::text::draw_text(&content, x as f32, y as f32, size, color);
    Ok(Value::NIL)
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
        Value::from_builtin("vec3", &["x", "y", "z"], vec3_impl),
    );

    // `cube(at: vec3, color: (r, g, b, a), size: float)` — queues a
    // unit-cube draw at `at`, scaled by `size`, tinted by `color`.
    // Only valid inside `on render():` (require_render guards).
    env.set(
        "cube".to_string(),
        Value::from_builtin("cube", &["at", "color", "size"], cube_impl),
    );

    // `sphere(at: vec3, color: (r, g, b, a), size: float)` — same
    // shape as `cube`, different mesh. Phase 6 session 7 (the
    // first v0.2 carry-over to actually ship in v0.1).
    env.set(
        "sphere".to_string(),
        Value::from_builtin("sphere", &["at", "color", "size"], sphere_impl),
    );

    // `mesh(path: string, at: vec3, color: (r, g, b, a), size: float)`
    // — load a glTF 2.0 binary (`.glb`) and queue an instanced draw.
    // Path is resolved relative to the working directory. The first
    // primitive of the first mesh is used; multi-primitive scenes
    // and node transforms are a follow-on. v0.2 session 1.
    env.set(
        "mesh".to_string(),
        Value::from_builtin("mesh", &["path", "at", "color", "size"], mesh_impl),
    );

    // `camera` ambient — both 2D (Phase 9 session 2) and 3D fields
    // live on one object. 3D: `eye` / `target` / `up` (Tuple fields
    // the script writes via `camera.eye = vec3(...)`; the `play3d`
    // render loop reads them each frame to build the view matrix).
    // 2D: `pos` / `zoom` (Tuple + float; the macroquad `play` loop
    // reads them each frame to build a `Camera2D`) plus three
    // builtin methods stored as fields:
    //   - `camera.follow(target_xy, lerp)` — smooth-track a 2D point
    //   - `camera.shake(amplitude, duration)` — screen shake
    //   - `camera.reset()` — snap pos/zoom to defaults + clear shake
    let mut fields = HashMap::new();
    fields.insert(
        "eye".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(0.0),
            Value::from_float(1.5),
            Value::from_float(3.0),
        ])),
    );
    fields.insert(
        "target".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(0.0),
            Value::from_float(0.0),
            Value::from_float(0.0),
        ])),
    );
    fields.insert(
        "up".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(0.0),
            Value::from_float(1.0),
            Value::from_float(0.0),
        ])),
    );
    fields.insert(
        "pos".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(0.0),
            Value::from_float(0.0),
        ])),
    );
    fields.insert("zoom".to_string(), Value::from_float(1.0));
    fields.insert(
        "follow".to_string(),
        Value::from_builtin("camera.follow", &["target", "lerp"], camera_follow_impl),
    );
    fields.insert(
        "shake".to_string(),
        Value::from_builtin(
            "camera.shake",
            &["amplitude", "duration"],
            camera_shake_impl,
        ),
    );
    fields.insert(
        "reset".to_string(),
        Value::from_builtin("camera.reset", &[], camera_reset_impl),
    );
    env.set(
        "camera".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
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
    Ok(Value::from_tuple(Rc::new(vec![
        Value::from_float(x),
        Value::from_float(y),
        Value::from_float(z),
    ])))
}

// `camera.follow(target_xy, lerp)` — exponential smoothing toward a
// 2D point. `target_xy` is a 2-tuple (so `camera.follow(player.pos,
// 0.1)` is the canonical call); `lerp` is the per-frame blend in
// [0, 1] where 0 means no movement and 1 means snap. Mutates the
// `camera.pos` field in place. Phase 9 session 2.
fn camera_follow_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "camera.follow")?;
    let (tx, ty) = xy_of(&args[0], "camera.follow.target")?;
    let lerp = as_f64(&args[1], "camera.follow.lerp")?;
    let cam = env.get("camera").ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: "camera.follow: `camera` ambient is missing".to_string(),
        help: None,
    })?;
    if !cam.is_object() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "camera.follow: `camera` is not an object, got {}",
                cam.type_name()
            ),
            help: None,
        });
    }
    let rc = cam.as_object();
    let mut o = rc.borrow_mut();
    let (cx, cy) = match o.get_field("pos") {
        Some(v) if v.is_tuple() => {
            let elems = v.as_tuple();
            if elems.len() >= 2 {
                (
                    number(&elems[0], "camera.pos.x")?,
                    number(&elems[1], "camera.pos.y")?,
                )
            } else {
                (0.0, 0.0)
            }
        }
        _ => (0.0, 0.0),
    };
    let nx = cx + (tx - cx) * lerp;
    let ny = cy + (ty - cy) * lerp;
    o.insert_field(
        "pos".to_string(),
        Value::from_tuple(Rc::new(vec![
            Value::from_float(nx),
            Value::from_float(ny),
        ])),
    );
    Ok(Value::NIL)
}

// `camera.shake(amplitude, duration)` — start (or extend) a screen
// shake. Amplitude is in pixels; duration is seconds (or a Duration
// quantity — `number()` accepts both). Stacking semantics: a stronger
// shake replaces a weaker one; a longer duration extends a shorter
// one. Both backends ignore shake; only `twec play` reads it.
fn camera_shake_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "camera.shake")?;
    let amp = as_f64(&args[0], "camera.shake.amplitude")?;
    let dur = number(&args[1], "camera.shake.duration")?;
    if amp < 0.0 || dur < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "camera.shake: amplitude and duration must be non-negative".to_string(),
            help: None,
        });
    }
    CAMERA_SHAKE.with(|c| {
        let mut s = c.borrow_mut();
        if amp > s.amplitude {
            s.amplitude = amp;
        }
        if dur > s.remaining {
            s.remaining = dur;
        }
    });
    Ok(Value::NIL)
}

// `camera.reset()` — snap pos to (0, 0), zoom to 1.0, kill any
// active shake. Useful in scene transitions.
fn camera_reset_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "camera.reset")?;
    CAMERA_SHAKE.with(|c| {
        let mut s = c.borrow_mut();
        s.amplitude = 0.0;
        s.remaining = 0.0;
    });
    if let Some(cam) = env.get("camera") {
        if cam.is_object() {
            let rc = cam.as_object();
            let mut o = rc.borrow_mut();
            o.insert_field(
                "pos".to_string(),
                Value::from_tuple(Rc::new(vec![
                    Value::from_float(0.0),
                    Value::from_float(0.0),
                ])),
            );
            o.insert_field("zoom".to_string(), Value::from_float(1.0));
        }
    }
    Ok(Value::NIL)
}

/// Inspect-only: how many seconds of shake remain. Tests use this to
/// verify decay; the play loop doesn't need it (it queries the offset
/// directly via `camera_shake_offset`).
pub fn camera_shake_remaining() -> f64 {
    CAMERA_SHAKE.with(|c| c.borrow().remaining)
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
    Ok(Value::NIL)
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
    Ok(Value::NIL)
}

fn mesh_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "mesh")?;
    arity(args, 4, "mesh")?;
    let path = {
        let __t = &args[0];
        if __t.is_str() {
            let s = __t.as_string();
            s.clone()
        } else {
            let other = *__t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("mesh.path expects a string, got {}", other.type_name()),
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
    Ok(Value::NIL)
}

/// Pull a 3-component float vector out of a Twe tuple. Used by the
/// 3D builtins. Mirrors `xy_of` but for the third axis.
fn xyz_of(v: &Value, what: &str) -> Result<[f32; 3], RuntimeError> {
    if v.is_tuple() && {
        let elems = v.as_tuple();
        elems.len() == 3
    } {
        let elems = v.as_tuple();
        Ok([
            number(&elems[0], what)? as f32,
            number(&elems[1], what)? as f32,
            number(&elems[2], what)? as f32,
        ])
    } else {
        let other = *v;
        Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects a 3-component tuple (vec3), got {}",
                other.type_name()
            ),
            help: Some("e.g. `vec3(0, 1, 0)` or `(0, 1, 0)`".to_string()),
        })
    }
}

/// Pull an RGBA float quartet out of a Twe tuple.
fn rgba_of(v: &Value, what: &str) -> Result<[f32; 4], RuntimeError> {
    if v.is_tuple() && {
        let elems = v.as_tuple();
        elems.len() == 4
    } {
        let elems = v.as_tuple();
        Ok([
            number(&elems[0], what)? as f32,
            number(&elems[1], what)? as f32,
            number(&elems[2], what)? as f32,
            number(&elems[3], what)? as f32,
        ])
    } else {
        let other = *v;
        Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects a 4-component color tuple, got {}",
                other.type_name()
            ),
            help: Some("use `color.red` etc. or build with `(r, g, b, a)` floats".to_string()),
        })
    }
}
