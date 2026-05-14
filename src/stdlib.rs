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
    // Phase 9 session 4: TTF font cache. Two-stage lazy load: the
    // raw font bytes are cached at `load_font` time (headless-safe,
    // path + TTF magic-bytes validation runs there), and macroquad's
    // `Font` parse happens on first `text_with_font` draw inside
    // `on render():` — same lazy-on-first-draw pattern as sprites.
    // Reason: macroquad's `load_ttf_font_from_bytes` asserts on
    // THREAD_ID, which is only initialised inside `Window::from_config`,
    // so eager parsing would crash `twec run` and the test harness.
    static FONT_BYTES_CACHE: RefCell<HashMap<String, Vec<u8>>>
        = RefCell::new(HashMap::new());
    static FONT_CACHE: RefCell<HashMap<String, macroquad::text::Font>>
        = RefCell::new(HashMap::new());
    // Phase 10 sessions 3 / 4 / 5: per-frame UI focus state shared
    // across the immediate-mode widget set. Sliders / dropdowns /
    // text-inputs are the stateful widgets; only one of each can
    // be "active" (dragging / open / focused) at a time, and the
    // identity is the widget's screen rect (top-left + size). Rect
    // coords come from script-side literals and are stable across
    // frames as long as the script doesn't mutate the layout, which
    // is the immediate-mode contract.
    static UI_STATE: RefCell<UiState> = RefCell::new(UiState {
        active_slider: None,
        open_dropdown: None,
        focused_text_input: None,
        focused_key_input: None,
        scroll_y: HashMap::new(),
    });
}

/// Widget rect identity. Comparing f64 bit-patterns is fine here
/// because the values come from script literals or arithmetic on
/// literals — same Twe expressions give the same bits — and a
/// drifted rect signals a layout that's no longer the same widget.
type RectId = (u64, u64, u64, u64);

fn rect_id(x: f64, y: f64, w: f64, h: f64) -> RectId {
    (x.to_bits(), y.to_bits(), w.to_bits(), h.to_bits())
}

#[derive(Clone)]
struct UiState {
    active_slider: Option<RectId>,
    open_dropdown: Option<RectId>,
    focused_text_input: Option<RectId>,
    /// Phase 10 session 11: focused key_input widget. Separate slot
    /// from text_input because both can coexist on the same screen
    /// (a name field and a "Bind right" field) — focus moves cleanly
    /// between them via the standard click-inside / click-outside
    /// transitions.
    focused_key_input: Option<RectId>,
    /// Phase 10 session 7: per-scroll-rect Y offset in content
    /// coordinates. The `scroll` builtin reads / writes this
    /// across frames so the scroll position survives between
    /// renders. Cleared on hot reload via `clear_asset_caches`.
    scroll_y: HashMap<RectId, f64>,
}

#[derive(Clone, Copy)]
struct CameraShake {
    amplitude: f64,
    remaining: f64,
}

/// Phase 9 session 5: gamepad button names exposed to Twe. Xbox-style
/// naming so scripts read naturally; the gilrs button enum mapping
/// lives in `play.rs::poll_gamepad`. `lt` / `rt` are the analog
/// triggers thresholded into booleans (gilrs's default 0.75 cutoff);
/// the analog values live in `gamepad_axis.lt` / `.rt`.
pub const GAMEPAD_BUTTON_NAMES: &[&str] = &[
    "a", "b", "x", "y", "lb", "rb", "lt", "rt", "start", "select", "dup", "ddown", "dleft",
    "dright",
];

/// Axis names for the analog sticks + triggers. Sticks are in
/// `[-1, 1]` per gilrs; triggers are `[0, 1]`. `ly` / `ry` follow
/// gilrs's "+y is up" convention — scripts that want screen-space
/// (+y down) should negate.
pub const GAMEPAD_AXIS_NAMES: &[&str] = &["lx", "ly", "rx", "ry", "lt", "rt"];

/// Drop every cached `Texture2D` and `Sound`. The play loop calls this
/// on hot reload so swapped asset paths pick up. Also resets camera
/// shake — a hot-reloaded script shouldn't inherit jitter from the
/// previous session.
pub fn clear_asset_caches() {
    SPRITE_CACHE.with(|c| c.borrow_mut().clear());
    SOUND_CACHE.with(|c| c.borrow_mut().clear());
    FONT_BYTES_CACHE.with(|c| c.borrow_mut().clear());
    FONT_CACHE.with(|c| c.borrow_mut().clear());
    CAMERA_SHAKE.with(|c| {
        let mut s = c.borrow_mut();
        s.amplitude = 0.0;
        s.remaining = 0.0;
    });
    UI_STATE.with(|c| {
        let mut s = c.borrow_mut();
        s.active_slider = None;
        s.open_dropdown = None;
        s.focused_text_input = None;
        s.focused_key_input = None;
        s.scroll_y.clear();
    });
    // Phase 29 session 5: hot-reload restarts the audio simulation
    // clock + drops any pending scheduled plays. Without this, a
    // reloaded script would inherit the previous run's `sound.now()`
    // and stale pending entries.
    reset_audio_schedule();
    // v1.0.1 session 1: drop every queued visual fx and any pending
    // hit-stop ticks. A hot-reloaded script that just lost its boss
    // shouldn't inherit the death-burst from the previous run.
    crate::fx::clear();
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
        if s.remaining > 0.0 {
            s.amplitude
        } else {
            0.0
        }
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
    // Phase 9 session 4: TTF / OTF font loading. `load_font(path)`
    // returns a font handle `{ path }`; `text_with_font(content,
    // at, size, color, font)` is the draw call that uses it (kept
    // separate from `text` for the same calling-convention reason
    // as sprite_frame vs sprite — required-kwargs only). Fonts
    // decode eagerly because they don't need a GL context.
    env.set(
        "load_font".to_string(),
        Value::from_builtin("load_font", &["path"], load_font_impl),
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

    // Phase 10 session 11: dynamic-name key lookup. The static
    // `key.<name>` / `key_press.<name>` accessors are still the
    // canonical surface; `key_held(name)` / `key_pressed(name)` are
    // for cases where the name isn't a literal — typically a settings
    // entry like `settings.get("keys.move_right")` driving rebindable
    // controls. Returns false for unknown / unmapped names so the
    // game keeps running when a binding hasn't been set.
    env.set(
        "key_held".to_string(),
        Value::from_builtin("key_held", &["name"], key_held_impl),
    );
    env.set(
        "key_pressed".to_string(),
        Value::from_builtin("key_pressed", &["name"], key_pressed_impl),
    );

    // Phase 20: lighting surface. light.* manages an 8-slot point
    // light array; sun.* tweaks the directional sun. The play3d
    // loop reads `lights_snapshot()` each frame and uploads.
    let mut light_fields = HashMap::new();
    light_fields.insert(
        "add".to_string(),
        Value::from_builtin("light.add", &["at", "color", "radius"], light_add_impl),
    );
    light_fields.insert(
        "remove".to_string(),
        Value::from_builtin("light.remove", &["handle"], light_remove_impl),
    );
    light_fields.insert(
        "ambient".to_string(),
        Value::from_builtin("light.ambient", &["color"], light_ambient_impl),
    );
    light_fields.insert(
        "set".to_string(),
        Value::from_builtin(
            "light.set",
            &["handle", "at", "color", "radius"],
            light_set_impl,
        ),
    );
    light_fields.insert(
        "set_radius".to_string(),
        Value::from_builtin(
            "light.set_radius",
            &["handle", "radius"],
            light_set_radius_impl,
        ),
    );
    light_fields.insert(
        "clear".to_string(),
        Value::from_builtin("light.clear", &[], light_clear_impl),
    );
    env.set(
        "light".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: light_fields,
            kind: "module",
        }))),
    );

    let mut sun_fields = HashMap::new();
    sun_fields.insert(
        "direction".to_string(),
        Value::from_builtin("sun.direction", &["v"], sun_direction_impl),
    );
    sun_fields.insert(
        "intensity".to_string(),
        Value::from_builtin("sun.intensity", &["i"], sun_intensity_impl),
    );
    // Phase 25: shadow controls. `sun.shadow(true)` enables PCF
    // shadow rendering from the sun direction; `sun.shadow_extent(r)`
    // sets the half-side of the orthographic shadow frustum
    // (default 30m). The render path writes a 2K shadow map each
    // frame when enabled.
    sun_fields.insert(
        "shadow".to_string(),
        Value::from_builtin("sun.shadow", &["enabled"], sun_shadow_impl),
    );
    sun_fields.insert(
        "shadow_extent".to_string(),
        Value::from_builtin("sun.shadow_extent", &["radius"], sun_shadow_extent_impl),
    );
    // Phase 26: post-processing namespace. `postfx.tonemap(true)`
    // enables ACES filmic tone mapping (HDR offscreen target +
    // fullscreen pass); `postfx.vignette(strength)` adds a soft
    // edge darkening; `postfx.frustum_cull(false)` disables
    // per-instance frustum culling (default on).
    let mut postfx_fields = HashMap::new();
    postfx_fields.insert(
        "tonemap".to_string(),
        Value::from_builtin("postfx.tonemap", &["enabled"], postfx_tonemap_impl),
    );
    postfx_fields.insert(
        "vignette".to_string(),
        Value::from_builtin("postfx.vignette", &["strength"], postfx_vignette_impl),
    );
    postfx_fields.insert(
        "vignette_color".to_string(),
        Value::from_builtin(
            "postfx.vignette_color",
            &["color"],
            postfx_vignette_color_impl,
        ),
    );
    postfx_fields.insert(
        "bloom".to_string(),
        Value::from_builtin("postfx.bloom", &["intensity"], postfx_bloom_impl),
    );
    postfx_fields.insert(
        "bloom_threshold".to_string(),
        Value::from_builtin(
            "postfx.bloom_threshold",
            &["threshold"],
            postfx_bloom_threshold_impl,
        ),
    );
    postfx_fields.insert(
        "frustum_cull".to_string(),
        Value::from_builtin(
            "postfx.frustum_cull",
            &["enabled"],
            postfx_frustum_cull_impl,
        ),
    );
    env.set(
        "postfx".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: postfx_fields,
            kind: "module",
        }))),
    );
    env.set(
        "sun".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: sun_fields,
            kind: "module",
        }))),
    );

    // Phase 18: 3D physics surface. All builtins forward to the
    // thread-local PhysicsWorld in src/physics3d.rs. The play3d
    // loop steps the world before each Twe `on update(dt)` so
    // scripts read authoritative positions.
    let mut physics_fields = HashMap::new();
    physics_fields.insert(
        "body".to_string(),
        Value::from_builtin("physics.body", &["shape", "at", "mass"], physics_body_impl),
    );
    physics_fields.insert(
        "static_box".to_string(),
        Value::from_builtin(
            "physics.static_box",
            &["at", "size"],
            physics_static_box_impl,
        ),
    );
    physics_fields.insert(
        "static_sphere".to_string(),
        Value::from_builtin(
            "physics.static_sphere",
            &["at", "radius"],
            physics_static_sphere_impl,
        ),
    );
    physics_fields.insert(
        "static_mesh".to_string(),
        Value::from_builtin(
            "physics.static_mesh",
            &["path", "at"],
            physics_static_mesh_impl,
        ),
    );
    physics_fields.insert(
        "raycast".to_string(),
        Value::from_builtin(
            "physics.raycast",
            &["origin", "direction", "max_dist"],
            physics_raycast_impl,
        ),
    );
    physics_fields.insert(
        "position".to_string(),
        Value::from_builtin("physics.position", &["handle"], physics_position_impl),
    );
    physics_fields.insert(
        "velocity".to_string(),
        Value::from_builtin("physics.velocity", &["handle", "v"], physics_velocity_impl),
    );
    physics_fields.insert(
        "impulse".to_string(),
        Value::from_builtin("physics.impulse", &["handle", "v"], physics_impulse_impl),
    );
    physics_fields.insert(
        "gravity".to_string(),
        Value::from_builtin("physics.gravity", &["v"], physics_gravity_impl),
    );
    physics_fields.insert(
        "character".to_string(),
        Value::from_builtin(
            "physics.character",
            &["at", "height", "radius"],
            physics_character_impl,
        ),
    );
    physics_fields.insert(
        "character_move".to_string(),
        Value::from_builtin(
            "physics.character_move",
            &["handle", "dir", "dt"],
            physics_character_move_impl,
        ),
    );
    physics_fields.insert(
        "collisions".to_string(),
        Value::from_builtin("physics.collisions", &[], physics_collisions_impl),
    );
    physics_fields.insert(
        "despawn".to_string(),
        Value::from_builtin("physics.despawn", &["handle"], physics_despawn_impl),
    );
    physics_fields.insert(
        "reset".to_string(),
        Value::from_builtin("physics.reset", &[], physics_reset_impl),
    );
    env.set(
        "physics".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: physics_fields,
            kind: "module",
        }))),
    );

    // Phase 17 session 3: cursor lock/unlock for FPS-style camera
    // control in `twec play3d`. The builtins write a pending flag
    // that the play3d event loop drains and applies to the window.
    // No-op in `twec play` (2D macroquad path doesn't expose
    // cursor-grab; macroquad scripts don't typically need it).
    let mut cursor_fields = HashMap::new();
    cursor_fields.insert(
        "lock".to_string(),
        Value::from_builtin("cursor.lock", &[], cursor_lock_impl),
    );
    cursor_fields.insert(
        "unlock".to_string(),
        Value::from_builtin("cursor.unlock", &[], cursor_unlock_impl),
    );
    env.set(
        "cursor".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: cursor_fields,
            kind: "module",
        }))),
    );

    // Phase 27: full 26-letter + 10-digit + F1-F12 + common-keys
    // namespace. Field-style access (`key.m`, `key_press.i`) now
    // mirrors what `key_held("m")` / `key_pressed("m")` already
    // supported. The 2D macroquad backend in `play.rs` and the 3D
    // winit backend in `play3d.rs` carry the matching VK→name
    // tables so the input plumbing actually drives these slots.
    let key_names = [
        // Movement / arrows.
        "right",
        "left",
        "up",
        "down",
        // Common control keys.
        "space",
        "escape",
        "enter",
        "tab",
        "backspace",
        "shift",
        "ctrl",
        "alt",
        // Letters a–z.
        "a",
        "b",
        "c",
        "d",
        "e",
        "f",
        "g",
        "h",
        "i",
        "j",
        "k",
        "l",
        "m",
        "n",
        "o",
        "p",
        "q",
        "r",
        "s",
        "t",
        "u",
        "v",
        "w",
        "x",
        "y",
        "z",
        // Digits 0–9.
        "0",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        // Function row F1–F12.
        "f1",
        "f2",
        "f3",
        "f4",
        "f5",
        "f6",
        "f7",
        "f8",
        "f9",
        "f10",
        "f11",
        "f12",
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
    // Phase 27: pre-register dx/dy so reading them in headless
    // `twec run` (no event loop) doesn't blow up. The play3d event
    // loop overwrites these each frame from raw DeviceEvent
    // mouse-motion deltas; the play (2D) loop leaves them at 0.
    mouse_fields.insert("dx".to_string(), Value::from_float(0.0));
    mouse_fields.insert("dy".to_string(), Value::from_float(0.0));
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

    // Phase 9 session 5: gamepad surface. First-connected gamepad
    // only — multi-gamepad routing is a follow-on. Mirrors the
    // key / key_press / mouse split: continuous in `gamepad`,
    // edge-triggered in `gamepad_press`, analog axes in
    // `gamepad_axis`. Twe field names follow Xbox-style naming so
    // scripts read naturally: `gamepad.a`, `gamepad.start`,
    // `gamepad_axis.lx`. The macroquad `play` loop polls gilrs each
    // frame and writes here; `twec run` (headless) leaves all
    // fields at their install-time defaults (false / 0.0).
    let mut gp = HashMap::new();
    let mut gp_press = HashMap::new();
    for name in GAMEPAD_BUTTON_NAMES {
        gp.insert((*name).to_string(), Value::FALSE);
        gp_press.insert((*name).to_string(), Value::FALSE);
    }
    gp.insert("connected".to_string(), Value::FALSE);
    env.set(
        "gamepad".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: gp,
            kind: "input",
        }))),
    );
    env.set(
        "gamepad_press".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: gp_press,
            kind: "input",
        }))),
    );
    let mut gp_axis = HashMap::new();
    for name in GAMEPAD_AXIS_NAMES {
        gp_axis.insert((*name).to_string(), Value::from_float(0.0));
    }
    env.set(
        "gamepad_axis".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: gp_axis,
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
    install_os(env);
    install_settings(env);
    install_lang(env);
    install_gc(env);
    install_replay(env);
    #[cfg(not(target_arch = "wasm32"))]
    install_net(env);
    #[cfg(not(target_arch = "wasm32"))]
    install_rollback(env);
    install_assets(env);
    install_touch(env);
    install_joystick_widget(env);
    install_safe_area(env);
    install_console(env);
    install_platform_services(env);
    install_mmo(env);
    install_workshop(env);
    // v1.0.1 session 1: procedural VFX library. 12 call-and-go
    // effects (hit_flash / screen_shake / hit_stop / damage_number /
    // crit_text / death_burst / pickup_pop / dash_trail /
    // level_up_ring / blood_splat / muzzle_flash / ground_shockwave).
    // State lives in `crate::fx`; the play loop calls
    // `fx::fx_tick`, `fx::consume_hit_stop_tick`, `fx::fx_draw_overlay`.
    install_fx(env);
    // v1.0.1 session 2: deterministic easing. Six pure functions —
    // `tween.ease(name, t)` / `tween.lerp(a, b, t)` /
    // `tween.lerp_eased(a, b, t, name)` / `tween.bounce(a, b, t)` /
    // `tween.shake(seed, t, freq)` / `tween.eases()`. No thread_local,
    // no global state — outputs are byte-identical functions of inputs
    // so replay determinism (Phase 29) is preserved automatically.
    install_tween(env);
    #[cfg(not(target_arch = "wasm32"))]
    install_world(env);
    #[cfg(not(target_arch = "wasm32"))]
    install_terrain(env);
    // Phase 10 session 8: explicit pause primitive. `pause(flag)`
    // toggles the runtime pause flag; `is_paused()` queries it.
    // While paused, the play loop skips `tick_frame` (no fibers
    // advance, no every-clocks fire) but `render_frame` still runs
    // so the scene stays visible — typical pause-menu behavior.
    // The exit-criterion-driven auto-pause-on-window-blur defers
    // to a winit-integration follow-on (macroquad 0.4 doesn't
    // expose focus events; per-state `pause: false` opt-out also
    // remains an open syntax question per CLAUDE.md "What is open").
    env.set(
        "pause".to_string(),
        Value::from_builtin("pause", &["flag"], pause_set),
    );
    env.set(
        "is_paused".to_string(),
        Value::from_builtin("is_paused", &[], pause_get),
    );
    // Phase 11 session 1: screenshot capture. `screenshot(path)`
    // queues a write that the play loop honors after the current
    // frame finishes rendering — calling `get_screen_data` inside
    // `on render():` would capture the *previous* frame, which is
    // confusing. The play loop also handles the F12 hotkey; the
    // builtin gives scripts a way to take screenshots from custom
    // bindings (e.g. an "Export" button in a level editor).
    env.set(
        "screenshot".to_string(),
        Value::from_builtin("screenshot", &["path"], screenshot_impl),
    );
    // Phase 11 session 11: opt-in pause-when-idle. Until macroquad
    // exposes desktop focus events (the win/macos paths in miniquad's
    // `window_minimized_event` are no-ops on 0.4), the player-walked-
    // away case is approximated by "no keyboard or mouse input for N
    // seconds". Set the threshold via `auto_pause_when_idle(seconds)`;
    // pass 0 to disable. The play loop calls `pause(true)` once the
    // idle-timer crosses the threshold and clears the auto-set flag
    // when input resumes (so manually-set pause stays paused).
    env.set(
        "auto_pause_when_idle".to_string(),
        Value::from_builtin(
            "auto_pause_when_idle",
            &["seconds"],
            auto_pause_when_idle_impl,
        ),
    );
    // Phase 11 follow-on (deeper): opt-in pause-when-window-blurs.
    // The play loop polls `window_focus::is_focused()` once per frame
    // and the `BlurAutoPause` state machine drives the pause flag on
    // focus transitions. Off by default (the existing examples assume
    // unattended demo / kiosk runs are fine); enable with
    // `auto_pause_on_blur(true)`. Disable with `auto_pause_on_blur(false)`.
    env.set(
        "auto_pause_on_blur".to_string(),
        Value::from_builtin("auto_pause_on_blur", &["enabled"], auto_pause_on_blur_impl),
    );

    // Phase 15 session 3: Steam SDK integration. All builtins are
    // registered unconditionally — the underlying steam.rs fns are
    // no-ops in non-steam builds or when Steam is not running so
    // scripts don't need to guard every call.
    let mut achievement_fields = HashMap::new();
    achievement_fields.insert(
        "unlock".to_string(),
        Value::from_builtin(
            "achievement.unlock",
            &["name"],
            crate::steam::achievement_unlock,
        ),
    );
    env.set(
        "achievement".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: achievement_fields,
            kind: "module",
        }))),
    );

    let mut stat_fields = HashMap::new();
    stat_fields.insert(
        "set".to_string(),
        Value::from_builtin("stat.set", &["name", "value"], crate::steam::stat_set),
    );
    stat_fields.insert(
        "get".to_string(),
        Value::from_builtin("stat.get", &["name"], crate::steam::stat_get),
    );
    stat_fields.insert(
        "commit".to_string(),
        Value::from_builtin("stat.commit", &[], crate::steam::stat_commit),
    );
    env.set(
        "stat".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: stat_fields,
            kind: "module",
        }))),
    );

    let mut cloud_fields = HashMap::new();
    cloud_fields.insert(
        "save".to_string(),
        Value::from_builtin(
            "cloud.save",
            &["filename", "data"],
            crate::steam::cloud_save,
        ),
    );
    cloud_fields.insert(
        "load".to_string(),
        Value::from_builtin("cloud.load", &["filename"], crate::steam::cloud_load),
    );
    env.set(
        "cloud".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: cloud_fields,
            kind: "module",
        }))),
    );
}

/// Phase 10 session 8: process-wide pause flag. Atomic + thread-local
/// is overkill for a single-threaded interpreter, but using a static
/// AtomicBool keeps `is_paused()` callable from the play loop without
/// having to thread an Env handle through. Hot reload preserves the
/// flag — a paused state survives a script edit; if that becomes
/// surprising in practice we can clear it inside `clear_asset_caches`.
static PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// Phase 11 session 1: screenshot request slot. The Twe `screenshot(path)`
// builtin writes the path here; the play loop drains it after each
// frame's render and writes the PNG via `get_screen_data().export_png`.
// Single-slot (last write wins) is fine — issuing two screenshot calls
// in the same frame is degenerate.
thread_local! {
    /// Phase 22: in-memory save store. Map of key → Value.
    /// `save.write(path)` flushes the whole map to a JSON file
    /// via the Phase 8 `save_to` codec; `save.read(path)`
    /// replaces the map from a file. Typed helpers (vec3, f32,
    /// int, string) read/write through this map.
    static SAVE_STORE: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
    /// Phase 21: per-mesh-handle animation state. Keyed by the
    /// texture-handle u32 the script passed to `mesh_textured`
    /// (we reuse texture handles as the mesh-instance identity
    /// for animation; a future revision could split them). Stores
    /// active clip name, current time, looping flag, and optional
    /// blend target for cross-fades.
    static MESH_ANIM_STATE: RefCell<HashMap<u32, MeshAnimEntry>> =
        RefCell::new(HashMap::new());
    /// Phase 20: script-controlled lighting state. The play3d loop
    /// reads this once per frame and uploads to the GPU lights
    /// uniform. Up to 8 simultaneous point lights; light.add()
    /// returns the slot index (1-based, so 0 means "all full").
    static LIGHTS_STATE: RefCell<crate::play3d::LightsUniform> =
        RefCell::new(crate::play3d::LightsUniform::new());
    /// Phase 25: shadow-pass enable flag (default off — opt-in via
    /// `sun.shadow(true)`). When off, the play3d frame loop still
    /// writes the shadow uniform, but with `flags.w = 0` so the
    /// main shader's PCF lookup short-circuits to fully lit.
    static SHADOW_ENABLED: RefCell<bool> = const { RefCell::new(false) };
    /// Phase 25: half-side of the orthographic shadow frustum, in
    /// world units. Default of 30m covers a typical character +
    /// surrounding playable area at moderate sharpness; bigger
    /// scenes need a larger value (sharpness scales inversely).
    static SHADOW_EXTENT: RefCell<f32> = const { RefCell::new(30.0) };
    /// Phase 26: enable/disable per-instance frustum culling.
    /// On by default — culling is a performance win, especially
    /// in open scenes with thousands of instances. Toggle off via
    /// `postfx.frustum_cull(false)` when benchmarking the cull
    /// path's contribution.
    static FRUSTUM_CULL_ENABLED: RefCell<bool> = const { RefCell::new(true) };
    /// Phase 26: ACES filmic tone mapping enable. The main
    /// pipeline always renders to an HDR offscreen target
    /// (Rgba16Float) and a fullscreen tonemap pass writes the
    /// swapchain. The toggle controls whether the tonemap shader
    /// applies the ACES curve (default on, commercial-grade)
    /// versus a straight linear→sRGB pass (off).
    static TONEMAP_ENABLED: RefCell<bool> = const { RefCell::new(true) };
    /// Phase 26: vignette strength, 0.0 (off) to 1.0 (full).
    /// Applied during the tonemap pass. Default off; opt-in via
    /// `postfx.vignette(strength)`.
    static VIGNETTE_STRENGTH: RefCell<f32> = const { RefCell::new(0.0) };
    /// Phase 28 session 4: vignette tint color (RGB, 0..1). Default
    /// black for the classic darkening look. Set via
    /// `postfx.vignette_color(r, g, b)` for stylised effects (e.g.
    /// twilight purples, dusk oranges).
    static VIGNETTE_COLOR: RefCell<[f32; 3]> = const { RefCell::new([0.0, 0.0, 0.0]) };
    /// Phase 28 session 3: inline-bloom intensity, 0.0 (off) to ~1.0
    /// (strong). Multiplied against the 12-tap bright-pixel sum
    /// before the ACES tonemap. Opt-in via `postfx.bloom(strength)`.
    static BLOOM_INTENSITY: RefCell<f32> = const { RefCell::new(0.0) };
    /// Phase 28 session 3: HDR luminance threshold above which a
    /// pixel contributes to bloom. 1.0 means "only blown highlights
    /// glow"; 0.5 means "anything brighter than mid-grey glows".
    /// Tune in tandem with bloom intensity.
    static BLOOM_THRESHOLD: RefCell<f32> = const { RefCell::new(1.0) };
    static PENDING_SCREENSHOT: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Phase 17 session 3: pending cursor-mode change requested by
    /// the script via cursor.lock() / cursor.unlock(). The play3d
    /// event loop drains this once per frame and applies the mode
    /// to the active window. Some(true) = lock, Some(false) = unlock.
    /// None means no change requested.
    static PENDING_CURSOR_MODE: RefCell<Option<bool>> = const { RefCell::new(None) };
}

pub fn take_pending_screenshot() -> Option<String> {
    PENDING_SCREENSHOT.with(|c| c.borrow_mut().take())
}

/// Phase 20: snapshot the current lighting state for the play3d
/// frame loop. Returns by value so the caller can write it
/// straight into a wgpu buffer without holding the thread-local.
pub fn lights_snapshot() -> crate::play3d::LightsUniform {
    LIGHTS_STATE.with(|s| *s.borrow())
}

/// Phase 25: read the script-controlled shadow enable flag.
pub fn shadow_enabled() -> bool {
    SHADOW_ENABLED.with(|s| *s.borrow())
}

/// Phase 25: read the script-controlled shadow frustum extent.
pub fn shadow_extent() -> f32 {
    SHADOW_EXTENT.with(|s| *s.borrow())
}

/// Phase 26: read the script-controlled frustum-cull toggle.
pub fn frustum_culling_enabled() -> bool {
    FRUSTUM_CULL_ENABLED.with(|s| *s.borrow())
}

/// Phase 26: read the script-controlled ACES tonemap toggle.
pub fn tonemap_enabled() -> bool {
    TONEMAP_ENABLED.with(|s| *s.borrow())
}

/// Phase 26: read the script-controlled vignette strength.
pub fn vignette_strength() -> f32 {
    VIGNETTE_STRENGTH.with(|s| *s.borrow())
}

/// Phase 28 session 4: read the script-controlled vignette tint.
pub fn vignette_color() -> [f32; 3] {
    VIGNETTE_COLOR.with(|s| *s.borrow())
}

/// Phase 28 session 3: read the script-controlled bloom intensity.
pub fn bloom_intensity() -> f32 {
    BLOOM_INTENSITY.with(|s| *s.borrow())
}

/// Phase 28 session 3: read the script-controlled bloom threshold.
pub fn bloom_threshold() -> f32 {
    BLOOM_THRESHOLD.with(|s| *s.borrow())
}

/// Phase 17 session 3: drain the cursor-mode request slot. play3d
/// calls this once per frame after rendering. Returns Some(true)
/// for "lock", Some(false) for "unlock", None for "no change".
pub fn take_pending_cursor_mode() -> Option<bool> {
    PENDING_CURSOR_MODE.with(|c| c.borrow_mut().take())
}

fn screenshot_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "screenshot")?;
    let path = string_arg(&args[0], "screenshot", "path")?;
    PENDING_SCREENSHOT.with(|c| *c.borrow_mut() = Some(path));
    Ok(Value::NIL)
}

fn cursor_lock_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    PENDING_CURSOR_MODE.with(|c| *c.borrow_mut() = Some(true));
    Ok(Value::NIL)
}

fn cursor_unlock_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    PENDING_CURSOR_MODE.with(|c| *c.borrow_mut() = Some(false));
    Ok(Value::NIL)
}

/// Phase 11 session 11: idle-pause threshold. The play loop reads
/// this each frame to decide whether to auto-pause; set to 0 to
/// disable. Storing as `f64::to_bits` keeps the static atomic happy.
static AUTO_PAUSE_IDLE_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn auto_pause_idle_threshold() -> f64 {
    f64::from_bits(AUTO_PAUSE_IDLE_SECS.load(std::sync::atomic::Ordering::Relaxed))
}

fn auto_pause_when_idle_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "auto_pause_when_idle")?;
    let secs = as_f64(&args[0], "auto_pause_when_idle.seconds")?;
    if secs.is_nan() || secs < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("auto_pause_when_idle expects a non-negative number, got {secs}"),
            help: Some(
                "pass 0 to disable, otherwise the seconds of input-idle before auto-pausing"
                    .to_string(),
            ),
        });
    }
    AUTO_PAUSE_IDLE_SECS.store(secs.to_bits(), std::sync::atomic::Ordering::Relaxed);
    Ok(Value::NIL)
}

/// Phase 11 follow-on (deeper): auto-pause-on-window-blur opt-in flag.
/// The play loop reads this each frame; when enabled, focus loss drives
/// the pause flag via `BlurAutoPause`. Defaulting off mirrors
/// `auto_pause_when_idle` — neither feature should fire on kiosk /
/// demo / CI runs that don't ask for it.
static AUTO_PAUSE_ON_BLUR: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn auto_pause_on_blur_enabled() -> bool {
    AUTO_PAUSE_ON_BLUR.load(std::sync::atomic::Ordering::Relaxed)
}

fn auto_pause_on_blur_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "auto_pause_on_blur")?;
    let v = &args[0];
    if !v.is_bool() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "auto_pause_on_blur expects a bool, got {}",
                (*v).type_name()
            ),
            help: Some(
                "call `auto_pause_on_blur(true)` to enable; `auto_pause_on_blur(false)` to disable"
                    .to_string(),
            ),
        });
    }
    AUTO_PAUSE_ON_BLUR.store(v.as_bool(), std::sync::atomic::Ordering::Relaxed);
    Ok(Value::NIL)
}

pub fn is_paused() -> bool {
    PAUSED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Phase 11 session 11: play-loop write path for the pause flag.
/// The idle-auto-pause machinery and any future focus-event
/// integration go through here so they don't have to import the
/// `PAUSED` static directly.
pub fn set_paused(flag: bool) {
    PAUSED.store(flag, std::sync::atomic::Ordering::Relaxed);
}

fn pause_set(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "pause")?;
    let v = &args[0];
    if !v.is_bool() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("pause expects a bool, got {}", (*v).type_name()),
            help: Some("call `pause(true)` to halt; `pause(false)` to resume".to_string()),
        });
    }
    PAUSED.store(v.as_bool(), std::sync::atomic::Ordering::Relaxed);
    Ok(Value::NIL)
}

fn pause_get(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "is_paused")?;
    Ok(Value::from_bool(is_paused()))
}

/// Phase 10 session 5b: `os` namespace. Currently houses
/// `os.clipboard.read()` / `os.clipboard.write(text)` for cross-
/// platform clipboard access via the arboard crate. Both fail
/// silently when no clipboard is available (headless CI, sandboxed
/// runtimes) — read returns the empty string, write drops — so
/// games stay portable without forcing every script to wrap calls
/// in error handling that doesn't yet exist as a language feature.
fn install_os(env: &mut Env) {
    let mut clipboard = HashMap::new();
    clipboard.insert(
        "read".to_string(),
        Value::from_builtin("os.clipboard.read", &[], clipboard_read),
    );
    clipboard.insert(
        "write".to_string(),
        Value::from_builtin("os.clipboard.write", &["text"], clipboard_write),
    );
    let mut os_obj = HashMap::new();
    os_obj.insert(
        "clipboard".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: clipboard,
            kind: "module",
        }))),
    );
    env.set(
        "os".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: os_obj,
            kind: "module",
        }))),
    );
}

fn clipboard_read(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "os.clipboard.read")?;
    // Phase 30 session 1: arboard is native-only; clipboard is a no-op
    // on WASM (browser clipboard access requires async + user permission
    // which doesn't fit the sync builtin model).
    #[cfg(not(target_arch = "wasm32"))]
    let s = arboard::Clipboard::new()
        .and_then(|mut c| c.get_text())
        .unwrap_or_default();
    #[cfg(target_arch = "wasm32")]
    let s = String::new();
    Ok(Value::from_string(s))
}

fn clipboard_write(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "os.clipboard.write")?;
    let text = {
        let v = &args[0];
        if v.is_str() {
            v.as_string().clone()
        } else {
            (*v).display()
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text));
    }
    Ok(Value::NIL)
}

fn key_held_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "key_held")?;
    let name = string_arg(&args[0], "key_held", "name")?;
    Ok(read_input_field(env, "key", &name))
}

fn key_pressed_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "key_pressed")?;
    let name = string_arg(&args[0], "key_pressed", "name")?;
    Ok(read_input_field(env, "key_press", &name))
}

fn read_input_field(env: &Env, ambient: &str, name: &str) -> Value {
    let Some(obj) = env.get(ambient) else {
        return Value::FALSE;
    };
    if !obj.is_object() {
        return Value::FALSE;
    }
    let rc = obj.as_object();
    let v = rc.borrow().get_field(name).unwrap_or(Value::FALSE);
    v
}

/// Phase 10 session 9: settings system + persistence.
///
/// Surface: `settings.set(key, value)` / `.get(key)` / `.has(key)` /
/// `.set_default(key, value)` / `.save(path)` / `.load(path)`. The
/// data lives in a `settings.data` Object (kind: "save") so users
/// can also introspect it directly via `settings.data.<field>` and
/// so `settings.save` can hand it straight to `save_to_path` without
/// a custom encoder. `set_default` only writes when the key is
/// absent — the canonical pattern for first-launch defaults that
/// shouldn't clobber a loaded config. `load` merges into the
/// existing data (last-write-wins per key) instead of replacing
/// wholesale, so calling `set_default` after `load` does the right
/// thing across schema additions between launches.
fn install_settings(env: &mut Env) {
    let data = Value::from_object(Rc::new(RefCell::new(Object {
        fields: HashMap::new(),
        kind: "save",
    })));
    let mut settings = HashMap::new();
    settings.insert("data".to_string(), data);
    settings.insert(
        "set".to_string(),
        Value::from_builtin("settings.set", &["key", "value"], settings_set),
    );
    settings.insert(
        "get".to_string(),
        Value::from_builtin("settings.get", &["key"], settings_get),
    );
    settings.insert(
        "has".to_string(),
        Value::from_builtin("settings.has", &["key"], settings_has),
    );
    settings.insert(
        "set_default".to_string(),
        Value::from_builtin(
            "settings.set_default",
            &["key", "value"],
            settings_set_default,
        ),
    );
    settings.insert(
        "save".to_string(),
        Value::from_builtin("settings.save", &["path"], settings_save),
    );
    settings.insert(
        "load".to_string(),
        Value::from_builtin("settings.load", &["path"], settings_load),
    );
    // `try_load` is the graceful-first-launch variant: returns true
    // when the file existed and merged in, false when it didn't, and
    // errors only on real corruption (bad JSON, wrong shape). The
    // typical bootstrap sequence is `set_default(...)` to seed
    // defaults, then `try_load` to overlay any persisted overrides.
    settings.insert(
        "try_load".to_string(),
        Value::from_builtin("settings.try_load", &["path"], settings_try_load),
    );
    env.set(
        "settings".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: settings,
            kind: "module",
        }))),
    );
}

fn settings_data_obj(env: &Env, op: &str) -> Result<Rc<RefCell<Object>>, RuntimeError> {
    let s = env.get("settings").ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!("{op}: `settings` ambient is missing"),
        help: None,
    })?;
    if !s.is_object() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op}: `settings` is not an object"),
            help: None,
        });
    }
    let rc = s.as_object();
    let o = rc.borrow();
    let data = o.get_field("data").ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!("{op}: `settings.data` is missing"),
        help: None,
    })?;
    if !data.is_object() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op}: `settings.data` is not an object"),
            help: None,
        });
    }
    Ok(data.as_object())
}

fn string_arg(v: &Value, op: &str, label: &str) -> Result<String, RuntimeError> {
    if v.is_str() {
        Ok(v.as_string().clone())
    } else {
        Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op} expects a string {label}, got {}", (*v).type_name()),
            help: None,
        })
    }
}

fn settings_set(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "settings.set")?;
    let key = string_arg(&args[0], "settings.set", "key")?;
    let data = settings_data_obj(env, "settings.set")?;
    data.borrow_mut().fields.insert(key, args[1]);
    Ok(Value::NIL)
}

fn settings_get(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "settings.get")?;
    let key = string_arg(&args[0], "settings.get", "key")?;
    let data = settings_data_obj(env, "settings.get")?;
    let val = data.borrow().get_field(&key).unwrap_or(Value::NIL);
    Ok(val)
}

fn settings_has(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "settings.has")?;
    let key = string_arg(&args[0], "settings.has", "key")?;
    let data = settings_data_obj(env, "settings.has")?;
    let present = data.borrow().fields.contains_key(&key);
    Ok(Value::from_bool(present))
}

fn settings_set_default(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "settings.set_default")?;
    let key = string_arg(&args[0], "settings.set_default", "key")?;
    let data = settings_data_obj(env, "settings.set_default")?;
    let mut o = data.borrow_mut();
    o.fields.entry(key).or_insert(args[1]);
    Ok(Value::NIL)
}

fn settings_save(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "settings.save")?;
    let path = string_arg(&args[0], "settings.save", "path")?;
    let data = settings_data_obj(env, "settings.save")?;
    let value = Value::from_object(data);
    crate::save::save_to_path(std::path::Path::new(&path), &value)
        .map_err(|m| crate::save::to_runtime_error(m, 0, 0))?;
    Ok(Value::NIL)
}

fn settings_load(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "settings.load")?;
    let path = string_arg(&args[0], "settings.load", "path")?;
    let loaded = crate::save::load_from_path(std::path::Path::new(&path))
        .map_err(|m| crate::save::to_runtime_error(m, 0, 0))?;
    merge_settings_data(env, "settings.load", &path, &loaded)?;
    Ok(Value::NIL)
}

fn settings_try_load(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "settings.try_load")?;
    let path = string_arg(&args[0], "settings.try_load", "path")?;
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Ok(Value::FALSE);
    }
    let loaded =
        crate::save::load_from_path(p).map_err(|m| crate::save::to_runtime_error(m, 0, 0))?;
    merge_settings_data(env, "settings.try_load", &path, &loaded)?;
    Ok(Value::TRUE)
}

fn merge_settings_data(
    env: &Env,
    op: &str,
    path: &str,
    loaded: &Value,
) -> Result<(), RuntimeError> {
    if !loaded.is_object() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{op}: file at {path} is not an object, got {}",
                loaded.type_name()
            ),
            help: Some("settings files are saved Objects of key:value pairs.".to_string()),
        });
    }
    let data = settings_data_obj(env, op)?;
    let loaded_rc = loaded.as_object();
    let loaded_o = loaded_rc.borrow();
    let mut data_o = data.borrow_mut();
    for (k, v) in &loaded_o.fields {
        data_o.fields.insert(k.clone(), *v);
    }
    Ok(())
}

/// Phase 10 session 10: localization scaffolding.
///
/// Surface: `lang.set_locale(name)` / `lang.locale()` / `lang.load(name,
/// path)` / `lang.t(key)` / `lang.tf(key, args)`.
///
/// Bundles are JSON files of `{ "menu.resume": "Resume", ... }` —
/// loaded via `load_from_path` (the same JSON path that backs save /
/// load) and stored under `lang.bundles[name]` as a flat Object of
/// key → string. `t(key)` looks up the key in the active bundle and
/// returns the key itself as fallback when missing — silent fallback
/// is the right default for shipping games (a missing translation
/// shouldn't crash the menu). `tf(key, args)` does positional
/// `{0}` / `{1}` / ... substitution from a List, so scripts can do
/// `lang.tf("greet", ["Alice"])` against a template like
/// `"Hi {0}!"`. Positional rather than named because Twe has no
/// object-literal syntax — passing a list keeps the call site
/// concise without forcing users to construct Objects from builtins.
/// Two builtins (`t` and `tf`) instead of one because Twe's calling
/// convention requires every kwarg supplied.
fn install_lang(env: &mut Env) {
    let mut lang = HashMap::new();
    lang.insert("active".to_string(), Value::from_string("en".to_string()));
    lang.insert(
        "bundles".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: HashMap::new(),
            kind: "module",
        }))),
    );
    lang.insert(
        "set_locale".to_string(),
        Value::from_builtin("lang.set_locale", &["name"], lang_set_locale),
    );
    lang.insert(
        "locale".to_string(),
        Value::from_builtin("lang.locale", &[], lang_locale),
    );
    lang.insert(
        "load".to_string(),
        Value::from_builtin("lang.load", &["name", "path"], lang_load),
    );
    lang.insert(
        "t".to_string(),
        Value::from_builtin("lang.t", &["key"], lang_t),
    );
    lang.insert(
        "tf".to_string(),
        Value::from_builtin("lang.tf", &["key", "args"], lang_tf),
    );
    env.set(
        "lang".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: lang,
            kind: "module",
        }))),
    );
}

fn lang_namespace(env: &Env, op: &str) -> Result<Rc<RefCell<Object>>, RuntimeError> {
    let l = env.get("lang").ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!("{op}: `lang` ambient is missing"),
        help: None,
    })?;
    if !l.is_object() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op}: `lang` is not an object"),
            help: None,
        });
    }
    Ok(l.as_object())
}

fn lang_set_locale(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "lang.set_locale")?;
    let name = string_arg(&args[0], "lang.set_locale", "name")?;
    let lang = lang_namespace(env, "lang.set_locale")?;
    lang.borrow_mut()
        .fields
        .insert("active".to_string(), Value::from_string(name));
    Ok(Value::NIL)
}

fn lang_locale(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "lang.locale")?;
    let lang = lang_namespace(env, "lang.locale")?;
    let active = lang
        .borrow()
        .get_field("active")
        .filter(|v| v.is_str())
        .map(|v| v.as_string().clone())
        .unwrap_or_else(|| "en".to_string());
    Ok(Value::from_string(active))
}

fn lang_load(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "lang.load")?;
    let name = string_arg(&args[0], "lang.load", "name")?;
    let path = string_arg(&args[1], "lang.load", "path")?;
    let loaded = crate::save::load_from_path(std::path::Path::new(&path))
        .map_err(|m| crate::save::to_runtime_error(m, 0, 0))?;
    if !loaded.is_object() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "lang.load: bundle at {path} is not an object, got {}",
                loaded.type_name()
            ),
            help: Some(
                "locale bundles are JSON Objects of key → string, e.g. {\"menu.resume\": \"Resume\"}."
                    .to_string(),
            ),
        });
    }
    let lang = lang_namespace(env, "lang.load")?;
    let bundles_v = lang
        .borrow()
        .get_field("bundles")
        .ok_or_else(|| RuntimeError {
            line: 0,
            col: 0,
            message: "lang.load: `lang.bundles` is missing".to_string(),
            help: None,
        })?;
    if !bundles_v.is_object() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "lang.load: `lang.bundles` is not an object".to_string(),
            help: None,
        });
    }
    let bundles = bundles_v.as_object();
    bundles.borrow_mut().fields.insert(name, loaded);
    Ok(Value::NIL)
}

/// Look up a translation string in the active bundle. Returns
/// `(found, string)` — `found = false` signals fallback to the key.
fn lang_lookup(env: &Env, key: &str) -> (bool, String) {
    let Ok(lang_rc) = lang_namespace(env, "lang.t") else {
        return (false, key.to_string());
    };
    let lang_o = lang_rc.borrow();
    let active = lang_o
        .get_field("active")
        .filter(|v| v.is_str())
        .map(|v| v.as_string().clone())
        .unwrap_or_else(|| "en".to_string());
    let Some(bundles_v) = lang_o.get_field("bundles") else {
        return (false, key.to_string());
    };
    if !bundles_v.is_object() {
        return (false, key.to_string());
    }
    let bundles = bundles_v.as_object();
    let bundles_o = bundles.borrow();
    let Some(bundle_v) = bundles_o.get_field(&active) else {
        return (false, key.to_string());
    };
    if !bundle_v.is_object() {
        return (false, key.to_string());
    }
    let bundle = bundle_v.as_object();
    let bundle_o = bundle.borrow();
    let Some(s) = bundle_o.get_field(key) else {
        return (false, key.to_string());
    };
    if s.is_str() {
        (true, s.as_string().clone())
    } else {
        (false, key.to_string())
    }
}

fn lang_t(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "lang.t")?;
    let key = string_arg(&args[0], "lang.t", "key")?;
    let (_, s) = lang_lookup(env, &key);
    Ok(Value::from_string(s))
}

fn lang_tf(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "lang.tf")?;
    let key = string_arg(&args[0], "lang.tf", "key")?;
    let args_v = &args[1];
    if !args_v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "lang.tf expects a list of positional args, got {}",
                (*args_v).type_name()
            ),
            help: Some(
                "pass a list, e.g. `lang.tf(\"greet\", [\"Alice\"])` against a template like \"Hi {0}!\"."
                    .to_string(),
            ),
        });
    }
    let (_, template) = lang_lookup(env, &key);
    let args_rc = args_v.as_list();
    let args_v = args_rc.borrow();
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            let mut closed = false;
            for c2 in chars.by_ref() {
                if c2 == '}' {
                    closed = true;
                    break;
                }
                name.push(c2);
            }
            if !closed {
                // Unterminated `{...` — emit literally so the user can spot it.
                out.push('{');
                out.push_str(&name);
                continue;
            }
            match name.parse::<usize>() {
                Ok(idx) if idx < args_v.len() => {
                    out.push_str(&args_v[idx].display());
                }
                _ => {
                    // Unknown index or non-numeric placeholder — emit
                    // literally so the missing arg is visible at runtime.
                    out.push('{');
                    out.push_str(&name);
                    out.push('}');
                }
            }
        } else {
            out.push(c);
        }
    }
    Ok(Value::from_string(out))
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
    // Phase 29 session 5: tick-accurate scheduling. The fixed-step
    // accumulator from session 1 advances simulation time by exactly
    // PHYSICS_DT per tick, so a sound queued for `t = 0.5s` fires
    // on the same tick across two runs. Underlying audio backend
    // (macroquad's quad-snd) is buffer-aligned, not sample-aligned —
    // honest deferral is in the closeout note.
    sound.insert(
        "schedule".to_string(),
        Value::from_builtin(
            "sound.schedule",
            &["handle", "when", "volume"],
            sound_schedule,
        ),
    );
    sound.insert(
        "now".to_string(),
        Value::from_builtin("sound.now", &[], sound_now),
    );
    sound.insert(
        "scheduled_count".to_string(),
        Value::from_builtin("sound.scheduled_count", &[], sound_scheduled_count),
    );
    sound.insert(
        "stop".to_string(),
        Value::from_builtin("sound.stop", &["handle"], sound_stop),
    );
    sound.insert(
        "set_volume".to_string(),
        Value::from_builtin("sound.set_volume", &["handle", "volume"], sound_set_volume),
    );
    // Phase 23: 3D spatial audio. Pans + attenuates a one-shot
    // play based on distance from the camera. Uses the Phase 9
    // audio layer underneath; no new crate dep.
    sound.insert(
        "play3d".to_string(),
        Value::from_builtin(
            "sound.play3d",
            &["handle", "at", "radius"],
            sound_play3d_impl,
        ),
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

/// Phase 23: 3D spatial sound. Reads `camera.eye` from the env
/// and computes a distance-based volume attenuation: 1.0 at the
/// source position, 0.0 at radius. macroquad's audio layer is
/// stereo-only with no built-in panning, so this is mono with
/// volume falloff — sufficient for "explosions feel further away
/// when you're further from them," not directional audio.
fn sound_play3d_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "sound.play3d")?;
    let path = sound_handle_path(&args[0], "sound.play3d")?;
    let at = xyz_of(&args[1], "sound.play3d.at")?;
    let radius = (number(&args[2], "sound.play3d.radius")? as f32).max(0.0);
    // Pull camera.eye if available; default to origin.
    let cam = env.get("camera");
    let eye = cam
        .and_then(|cam| {
            if !cam.is_object() {
                return None;
            }
            let rc = cam.as_object();
            let o = rc.borrow();
            o.get_field("eye").and_then(|v| {
                if v.is_tuple() && v.as_tuple().len() == 3 {
                    let elems = v.as_tuple();
                    Some([
                        number(&elems[0], "camera.eye").unwrap_or(0.0) as f32,
                        number(&elems[1], "camera.eye").unwrap_or(0.0) as f32,
                        number(&elems[2], "camera.eye").unwrap_or(0.0) as f32,
                    ])
                } else {
                    None
                }
            })
        })
        .unwrap_or([0.0, 0.0, 0.0]);
    let dx = at[0] - eye[0];
    let dy = at[1] - eye[1];
    let dz = at[2] - eye[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let vol = if radius <= 0.0 || dist >= radius {
        0.0
    } else {
        let t = 1.0 - (dist / radius);
        // Quadratic falloff matches the point-light attenuation
        // model so 3D audio "feels" the same as point lights.
        t * t
    };
    if vol > 0.0 {
        play_sound_path(&path, "sound.play3d", vol, false)?;
    }
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

// ---------- Phase 29 session 5: tick-accurate audio scheduling ----------

thread_local! {
    /// Simulation time in seconds, accumulated by `tick_audio_schedule`.
    /// Advances by exactly `PHYSICS_DT` per substep under the
    /// session-1 fixed-timestep loop. Reset on `clear_asset_caches`
    /// (which is also called on hot-reload) so a reloaded script
    /// starts from t=0.
    static SIM_TIME_S: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };

    /// Pending one-shots, ordered by `when` ascending. Inserted in
    /// sorted position by `sound_schedule`; drained from the front
    /// by `tick_audio_schedule` whenever `when <= SIM_TIME_S`.
    static SCHEDULED_SOUNDS: RefCell<Vec<ScheduledSound>> = const { RefCell::new(Vec::new()) };

    /// Test/observability counter: number of scheduled sounds
    /// dispatched (i.e., handed to the audio backend) since program
    /// start. Tests assert on this in lieu of inspecting macroquad's
    /// audio mixer directly (no headless audio backend exists in
    /// the test harness).
    static SOUND_DISPATCHED_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[derive(Clone)]
struct ScheduledSound {
    when: f64,
    path: String,
    volume: f32,
}

thread_local! {
    /// Test mode flag: when true, `tick_audio_schedule` skips the
    /// macroquad `play_sound` call (which requires a real window
    /// context) and just counts dispatches in
    /// `SOUND_DISPATCHED_COUNT`. Production code never sets this;
    /// the headless test harness in `tests/eval.rs` does.
    static AUDIO_DISPATCH_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test-only: suppress real audio dispatch. Subsequent
/// `tick_audio_schedule` calls drain the queue and bump the
/// dispatched counter but skip the `play_sound_path` call. Used by
/// tests that drive `tick_frame` headlessly — macroquad's audio
/// backend asserts a thread-local THREAD_ID that's only set up by
/// the play loop.
pub fn set_audio_dispatch_disabled(disabled: bool) {
    AUDIO_DISPATCH_DISABLED.with(|c| c.set(disabled));
}

/// Called by `eval::tick_frame` once per fixed-step substep. Advances
/// the simulation clock by `dt` and dispatches any scheduled sounds
/// whose deadline has passed. Failures (cache miss, decode error)
/// are surfaced to stderr and the entry is dropped — a broken sound
/// shouldn't kill the simulation.
pub fn tick_audio_schedule(dt: f64) {
    SIM_TIME_S.with(|t| t.set(t.get() + dt));
    let now = SIM_TIME_S.with(|t| t.get());
    let due: Vec<ScheduledSound> = SCHEDULED_SOUNDS.with(|s| {
        let mut q = s.borrow_mut();
        let mut split = 0usize;
        while split < q.len() && q[split].when <= now {
            split += 1;
        }
        q.drain(..split).collect()
    });
    let dispatch_disabled = AUDIO_DISPATCH_DISABLED.with(|c| c.get());
    for s in due {
        if !dispatch_disabled {
            // play_sound_path tolerates missing files etc. — surface
            // the error but don't bubble it to the script (the
            // schedule call already succeeded; this is the deferred
            // fire).
            if let Err(e) = play_sound_path(&s.path, "sound.schedule", s.volume, false) {
                eprintln!("[twec] scheduled audio dispatch failed: {e}");
            }
        }
        SOUND_DISPATCHED_COUNT.with(|c| c.set(c.get() + 1));
    }
}

/// Reset audio scheduling state. Called by `clear_asset_caches` so
/// hot-reload starts fresh, and from tests via the same path.
pub fn reset_audio_schedule() {
    SIM_TIME_S.with(|t| t.set(0.0));
    SCHEDULED_SOUNDS.with(|s| s.borrow_mut().clear());
    SOUND_DISPATCHED_COUNT.with(|c| c.set(0));
}

fn sound_schedule(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "sound.schedule")?;
    let path = sound_handle_path(&args[0], "sound.schedule")?;
    let when = number(&args[1], "sound.schedule.when")?;
    let volume = number(&args[2], "sound.schedule.volume")?.clamp(0.0, 1.0) as f32;
    if !when.is_finite() || when < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("sound.schedule expects a non-negative finite `when`, got {when}"),
            help: Some("`when` is absolute simulation seconds; use `sound.now() + offset` for relative".to_string()),
        });
    }
    let entry = ScheduledSound {
        when,
        path,
        volume,
    };
    SCHEDULED_SOUNDS.with(|s| {
        let mut q = s.borrow_mut();
        // Insertion sort — typical schedule depth is small (a handful
        // of upcoming beats). For deeper schedules a BinaryHeap would
        // be faster but we'd lose the simple drain-from-front pattern.
        let pos = q.partition_point(|x| x.when <= entry.when);
        q.insert(pos, entry);
    });
    Ok(Value::NIL)
}

fn sound_now(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "sound.now")?;
    Ok(Value::from_float(SIM_TIME_S.with(|t| t.get())))
}

fn sound_scheduled_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "sound.scheduled_count")?;
    let count = SCHEDULED_SOUNDS.with(|s| s.borrow().len());
    Ok(Value::from_int(count as i64))
}

/// Test-only accessor for the dispatched counter. Production code
/// shouldn't need it.
pub fn sound_dispatched_count() -> u32 {
    SOUND_DISPATCHED_COUNT.with(|c| c.get())
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
            // Phase 12 session 3: route through the active bundle
            // first; falls through to filesystem otherwise.
            let bytes = crate::bundle::read_asset_bytes(path).map_err(|e| RuntimeError {
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
    //
    // Phase 29 session 1: under the new fixed-timestep loop, `time.dt`
    // equals `time.physics_dt` (60 Hz default) on every tick. The
    // `physics_dt` field is exposed as a stable read-only constant
    // that scripts can read at top level (before any `tick_frame`
    // has run, when `time.dt` is still 0.0) — useful for sizing
    // velocity-per-step state or comparing against the simulation
    // rate the engine guarantees.
    let mut fields = HashMap::new();
    fields.insert("dt".to_string(), Value::from_float(0.0));
    fields.insert(
        "physics_dt".to_string(),
        Value::from_float(crate::eval::PHYSICS_DT),
    );
    env.set(
        "time".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "module",
        }))),
    );
}

/// Phase 29 session 2: `gc.*` namespace. Exposes the per-frame sweep
/// budget and observability into the tracing-GC heap. The play loop
/// drains a budgeted sweep step at every safepoint; scripts can
/// adjust the budget if they need a different latency / throughput
/// trade-off, and read `gc.last_collect_ms()` to see how a tuning
/// choice played out.
fn install_gc(env: &mut Env) {
    let mut gc = HashMap::new();
    gc.insert(
        "budget_ms".to_string(),
        Value::from_builtin("gc.budget_ms", &["ms"], gc_budget_ms),
    );
    gc.insert(
        "last_collect_ms".to_string(),
        Value::from_builtin("gc.last_collect_ms", &[], gc_last_collect_ms),
    );
    gc.insert(
        "bytes_alive".to_string(),
        Value::from_builtin("gc.bytes_alive", &[], gc_bytes_alive),
    );
    env.set(
        "gc".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: gc,
            kind: "module",
        }))),
    );
}

fn gc_budget_ms(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "gc.budget_ms")?;
    let ms = as_f64(&args[0], "gc.budget_ms")?;
    if !ms.is_finite() || ms < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("gc.budget_ms expects a non-negative finite number, got {ms}"),
            help: Some("pass 0 to drain greedily, or a positive ms cap per safepoint".to_string()),
        });
    }
    let ns = (ms * 1_000_000.0).round() as u64;
    crate::heap::gc_set_budget_ns(ns.max(1));
    Ok(Value::NIL)
}

fn gc_last_collect_ms(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "gc.last_collect_ms")?;
    let ns = crate::heap::gc_last_collect_ns();
    Ok(Value::from_float(ns as f64 / 1_000_000.0))
}

fn gc_bytes_alive(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "gc.bytes_alive")?;
    Ok(Value::from_int(crate::heap::gc_bytes_alive() as i64))
}

/// Phase 29 session 4: `replay.*` namespace. Captures input ambient
/// state per frame to a file, or replays from one — the foundation
/// for deterministic regression tests, lockstep multiplayer (Phase
/// 31), and bug-report-with-input-log workflows. The play loop calls
/// `crate::replay::tick(env)` once per simulation step; recording
/// snapshots `key`, `key_press`, `mouse`, `mouse_held`, `mouse_press`
/// to a small text log; playback overrides those ambients from the
/// log so the script sees identical input across runs.
fn install_replay(env: &mut Env) {
    let mut r = HashMap::new();
    r.insert(
        "record".to_string(),
        Value::from_builtin("replay.record", &["path"], replay_record),
    );
    r.insert(
        "play".to_string(),
        Value::from_builtin("replay.play", &["path"], replay_play),
    );
    r.insert(
        "stop".to_string(),
        Value::from_builtin("replay.stop", &[], replay_stop),
    );
    r.insert(
        "is_playing".to_string(),
        Value::from_builtin("replay.is_playing", &[], replay_is_playing),
    );
    env.set(
        "replay".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: r,
            kind: "module",
        }))),
    );
}

fn replay_record(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "replay.record")?;
    let path = string_arg(&args[0], "replay.record", "path")?;
    crate::replay::start_recording(&path).map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: e,
        help: None,
    })?;
    Ok(Value::NIL)
}

fn replay_play(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "replay.play")?;
    let path = string_arg(&args[0], "replay.play", "path")?;
    crate::replay::start_playing(&path).map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: e,
        help: None,
    })?;
    Ok(Value::NIL)
}

fn replay_stop(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "replay.stop")?;
    crate::replay::stop();
    Ok(Value::NIL)
}

fn replay_is_playing(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "replay.is_playing")?;
    Ok(Value::from_bool(crate::replay::is_playing()))
}

/// Phase 31: `net.*` namespace. Lockstep multiplayer over UDP.
/// See `docs/changes/2026-05-10-multiplayer-rfc.md` for the full
/// design. The script-facing surface is intentionally small:
/// `net.host` / `net.connect` open a session; `net.send_input` and
/// `net.tick_ready` drive the lockstep exchange; `net.close` ends.
/// `net.local_peer_id` / `net.peer_count` / `net.is_connected` /
/// `net.state_hash` are introspection helpers.
#[cfg(not(target_arch = "wasm32"))]
fn install_net(env: &mut Env) {
    let mut n = HashMap::new();
    n.insert(
        "host".to_string(),
        Value::from_builtin("net.host", &["port", "expected_peers"], net_host),
    );
    n.insert(
        "connect".to_string(),
        Value::from_builtin("net.connect", &["addr"], net_connect),
    );
    n.insert(
        "close".to_string(),
        Value::from_builtin("net.close", &[], net_close),
    );
    n.insert(
        "is_connected".to_string(),
        Value::from_builtin("net.is_connected", &[], net_is_connected),
    );
    n.insert(
        "local_peer_id".to_string(),
        Value::from_builtin("net.local_peer_id", &[], net_local_peer_id),
    );
    n.insert(
        "peer_count".to_string(),
        Value::from_builtin("net.peer_count", &[], net_peer_count),
    );
    n.insert(
        "send_input".to_string(),
        Value::from_builtin("net.send_input", &["tick"], net_send_input),
    );
    n.insert(
        "tick_ready".to_string(),
        Value::from_builtin("net.tick_ready", &["tick"], net_tick_ready),
    );
    n.insert(
        "advance_tick".to_string(),
        Value::from_builtin("net.advance_tick", &["tick"], net_advance_tick),
    );
    n.insert(
        "state_hash".to_string(),
        Value::from_builtin("net.state_hash", &[], net_state_hash),
    );
    n.insert(
        "send_state_hash".to_string(),
        Value::from_builtin(
            "net.send_state_hash",
            &["tick", "hash"],
            net_send_state_hash,
        ),
    );
    n.insert(
        "input_delay".to_string(),
        Value::from_builtin("net.input_delay", &[], net_input_delay),
    );
    n.insert(
        "session_ready".to_string(),
        Value::from_builtin("net.session_ready", &[], net_session_ready),
    );
    n.insert(
        "hash".to_string(),
        Value::from_builtin("net.hash", &["value"], net_hash),
    );
    n.insert(
        "snapshot_json".to_string(),
        Value::from_builtin("net.snapshot_json", &["value"], net_snapshot_json),
    );
    // Phase 36 session 2: Steam P2P transport.
    n.insert(
        "steam_p2p_available".to_string(),
        Value::from_builtin("net.steam_p2p_available", &[], net_steam_p2p_available),
    );
    n.insert(
        "local_steam_id".to_string(),
        Value::from_builtin("net.local_steam_id", &[], net_local_steam_id),
    );
    n.insert(
        "host_p2p".to_string(),
        Value::from_builtin("net.host_p2p", &["expected_peers"], net_host_p2p),
    );
    n.insert(
        "connect_p2p".to_string(),
        Value::from_builtin("net.connect_p2p", &["steam_id"], net_connect_p2p),
    );
    // Phase 36 session 3: STUN + rendezvous primitives. The lockstep
    // runner integration (role assignment from rendezvous outcome)
    // lands in Phase 36 session 4 alongside lobbies, where the
    // host/client split is naturally driven by lobby join order.
    n.insert(
        "discover_public_address".to_string(),
        Value::from_builtin(
            "net.discover_public_address",
            &["stun_server"],
            net_discover_public_address,
        ),
    );
    n.insert(
        "rendezvous_exchange".to_string(),
        Value::from_builtin(
            "net.rendezvous_exchange",
            &["rendezvous_addr", "lobby_name", "my_addr", "timeout_ms"],
            net_rendezvous_exchange,
        ),
    );
    n.insert(
        "punch".to_string(),
        Value::from_builtin("net.punch", &["peer_addr"], net_punch),
    );
    // Phase 36 session 4: lobby primitives. Steam-feature path uses
    // Steam Matchmaking; no-feature path returns an informative
    // "rebuild with --features steam-net" error.
    n.insert(
        "create_lobby".to_string(),
        Value::from_builtin(
            "net.create_lobby",
            &["name", "max_peers"],
            net_create_lobby,
        ),
    );
    n.insert(
        "find_lobbies".to_string(),
        Value::from_builtin("net.find_lobbies", &["query"], net_find_lobbies),
    );
    n.insert(
        "join_lobby".to_string(),
        Value::from_builtin("net.join_lobby", &["lobby_id"], net_join_lobby),
    );
    n.insert(
        "leave_lobby".to_string(),
        Value::from_builtin("net.leave_lobby", &[], net_leave_lobby),
    );
    // Phase 36 session 5: reconnect handling.
    n.insert(
        "peer_disconnected".to_string(),
        Value::from_builtin("net.peer_disconnected", &[], net_peer_disconnected),
    );
    n.insert(
        "last_disconnected_peer".to_string(),
        Value::from_builtin(
            "net.last_disconnected_peer",
            &[],
            net_last_disconnected_peer,
        ),
    );
    n.insert(
        "try_reconnect".to_string(),
        Value::from_builtin(
            "net.try_reconnect",
            &["peer_id", "timeout_ms"],
            net_try_reconnect,
        ),
    );
    n.insert(
        "host_migrate_if_host_lost".to_string(),
        Value::from_builtin(
            "net.host_migrate_if_host_lost",
            &[],
            net_host_migrate_if_host_lost,
        ),
    );
    n.insert(
        "disconnect_timeout".to_string(),
        Value::from_builtin(
            "net.disconnect_timeout",
            &["seconds"],
            net_disconnect_timeout,
        ),
    );
    // Phase 37: rollback netcode mode switch.
    n.insert(
        "set_mode".to_string(),
        Value::from_builtin("net.set_mode", &["mode"], net_set_mode),
    );
    n.insert(
        "mode".to_string(),
        Value::from_builtin("net.mode", &[], net_mode),
    );
    env.set(
        "net".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: n,
            kind: "module",
        }))),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn net_host(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "net.host")?;
    let port = as_i64(&args[0], "net.host")? as u16;
    let n = as_i64(&args[1], "net.host")?;
    if !(2..=4).contains(&n) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("net.host: expected_peers must be 2..=4 (got {n})"),
            help: None,
        });
    }
    crate::net::host(port, n as u8).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn net_connect(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.connect")?;
    let addr = string_arg(&args[0], "net.connect", "addr")?;
    crate::net::connect(&addr).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn net_close(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.close")?;
    crate::net::close();
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn net_is_connected(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.is_connected")?;
    Ok(Value::from_bool(crate::net::is_connected()))
}

#[cfg(not(target_arch = "wasm32"))]
fn net_local_peer_id(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.local_peer_id")?;
    Ok(Value::from_int(crate::net::local_peer_id() as i64))
}

#[cfg(not(target_arch = "wasm32"))]
fn net_peer_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.peer_count")?;
    Ok(Value::from_int(crate::net::peer_count() as i64))
}

#[cfg(not(target_arch = "wasm32"))]
fn net_send_input(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.send_input")?;
    let tick = as_i64(&args[0], "net.send_input")? as u32;
    let frame = crate::net::snapshot_local(env);
    crate::net::send_input(tick, frame);
    crate::net::poll();
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn net_tick_ready(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.tick_ready")?;
    let tick = as_i64(&args[0], "net.tick_ready")? as u32;
    crate::net::poll();
    Ok(Value::from_bool(crate::net::tick_ready(tick)))
}

/// Pull the per-peer Frames for the requested tick and overwrite the
/// input ambients with the merged view (plus the per-peer `peer`
/// list). Scripts call this immediately before reading inputs in a
/// state's `on update(dt)`. Returns true when the tick was advanced;
/// false when not all peers had input yet (in which case the caller
/// should skip simulation this frame and try again next).
#[cfg(not(target_arch = "wasm32"))]
fn net_advance_tick(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.advance_tick")?;
    let tick = as_i64(&args[0], "net.advance_tick")? as u32;
    crate::net::poll();
    if let Some(frames) = crate::net::take_inputs(tick) {
        crate::net::apply_merged(env, &frames);
        Ok(Value::from_bool(true))
    } else {
        Ok(Value::from_bool(false))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn net_state_hash(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.state_hash")?;
    Ok(Value::from_int(crate::net::local_state_hash() as i64))
}

/// Broadcast a state hash for `tick`. The lockstep runner uses this
/// to detect divergence — if two peers report different hashes for
/// the same tick, the simulation has desynced and the game is now
/// unplayable. The script computes its own hash (typically a fold
/// over relevant entity positions); this builtin does the wire send
/// + cross-peer compare.
#[cfg(not(target_arch = "wasm32"))]
fn net_send_state_hash(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "net.send_state_hash")?;
    let tick = as_i64(&args[0], "net.send_state_hash")? as u32;
    let hash = as_i64(&args[1], "net.send_state_hash")? as u64;
    crate::net::send_state_hash(tick, hash);
    Ok(Value::NIL)
}

/// Input-delay configuration in ticks. Scripts that need a tighter
/// input-feel knob can read this constant and tune their UI text
/// (e.g. "Network: 4-tick delay"). The current value is fixed at
/// `DEFAULT_INPUT_DELAY` (4 ticks at 60Hz = 66ms); a runtime setter
/// is a follow-on session.
#[cfg(not(target_arch = "wasm32"))]
fn net_input_delay(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.input_delay")?;
    Ok(Value::from_int(crate::net::DEFAULT_INPUT_DELAY as i64))
}

/// True once every expected peer has joined the session. Scripts
/// poll this on the host before advancing past tick 0 — otherwise
/// late-joining clients would be locked out of the lockstep window.
#[cfg(not(target_arch = "wasm32"))]
fn net_session_ready(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.session_ready")?;
    crate::net::poll();
    Ok(Value::from_bool(crate::net::session_ready()))
}

/// Hash a Twe value to a u64 via canonical JSON + FNV-1a. Scripts
/// pass relevant game state (typically a list/tuple of positions,
/// scores, and RNG state) and use the result as the argument to
/// `net.send_state_hash(tick, hash)`. Cross-peer hash divergence
/// triggers a desync warning (see `[twec net] DESYNC` in stderr).
/// Errors if `value` contains anything outside the serializable
/// subset (functions, classes, builtins). The script-side workaround
/// is to fold the relevant scalars into a tuple before hashing.
#[cfg(not(target_arch = "wasm32"))]
fn net_hash(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.hash")?;
    let h = crate::net::hash_value(&args[0]).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: format!("net.hash: {m}"),
        help: None,
    })?;
    Ok(Value::from_int(h as i64))
}

/// Serialize a Twe value to canonical JSON (BTreeMap-sorted, no
/// whitespace). The debug counterpart to `net.hash` — scripts use
/// this to log full state when a desync is reported, so the bug
/// report carries a per-peer JSON snapshot the dev can diff.
#[cfg(not(target_arch = "wasm32"))]
fn net_snapshot_json(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.snapshot_json")?;
    let s = crate::net::snapshot_json(&args[0]).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: format!("net.snapshot_json: {m}"),
        help: None,
    })?;
    Ok(Value::from_string(s))
}

// ---------------------------------------------------------------
// Phase 36 session 2: Steam P2P transport builtins.
// ---------------------------------------------------------------

/// True when `--features steam-net` is compiled in AND the Steam
/// client is available. Scripts call this before `net.host_p2p` /
/// `net.connect_p2p` to decide whether to take the Steam path or
/// fall back to direct-IP / STUN.
#[cfg(not(target_arch = "wasm32"))]
fn net_steam_p2p_available(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.steam_p2p_available")?;
    Ok(Value::from_bool(crate::net_steam::is_available()))
}

/// Returns the local user's SteamID64 as an int. 0 if Steam is not
/// available. Used for the Steam-Friends-invite path: a host posts
/// their SteamID, a client calls `net.connect_p2p(that_id)`.
#[cfg(not(target_arch = "wasm32"))]
fn net_local_steam_id(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.local_steam_id")?;
    let id = crate::net_steam::local_steam_id();
    // SteamID64 fits in a u64 but Twe ints are i64. Steam IDs are in
    // the 7656119xxxxxxxxxx range which sits well below 2^63, so the
    // cast is safe.
    Ok(Value::from_int(id as i64))
}

/// Become host of a Steam P2P session with `expected_peers` total
/// peers (including self). Returns nil on success; raises on the
/// "Steam not available" path with an operator-actionable message.
#[cfg(not(target_arch = "wasm32"))]
fn net_host_p2p(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.host_p2p")?;
    let n = as_i64(&args[0], "net.host_p2p")?;
    if !(2..=4).contains(&n) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("net.host_p2p: expected_peers must be 2..=4 (got {n})"),
            help: None,
        });
    }
    crate::net_steam::host_p2p(n as u8).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    Ok(Value::NIL)
}

/// Connect to a Steam peer by SteamID64. The remote peer must be
/// hosting (called `net.host_p2p` or `net.create_lobby`).
#[cfg(not(target_arch = "wasm32"))]
fn net_connect_p2p(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.connect_p2p")?;
    let id = as_i64(&args[0], "net.connect_p2p")?;
    if id <= 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("net.connect_p2p: SteamID must be positive (got {id})"),
            help: None,
        });
    }
    crate::net_steam::connect_p2p(id as u64).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    Ok(Value::NIL)
}

// ---------------------------------------------------------------
// Phase 36 session 3: STUN + rendezvous builtins.
// ---------------------------------------------------------------

/// Discover the public-facing IP:port the local UDP play socket is
/// mapped to via a STUN binding request. Returns "ip:port" as a
/// string. The caller must already have called `net.host` so the
/// underlying socket exists — STUN reuses that socket so the NAT
/// mapping the response sees is the one the lockstep traffic will
/// later use.
///
/// Empty `stun_server` → uses `DEFAULT_STUN_SERVER` (Google's public
/// STUN at `stun.l.google.com:19302`).
#[cfg(not(target_arch = "wasm32"))]
fn net_discover_public_address(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.discover_public_address")?;
    let stun_server = string_arg(&args[0], "net.discover_public_address", "stun_server")?;
    let stun_server = if stun_server.is_empty() {
        crate::net_stun::DEFAULT_STUN_SERVER.to_string()
    } else {
        stun_server
    };
    let socket = crate::net::socket_clone().map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    let addr = crate::net_stun::discover_public_address(&socket, &stun_server).map_err(|m| {
        RuntimeError {
            line: 0,
            col: 0,
            message: m,
            help: None,
        }
    })?;
    Ok(Value::from_string(addr.to_string()))
}

/// Exchange public addresses with a peer through a TCP rendezvous
/// server. Send our address for `lobby_name`; block up to
/// `timeout_ms` for the rendezvous to pair us with another peer that
/// joined the same lobby. Returns the peer's "ip:port" as a string.
///
/// The lockstep runner integration (role assignment + lockstep
/// MSG_HELLO over the punched path) lands in Phase 36 session 4
/// alongside lobbies. This builtin is the non-Steam matchmaking
/// primitive scripts can compose with `net.host` / `net.connect`.
#[cfg(not(target_arch = "wasm32"))]
fn net_rendezvous_exchange(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "net.rendezvous_exchange")?;
    let rendezvous_addr = string_arg(&args[0], "net.rendezvous_exchange", "rendezvous_addr")?;
    let lobby_name = string_arg(&args[1], "net.rendezvous_exchange", "lobby_name")?;
    let my_addr_str = string_arg(&args[2], "net.rendezvous_exchange", "my_addr")?;
    let my_addr: std::net::SocketAddr = my_addr_str.parse().map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: format!("net.rendezvous_exchange: parse my_addr {my_addr_str:?}: {e}"),
        help: None,
    })?;
    let timeout_ms = as_i64(&args[3], "net.rendezvous_exchange")?;
    if timeout_ms <= 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "net.rendezvous_exchange: timeout_ms must be > 0 (got {timeout_ms})"
            ),
            help: None,
        });
    }
    let peer = crate::net_stun::rendezvous_exchange(
        &rendezvous_addr,
        &lobby_name,
        my_addr,
        timeout_ms as u64,
    )
    .map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    Ok(Value::from_string(peer.to_string()))
}

/// Send a few small UDP packets to `peer_addr` to install a NAT
/// return-path mapping. Used after a rendezvous exchange, before the
/// lockstep MSG_HELLO. The packets carry no game-relevant payload —
/// they're just there to wake up the local NAT.
#[cfg(not(target_arch = "wasm32"))]
fn net_punch(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.punch")?;
    let peer_addr_str = string_arg(&args[0], "net.punch", "peer_addr")?;
    let peer_addr: std::net::SocketAddr = peer_addr_str.parse().map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: format!("net.punch: parse peer_addr {peer_addr_str:?}: {e}"),
        help: None,
    })?;
    let socket = crate::net::socket_clone().map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    crate::net_stun::punch(&socket, peer_addr).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    Ok(Value::NIL)
}

// ---------------------------------------------------------------
// Phase 39 session 2 + 3: touch.* namespace — multi-touch input +
// virtual joystick widget.
//
// macroquad exposes `touches() -> Vec<Touch>` on every backend; on
// desktop the vec is empty (no touch hardware), on mobile + browser
// it contains active TouchPhase entries with `id` + `position`.
// The `touch.*` builtins wrap this — primary touch fields for the
// common single-finger case, `touch.pointer(i)` for multi-touch.
//
// `joystick(at:, size:, deadzone:)` is a virtual-joystick widget:
// scripts render a translucent stick somewhere on screen, the
// builtin returns a normalized 2D vector reflecting the user's
// touch position relative to the stick center (or {0, 0} if the
// stick isn't currently touched).
// ---------------------------------------------------------------

fn install_touch(env: &mut Env) {
    let mut t = HashMap::new();
    t.insert(
        "is_active".to_string(),
        Value::from_builtin("touch.is_active", &[], touch_is_active),
    );
    t.insert(
        "x".to_string(),
        Value::from_builtin("touch.x", &[], touch_x),
    );
    t.insert(
        "y".to_string(),
        Value::from_builtin("touch.y", &[], touch_y),
    );
    t.insert(
        "count".to_string(),
        Value::from_builtin("touch.count", &[], touch_count),
    );
    t.insert(
        "pointer".to_string(),
        Value::from_builtin("touch.pointer", &["i"], touch_pointer),
    );
    t.insert(
        "tap_count".to_string(),
        Value::from_builtin("touch.tap_count", &[], touch_tap_count),
    );
    env.set(
        "touch".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: t,
            kind: "module",
        }))),
    );
}

/// True iff at least one touch is currently active. On desktop
/// (no touch hardware) this always returns false.
fn touch_is_active(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "touch.is_active")?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let n = macroquad::input::touches().len();
        Ok(Value::from_bool(n > 0))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Ok(Value::from_bool(false))
    }
}

/// X-coordinate of the primary (first active) touch in screen
/// pixels. Returns 0.0 if no touch is active.
fn touch_x(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "touch.x")?;
    Ok(Value::from_float(touch_primary().map(|(x, _)| x).unwrap_or(0.0) as f64))
}

/// Y-coordinate of the primary (first active) touch.
fn touch_y(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "touch.y")?;
    Ok(Value::from_float(touch_primary().map(|(_, y)| y).unwrap_or(0.0) as f64))
}

/// Number of currently-active touches (1 for a single tap, 2 for
/// two-finger pinch, etc).
fn touch_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "touch.count")?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(Value::from_int(macroquad::input::touches().len() as i64))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Ok(Value::from_int(0))
    }
}

/// Returns `{x, y, id}` for the i-th active touch, or nil if i is
/// out of range. Touches are stable across frames by `id` (until
/// they're released).
fn touch_pointer(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "touch.pointer")?;
    let i = as_i64(&args[0], "touch.pointer")?;
    if i < 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("touch.pointer: index must be >= 0 (got {i})"),
            help: None,
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let tlist = macroquad::input::touches();
        if let Some(t) = tlist.get(i as usize) {
            let mut fields: HashMap<String, Value> = HashMap::new();
            fields.insert("x".to_string(), Value::from_float(t.position.x as f64));
            fields.insert("y".to_string(), Value::from_float(t.position.y as f64));
            fields.insert("id".to_string(), Value::from_int(t.id as i64));
            return Ok(Value::from_object(Rc::new(RefCell::new(Object {
                fields,
                kind: "touch",
            }))));
        }
    }
    Ok(Value::NIL)
}

/// Number of tap-release events in the last 500ms — a simple counter
/// for "did the user tap recently" without scripts having to track
/// touch state across frames. Today returns 0 always; the play loop
/// hooks for tap detection land in the Phase 39 mobile-runtime
/// follow-on session.
fn touch_tap_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "touch.tap_count")?;
    Ok(Value::from_int(0))
}

#[cfg(not(target_arch = "wasm32"))]
fn touch_primary() -> Option<(f32, f32)> {
    let tlist = macroquad::input::touches();
    tlist.first().map(|t| (t.position.x, t.position.y))
}

#[cfg(target_arch = "wasm32")]
fn touch_primary() -> Option<(f32, f32)> {
    None
}

// ---------------------------------------------------------------
// Phase 40 sessions 2 + 3: console.* abstract controller + glyphs.
//
// Per `docs/changes/2026-05-11-console-targets-rfc.md` the public
// surface ships platform-agnostic abstractions; SDK-specific
// implementations live in partner private forks. `console.controller(i)`
// wraps the gamepad ambient (Phase 9 / gilrs on PC + Steam Deck);
// partner forks replace the wiring per platform.
//
// Button names use the **Xbox layout as canonical** (a / b / x / y).
// `console.glyph(button, style)` returns the per-style glyph string
// for UI rendering.
// ---------------------------------------------------------------

fn install_console(env: &mut Env) {
    let mut c = HashMap::new();
    c.insert(
        "controller".to_string(),
        Value::from_builtin("console.controller", &["i"], console_controller),
    );
    c.insert(
        "controller_count".to_string(),
        Value::from_builtin("console.controller_count", &[], console_controller_count),
    );
    c.insert(
        "glyph".to_string(),
        Value::from_builtin("console.glyph", &["button", "style"], console_glyph),
    );
    c.insert(
        "glyph_asset".to_string(),
        Value::from_builtin(
            "console.glyph_asset",
            &["button", "style"],
            console_glyph_asset,
        ),
    );
    c.insert(
        "detect_style".to_string(),
        Value::from_builtin("console.detect_style", &[], console_detect_style),
    );
    env.set(
        "console".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: c,
            kind: "module",
        }))),
    );
}

/// Returns a controller record for the i-th connected gamepad.
/// `i = 0` reads from the existing `gamepad` / `gamepad_axis`
/// ambients (Phase 9 wires gilrs to controller 0). Higher indices
/// return `connected = false` today; multi-controller support is a
/// partner-fork extension per the Phase 40 RFC.
fn console_controller(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "console.controller")?;
    let i = as_i64(&args[0], "console.controller")?;
    if i < 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("console.controller: i must be >= 0 (got {i})"),
            help: None,
        });
    }
    let mut fields: HashMap<String, Value> = HashMap::new();
    if i == 0 {
        // Read from Phase 9's gamepad ambient.
        let (a, b, x, y, lb, rb, lt_button, rt_button, start, select, dup, ddown, dleft, dright,
             connected) = read_gamepad_buttons(env);
        let (lx, ly, rx, ry, lt_axis, rt_axis) = read_gamepad_axes(env);
        fields.insert("connected".to_string(), Value::from_bool(connected));
        fields.insert("a".to_string(), Value::from_bool(a));
        fields.insert("b".to_string(), Value::from_bool(b));
        fields.insert("x".to_string(), Value::from_bool(x));
        fields.insert("y".to_string(), Value::from_bool(y));
        fields.insert("left_shoulder".to_string(), Value::from_bool(lb));
        fields.insert("right_shoulder".to_string(), Value::from_bool(rb));
        fields.insert("left_trigger".to_string(), Value::from_float(lt_axis));
        fields.insert("right_trigger".to_string(), Value::from_float(rt_axis));
        // Boolean trigger forms thresholded at 0.5 — convenience for
        // scripts that just want "is the trigger pulled".
        fields.insert(
            "left_trigger_pressed".to_string(),
            Value::from_bool(lt_button || lt_axis > 0.5),
        );
        fields.insert(
            "right_trigger_pressed".to_string(),
            Value::from_bool(rt_button || rt_axis > 0.5),
        );
        fields.insert("dpad_up".to_string(), Value::from_bool(dup));
        fields.insert("dpad_down".to_string(), Value::from_bool(ddown));
        fields.insert("dpad_left".to_string(), Value::from_bool(dleft));
        fields.insert("dpad_right".to_string(), Value::from_bool(dright));
        fields.insert("start".to_string(), Value::from_bool(start));
        fields.insert("select".to_string(), Value::from_bool(select));
        // Sticks as nested records — scripts read `pad.left_stick.x`.
        let mut ls: HashMap<String, Value> = HashMap::new();
        ls.insert("x".to_string(), Value::from_float(lx));
        ls.insert("y".to_string(), Value::from_float(ly));
        fields.insert(
            "left_stick".to_string(),
            Value::from_object(Rc::new(RefCell::new(Object {
                fields: ls,
                kind: "stick",
            }))),
        );
        let mut rs: HashMap<String, Value> = HashMap::new();
        rs.insert("x".to_string(), Value::from_float(rx));
        rs.insert("y".to_string(), Value::from_float(ry));
        fields.insert(
            "right_stick".to_string(),
            Value::from_object(Rc::new(RefCell::new(Object {
                fields: rs,
                kind: "stick",
            }))),
        );
        // L3 / R3 / Home are honest-deferred until the partner fork
        // (or a follow-on gilrs upgrade) wires them. Today report
        // false so scripts reading them get a definite answer.
        fields.insert("left_stick_button".to_string(), Value::from_bool(false));
        fields.insert("right_stick_button".to_string(), Value::from_bool(false));
        fields.insert("home".to_string(), Value::from_bool(false));
    } else {
        // Higher indices: scaffolding only — partner forks wire
        // multi-pad gilrs (or platform-native input) here.
        fill_disconnected_controller(&mut fields);
    }
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "controller",
    }))))
}

fn fill_disconnected_controller(fields: &mut HashMap<String, Value>) {
    fields.insert("connected".to_string(), Value::from_bool(false));
    for name in [
        "a",
        "b",
        "x",
        "y",
        "left_shoulder",
        "right_shoulder",
        "left_trigger_pressed",
        "right_trigger_pressed",
        "dpad_up",
        "dpad_down",
        "dpad_left",
        "dpad_right",
        "start",
        "select",
        "left_stick_button",
        "right_stick_button",
        "home",
    ] {
        fields.insert(name.to_string(), Value::from_bool(false));
    }
    fields.insert("left_trigger".to_string(), Value::from_float(0.0));
    fields.insert("right_trigger".to_string(), Value::from_float(0.0));
    for stick_name in ["left_stick", "right_stick"] {
        let mut s: HashMap<String, Value> = HashMap::new();
        s.insert("x".to_string(), Value::from_float(0.0));
        s.insert("y".to_string(), Value::from_float(0.0));
        fields.insert(
            stick_name.to_string(),
            Value::from_object(Rc::new(RefCell::new(Object {
                fields: s,
                kind: "stick",
            }))),
        );
    }
}

/// Tuple of 14 button bools + connected flag, returned from
/// `read_gamepad_buttons`. Keyed positionally so the caller binds
/// fields by name; the type alias keeps clippy's complex-type lint
/// happy.
type GamepadButtonState = (
    bool, // a
    bool, // b
    bool, // x
    bool, // y
    bool, // lb
    bool, // rb
    bool, // lt (boolean threshold)
    bool, // rt (boolean threshold)
    bool, // start
    bool, // select
    bool, // dpad up
    bool, // dpad down
    bool, // dpad left
    bool, // dpad right
    bool, // connected
);

fn read_gamepad_buttons(env: &Env) -> GamepadButtonState {
    let opt = env.get("gamepad");
    let v = match opt.as_ref() {
        Some(v) if v.is_object() => *v,
        _ => {
            return (
                false, false, false, false, false, false, false, false, false, false, false,
                false, false, false, false,
            )
        }
    };
    let rc = v.as_object();
    let o = rc.borrow();
    let g = |k: &str| {
        o.fields
            .get(k)
            .filter(|v| v.is_bool())
            .map(|v| v.as_bool())
            .unwrap_or(false)
    };
    (
        g("a"),
        g("b"),
        g("x"),
        g("y"),
        g("lb"),
        g("rb"),
        g("lt"),
        g("rt"),
        g("start"),
        g("select"),
        g("dup"),
        g("ddown"),
        g("dleft"),
        g("dright"),
        g("connected"),
    )
}

fn read_gamepad_axes(env: &Env) -> (f64, f64, f64, f64, f64, f64) {
    let opt = env.get("gamepad_axis");
    let v = match opt.as_ref() {
        Some(v) if v.is_object() => *v,
        _ => return (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
    };
    let rc = v.as_object();
    let o = rc.borrow();
    let g = |k: &str| {
        o.fields
            .get(k)
            .and_then(|v| {
                if v.is_float() {
                    Some(v.as_float())
                } else if v.is_int_or_boxed_int() {
                    Some(v.as_int() as f64)
                } else {
                    None
                }
            })
            .unwrap_or(0.0)
    };
    (g("lx"), g("ly"), g("rx"), g("ry"), g("lt"), g("rt"))
}

fn console_controller_count(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "console.controller_count")?;
    // Today: 1 if controller 0 reports connected, 0 otherwise.
    // Partner forks override with real multi-pad enumeration.
    let opt = env.get("gamepad");
    let connected = match opt.as_ref() {
        Some(v) if v.is_object() => {
            let rc = v.as_object();
            let o = rc.borrow();
            o.fields
                .get("connected")
                .filter(|f| f.is_bool())
                .map(|f| f.as_bool())
                .unwrap_or(false)
        }
        _ => false,
    };
    Ok(Value::from_int(if connected { 1 } else { 0 }))
}

/// Per-style glyph string for a canonical (Xbox-named) button.
/// Empty string for unknown buttons.
fn console_glyph(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "console.glyph")?;
    let button = string_arg(&args[0], "console.glyph", "button")?;
    let style = string_arg(&args[1], "console.glyph", "style")?;
    let resolved_style = if style == "auto" {
        // Auto-detect from connected controller — today always falls
        // back to xbox since the gilrs path doesn't surface the
        // controller's vendor. Partner forks override this.
        "xbox".to_string()
    } else {
        style
    };
    Ok(Value::from_string(
        glyph_lookup(&button, &resolved_style).to_string(),
    ))
}

fn glyph_lookup(button: &str, style: &str) -> &'static str {
    match (style, button) {
        ("xbox", "a") => "(A)",
        ("xbox", "b") => "(B)",
        ("xbox", "x") => "(X)",
        ("xbox", "y") => "(Y)",
        ("xbox", "left_shoulder") => "[LB]",
        ("xbox", "right_shoulder") => "[RB]",
        ("xbox", "left_trigger") => "[LT]",
        ("xbox", "right_trigger") => "[RT]",
        ("xbox", "left_stick_button") => "[L3]",
        ("xbox", "right_stick_button") => "[R3]",
        ("xbox", "start") => "[Menu]",
        ("xbox", "select") => "[View]",
        ("playstation", "a") => "✕",
        ("playstation", "b") => "◯",
        ("playstation", "x") => "□",
        ("playstation", "y") => "△",
        ("playstation", "left_shoulder") => "[L1]",
        ("playstation", "right_shoulder") => "[R1]",
        ("playstation", "left_trigger") => "[L2]",
        ("playstation", "right_trigger") => "[R2]",
        ("playstation", "left_stick_button") => "[L3]",
        ("playstation", "right_stick_button") => "[R3]",
        ("playstation", "start") => "[Options]",
        ("playstation", "select") => "[Share]",
        ("switch", "a") => "(A)",
        ("switch", "b") => "(B)",
        ("switch", "x") => "(X)",
        ("switch", "y") => "(Y)",
        ("switch", "left_shoulder") => "[L]",
        ("switch", "right_shoulder") => "[R]",
        ("switch", "left_trigger") => "[ZL]",
        ("switch", "right_trigger") => "[ZR]",
        ("switch", "left_stick_button") => "[LS]",
        ("switch", "right_stick_button") => "[RS]",
        ("switch", "start") => "[+]",
        ("switch", "select") => "[-]",
        _ => "",
    }
}

/// Asset key for a glyph sprite. Returns a key like
/// `"glyph/xbox/a.png"` that scripts pass to `image()`. The asset
/// itself ships with partner forks (signed glyphs from the platform
/// SDK); the open-source repo does not bundle the platform-owned
/// glyphs. Returns empty string when no asset is available.
fn console_glyph_asset(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "console.glyph_asset")?;
    let button = string_arg(&args[0], "console.glyph_asset", "button")?;
    let style = string_arg(&args[1], "console.glyph_asset", "style")?;
    if button.is_empty() || style.is_empty() {
        return Ok(Value::from_string(String::new()));
    }
    Ok(Value::from_string(format!("glyph/{style}/{button}.png")))
}

/// Returns the detected glyph style for the connected controller.
/// Today always returns `"xbox"` (matches the gilrs PC path); partner
/// forks override per-platform.
fn console_detect_style(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "console.detect_style")?;
    Ok(Value::from_string("xbox".to_string()))
}

// ---------------------------------------------------------------
// Phase 40 session 4: platform-service traits.
//
// `achievements.unlock` / `cloud_save.save` / `friends.list` — trait
// stubs that route through `crate::steam::*` on Steam builds, no-op
// on every other build. Partner forks provide platform-specific
// implementations behind feature flags (analogous to `--features
// steam` / `--features steam-net`).
// ---------------------------------------------------------------

fn install_platform_services(env: &mut Env) {
    let mut a = HashMap::new();
    a.insert(
        "unlock".to_string(),
        Value::from_builtin("achievements.unlock", &["id"], achievements_unlock),
    );
    a.insert(
        "is_unlocked".to_string(),
        Value::from_builtin(
            "achievements.is_unlocked",
            &["id"],
            achievements_is_unlocked,
        ),
    );
    env.set(
        "achievements".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: a,
            kind: "module",
        }))),
    );

    let mut c = HashMap::new();
    c.insert(
        "save".to_string(),
        Value::from_builtin("cloud_save.save", &["slot", "value"], cloud_save_save),
    );
    c.insert(
        "load".to_string(),
        Value::from_builtin("cloud_save.load", &["slot"], cloud_save_load),
    );
    env.set(
        "cloud_save".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: c,
            kind: "module",
        }))),
    );

    let mut f = HashMap::new();
    f.insert(
        "list".to_string(),
        Value::from_builtin("friends.list", &[], friends_list),
    );
    f.insert(
        "is_friend".to_string(),
        Value::from_builtin("friends.is_friend", &["id"], friends_is_friend),
    );
    env.set(
        "friends".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: f,
            kind: "module",
        }))),
    );
}

fn achievements_unlock(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "achievements.unlock")?;
    // Route through the Phase 15 Steam achievement path. On non-Steam
    // builds this is a no-op (the Steam stub returns nil). Partner
    // forks add platform-specific routes alongside.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = crate::steam::achievement_unlock(env, args);
    }
    let _ = env;
    Ok(Value::NIL)
}

fn achievements_is_unlocked(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "achievements.is_unlocked")?;
    // No-op for the open-source repo. Partner forks query the
    // platform-specific achievement state.
    Ok(Value::from_bool(false))
}

fn cloud_save_save(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "cloud_save.save")?;
    let slot = string_arg(&args[0], "cloud_save.save", "slot")?;
    let payload = args[1].display();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = crate::steam::cloud_save(
            env,
            &[
                Value::from_string(slot),
                Value::from_string(payload),
            ],
        );
    }
    let _ = env;
    Ok(Value::NIL)
}

fn cloud_save_load(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "cloud_save.load")?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::steam::cloud_load(env, args)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = env;
        let _ = args;
        Ok(Value::NIL)
    }
}

fn friends_list(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "friends.list")?;
    // Empty list on the open-source repo. Partner forks return the
    // platform-specific friend list (Steam Friends, PSN, etc).
    Ok(Value::from_list(Rc::new(RefCell::new(Vec::new()))))
}

fn friends_is_friend(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "friends.is_friend")?;
    Ok(Value::from_bool(false))
}

// ---------------------------------------------------------------
// Phase 41: mmo.* + workshop.* — MMO architecture stubs.
//
// Per `docs/changes/2026-05-11-mmo-rfc.md` the runtime is honest-
// deferred to a future-implementer-with-bandwidth opening the phase
// properly. This module ships the *author-facing API* that compiles
// + runs today as single-player no-ops, so scripts written against
// the contract keep working when (if) a server runtime appears.
//
// Naming convention:
//   - `mmo.replicate(name, value)` — declare replicated state.
//     Today no-op locally; future runtime broadcasts to peers.
//   - `mmo.persist(key, value)` / `mmo.load(key)` — server-side DB
//     stub. Today saves to a thread-local Map; future runtime
//     routes to SQL / Redis.
//   - `mmo.broadcast(channel, payload)` + `mmo.next_event()` —
//     event queue. Today scripts observe their own broadcasts;
//     future runtime routes to peers in the same shard.
//   - `mmo.entities_near(x, y, z, r)` — AOI query. Composes with
//     `world.spatial_query_radius` directly; the server-side
//     version filters by what the player can see.
//   - `mmo.shard_id()` / `mmo.transfer_to(shard)` — sharding
//     lifecycle. Today returns "default"; future runtime returns
//     the active zone name and handles cross-shard handoff.
// ---------------------------------------------------------------

thread_local! {
    /// Persistent-world database stub. Future runtime replaces this
    /// with a SQL / Redis route. (Non-const init because
    /// `HashMap::new()` with the default RandomState hasher isn't
    /// const-callable.)
    #[allow(clippy::missing_const_for_thread_local)]
    static MMO_DB: RefCell<HashMap<String, crate::json::Value>> =
        RefCell::new(HashMap::new());
    /// Event queue stub. `mmo.broadcast` pushes onto this; the local
    /// script's `mmo.next_event` drains it. Future runtime routes
    /// to peers in the same shard.
    static MMO_EVENTS: RefCell<std::collections::VecDeque<(String, String, String)>> =
        const { RefCell::new(std::collections::VecDeque::new()) };
    /// Active shard id. Future runtime sets this on handoff. Stored
    /// behind a `RefCell<Option<String>>` so the init is const; `None`
    /// is the default-shard sentinel, surfaced as `"default"`.
    static MMO_SHARD_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn install_mmo(env: &mut Env) {
    let mut m = HashMap::new();
    m.insert(
        "replicate".to_string(),
        Value::from_builtin("mmo.replicate", &["name", "value"], mmo_replicate),
    );
    m.insert(
        "persist".to_string(),
        Value::from_builtin("mmo.persist", &["key", "value"], mmo_persist),
    );
    m.insert(
        "load".to_string(),
        Value::from_builtin("mmo.load", &["key"], mmo_load),
    );
    m.insert(
        "broadcast".to_string(),
        Value::from_builtin(
            "mmo.broadcast",
            &["channel", "payload"],
            mmo_broadcast,
        ),
    );
    m.insert(
        "next_event".to_string(),
        Value::from_builtin("mmo.next_event", &[], mmo_next_event),
    );
    m.insert(
        "entities_near".to_string(),
        Value::from_builtin(
            "mmo.entities_near",
            &["x", "y", "z", "radius"],
            mmo_entities_near,
        ),
    );
    m.insert(
        "shard_id".to_string(),
        Value::from_builtin("mmo.shard_id", &[], mmo_shard_id),
    );
    m.insert(
        "transfer_to".to_string(),
        Value::from_builtin("mmo.transfer_to", &["shard"], mmo_transfer_to),
    );
    env.set(
        "mmo".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: m,
            kind: "module",
        }))),
    );
}

/// Declare a replicated state slot. Today no-op — the value is
/// already local. Future runtime broadcasts the change to peers in
/// the same shard within the player's AOI.
fn mmo_replicate(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mmo.replicate")?;
    let _name = string_arg(&args[0], "mmo.replicate", "name")?;
    // _value = args[1] — passed through unchanged today.
    Ok(Value::NIL)
}

/// Persist a value to the server-side DB. Today saves to a thread-
/// local map. Future runtime flushes through a snapshot ring buffer
/// to SQL / Redis.
fn mmo_persist(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mmo.persist")?;
    let key = string_arg(&args[0], "mmo.persist", "key")?;
    let json = crate::save::encode(&args[1]).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: format!("mmo.persist: {m}"),
        help: None,
    })?;
    MMO_DB.with(|db| {
        db.borrow_mut().insert(key, json);
    });
    Ok(Value::NIL)
}

/// Read a previously-persisted value, or nil if absent.
fn mmo_load(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mmo.load")?;
    let key = string_arg(&args[0], "mmo.load", "key")?;
    let v = MMO_DB.with(|db| db.borrow().get(&key).cloned());
    Ok(v.map(|j| crate::save::decode(&j)).unwrap_or(Value::NIL))
}

/// Broadcast a one-shot event to peers in the same shard. Today the
/// event lands on the local event queue (so the local script can
/// observe its own broadcasts). Future runtime delivers to other
/// peers in the same AOI.
fn mmo_broadcast(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mmo.broadcast")?;
    let channel = string_arg(&args[0], "mmo.broadcast", "channel")?;
    let payload = args[1].display();
    // Sender id is "local" today; future runtime fills in the real
    // SteamID / SessionID of the sending peer.
    MMO_EVENTS.with(|q| {
        q.borrow_mut().push_back(("local".to_string(), channel, payload));
    });
    Ok(Value::NIL)
}

/// Drain one event from the queue. Returns `{sender_id, channel,
/// payload}` or nil if empty.
fn mmo_next_event(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "mmo.next_event")?;
    let evt = MMO_EVENTS.with(|q| q.borrow_mut().pop_front());
    match evt {
        Some((sender, channel, payload)) => {
            let mut fields: HashMap<String, Value> = HashMap::new();
            fields.insert("sender_id".to_string(), Value::from_string(sender));
            fields.insert("channel".to_string(), Value::from_string(channel));
            fields.insert("payload".to_string(), Value::from_string(payload));
            Ok(Value::from_object(Rc::new(RefCell::new(Object {
                fields,
                kind: "mmo_event",
            }))))
        }
        None => Ok(Value::NIL),
    }
}

/// Area-of-interest query: which entities are near `(x, y, z)`
/// within `radius`? Composes with Phase 32's
/// `world.spatial_query_radius`. Today returns the same result as
/// the underlying spatial query; future runtime additionally
/// filters by what the player is allowed to see (visibility, friend
/// list, party membership).
fn mmo_entities_near(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "mmo.entities_near")?;
    let x = as_f64(&args[0], "mmo.entities_near")?;
    let y = as_f64(&args[1], "mmo.entities_near")?;
    let z = as_f64(&args[2], "mmo.entities_near")?;
    let r = as_f64(&args[3], "mmo.entities_near")?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ids = crate::spatial::with_world(|w| {
            w.query_radius(x as f32, y as f32, z as f32, r as f32)
        });
        let items: Vec<Value> = ids
            .into_iter()
            .map(|id| Value::from_int(id as i64))
            .collect();
        Ok(Value::from_list(Rc::new(RefCell::new(items))))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (x, y, z, r);
        Ok(Value::from_list(Rc::new(RefCell::new(Vec::new()))))
    }
}

/// Active shard id. Today always `"default"`; future runtime sets
/// this on shard handoff.
fn mmo_shard_id(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "mmo.shard_id")?;
    let id = MMO_SHARD_ID.with(|s| {
        s.borrow()
            .clone()
            .unwrap_or_else(|| "default".to_string())
    });
    Ok(Value::from_string(id))
}

/// Request a transfer to `shard`. Today sets the local shard id
/// without any network coordination — useful for prototyping multi-
/// zone games as a single-player simulation. Future runtime
/// orchestrates the cross-shard handoff (serialise player state on
/// source, deserialise on destination, loading-screen UX).
fn mmo_transfer_to(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mmo.transfer_to")?;
    let shard = string_arg(&args[0], "mmo.transfer_to", "shard")?;
    MMO_SHARD_ID.with(|s| *s.borrow_mut() = Some(shard));
    Ok(Value::NIL)
}

// ---------------------------------------------------------------
// Phase 41 session 7: workshop.* — user-generated-content traits.
//
// Trait stubs for a Steam Workshop-style publishing pipeline. The
// Steam path could route through `steamworks::UGC` on
// `--features steam-workshop` (a follow-on feature flag); the open-
// source repo ships the contract + no-op fallbacks.
// ---------------------------------------------------------------

fn install_workshop(env: &mut Env) {
    let mut w = HashMap::new();
    w.insert(
        "publish".to_string(),
        Value::from_builtin(
            "workshop.publish",
            &["title", "content_path"],
            workshop_publish,
        ),
    );
    w.insert(
        "list_subscribed".to_string(),
        Value::from_builtin(
            "workshop.list_subscribed",
            &[],
            workshop_list_subscribed,
        ),
    );
    w.insert(
        "install".to_string(),
        Value::from_builtin("workshop.install", &["id"], workshop_install),
    );
    env.set(
        "workshop".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: w,
            kind: "module",
        }))),
    );
}

fn workshop_publish(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "workshop.publish")?;
    // No-op on the open-source repo. Steam-feature route + future
    // server runtime fill in the real publish call.
    Ok(Value::NIL)
}

fn workshop_list_subscribed(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "workshop.list_subscribed")?;
    Ok(Value::from_list(Rc::new(RefCell::new(Vec::new()))))
}

fn workshop_install(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "workshop.install")?;
    // Returns false today — no actual workshop integration.
    Ok(Value::from_bool(false))
}

// ---------------------------------------------------------------
// v1.0.1 session 1: `fx.*` — procedural VFX library.
//
// Twelve call-and-go effects. Each impl reads its args, builds an
// `fx::ActiveFx` (visual) or schedules a hit-stop (gameplay), and
// returns NIL. The play loop calls `fx::fx_tick(frame_dt)` to decay
// animations and `fx::fx_draw_overlay()` to render them.
//
// Determinism: `fx.screen_shake` writes to `CAMERA_SHAKE` (visual,
// wall-clock). `fx.hit_stop` is the one gameplay-visible primitive
// and counts in physics ticks — `eval::PHYSICS_DT` — so replay
// determinism is preserved regardless of host FPS.
// ---------------------------------------------------------------

fn install_fx(env: &mut Env) {
    let mut f = HashMap::new();
    f.insert(
        "hit_flash".to_string(),
        Value::from_builtin(
            "fx.hit_flash",
            &["at", "size", "color", "duration"],
            fx_hit_flash,
        ),
    );
    f.insert(
        "screen_shake".to_string(),
        Value::from_builtin(
            "fx.screen_shake",
            &["amount", "duration"],
            fx_screen_shake,
        ),
    );
    f.insert(
        "hit_stop".to_string(),
        Value::from_builtin("fx.hit_stop", &["duration"], fx_hit_stop),
    );
    f.insert(
        "damage_number".to_string(),
        Value::from_builtin(
            "fx.damage_number",
            &["at", "value", "color"],
            fx_damage_number,
        ),
    );
    f.insert(
        "crit_text".to_string(),
        Value::from_builtin("fx.crit_text", &["at", "value"], fx_crit_text),
    );
    f.insert(
        "death_burst".to_string(),
        Value::from_builtin(
            "fx.death_burst",
            &["at", "count", "color"],
            fx_death_burst,
        ),
    );
    f.insert(
        "pickup_pop".to_string(),
        Value::from_builtin("fx.pickup_pop", &["at", "color"], fx_pickup_pop),
    );
    f.insert(
        "dash_trail".to_string(),
        Value::from_builtin("fx.dash_trail", &["at", "color"], fx_dash_trail),
    );
    f.insert(
        "level_up_ring".to_string(),
        Value::from_builtin("fx.level_up_ring", &["at", "color"], fx_level_up_ring),
    );
    f.insert(
        "blood_splat".to_string(),
        Value::from_builtin(
            "fx.blood_splat",
            &["at", "dir", "color"],
            fx_blood_splat,
        ),
    );
    f.insert(
        "muzzle_flash".to_string(),
        Value::from_builtin("fx.muzzle_flash", &["at", "dir"], fx_muzzle_flash),
    );
    f.insert(
        "ground_shockwave".to_string(),
        Value::from_builtin(
            "fx.ground_shockwave",
            &["at", "radius"],
            fx_ground_shockwave,
        ),
    );
    env.set(
        "fx".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: f,
            kind: "module",
        }))),
    );
}

fn fx_hit_flash(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "fx.hit_flash")?;
    let (x, y) = xy_of(&args[0], "fx.hit_flash.at")?;
    let (w, h) = xy_of(&args[1], "fx.hit_flash.size")?;
    let color = color_of(&args[2], "fx.hit_flash.color")?;
    let duration = number(&args[3], "fx.hit_flash.duration")?;
    if duration <= 0.0 {
        return Ok(Value::NIL);
    }
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::HitFlash {
            color,
            width: w as f32,
            height: h as f32,
        },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: duration as f32,
    });
    Ok(Value::NIL)
}

fn fx_screen_shake(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "fx.screen_shake")?;
    let amp = number(&args[0], "fx.screen_shake.amount")?;
    let dur = number(&args[1], "fx.screen_shake.duration")?;
    if amp < 0.0 || dur < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "fx.screen_shake: amount and duration must be non-negative".to_string(),
            help: None,
        });
    }
    // P2 (one obvious way): route through the existing CAMERA_SHAKE
    // thread_local so `fx.screen_shake` and the older `camera.shake`
    // share state — last-write-with-max-amp / max-duration wins.
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

fn fx_hit_stop(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "fx.hit_stop")?;
    let dur = number(&args[0], "fx.hit_stop.duration")?;
    if dur <= 0.0 {
        return Ok(Value::NIL);
    }
    crate::fx::schedule_hit_stop(dur, crate::eval::PHYSICS_DT);
    Ok(Value::NIL)
}

fn fx_damage_number(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "fx.damage_number")?;
    let (x, y) = xy_of(&args[0], "fx.damage_number.at")?;
    let value = number(&args[1], "fx.damage_number.value")?;
    let color = color_of(&args[2], "fx.damage_number.color")?;
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::DamageNumber { value, color },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 0.8,
    });
    Ok(Value::NIL)
}

fn fx_crit_text(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "fx.crit_text")?;
    let (x, y) = xy_of(&args[0], "fx.crit_text.at")?;
    let value = number(&args[1], "fx.crit_text.value")?;
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::CritText { value },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 1.0,
    });
    Ok(Value::NIL)
}

fn fx_death_burst(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "fx.death_burst")?;
    let (x, y) = xy_of(&args[0], "fx.death_burst.at")?;
    let count = number(&args[1], "fx.death_burst.count")?.max(0.0) as usize;
    let color = color_of(&args[2], "fx.death_burst.color")?;
    // Deterministic particle pattern (no RNG) — each particle is
    // a point on a circle, given an outward velocity. Replay-safe
    // even though render decay is wall-clock, because the spawn
    // pattern is fixed for a given `count`.
    let count = count.min(64);
    let mut particles = Vec::with_capacity(count);
    let n = count.max(1) as f32;
    for i in 0..count {
        let theta = (i as f32 / n) * std::f32::consts::TAU;
        let (s, c) = theta.sin_cos();
        let speed = 90.0;
        particles.push((0.0, 0.0, c * speed, s * speed));
    }
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::Burst { color, particles },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 0.7,
    });
    Ok(Value::NIL)
}

fn fx_pickup_pop(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "fx.pickup_pop")?;
    let (x, y) = xy_of(&args[0], "fx.pickup_pop.at")?;
    let color = color_of(&args[1], "fx.pickup_pop.color")?;
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::Pop { color },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 0.35,
    });
    Ok(Value::NIL)
}

fn fx_dash_trail(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "fx.dash_trail")?;
    let (x, y) = xy_of(&args[0], "fx.dash_trail.at")?;
    let color = color_of(&args[1], "fx.dash_trail.color")?;
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::Trail { color },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 0.4,
    });
    Ok(Value::NIL)
}

fn fx_level_up_ring(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "fx.level_up_ring")?;
    let (x, y) = xy_of(&args[0], "fx.level_up_ring.at")?;
    let color = color_of(&args[1], "fx.level_up_ring.color")?;
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::Ring {
            color,
            radius_from: 8.0,
            radius_to: 64.0,
        },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 0.6,
    });
    Ok(Value::NIL)
}

fn fx_blood_splat(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "fx.blood_splat")?;
    let (x, y) = xy_of(&args[0], "fx.blood_splat.at")?;
    let (dx, dy) = xy_of(&args[1], "fx.blood_splat.dir")?;
    let color = color_of(&args[2], "fx.blood_splat.color")?;
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::Splat {
            dir: (dx as f32, dy as f32),
            color,
        },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 0.5,
    });
    Ok(Value::NIL)
}

fn fx_muzzle_flash(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "fx.muzzle_flash")?;
    let (x, y) = xy_of(&args[0], "fx.muzzle_flash.at")?;
    let (dx, dy) = xy_of(&args[1], "fx.muzzle_flash.dir")?;
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::MuzzleFlash {
            dir: (dx as f32, dy as f32),
        },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 0.08,
    });
    Ok(Value::NIL)
}

fn fx_ground_shockwave(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "fx.ground_shockwave")?;
    let (x, y) = xy_of(&args[0], "fx.ground_shockwave.at")?;
    let radius = number(&args[1], "fx.ground_shockwave.radius")?.max(0.0);
    crate::fx::spawn(crate::fx::ActiveFx {
        kind: crate::fx::FxKind::Shockwave {
            radius: radius as f32,
        },
        pos: (x as f32, y as f32),
        age: 0.0,
        lifetime: 0.5,
    });
    Ok(Value::NIL)
}

// ---------------------------------------------------------------
// v1.0.1 session 2: `tween.*` — deterministic easing primitives.
//
// Six pure functions implemented in `crate::tween`. Each wrapper
// parses its args, calls the pure function, returns the result.
// No state, no thread_local — replay-safe by construction.
// ---------------------------------------------------------------

fn install_tween(env: &mut Env) {
    let mut t = HashMap::new();
    t.insert(
        "ease".to_string(),
        Value::from_builtin("tween.ease", &["name", "t"], tween_ease),
    );
    t.insert(
        "lerp".to_string(),
        Value::from_builtin("tween.lerp", &["a", "b", "t"], tween_lerp),
    );
    t.insert(
        "lerp_eased".to_string(),
        Value::from_builtin(
            "tween.lerp_eased",
            &["a", "b", "t", "ease"],
            tween_lerp_eased,
        ),
    );
    t.insert(
        "bounce".to_string(),
        Value::from_builtin("tween.bounce", &["a", "b", "t"], tween_bounce),
    );
    t.insert(
        "shake".to_string(),
        Value::from_builtin("tween.shake", &["seed", "t", "freq"], tween_shake),
    );
    t.insert(
        "eases".to_string(),
        Value::from_builtin("tween.eases", &[], tween_eases),
    );
    env.set(
        "tween".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: t,
            kind: "module",
        }))),
    );
}

fn tween_ease(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "tween.ease")?;
    let name = string_arg(&args[0], "tween.ease", "name")?;
    let t = number(&args[1], "tween.ease.t")?;
    match crate::tween::ease(&name, t) {
        Some(v) => Ok(Value::from_float(v)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("tween.ease: unknown ease '{name}'"),
            help: Some(format!(
                "supported eases: {}",
                crate::tween::EASE_NAMES.join(", ")
            )),
        }),
    }
}

fn tween_lerp(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "tween.lerp")?;
    let a = number(&args[0], "tween.lerp.a")?;
    let b = number(&args[1], "tween.lerp.b")?;
    let t = number(&args[2], "tween.lerp.t")?;
    Ok(Value::from_float(crate::tween::lerp(a, b, t)))
}

fn tween_lerp_eased(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "tween.lerp_eased")?;
    let a = number(&args[0], "tween.lerp_eased.a")?;
    let b = number(&args[1], "tween.lerp_eased.b")?;
    let t = number(&args[2], "tween.lerp_eased.t")?;
    let name = string_arg(&args[3], "tween.lerp_eased", "ease")?;
    match crate::tween::lerp_eased(a, b, t, &name) {
        Some(v) => Ok(Value::from_float(v)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("tween.lerp_eased: unknown ease '{name}'"),
            help: Some(format!(
                "supported eases: {}",
                crate::tween::EASE_NAMES.join(", ")
            )),
        }),
    }
}

fn tween_bounce(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "tween.bounce")?;
    let a = number(&args[0], "tween.bounce.a")?;
    let b = number(&args[1], "tween.bounce.b")?;
    let t = number(&args[2], "tween.bounce.t")?;
    Ok(Value::from_float(crate::tween::bounce_value(a, b, t)))
}

fn tween_shake(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "tween.shake")?;
    let seed = number(&args[0], "tween.shake.seed")?;
    let t = number(&args[1], "tween.shake.t")?;
    let freq = number(&args[2], "tween.shake.freq")?;
    Ok(Value::from_float(crate::tween::shake(seed, t, freq)))
}

fn tween_eases(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "tween.eases")?;
    let items: Vec<Value> = crate::tween::EASE_NAMES
        .iter()
        .map(|s| Value::from_string((*s).to_string()))
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(items))))
}

// ---------------------------------------------------------------
// Phase 39 session 3: virtual joystick widget.
//
// `joystick(at: (cx, cy), size: r, deadzone: d)` returns a record
// `{x, y, active, magnitude}` where (x, y) is the normalized 2D
// direction the user's touch makes relative to the stick center,
// `active` is true when the stick is being touched, and `magnitude`
// is the touch-distance / (size - deadzone) clamped to [0, 1].
//
// Scripts compose the widget with whatever rendering they want —
// the builtin does no drawing of its own. Reference render shapes
// are in `examples/survive_beta_mobile/main.twe`.
// ---------------------------------------------------------------

fn install_joystick_widget(env: &mut Env) {
    env.set(
        "joystick".to_string(),
        Value::from_builtin(
            "joystick",
            &["at", "size", "deadzone"],
            joystick_builtin,
        ),
    );
}

fn joystick_builtin(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "joystick")?;
    let (cx, cy) = tuple2_f64(&args[0], "joystick", "at")?;
    let size = as_f64(&args[1], "joystick")?;
    let deadzone = as_f64(&args[2], "joystick")?;
    if size <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("joystick: size must be > 0 (got {size})"),
            help: None,
        });
    }
    if deadzone < 0.0 || deadzone >= size {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "joystick: deadzone must be 0 <= d < size (got d={deadzone}, size={size})"
            ),
            help: None,
        });
    }
    // Find the touch closest to the stick center. Multi-touch
    // games can have multiple sticks; each call to `joystick`
    // picks the closest active touch within `size` of its center.
    let mut nearest: Option<(f64, f64, f64)> = None; // (dist, dx, dy)
    #[cfg(not(target_arch = "wasm32"))]
    {
        for t in macroquad::input::touches() {
            let dx = t.position.x as f64 - cx;
            let dy = t.position.y as f64 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= size {
                match nearest {
                    None => nearest = Some((d, dx, dy)),
                    Some((nd, _, _)) if d < nd => nearest = Some((d, dx, dy)),
                    _ => {}
                }
            }
        }
    }
    let mut fields: HashMap<String, Value> = HashMap::new();
    if let Some((d, dx, dy)) = nearest {
        if d <= deadzone {
            // Inside the deadzone — report active but zero direction.
            fields.insert("x".to_string(), Value::from_float(0.0));
            fields.insert("y".to_string(), Value::from_float(0.0));
            fields.insert("active".to_string(), Value::from_bool(true));
            fields.insert("magnitude".to_string(), Value::from_float(0.0));
        } else {
            // Normalize direction; scale magnitude over the
            // (deadzone, size] band.
            let len = d.max(1e-6);
            let nx = dx / len;
            let ny = dy / len;
            let mag = ((d - deadzone) / (size - deadzone)).clamp(0.0, 1.0);
            fields.insert("x".to_string(), Value::from_float(nx));
            fields.insert("y".to_string(), Value::from_float(ny));
            fields.insert("active".to_string(), Value::from_bool(true));
            fields.insert("magnitude".to_string(), Value::from_float(mag));
        }
    } else {
        fields.insert("x".to_string(), Value::from_float(0.0));
        fields.insert("y".to_string(), Value::from_float(0.0));
        fields.insert("active".to_string(), Value::from_bool(false));
        fields.insert("magnitude".to_string(), Value::from_float(0.0));
    }
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "joystick",
    }))))
}

// ---------------------------------------------------------------
// Phase 39 session 6: safe-area inset builtins.
//
// Mobile devices have hardware that intrudes on the rectangular
// screen — iPhone X-style camera notches, dynamic-island cutouts,
// Android punch-holes, rounded display corners, the system gesture
// bar at the bottom. Games that draw UI flush to the screen edge
// look broken on these devices. The safe area is the rectangle
// guaranteed to be clear of hardware overlap.
//
// On desktop the safe area equals the full window — every inset is
// 0. On iOS / Android the runtime queries the platform safe-area
// API (UIView's safeAreaInsets on iOS; WindowInsets.systemBars on
// Android) and feeds the values into a thread-local; scripts read
// them via `safe_area.top()` etc.
//
// **Honest scaffolding:** the per-platform setter is wired but the
// platform-side query lives in the mobile-runtime follow-on session.
// Today every getter returns 0.0; scripts written against these
// builtins keep working unchanged once the mobile runtime lands.
// ---------------------------------------------------------------

fn install_safe_area(env: &mut Env) {
    let mut sa = HashMap::new();
    sa.insert(
        "top".to_string(),
        Value::from_builtin("safe_area.top", &[], safe_area_top),
    );
    sa.insert(
        "bottom".to_string(),
        Value::from_builtin("safe_area.bottom", &[], safe_area_bottom),
    );
    sa.insert(
        "left".to_string(),
        Value::from_builtin("safe_area.left", &[], safe_area_left),
    );
    sa.insert(
        "right".to_string(),
        Value::from_builtin("safe_area.right", &[], safe_area_right),
    );
    sa.insert(
        "rect".to_string(),
        Value::from_builtin("safe_area.rect", &[], safe_area_rect),
    );
    env.set(
        "safe_area".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: sa,
            kind: "module",
        }))),
    );
}

thread_local! {
    /// (top, bottom, left, right) inset in pixels. Default zero on
    /// every platform; the mobile runtime overrides via
    /// `set_safe_area_insets`.
    static SAFE_AREA_INSETS: std::cell::Cell<(f64, f64, f64, f64)> =
        const { std::cell::Cell::new((0.0, 0.0, 0.0, 0.0)) };
}

/// Set the safe-area insets. Called by the platform runtime hook
/// (iOS UIView.safeAreaInsets observer, Android WindowInsets
/// listener). Today only the test path exercises this; the live
/// platform hooks land in the Phase 39 mobile-runtime follow-on.
pub fn set_safe_area_insets(top: f64, bottom: f64, left: f64, right: f64) {
    SAFE_AREA_INSETS.with(|c| c.set((top, bottom, left, right)));
}

fn safe_area_top(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "safe_area.top")?;
    Ok(Value::from_float(SAFE_AREA_INSETS.with(|c| c.get().0)))
}

fn safe_area_bottom(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "safe_area.bottom")?;
    Ok(Value::from_float(SAFE_AREA_INSETS.with(|c| c.get().1)))
}

fn safe_area_left(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "safe_area.left")?;
    Ok(Value::from_float(SAFE_AREA_INSETS.with(|c| c.get().2)))
}

fn safe_area_right(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "safe_area.right")?;
    Ok(Value::from_float(SAFE_AREA_INSETS.with(|c| c.get().3)))
}

fn safe_area_rect(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "safe_area.rect")?;
    let (top, bottom, left, right) = SAFE_AREA_INSETS.with(|c| c.get());
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("top".to_string(), Value::from_float(top));
    fields.insert("bottom".to_string(), Value::from_float(bottom));
    fields.insert("left".to_string(), Value::from_float(left));
    fields.insert("right".to_string(), Value::from_float(right));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "safe_area",
    }))))
}

fn tuple2_f64(v: &Value, fn_name: &str, arg_name: &str) -> Result<(f64, f64), RuntimeError> {
    if v.is_list() {
        let list = v.as_list();
        let l = list.borrow();
        if l.len() == 2 {
            return Ok((
                as_f64(&l[0], fn_name)?,
                as_f64(&l[1], fn_name)?,
            ));
        }
    }
    Err(RuntimeError {
        line: 0,
        col: 0,
        message: format!("{fn_name}: {arg_name} must be a 2-tuple (x, y)"),
        help: None,
    })
}

// ---------------------------------------------------------------
// Phase 38 session 3: assets namespace — environment introspection.
//
// Per the wgpu-on-web audit (Phase 38 session 2), the actual asset
// routing on browser builds happens inside the runner: existing
// `texture(path)` / `mesh(path)` / `sound.play(path)` builtins
// reroute through `fetch` on wasm32, transparent to scripts.
//
// What scripts sometimes need is a way to ask "am I running in a
// browser?" so they can adapt e.g. controls (touch-only) or UI
// (skip the fullscreen toggle that browsers don't allow without a
// gesture). `assets.is_browser()` is that primitive.
// ---------------------------------------------------------------

fn install_assets(env: &mut Env) {
    let mut a = HashMap::new();
    a.insert(
        "is_browser".to_string(),
        Value::from_builtin("assets.is_browser", &[], assets_is_browser),
    );
    a.insert(
        "is_mobile".to_string(),
        Value::from_builtin("assets.is_mobile", &[], assets_is_mobile),
    );
    a.insert(
        "platform".to_string(),
        Value::from_builtin("assets.platform", &[], assets_platform),
    );
    env.set(
        "assets".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: a,
            kind: "module",
        }))),
    );
}

/// True iff this build is running in a browser (wasm32 target).
fn assets_is_browser(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "assets.is_browser")?;
    #[cfg(target_arch = "wasm32")]
    {
        Ok(Value::from_bool(true))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(Value::from_bool(false))
    }
}

/// True iff this build is running on a mobile device. Today this
/// returns false (no mobile runtime ships yet); the iOS / Android
/// runtime hooks (Phase 39 follow-on session) flip this true.
fn assets_is_mobile(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "assets.is_mobile")?;
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        Ok(Value::from_bool(true))
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Ok(Value::from_bool(false))
    }
}

/// Returns a string describing the host platform: "windows", "macos",
/// "linux", "ios", "android", "browser", "unknown".
fn assets_platform(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "assets.platform")?;
    let s = if cfg!(target_arch = "wasm32") {
        "browser"
    } else if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };
    Ok(Value::from_string(s.to_string()))
}

// ---------------------------------------------------------------
// Phase 37: rollback netcode builtins.
// ---------------------------------------------------------------

/// Install the `rollback.*` namespace. Per the Phase 37 RFC,
/// rollback is opt-in via `net.set_mode("rollback")`; this namespace
/// holds the rollback-specific knobs (input prediction, smoothing,
/// max rewind frames) plus the snapshot primitives the rewind
/// engine uses internally.
#[cfg(not(target_arch = "wasm32"))]
fn install_rollback(env: &mut Env) {
    let mut r = HashMap::new();
    r.insert(
        "snapshot".to_string(),
        Value::from_builtin(
            "rollback.snapshot",
            &["name", "value"],
            rollback_snapshot,
        ),
    );
    r.insert(
        "restore".to_string(),
        Value::from_builtin("rollback.restore", &["name"], rollback_restore),
    );
    r.insert(
        "advance_tick".to_string(),
        Value::from_builtin(
            "rollback.advance_tick",
            &["tick"],
            rollback_advance_tick,
        ),
    );
    r.insert(
        "current_tick".to_string(),
        Value::from_builtin("rollback.current_tick", &[], rollback_current_tick),
    );
    r.insert(
        "discard_after".to_string(),
        Value::from_builtin(
            "rollback.discard_after",
            &["tick"],
            rollback_discard_after,
        ),
    );
    r.insert(
        "set_input_prediction".to_string(),
        Value::from_builtin(
            "rollback.set_input_prediction",
            &["policy"],
            rollback_set_input_prediction,
        ),
    );
    r.insert(
        "input_prediction".to_string(),
        Value::from_builtin(
            "rollback.input_prediction",
            &[],
            rollback_input_prediction,
        ),
    );
    r.insert(
        "set_smoothing".to_string(),
        Value::from_builtin(
            "rollback.set_smoothing",
            &["on"],
            rollback_set_smoothing,
        ),
    );
    r.insert(
        "smoothing".to_string(),
        Value::from_builtin("rollback.smoothing", &[], rollback_smoothing),
    );
    r.insert(
        "max_rewind_frames".to_string(),
        Value::from_builtin(
            "rollback.max_rewind_frames",
            &["n"],
            rollback_set_max_rewind_frames,
        ),
    );
    r.insert(
        "is_replaying".to_string(),
        Value::from_builtin("rollback.is_replaying", &[], rollback_is_replaying),
    );
    r.insert(
        "stats".to_string(),
        Value::from_builtin("rollback.stats", &[], rollback_stats),
    );
    env.set(
        "rollback".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: r,
            kind: "module",
        }))),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn net_set_mode(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.set_mode")?;
    let s = string_arg(&args[0], "net.set_mode", "mode")?;
    let mode = crate::rollback::Mode::parse(&s).ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!(
            "net.set_mode: unknown mode {s:?} — expected \"lockstep\" or \"rollback\""
        ),
        help: Some(
            "lockstep is the default (Phase 31); rollback is the second mode shipped in Phase 37."
                .to_string(),
        ),
    })?;
    crate::rollback::set_mode(mode);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn net_mode(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.mode")?;
    Ok(Value::from_string(crate::rollback::mode().as_str().to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_snapshot(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "rollback.snapshot")?;
    let name = string_arg(&args[0], "rollback.snapshot", "name")?;
    crate::rollback::snapshot(&name, &args[1]).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: format!("rollback.snapshot: {m}"),
        help: None,
    })?;
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_restore(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.restore")?;
    let name = string_arg(&args[0], "rollback.restore", "name")?;
    Ok(crate::rollback::restore(&name).unwrap_or(Value::NIL))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_advance_tick(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.advance_tick")?;
    let tick = as_i64(&args[0], "rollback.advance_tick")?;
    if !(0..=i64::from(u32::MAX)).contains(&tick) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("rollback.advance_tick: tick out of range (got {tick})"),
            help: None,
        });
    }
    crate::rollback::advance_tick(tick as u32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_current_tick(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.current_tick")?;
    Ok(Value::from_int(crate::rollback::current_tick() as i64))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_discard_after(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.discard_after")?;
    let tick = as_i64(&args[0], "rollback.discard_after")?;
    if !(0..=i64::from(u32::MAX)).contains(&tick) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("rollback.discard_after: tick out of range (got {tick})"),
            help: None,
        });
    }
    crate::rollback::discard_after(tick as u32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_set_input_prediction(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.set_input_prediction")?;
    let s = string_arg(&args[0], "rollback.set_input_prediction", "policy")?;
    let p = crate::rollback::InputPrediction::parse(&s).ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!(
            "rollback.set_input_prediction: unknown policy {s:?} — expected \
             \"last-input-repeat\" or \"velocity-extrapolate\""
        ),
        help: None,
    })?;
    crate::rollback::set_input_prediction(p);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_input_prediction(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.input_prediction")?;
    Ok(Value::from_string(
        crate::rollback::input_prediction().as_str().to_string(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_set_smoothing(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.set_smoothing")?;
    if !args[0].is_bool() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "rollback.set_smoothing: expected bool for `on`".to_string(),
            help: None,
        });
    }
    crate::rollback::set_smoothing(args[0].as_bool());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_smoothing(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.smoothing")?;
    Ok(Value::from_bool(crate::rollback::smoothing()))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_set_max_rewind_frames(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "rollback.max_rewind_frames")?;
    let n = as_i64(&args[0], "rollback.max_rewind_frames")?;
    if !(1..=60).contains(&n) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("rollback.max_rewind_frames: must be 1..=60 (got {n})"),
            help: None,
        });
    }
    crate::rollback::set_max_rewind_frames(n as u32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_is_replaying(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.is_replaying")?;
    Ok(Value::from_bool(crate::rollback::is_replaying()))
}

#[cfg(not(target_arch = "wasm32"))]
fn rollback_stats(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "rollback.stats")?;
    let s = crate::rollback::stats();
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("predicted".to_string(), Value::from_int(s.predicted as i64));
    fields.insert("corrected".to_string(), Value::from_int(s.corrected as i64));
    fields.insert(
        "last_correction_frames".to_string(),
        Value::from_int(s.last_correction_frames as i64),
    );
    fields.insert(
        "ring_len".to_string(),
        Value::from_int(s.ring_len as i64),
    );
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "rollback_stats",
    }))))
}

// ---------------------------------------------------------------
// Phase 36 session 5: reconnect builtins.
// ---------------------------------------------------------------

/// True once per drop. Pops a peer from the disconnected queue and
/// stores it as the value `net.last_disconnected_peer` will read.
/// Scripts call this in their play loop; the runner detects drops
/// from `last_seen_at` timeouts inside `poll`.
#[cfg(not(target_arch = "wasm32"))]
fn net_peer_disconnected(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.peer_disconnected")?;
    Ok(Value::from_bool(crate::net::peer_disconnected()))
}

/// Internal id of the most recently popped disconnect, or -1 if none.
#[cfg(not(target_arch = "wasm32"))]
fn net_last_disconnected_peer(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.last_disconnected_peer")?;
    Ok(Value::from_int(crate::net::last_disconnected_peer() as i64))
}

/// Best-effort re-handshake with a dropped peer. Returns true if the
/// peer is back in the session, false on timeout.
#[cfg(not(target_arch = "wasm32"))]
fn net_try_reconnect(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "net.try_reconnect")?;
    let peer_id = as_i64(&args[0], "net.try_reconnect")?;
    let timeout_ms = as_i64(&args[1], "net.try_reconnect")?;
    if !(0..=255).contains(&peer_id) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("net.try_reconnect: peer_id must be 0..=255 (got {peer_id})"),
            help: None,
        });
    }
    if timeout_ms < 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "net.try_reconnect: timeout_ms must be >= 0 (got {timeout_ms})"
            ),
            help: None,
        });
    }
    let ok = crate::net::try_reconnect(peer_id as u8, timeout_ms as u64).map_err(|m| {
        RuntimeError {
            line: 0,
            col: 0,
            message: m,
            help: None,
        }
    })?;
    Ok(Value::from_bool(ok))
}

/// Promote the lowest-id surviving peer to host (internal id 0) if
/// the previous host has dropped. Idempotent. Returns true if
/// migration ran.
#[cfg(not(target_arch = "wasm32"))]
fn net_host_migrate_if_host_lost(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.host_migrate_if_host_lost")?;
    Ok(Value::from_bool(crate::net::host_migrate_if_host_lost()))
}

/// Override the per-session disconnect timeout. Default is 5 seconds.
#[cfg(not(target_arch = "wasm32"))]
fn net_disconnect_timeout(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.disconnect_timeout")?;
    let seconds = as_i64(&args[0], "net.disconnect_timeout")?;
    if !(1..=600).contains(&seconds) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "net.disconnect_timeout: seconds must be 1..=600 (got {seconds})"
            ),
            help: None,
        });
    }
    crate::net::set_disconnect_timeout(seconds as u64);
    Ok(Value::NIL)
}

// ---------------------------------------------------------------
// Phase 36 session 4: lobby builtins.
// ---------------------------------------------------------------

/// Create a public Steam Lobby. Returns the lobby's SteamID as an
/// int. Local user becomes peer 0.
#[cfg(not(target_arch = "wasm32"))]
fn net_create_lobby(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "net.create_lobby")?;
    let name = string_arg(&args[0], "net.create_lobby", "name")?;
    let max_peers = as_i64(&args[1], "net.create_lobby")?;
    if !(2..=4).contains(&max_peers) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("net.create_lobby: max_peers must be 2..=4 (got {max_peers})"),
            help: None,
        });
    }
    let lobby_id = crate::net_steam::create_lobby(&name, max_peers as u32).map_err(|m| {
        RuntimeError {
            line: 0,
            col: 0,
            message: m,
            help: None,
        }
    })?;
    Ok(Value::from_int(lobby_id as i64))
}

/// Find public Steam Lobbies matching the substring `query`. Empty
/// query → all lobbies. Returns a list of `{id, name, peer_count,
/// max_peers}` records.
#[cfg(not(target_arch = "wasm32"))]
fn net_find_lobbies(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.find_lobbies")?;
    let query = string_arg(&args[0], "net.find_lobbies", "query")?;
    let lobbies = crate::net_steam::find_lobbies(&query).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    let mut items: Vec<Value> = Vec::with_capacity(lobbies.len());
    for info in lobbies {
        let mut fields: HashMap<String, Value> = HashMap::new();
        fields.insert("id".to_string(), Value::from_int(info.id as i64));
        fields.insert("name".to_string(), Value::from_string(info.name));
        fields.insert(
            "peer_count".to_string(),
            Value::from_int(info.peer_count as i64),
        );
        fields.insert(
            "max_peers".to_string(),
            Value::from_int(info.max_peers as i64),
        );
        items.push(Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "lobby",
        }))));
    }
    Ok(Value::from_list(Rc::new(RefCell::new(items))))
}

/// Join a Steam Lobby by id. Returns true on success, false when the
/// lobby is full or no longer exists.
#[cfg(not(target_arch = "wasm32"))]
fn net_join_lobby(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "net.join_lobby")?;
    let lobby_id = as_i64(&args[0], "net.join_lobby")?;
    if lobby_id <= 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("net.join_lobby: lobby_id must be positive (got {lobby_id})"),
            help: None,
        });
    }
    let ok = crate::net_steam::join_lobby(lobby_id as u64).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: m,
        help: None,
    })?;
    Ok(Value::from_bool(ok))
}

/// Leave the current Steam Lobby + close the Steam-side session.
/// Idempotent.
#[cfg(not(target_arch = "wasm32"))]
fn net_leave_lobby(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "net.leave_lobby")?;
    crate::net_steam::leave_lobby();
    Ok(Value::NIL)
}

/// Phase 32: `world.*` namespace — spatial partitioning + chunked
/// streaming for open-world 3D. Sessions 2 and 3 ship the spatial
/// query API + the streaming state machine; later sessions plumb
/// these into the 3D renderer for LOD + occlusion + frustum culling.
///
/// The spatial structures live in `crate::spatial::WORLD` (a global
/// Mutex<Option<WorldSpatial>>) so engine-internal worker pool
/// integrations (Phase 32 session 1 lock revision) have somewhere
/// to share state. Scripts always go through these builtins; the
/// raw structures aren't exposed.
#[cfg(not(target_arch = "wasm32"))]
fn install_world(env: &mut Env) {
    let mut w = HashMap::new();
    w.insert(
        "spatial_clear".to_string(),
        Value::from_builtin("world.spatial_clear", &[], world_spatial_clear),
    );
    w.insert(
        "spatial_insert_dynamic".to_string(),
        Value::from_builtin(
            "world.spatial_insert_dynamic",
            &["id", "x", "y", "z", "radius"],
            world_spatial_insert_dynamic,
        ),
    );
    w.insert(
        "spatial_remove_dynamic".to_string(),
        Value::from_builtin(
            "world.spatial_remove_dynamic",
            &["id"],
            world_spatial_remove_dynamic,
        ),
    );
    w.insert(
        "spatial_add_static".to_string(),
        Value::from_builtin(
            "world.spatial_add_static",
            &["id", "x", "y", "z", "radius"],
            world_spatial_add_static,
        ),
    );
    w.insert(
        "spatial_build_static".to_string(),
        Value::from_builtin("world.spatial_build_static", &[], world_spatial_build_static),
    );
    w.insert(
        "spatial_query_radius".to_string(),
        Value::from_builtin(
            "world.spatial_query_radius",
            &["x", "y", "z", "radius"],
            world_spatial_query_radius,
        ),
    );
    w.insert(
        "spatial_query_box".to_string(),
        Value::from_builtin(
            "world.spatial_query_box",
            &["x0", "y0", "z0", "x1", "y1", "z1"],
            world_spatial_query_box,
        ),
    );
    // ---- Phase 32 session 3: chunked streaming ----
    w.insert(
        "set_chunk_size".to_string(),
        Value::from_builtin(
            "world.set_chunk_size",
            &["meters"],
            world_set_chunk_size,
        ),
    );
    w.insert(
        "set_stream_radius".to_string(),
        Value::from_builtin(
            "world.set_stream_radius",
            &["chunks"],
            world_set_stream_radius,
        ),
    );
    w.insert(
        "set_stream_budget".to_string(),
        Value::from_builtin(
            "world.set_stream_budget",
            &["loads_per_frame", "unloads_per_frame"],
            world_set_stream_budget,
        ),
    );
    w.insert(
        "stream_step".to_string(),
        Value::from_builtin(
            "world.stream_step",
            &["camera_x", "camera_z"],
            world_stream_step,
        ),
    );
    w.insert(
        "mark_chunk_loaded".to_string(),
        Value::from_builtin(
            "world.mark_chunk_loaded",
            &["chunk_id"],
            world_mark_chunk_loaded,
        ),
    );
    w.insert(
        "mark_chunk_unloaded".to_string(),
        Value::from_builtin(
            "world.mark_chunk_unloaded",
            &["chunk_id"],
            world_mark_chunk_unloaded,
        ),
    );
    w.insert(
        "loaded_chunk_count".to_string(),
        Value::from_builtin("world.loaded_chunk_count", &[], world_loaded_chunk_count),
    );
    w.insert(
        "stream_clear".to_string(),
        Value::from_builtin("world.stream_clear", &[], world_stream_clear),
    );
    // ---- Phase 32 session 4: LOD chains ----
    w.insert(
        "set_lod_chain".to_string(),
        Value::from_builtin(
            "world.set_lod_chain",
            &["class", "assets", "switch_distances"],
            world_set_lod_chain,
        ),
    );
    w.insert(
        "lod_for_distance".to_string(),
        Value::from_builtin(
            "world.lod_for_distance",
            &["class", "distance"],
            world_lod_for_distance,
        ),
    );
    w.insert(
        "lod_index_for_distance".to_string(),
        Value::from_builtin(
            "world.lod_index_for_distance",
            &["class", "distance"],
            world_lod_index_for_distance,
        ),
    );
    w.insert(
        "clear_lod".to_string(),
        Value::from_builtin("world.clear_lod", &[], world_clear_lod),
    );
    // ---- Phase 32 session 6: frustum culling ----
    w.insert(
        "spatial_query_frustum".to_string(),
        Value::from_builtin(
            "world.spatial_query_frustum",
            &["matrix"],
            world_spatial_query_frustum,
        ),
    );
    w.insert(
        "frustum_contains_sphere".to_string(),
        Value::from_builtin(
            "world.frustum_contains_sphere",
            &["matrix", "x", "y", "z", "radius"],
            world_frustum_contains_sphere,
        ),
    );
    // ---- Phase 32 session 7: per-asset instance buckets ----
    w.insert(
        "instance_clear".to_string(),
        Value::from_builtin("world.instance_clear", &[], world_instance_clear),
    );
    w.insert(
        "instance_reset".to_string(),
        Value::from_builtin("world.instance_reset", &[], world_instance_reset),
    );
    w.insert(
        "instance_add".to_string(),
        Value::from_builtin(
            "world.instance_add",
            &["asset", "transform"],
            world_instance_add,
        ),
    );
    w.insert(
        "instance_count".to_string(),
        Value::from_builtin(
            "world.instance_count",
            &["asset"],
            world_instance_count,
        ),
    );
    w.insert(
        "instance_total".to_string(),
        Value::from_builtin("world.instance_total", &[], world_instance_total),
    );
    w.insert(
        "instance_bucket_count".to_string(),
        Value::from_builtin(
            "world.instance_bucket_count",
            &[],
            world_instance_bucket_count,
        ),
    );
    w.insert(
        "instance_assets".to_string(),
        Value::from_builtin("world.instance_assets", &[], world_instance_assets),
    );
    // ---- Phase 32 session 8: ergonomic helpers ----
    w.insert(
        "stream_radius_meters".to_string(),
        Value::from_builtin(
            "world.stream_radius_meters",
            &["meters"],
            world_stream_radius_meters,
        ),
    );
    w.insert(
        "entity_lod".to_string(),
        Value::from_builtin(
            "world.entity_lod",
            &["class", "lod_pairs"],
            world_entity_lod,
        ),
    );
    w.insert(
        "world_to_lod".to_string(),
        Value::from_builtin(
            "world.world_to_lod",
            &["class", "ex", "ey", "ez", "cx", "cy", "cz"],
            world_world_to_lod,
        ),
    );
    w.insert(
        "distance_xyz".to_string(),
        Value::from_builtin(
            "world.distance_xyz",
            &["ax", "ay", "az", "bx", "by", "bz"],
            world_distance_xyz,
        ),
    );
    env.set(
        "world".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: w,
            kind: "module",
        }))),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_clear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.spatial_clear")?;
    crate::spatial::with_world(|w| w.clear());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_insert_dynamic(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "world.spatial_insert_dynamic")?;
    let id = as_i64(&args[0], "world.spatial_insert_dynamic")? as u64;
    let x = as_f64(&args[1], "world.spatial_insert_dynamic")? as f32;
    let y = as_f64(&args[2], "world.spatial_insert_dynamic")? as f32;
    let z = as_f64(&args[3], "world.spatial_insert_dynamic")? as f32;
    let r = as_f64(&args[4], "world.spatial_insert_dynamic")? as f32;
    crate::spatial::with_world(|w| {
        w.insert_dynamic(id, crate::spatial::Aabb::from_center_radius(x, y, z, r));
    });
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_remove_dynamic(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.spatial_remove_dynamic")?;
    let id = as_i64(&args[0], "world.spatial_remove_dynamic")? as u64;
    let removed = crate::spatial::with_world(|w| w.remove_dynamic(id));
    Ok(Value::from_bool(removed))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_add_static(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "world.spatial_add_static")?;
    let id = as_i64(&args[0], "world.spatial_add_static")? as u64;
    let x = as_f64(&args[1], "world.spatial_add_static")? as f32;
    let y = as_f64(&args[2], "world.spatial_add_static")? as f32;
    let z = as_f64(&args[3], "world.spatial_add_static")? as f32;
    let r = as_f64(&args[4], "world.spatial_add_static")? as f32;
    crate::spatial::with_world(|w| {
        w.add_static(id, crate::spatial::Aabb::from_center_radius(x, y, z, r));
    });
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_build_static(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.spatial_build_static")?;
    crate::spatial::with_world(|w| w.build_static());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_query_radius(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "world.spatial_query_radius")?;
    let x = as_f64(&args[0], "world.spatial_query_radius")? as f32;
    let y = as_f64(&args[1], "world.spatial_query_radius")? as f32;
    let z = as_f64(&args[2], "world.spatial_query_radius")? as f32;
    let r = as_f64(&args[3], "world.spatial_query_radius")? as f32;
    let hits: Vec<Value> = crate::spatial::with_world(|w| w.query_radius(x, y, z, r))
        .into_iter()
        .map(|id| Value::from_int(id as i64))
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(hits))))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_query_box(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 6, "world.spatial_query_box")?;
    let x0 = as_f64(&args[0], "world.spatial_query_box")? as f32;
    let y0 = as_f64(&args[1], "world.spatial_query_box")? as f32;
    let z0 = as_f64(&args[2], "world.spatial_query_box")? as f32;
    let x1 = as_f64(&args[3], "world.spatial_query_box")? as f32;
    let y1 = as_f64(&args[4], "world.spatial_query_box")? as f32;
    let z1 = as_f64(&args[5], "world.spatial_query_box")? as f32;
    let q = crate::spatial::Aabb {
        min: [x0.min(x1), y0.min(y1), z0.min(z1)],
        max: [x0.max(x1), y0.max(y1), z0.max(z1)],
    };
    let hits: Vec<Value> = crate::spatial::with_world(|w| w.query_box(&q))
        .into_iter()
        .map(|id| Value::from_int(id as i64))
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(hits))))
}

// ---- Phase 32 session 3: chunked streaming builtins ----

#[cfg(not(target_arch = "wasm32"))]
fn world_set_chunk_size(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.set_chunk_size")?;
    let meters = as_f64(&args[0], "world.set_chunk_size")? as f32;
    if meters <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.set_chunk_size: meters must be positive (got {meters})"),
            help: None,
        });
    }
    crate::streaming::with_streaming(|s| s.chunk_size = meters);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_set_stream_radius(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.set_stream_radius")?;
    let chunks = as_i64(&args[0], "world.set_stream_radius")?;
    if !(1..=64).contains(&chunks) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.set_stream_radius: chunks must be 1..=64 (got {chunks})"),
            help: None,
        });
    }
    crate::streaming::with_streaming(|s| s.stream_radius_chunks = chunks as i32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_set_stream_budget(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.set_stream_budget")?;
    let loads = as_i64(&args[0], "world.set_stream_budget")?.max(0) as u32;
    let unloads = as_i64(&args[1], "world.set_stream_budget")?.max(0) as u32;
    crate::streaming::with_streaming(|s| {
        s.loads_per_frame = loads;
        s.unloads_per_frame = unloads;
    });
    Ok(Value::NIL)
}

/// Compute one frame's streaming work given the camera position.
/// Returns a tuple `(to_load, to_unload)`, where each side is a list
/// of opaque chunk-id integers. The script forwards loaded chunks
/// to its asset loader (mesh / texture / NPC spawn), and confirms
/// completion via `world.mark_chunk_loaded` / `world.mark_chunk_unloaded`.
/// The actual asset I/O is the script's responsibility — this
/// function is pure bookkeeping.
#[cfg(not(target_arch = "wasm32"))]
fn world_stream_step(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.stream_step")?;
    let cx = as_f64(&args[0], "world.stream_step")? as f32;
    let cz = as_f64(&args[1], "world.stream_step")? as f32;
    let step = crate::streaming::with_streaming(|s| s.step(cx, cz));
    let to_load: Vec<Value> = step
        .to_load
        .iter()
        .map(|c| Value::from_int(c.0 as i64))
        .collect();
    let to_unload: Vec<Value> = step
        .to_unload
        .iter()
        .map(|c| Value::from_int(c.0 as i64))
        .collect();
    let load_list = Value::from_list(Rc::new(RefCell::new(to_load)));
    let unload_list = Value::from_list(Rc::new(RefCell::new(to_unload)));
    Ok(Value::from_tuple(Rc::new(vec![load_list, unload_list])))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_mark_chunk_loaded(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.mark_chunk_loaded")?;
    let id = as_i64(&args[0], "world.mark_chunk_loaded")? as u64;
    crate::streaming::with_streaming(|s| s.mark_loaded(crate::streaming::ChunkId(id)));
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_mark_chunk_unloaded(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.mark_chunk_unloaded")?;
    let id = as_i64(&args[0], "world.mark_chunk_unloaded")? as u64;
    crate::streaming::with_streaming(|s| s.mark_unloaded(crate::streaming::ChunkId(id)));
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_loaded_chunk_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.loaded_chunk_count")?;
    let n = crate::streaming::with_streaming(|s| s.loaded_count()) as i64;
    Ok(Value::from_int(n))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_stream_clear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.stream_clear")?;
    crate::streaming::with_streaming(|s| s.clear());
    Ok(Value::NIL)
}

// ---- Phase 32 session 4: LOD-chain builtins ----

#[cfg(not(target_arch = "wasm32"))]
fn world_set_lod_chain(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "world.set_lod_chain")?;
    let class = string_arg(&args[0], "world.set_lod_chain", "class")?;
    let assets = list_of_strings(&args[1], "world.set_lod_chain", "assets")?;
    let switches = list_of_floats(&args[2], "world.set_lod_chain", "switch_distances")?;
    let chain = crate::lod::LodChain::new(assets, switches.iter().map(|f| *f as f32).collect())
        .map_err(|m| RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.set_lod_chain: {m}"),
            help: None,
        })?;
    crate::lod::with_table(|t| {
        t.insert(class, chain);
    });
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_lod_for_distance(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.lod_for_distance")?;
    let class = string_arg(&args[0], "world.lod_for_distance", "class")?;
    let distance = as_f64(&args[1], "world.lod_for_distance")? as f32;
    let asset = crate::lod::with_table(|t| {
        t.get(&class)
            .map(|chain| chain.asset_for_distance(distance).to_string())
    });
    match asset {
        Some(s) => Ok(Value::from_string(s)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.lod_for_distance: no LOD chain registered for class '{class}'"),
            help: Some("call world.set_lod_chain(class, assets, switches) first".to_string()),
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn world_lod_index_for_distance(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.lod_index_for_distance")?;
    let class = string_arg(&args[0], "world.lod_index_for_distance", "class")?;
    let distance = as_f64(&args[1], "world.lod_index_for_distance")? as f32;
    let idx = crate::lod::with_table(|t| t.get(&class).map(|chain| chain.select(distance) as i64));
    match idx {
        Some(i) => Ok(Value::from_int(i)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "world.lod_index_for_distance: no LOD chain registered for class '{class}'"
            ),
            help: None,
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn world_clear_lod(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.clear_lod")?;
    crate::lod::with_table(|t| t.clear());
    Ok(Value::NIL)
}

// ---- Phase 32 session 6: frustum-culling builtins ----

/// Read a 4x4 matrix from a Twe value: a list of 4 tuples of 4 floats
/// each (row-major), or a flat list of 16 floats. Either form is
/// accepted because scripts naturally produce both — `[(1.0, 0.0,
/// 0.0, 0.0), (0.0, 1.0, ...)]` mirrors GLSL row-format, while a
/// flat 16-element list is what comes back from a future
/// `camera.view_proj()` builtin.
#[cfg(not(target_arch = "wasm32"))]
fn read_matrix4x4(v: &Value, op: &str) -> Result<[[f32; 4]; 4], RuntimeError> {
    if !v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op}: matrix must be a list (got {})", v.type_name()),
            help: None,
        });
    }
    let rc = v.as_list();
    let outer = rc.borrow();
    if outer.len() == 16 {
        // Flat row-major form.
        let mut m = [[0.0; 4]; 4];
        for (i, val) in outer.iter().enumerate() {
            m[i / 4][i % 4] = as_f64(val, op)? as f32;
        }
        return Ok(m);
    }
    if outer.len() != 4 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{op}: matrix must have 4 rows or 16 flat entries (got {})",
                outer.len()
            ),
            help: None,
        });
    }
    let mut m = [[0.0; 4]; 4];
    for (i, row_v) in outer.iter().enumerate() {
        let elems: Vec<Value> = if row_v.is_tuple() {
            row_v.as_tuple().iter().cloned().collect()
        } else if row_v.is_list() {
            let r = row_v.as_list();
            let cloned = r.borrow().iter().cloned().collect();
            cloned
        } else {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{op}: matrix row {i} must be a tuple or list"),
                help: None,
            });
        };
        if elems.len() != 4 {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "{op}: matrix row {i} must have 4 elements (got {})",
                    elems.len()
                ),
                help: None,
            });
        }
        for (j, val) in elems.iter().enumerate() {
            m[i][j] = as_f64(val, op)? as f32;
        }
    }
    Ok(m)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_spatial_query_frustum(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.spatial_query_frustum")?;
    let m = read_matrix4x4(&args[0], "world.spatial_query_frustum")?;
    let frustum = crate::cull::Frustum::from_view_proj_row_major(m);
    let hits: Vec<Value> = crate::spatial::with_world(|w| w.query_frustum(&frustum))
        .into_iter()
        .map(|id| Value::from_int(id as i64))
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(hits))))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_frustum_contains_sphere(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "world.frustum_contains_sphere")?;
    let m = read_matrix4x4(&args[0], "world.frustum_contains_sphere")?;
    let x = as_f64(&args[1], "world.frustum_contains_sphere")? as f32;
    let y = as_f64(&args[2], "world.frustum_contains_sphere")? as f32;
    let z = as_f64(&args[3], "world.frustum_contains_sphere")? as f32;
    let r = as_f64(&args[4], "world.frustum_contains_sphere")? as f32;
    let frustum = crate::cull::Frustum::from_view_proj_row_major(m);
    Ok(Value::from_bool(frustum.may_contain_sphere(x, y, z, r)))
}

// ---- Phase 32 session 7: instance-bucket builtins ----

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_clear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_clear")?;
    crate::instance::with_buckets(|b| b.clear());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_reset(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_reset")?;
    crate::instance::with_buckets(|b| b.reset());
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_add(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.instance_add")?;
    let asset = string_arg(&args[0], "world.instance_add", "asset")?;
    let m = read_matrix4x4(&args[1], "world.instance_add")?;
    // Flatten row-major 4x4 to [f32; 16].
    let mut t = [0.0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            t[i * 4 + j] = m[i][j];
        }
    }
    crate::instance::with_buckets(|b| b.add(&asset, t));
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.instance_count")?;
    let asset = string_arg(&args[0], "world.instance_count", "asset")?;
    let n = crate::instance::with_buckets(|b| b.count(&asset)) as i64;
    Ok(Value::from_int(n))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_total(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_total")?;
    let n = crate::instance::with_buckets(|b| b.total_instances()) as i64;
    Ok(Value::from_int(n))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_bucket_count(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_bucket_count")?;
    let n = crate::instance::with_buckets(|b| b.bucket_count()) as i64;
    Ok(Value::from_int(n))
}

#[cfg(not(target_arch = "wasm32"))]
fn world_instance_assets(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "world.instance_assets")?;
    let assets: Vec<Value> = crate::instance::with_buckets(|b| b.assets())
        .into_iter()
        .map(Value::from_string)
        .collect();
    Ok(Value::from_list(Rc::new(RefCell::new(assets))))
}

// ---- Phase 32 session 8: ergonomic helpers ----

/// Set the stream radius via meters rather than chunk count. Reads
/// the current `chunk_size` and rounds up so the script doesn't have
/// to remember the chunk grid size. Convenience wrapper over
/// [`world.set_stream_radius`].
#[cfg(not(target_arch = "wasm32"))]
fn world_stream_radius_meters(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "world.stream_radius_meters")?;
    let m = as_f64(&args[0], "world.stream_radius_meters")? as f32;
    if m <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.stream_radius_meters: meters must be positive (got {m})"),
            help: None,
        });
    }
    let chunks = crate::streaming::with_streaming(|s| (m / s.chunk_size).ceil() as i32);
    let chunks = chunks.clamp(1, 64);
    crate::streaming::with_streaming(|s| s.stream_radius_chunks = chunks);
    Ok(Value::from_int(chunks as i64))
}

/// Declare a LOD chain via (asset, max_distance) pairs — a more
/// ergonomic shape than the parallel-arrays form of
/// `world.set_lod_chain`. The last pair's `max_distance` is ignored
/// (its asset covers everything beyond the previous switch); pass
/// any sentinel value (typically a large number).
///
/// Example: `world.entity_lod("Tree", [("near.glb", 25.0),
/// ("med.glb", 100.0), ("far.glb", 1e9)])` registers the same chain
/// as `world.set_lod_chain("Tree", ["near.glb", "med.glb",
/// "far.glb"], [25.0, 100.0])`.
#[cfg(not(target_arch = "wasm32"))]
fn world_entity_lod(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "world.entity_lod")?;
    let class = string_arg(&args[0], "world.entity_lod", "class")?;
    if !args[1].is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "world.entity_lod: lod_pairs must be a list of (asset, max_distance) tuples"
                .to_string(),
            help: None,
        });
    }
    let rc = args[1].as_list();
    let pairs = rc.borrow();
    if pairs.is_empty() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "world.entity_lod: lod_pairs must have at least one entry".to_string(),
            help: None,
        });
    }
    let mut assets: Vec<String> = Vec::with_capacity(pairs.len());
    let mut switches: Vec<f32> = Vec::with_capacity(pairs.len().saturating_sub(1));
    for (i, p) in pairs.iter().enumerate() {
        if !p.is_tuple() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("world.entity_lod: pair {i} must be a tuple (asset, distance)"),
                help: None,
            });
        }
        let elems = p.as_tuple();
        if elems.len() != 2 {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "world.entity_lod: pair {i} must be a 2-tuple (asset, distance), got {} elements",
                    elems.len()
                ),
                help: None,
            });
        }
        if !elems[0].is_str() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("world.entity_lod: pair {i} asset must be a string"),
                help: None,
            });
        }
        assets.push(elems[0].as_string().clone());
        // Skip the last pair's distance — it's implicit +∞.
        if i + 1 < pairs.len() {
            switches.push(as_f64(&elems[1], "world.entity_lod")? as f32);
        }
    }
    let chain = crate::lod::LodChain::new(assets, switches).map_err(|m| RuntimeError {
        line: 0,
        col: 0,
        message: format!("world.entity_lod: {m}"),
        help: None,
    })?;
    crate::lod::with_table(|t| {
        t.insert(class, chain);
    });
    Ok(Value::NIL)
}

/// Compute distance from camera to entity, then return the LOD
/// asset for that class at that distance. Combines the two most
/// common per-frame queries into one builtin so scripts don't pay
/// the lookup overhead twice.
#[cfg(not(target_arch = "wasm32"))]
fn world_world_to_lod(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 7, "world.world_to_lod")?;
    let class = string_arg(&args[0], "world.world_to_lod", "class")?;
    let ex = as_f64(&args[1], "world.world_to_lod")? as f32;
    let ey = as_f64(&args[2], "world.world_to_lod")? as f32;
    let ez = as_f64(&args[3], "world.world_to_lod")? as f32;
    let cx = as_f64(&args[4], "world.world_to_lod")? as f32;
    let cy = as_f64(&args[5], "world.world_to_lod")? as f32;
    let cz = as_f64(&args[6], "world.world_to_lod")? as f32;
    let dx = ex - cx;
    let dy = ey - cy;
    let dz = ez - cz;
    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
    let asset = crate::lod::with_table(|t| {
        t.get(&class)
            .map(|chain| chain.asset_for_distance(distance).to_string())
    });
    match asset {
        Some(s) => Ok(Value::from_string(s)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("world.world_to_lod: no LOD chain registered for class '{class}'"),
            help: Some("call world.entity_lod or world.set_lod_chain first".to_string()),
        }),
    }
}

/// Euclidean distance between two 3D points. Bog-standard but pulled
/// out as a builtin so the per-frame visibility-pass loop doesn't
/// have to allocate a tuple/list to compute it.
#[cfg(not(target_arch = "wasm32"))]
fn world_distance_xyz(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 6, "world.distance_xyz")?;
    let ax = as_f64(&args[0], "world.distance_xyz")?;
    let ay = as_f64(&args[1], "world.distance_xyz")?;
    let az = as_f64(&args[2], "world.distance_xyz")?;
    let bx = as_f64(&args[3], "world.distance_xyz")?;
    let by = as_f64(&args[4], "world.distance_xyz")?;
    let bz = as_f64(&args[5], "world.distance_xyz")?;
    let dx = ax - bx;
    let dy = ay - by;
    let dz = az - bz;
    Ok(Value::from_float((dx * dx + dy * dy + dz * dz).sqrt()))
}

// ---- Phase 32 session 5: terrain.* namespace ----

#[cfg(not(target_arch = "wasm32"))]
fn install_terrain(env: &mut Env) {
    let mut t = HashMap::new();
    t.insert(
        "set_chunk_size".to_string(),
        Value::from_builtin(
            "terrain.set_chunk_size",
            &["meters"],
            terrain_set_chunk_size,
        ),
    );
    t.insert(
        "set_chunk_resolution".to_string(),
        Value::from_builtin(
            "terrain.set_chunk_resolution",
            &["samples"],
            terrain_set_chunk_resolution,
        ),
    );
    t.insert(
        "set_chunk".to_string(),
        Value::from_builtin(
            "terrain.set_chunk",
            &["cx", "cz", "heights"],
            terrain_set_chunk,
        ),
    );
    t.insert(
        "has_chunk".to_string(),
        Value::from_builtin("terrain.has_chunk", &["cx", "cz"], terrain_has_chunk),
    );
    t.insert(
        "height_at".to_string(),
        Value::from_builtin("terrain.height_at", &["x", "z"], terrain_height_at),
    );
    t.insert(
        "normal_at".to_string(),
        Value::from_builtin("terrain.normal_at", &["x", "z"], terrain_normal_at),
    );
    t.insert(
        "clear".to_string(),
        Value::from_builtin("terrain.clear", &[], terrain_clear),
    );
    env.set(
        "terrain".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: t,
            kind: "module",
        }))),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_set_chunk_size(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "terrain.set_chunk_size")?;
    let m = as_f64(&args[0], "terrain.set_chunk_size")? as f32;
    if m <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("terrain.set_chunk_size: meters must be positive (got {m})"),
            help: None,
        });
    }
    crate::terrain::with_terrain(|t| t.chunk_size = m);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_set_chunk_resolution(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "terrain.set_chunk_resolution")?;
    let n = as_i64(&args[0], "terrain.set_chunk_resolution")?;
    if !(2..=1024).contains(&n) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("terrain.set_chunk_resolution: must be 2..=1024 (got {n})"),
            help: None,
        });
    }
    crate::terrain::with_terrain(|t| t.chunk_resolution = n as u32);
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_set_chunk(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "terrain.set_chunk")?;
    let cx = as_i64(&args[0], "terrain.set_chunk")? as i32;
    let cz = as_i64(&args[1], "terrain.set_chunk")? as i32;
    let heights = list_of_floats(&args[2], "terrain.set_chunk", "heights")?;
    let heights_f32: Vec<f32> = heights.iter().map(|f| *f as f32).collect();
    crate::terrain::with_terrain(|t| t.set_chunk(cx, cz, heights_f32)).map_err(|m| {
        RuntimeError {
            line: 0,
            col: 0,
            message: m,
            help: None,
        }
    })?;
    Ok(Value::NIL)
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_has_chunk(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "terrain.has_chunk")?;
    let cx = as_i64(&args[0], "terrain.has_chunk")? as i32;
    let cz = as_i64(&args[1], "terrain.has_chunk")? as i32;
    Ok(Value::from_bool(crate::terrain::with_terrain(|t| {
        t.has_chunk(cx, cz)
    })))
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_height_at(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "terrain.height_at")?;
    let x = as_f64(&args[0], "terrain.height_at")? as f32;
    let z = as_f64(&args[1], "terrain.height_at")? as f32;
    match crate::terrain::with_terrain(|t| t.height_at(x, z)) {
        Some(h) => Ok(Value::from_float(h as f64)),
        None => Ok(Value::NIL),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_normal_at(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "terrain.normal_at")?;
    let x = as_f64(&args[0], "terrain.normal_at")? as f32;
    let z = as_f64(&args[1], "terrain.normal_at")? as f32;
    match crate::terrain::with_terrain(|t| t.normal_at(x, z)) {
        Some(n) => Ok(Value::from_tuple(Rc::new(vec![
            Value::from_float(n[0] as f64),
            Value::from_float(n[1] as f64),
            Value::from_float(n[2] as f64),
        ]))),
        None => Ok(Value::NIL),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_clear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "terrain.clear")?;
    crate::terrain::with_terrain(|t| t.clear());
    Ok(Value::NIL)
}

// ---- Helpers shared by Phase 32 session 4 / 5 / 6 ----

#[cfg(not(target_arch = "wasm32"))]
fn list_of_strings(v: &Value, op: &str, label: &str) -> Result<Vec<String>, RuntimeError> {
    if !v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op} expects a list of strings for {label}"),
            help: None,
        });
    }
    let rc = v.as_list();
    let elems = rc.borrow();
    let mut out = Vec::with_capacity(elems.len());
    for e in elems.iter() {
        if !e.is_str() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{op}: {label} entry must be a string"),
                help: None,
            });
        }
        out.push(e.as_string().clone());
    }
    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
fn list_of_floats(v: &Value, op: &str, label: &str) -> Result<Vec<f64>, RuntimeError> {
    if !v.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{op} expects a list of numbers for {label}"),
            help: None,
        });
    }
    let rc = v.as_list();
    let elems = rc.borrow();
    let mut out = Vec::with_capacity(elems.len());
    for e in elems.iter() {
        out.push(as_f64(e, op)?);
    }
    Ok(out)
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
    // Phase 27 session 4: int / float modulo. Twe's `%` is the
    // percent-literal suffix, not a binary modulo operator (see
    // docs/06-design-document.md §3.6). `math.mod(a, b)` fills the
    // gap surfaced by examples/tetris.twe rotation wrap.
    math.insert(
        "mod".to_string(),
        Value::from_builtin("math.mod", &["a", "b"], math_mod),
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
    // Phase 17 session 5: vector math for 3D games. Tuples already
    // support +/-/* via tuple arithmetic, so add only the operations
    // that aren't expressible as operators: dot, cross, length,
    // normalize. All work on 2D, 3D, or 4D tuples (cross is 3D-only).
    math.insert(
        "dot".to_string(),
        Value::from_builtin("math.dot", &["a", "b"], math_dot),
    );
    math.insert(
        "cross".to_string(),
        Value::from_builtin("math.cross", &["a", "b"], math_cross),
    );
    math.insert(
        "length".to_string(),
        Value::from_builtin("math.length", &["v"], math_length),
    );
    math.insert(
        "normalize".to_string(),
        Value::from_builtin("math.normalize", &["v"], math_normalize),
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

    // Phase 21: quat namespace. Stored as a tagged 4-element list
    // Object with kind="quat" — same scheme as mat4. Order is
    // (x, y, z, w) matching glTF / WGSL convention.
    let mut quat = HashMap::new();
    quat.insert(
        "identity".to_string(),
        Value::from_builtin("quat.identity", &[], quat_identity_impl),
    );
    quat.insert(
        "from_axis_angle".to_string(),
        Value::from_builtin(
            "quat.from_axis_angle",
            &["axis", "angle"],
            quat_from_axis_angle_impl,
        ),
    );
    quat.insert(
        "slerp".to_string(),
        Value::from_builtin("quat.slerp", &["a", "b", "t"], quat_slerp_impl),
    );
    quat.insert(
        "to_mat4".to_string(),
        Value::from_builtin("quat.to_mat4", &["q"], quat_to_mat4_impl),
    );
    quat.insert(
        "mul".to_string(),
        Value::from_builtin("quat.mul", &["a", "b"], quat_mul_impl),
    );
    env.set(
        "quat".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: quat,
            kind: "module",
        }))),
    );

    // Phase 19: mat4 namespace. Stored as a 16-element tagged
    // Object with kind="mat4" — no new Value variant, no GC
    // changes, no parser changes. Element order is column-major
    // (matches glTF + WGSL convention).
    let mut mat4 = HashMap::new();
    mat4.insert(
        "identity".to_string(),
        Value::from_builtin("mat4.identity", &[], mat4_identity_impl),
    );
    mat4.insert(
        "translate".to_string(),
        Value::from_builtin("mat4.translate", &["v"], mat4_translate_impl),
    );
    mat4.insert(
        "rotate_x".to_string(),
        Value::from_builtin("mat4.rotate_x", &["angle"], mat4_rotate_x_impl),
    );
    mat4.insert(
        "rotate_y".to_string(),
        Value::from_builtin("mat4.rotate_y", &["angle"], mat4_rotate_y_impl),
    );
    mat4.insert(
        "rotate_z".to_string(),
        Value::from_builtin("mat4.rotate_z", &["angle"], mat4_rotate_z_impl),
    );
    mat4.insert(
        "scale".to_string(),
        Value::from_builtin("mat4.scale", &["v"], mat4_scale_impl),
    );
    mat4.insert(
        "mul".to_string(),
        Value::from_builtin("mat4.mul", &["a", "b"], mat4_mul_impl),
    );
    mat4.insert(
        "transform_vec3".to_string(),
        Value::from_builtin("mat4.transform_vec3", &["m", "v"], mat4_transform_vec3_impl),
    );
    env.set(
        "mat4".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: mat4,
            kind: "module",
        }))),
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
            message: format!("load_atlas: grid must be positive integers, got ({cols}, {rows})"),
            help: Some("e.g. `load_atlas(\"walk.png\", (8, 4))`".to_string()),
        });
    }
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::from_string(path));
    fields.insert(
        "grid".to_string(),
        Value::from_tuple(Rc::new(vec![Value::from_int(cols), Value::from_int(rows)])),
    );
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "atlas",
    }))))
}

/// `load_font(path)` — read + validate a TTF/OTF font and cache its
/// bytes for lazy parsing on first draw. Returns a font handle
/// `{ path, kind: "font" }`. We do our own magic-bytes check here
/// (cheap, headless-safe) instead of calling macroquad's
/// `load_ttf_font_from_bytes` — that asserts on THREAD_ID and only
/// works inside `Window::from_config`, so eagerly decoding would
/// crash `twec run` and the test harness. The actual `Font` is
/// constructed on first `text_with_font` call inside `on render():`,
/// the same lazy-on-first-draw pattern as `sprite()` for textures.
/// Phase 9 session 4.
fn load_font_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "load_font")?;
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
                    "load_font expected a string path, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `load_font(\"fonts/Inter-Regular.ttf\")`".to_string()),
            });
        }
    };
    // Phase 12 session 3: check the active bundle first; if absent,
    // fall through to the filesystem. NotFound from either path
    // produces the original "cannot find asset" message so existing
    // diagnostics survive.
    let bytes = match crate::bundle::read_asset_bytes(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("load_font: cannot find asset '{path}'"),
                help: Some(
                    "the path is relative to the working directory; check spelling and case"
                        .to_string(),
                ),
            });
        }
        Err(e) => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("load_font: cannot read '{path}': {e}"),
                help: None,
            });
        }
    };
    if !is_ttf_or_otf(&bytes) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("load_font: '{path}' is not a valid TTF/OTF font"),
            help: Some(
                "expected TTF (0x00010000 or 'true'), OTF ('OTTO'), or TTC ('ttcf') magic bytes"
                    .to_string(),
            ),
        });
    }
    FONT_BYTES_CACHE.with(|c| {
        c.borrow_mut().insert(path.clone(), bytes);
    });
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::from_string(path));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "font",
    }))))
}

/// Sniff the standard TrueType / OpenType / TrueType-collection
/// magic-byte signatures. Web font formats (WOFF/WOFF2) intentionally
/// don't match — macroquad's parser doesn't accept them either, so
/// erroring up-front is friendlier than a confusing render-time crash.
fn is_ttf_or_otf(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    matches!(
        &bytes[..4],
        // TrueType (sfnt 0x00010000)
        [0x00, 0x01, 0x00, 0x00]
        // OpenType with CFF outlines: "OTTO"
        | [0x4F, 0x54, 0x54, 0x4F]
        // Apple TrueType: "true"
        | [0x74, 0x72, 0x75, 0x65]
        // TrueType collection: "ttcf"
        | [0x74, 0x74, 0x63, 0x66]
    )
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

// Phase 27 session 4: int / float modulo. Returns the Euclidean
// remainder so that `math.mod(-1, 4) == 3` (not -1) — matches the
// "rotation wrap" intuition that a negative angle is just the
// positive equivalent. `math.mod(a, 0)` errors. Result is int when
// both args are ints; float otherwise.
fn math_mod(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "math.mod")?;
    if args[0].is_int_or_boxed_int() && args[1].is_int_or_boxed_int() {
        let b = args[1].as_int();
        if b == 0 {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: "math.mod by zero".to_string(),
                help: None,
            });
        }
        let a = args[0].as_int();
        let r = a.rem_euclid(b);
        return Ok(Value::from_int(r));
    }
    let bf = as_f64(&args[1], "math.mod")?;
    if bf == 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "math.mod by zero".to_string(),
            help: None,
        });
    }
    let af = as_f64(&args[0], "math.mod")?;
    Ok(Value::from_float(af.rem_euclid(bf)))
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

/// Pull an N-component float array from a Twe tuple. Used by the
/// vector math builtins so they accept 2D, 3D, or 4D tuples by
/// length rather than fixing one dimension.
fn tuple_floats(v: &Value, what: &str) -> Result<Vec<f64>, RuntimeError> {
    if !v.is_tuple() {
        let other = *v;
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what} expects a tuple, got {}", other.type_name()),
            help: Some("e.g. (x, y) or (x, y, z)".to_string()),
        });
    }
    let elems = v.as_tuple();
    let mut out = Vec::with_capacity(elems.len());
    for e in elems.iter() {
        out.push(number(e, what)?);
    }
    Ok(out)
}

fn math_dot(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "math.dot")?;
    let a = tuple_floats(&args[0], "math.dot.a")?;
    let b = tuple_floats(&args[1], "math.dot.b")?;
    if a.len() != b.len() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "math.dot: tuple length mismatch — got {} and {}",
                a.len(),
                b.len()
            ),
            help: None,
        });
    }
    let sum: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    Ok(Value::from_float(sum))
}

fn math_cross(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "math.cross")?;
    let a = tuple_floats(&args[0], "math.cross.a")?;
    let b = tuple_floats(&args[1], "math.cross.b")?;
    if a.len() != 3 || b.len() != 3 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "math.cross: requires two 3-component tuples".to_string(),
            help: Some("cross product is only defined in 3D".to_string()),
        });
    }
    Ok(Value::from_tuple(Rc::new(vec![
        Value::from_float(a[1] * b[2] - a[2] * b[1]),
        Value::from_float(a[2] * b[0] - a[0] * b[2]),
        Value::from_float(a[0] * b[1] - a[1] * b[0]),
    ])))
}

fn math_length(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.length")?;
    let v = tuple_floats(&args[0], "math.length.v")?;
    let sq: f64 = v.iter().map(|x| x * x).sum();
    Ok(Value::from_float(sq.sqrt()))
}

// ---------- Phase 22: typed save helpers ----------

fn save_vec3_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save.vec3")?;
    let key = string_arg(&args[0], "save.vec3", "key")?;
    let v = xyz_of(&args[1], "save.vec3.v")?;
    let tup = Value::from_tuple(Rc::new(vec![
        Value::from_float(v[0] as f64),
        Value::from_float(v[1] as f64),
        Value::from_float(v[2] as f64),
    ]));
    SAVE_STORE.with(|s| s.borrow_mut().insert(key, tup));
    Ok(Value::NIL)
}

fn save_f32_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save.f32")?;
    let key = string_arg(&args[0], "save.f32", "key")?;
    let f = number(&args[1], "save.f32.v")?;
    SAVE_STORE.with(|s| s.borrow_mut().insert(key, Value::from_float(f)));
    Ok(Value::NIL)
}

fn save_int_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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

fn save_string_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "save.string")?;
    let key = string_arg(&args[0], "save.string", "key")?;
    let v = string_arg(&args[1], "save.string", "v")?;
    SAVE_STORE.with(|s| s.borrow_mut().insert(key, Value::from_string(v)));
    Ok(Value::NIL)
}

fn save_get_vec3_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.get_vec3")?;
    let key = string_arg(&args[0], "save.get_vec3", "key")?;
    let result = SAVE_STORE.with(|s| s.borrow().get(&key).copied());
    match result {
        Some(v) if v.is_tuple() && v.as_tuple().len() == 3 => Ok(v),
        _ => Ok(Value::NIL),
    }
}

fn save_get_f32_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.get_f32")?;
    let key = string_arg(&args[0], "save.get_f32", "key")?;
    let result = SAVE_STORE.with(|s| s.borrow().get(&key).copied());
    match result {
        Some(v) if v.is_float() => Ok(v),
        Some(v) if v.is_int() => Ok(Value::from_float(v.as_int() as f64)),
        _ => Ok(Value::NIL),
    }
}

fn save_get_int_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.get_int")?;
    let key = string_arg(&args[0], "save.get_int", "key")?;
    let result = SAVE_STORE.with(|s| s.borrow().get(&key).copied());
    match result {
        Some(v) if v.is_int() => Ok(v),
        _ => Ok(Value::NIL),
    }
}

fn save_get_string_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.get_string")?;
    let key = string_arg(&args[0], "save.get_string", "key")?;
    let result = SAVE_STORE.with(|s| s.borrow().get(&key).copied());
    match result {
        Some(v) if v.is_str() => Ok(v),
        _ => Ok(Value::NIL),
    }
}

fn save_has_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.has")?;
    let key = string_arg(&args[0], "save.has", "key")?;
    let has = SAVE_STORE.with(|s| s.borrow().contains_key(&key));
    Ok(Value::from_bool(has))
}

fn save_remove_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.remove")?;
    let key = string_arg(&args[0], "save.remove", "key")?;
    let removed = SAVE_STORE.with(|s| s.borrow_mut().remove(&key).is_some());
    Ok(Value::from_bool(removed))
}

fn save_clear_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    SAVE_STORE.with(|s| s.borrow_mut().clear());
    Ok(Value::NIL)
}

fn save_write_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.write")?;
    let path = string_arg(&args[0], "save.write", "path")?;
    // Build an Object Value from the store map and pipe through
    // the Phase 8 save_to codec. Avoids a parallel JSON writer.
    let value = SAVE_STORE.with(|s| {
        let mut obj_fields = HashMap::new();
        for (k, v) in s.borrow().iter() {
            obj_fields.insert(k.clone(), *v);
        }
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: obj_fields,
            kind: "save_store",
        })))
    });
    crate::save::save_to_path(std::path::Path::new(&path), &value)
        .map_err(|e| crate::save::to_runtime_error(e, 0, 0))?;
    Ok(Value::NIL)
}

fn save_read_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.read")?;
    let path = string_arg(&args[0], "save.read", "path")?;
    let value = crate::save::load_from_path(std::path::Path::new(&path))
        .map_err(|e| crate::save::to_runtime_error(e, 0, 0))?;
    apply_save_from_value(value);
    Ok(Value::NIL)
}

fn save_try_read_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "save.try_read")?;
    let key = string_arg(&args[0], "save.try_read", "path")?;
    if !std::path::Path::new(&key).exists() {
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

fn apply_save_from_value(value: Value) {
    if !value.is_object() {
        return;
    }
    let rc = value.as_object();
    let o = rc.borrow();
    SAVE_STORE.with(|s| {
        let mut store = s.borrow_mut();
        store.clear();
        for (k, v) in o.fields.iter() {
            store.insert(k.clone(), *v);
        }
    });
}

// ---------- Phase 21: animation state ----------

#[derive(Clone, Default)]
struct MeshAnimEntry {
    /// Active clip name; empty string = stopped.
    clip: String,
    /// Time elapsed since clip started, in seconds.
    time: f32,
    /// Whether the clip loops at the end (vs holding the last frame).
    looping: bool,
    /// Optional blend target — when present, the renderer (or the
    /// script's own logic) interpolates joints between `clip` and
    /// `blend_clip` by `blend_t` ∈ [0, 1].
    blend_clip: Option<String>,
    blend_t: f32,
}

fn mesh_play_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "mesh.play")?;
    let handle = handle_int(&args[0], "mesh.play")?;
    let clip = string_arg(&args[1], "mesh.play", "clip")?;
    let looping = if args[2].is_bool() {
        args[2].as_bool()
    } else {
        false
    };
    MESH_ANIM_STATE.with(|s| {
        let mut st = s.borrow_mut();
        let entry = st.entry(handle).or_default();
        if entry.clip != clip {
            entry.time = 0.0;
        }
        entry.clip = clip;
        entry.looping = looping;
        entry.blend_clip = None;
        entry.blend_t = 0.0;
    });
    Ok(Value::NIL)
}

fn mesh_stop_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mesh.stop")?;
    let handle = handle_int(&args[0], "mesh.stop")?;
    MESH_ANIM_STATE.with(|s| {
        s.borrow_mut().remove(&handle);
    });
    Ok(Value::NIL)
}

fn mesh_blend_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "mesh.blend")?;
    let handle = handle_int(&args[0], "mesh.blend")?;
    let clip_a = string_arg(&args[1], "mesh.blend", "clip_a")?;
    let clip_b = string_arg(&args[2], "mesh.blend", "clip_b")?;
    let t = (number(&args[3], "mesh.blend.t")? as f32).clamp(0.0, 1.0);
    MESH_ANIM_STATE.with(|s| {
        let mut st = s.borrow_mut();
        let entry = st.entry(handle).or_default();
        if entry.clip != clip_a {
            entry.clip = clip_a;
            entry.time = 0.0;
        }
        entry.blend_clip = Some(clip_b);
        entry.blend_t = t;
    });
    Ok(Value::NIL)
}

fn mesh_current_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mesh.current")?;
    let handle = handle_int(&args[0], "mesh.current")?;
    MESH_ANIM_STATE.with(|s| {
        let st = s.borrow();
        let entry = st.get(&handle).cloned().unwrap_or_default();
        let mut fields = HashMap::new();
        fields.insert("clip".to_string(), Value::from_string(entry.clip.clone()));
        fields.insert("time".to_string(), Value::from_float(entry.time as f64));
        fields.insert("looping".to_string(), Value::from_bool(entry.looping));
        Ok(Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "anim_state",
        }))))
    })
}

fn mesh_advance_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mesh.advance")?;
    let dt = number(&args[0], "mesh.advance.dt")? as f32;
    MESH_ANIM_STATE.with(|s| {
        for entry in s.borrow_mut().values_mut() {
            entry.time += dt;
        }
    });
    Ok(Value::NIL)
}

/// Phase 24: expose the per-mesh animation state to the renderer
/// in a form decoupled from `Value` ownership. Returns an empty
/// snapshot (clip=""), which the renderer treats as "rest pose,
/// no animation," when the script never called `mesh_anim.play`
/// for this handle.
pub(crate) fn mesh_anim_state(handle: u32) -> crate::play3d::AnimSnapshot {
    MESH_ANIM_STATE.with(|s| {
        let st = s.borrow();
        match st.get(&handle) {
            Some(e) => crate::play3d::AnimSnapshot {
                clip: e.clip.clone(),
                time: e.time,
                blend_clip: e.blend_clip.clone(),
                blend_t: e.blend_t,
            },
            None => crate::play3d::AnimSnapshot::default(),
        }
    })
}

// ---------- Phase 21: quat helpers ----------

fn quat_to_value(q: [f32; 4]) -> Value {
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

fn quat_from_value(v: &Value, what: &str) -> Result<[f32; 4], RuntimeError> {
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

fn quat_identity_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    Ok(quat_to_value([0.0, 0.0, 0.0, 1.0]))
}

fn quat_from_axis_angle_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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

fn quat_slerp_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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

fn quat_mul_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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

fn quat_to_mat4_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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

// ---------- Phase 19: mat4 helpers ----------

/// Build a Twe mat4 Object from a flat column-major [f32; 16] array.
/// Stored as `kind="mat4"` Object with a single `data` field that's
/// a 16-element list of floats. Avoids needing a new Value variant.
fn mat4_to_value(m: &[f32; 16]) -> Value {
    let elems: Vec<Value> = m.iter().map(|f| Value::from_float(*f as f64)).collect();
    let mut fields = HashMap::new();
    fields.insert(
        "data".to_string(),
        Value::from_list(Rc::new(RefCell::new(elems))),
    );
    Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "mat4",
    })))
}

/// Pull a [f32; 16] out of a Twe mat4 handle. Errors if not a mat4
/// object or if `data` is not a 16-element float list.
fn mat4_from_value(v: &Value, what: &str) -> Result<[f32; 16], RuntimeError> {
    if !v.is_object() {
        let other = *v;
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: expected a mat4, got {}", other.type_name()),
            help: Some("create one with mat4.identity() / mat4.translate(v) / etc.".to_string()),
        });
    }
    let rc = v.as_object();
    let o = rc.borrow();
    if o.kind != "mat4" {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: expected a mat4 Object, got kind '{}'", o.kind),
            help: None,
        });
    }
    let data = o.get_field("data").ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!("{what}: mat4 missing data field"),
        help: None,
    })?;
    if !data.is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: mat4.data is not a list"),
            help: None,
        });
    }
    let list = data.as_list();
    let list = list.borrow();
    if list.len() != 16 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: mat4.data has {} elements, expected 16", list.len()),
            help: None,
        });
    }
    let mut out = [0.0f32; 16];
    for (i, e) in list.iter().enumerate() {
        out[i] = number(e, what)? as f32;
    }
    Ok(out)
}

fn mat4_identity_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    let m: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    Ok(mat4_to_value(&m))
}

fn mat4_translate_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mat4.translate")?;
    let v = xyz_of(&args[0], "mat4.translate.v")?;
    // Column-major: translation lives in column 3 (indices 12,13,14).
    let m: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, v[0], v[1], v[2], 1.0,
    ];
    Ok(mat4_to_value(&m))
}

fn mat4_rotate_x_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mat4.rotate_x")?;
    let a = number(&args[0], "mat4.rotate_x.angle")? as f32;
    let c = a.cos();
    let s = a.sin();
    let m: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    Ok(mat4_to_value(&m))
}

fn mat4_rotate_y_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mat4.rotate_y")?;
    let a = number(&args[0], "mat4.rotate_y.angle")? as f32;
    let c = a.cos();
    let s = a.sin();
    let m: [f32; 16] = [
        c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    Ok(mat4_to_value(&m))
}

fn mat4_rotate_z_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mat4.rotate_z")?;
    let a = number(&args[0], "mat4.rotate_z.angle")? as f32;
    let c = a.cos();
    let s = a.sin();
    let m: [f32; 16] = [
        c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    Ok(mat4_to_value(&m))
}

fn mat4_scale_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "mat4.scale")?;
    let v = xyz_of(&args[0], "mat4.scale.v")?;
    let m: [f32; 16] = [
        v[0], 0.0, 0.0, 0.0, 0.0, v[1], 0.0, 0.0, 0.0, 0.0, v[2], 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    Ok(mat4_to_value(&m))
}

fn mat4_mul_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mat4.mul")?;
    let a = mat4_from_value(&args[0], "mat4.mul.a")?;
    let b = mat4_from_value(&args[1], "mat4.mul.b")?;
    let mut out = [0.0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            // out[col=j, row=i] = sum over k of a[col=k,row=i] * b[col=j,row=k]
            out[j * 4 + i] = a[i] * b[j * 4]
                + a[4 + i] * b[j * 4 + 1]
                + a[8 + i] * b[j * 4 + 2]
                + a[12 + i] * b[j * 4 + 3];
        }
    }
    Ok(mat4_to_value(&out))
}

fn mat4_transform_vec3_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "mat4.transform_vec3")?;
    let m = mat4_from_value(&args[0], "mat4.transform_vec3.m")?;
    let v = xyz_of(&args[1], "mat4.transform_vec3.v")?;
    // Treat v as a point (w=1).
    let x = m[0] * v[0] + m[4] * v[1] + m[8] * v[2] + m[12];
    let y = m[1] * v[0] + m[5] * v[1] + m[9] * v[2] + m[13];
    let z = m[2] * v[0] + m[6] * v[1] + m[10] * v[2] + m[14];
    Ok(Value::from_tuple(Rc::new(vec![
        Value::from_float(x as f64),
        Value::from_float(y as f64),
        Value::from_float(z as f64),
    ])))
}

fn math_normalize(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "math.normalize")?;
    let v = tuple_floats(&args[0], "math.normalize.v")?;
    let len: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if len < 1e-12 {
        // Zero-length vector: return as-is rather than NaN, matching
        // the convention most game engines use for safe normalisation.
        return Ok(args[0]);
    }
    let normalized: Vec<Value> = v.iter().map(|x| Value::from_float(x / len)).collect();
    Ok(Value::from_tuple(Rc::new(normalized)))
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
    // Phase 27 session 4: in-place Fisher-Yates shuffle. Mutates
    // the list. Returns nil. Replaces ~10 lines of inline shuffling
    // every example needed (tetris 7-bag, cards deal, level
    // randomizers).
    random.insert(
        "shuffle".to_string(),
        Value::from_builtin("random.shuffle", &["list"], random_shuffle),
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

// Phase 27 session 4: Fisher-Yates in-place shuffle. Returns nil.
// Empty / single-element lists are no-ops (already "shuffled").
fn random_shuffle(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "random.shuffle")?;
    if !args[0].is_list() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "random.shuffle expected a list, got {}",
                args[0].type_name()
            ),
            help: None,
        });
    }
    let rc = args[0].as_list();
    let mut v = rc.borrow_mut();
    let n = v.len();
    if n < 2 {
        return Ok(Value::NIL);
    }
    let mut i = n - 1;
    while i > 0 {
        // Uniform pick in 0..=i. `next_random_u64() % (i + 1)` has
        // a small bias for non-power-of-two upper bounds; matches
        // what `random.int(0..<n)` already does, so two independent
        // shuffles seeded the same way produce the same permutation.
        let j = (env.next_random_u64() as usize) % (i + 1);
        v.swap(i, j);
        i -= 1;
    }
    Ok(Value::NIL)
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
        ("cyan", 0.0, 1.0, 1.0, 1.0),
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
    // Phase 9 session 6: color pipeline.
    // - from_hex: parse "#rrggbb" / "#rrggbbaa" (the leading '#' is
    //   optional) into a linear-space [0, 1] tuple.
    // - hsv: HSV → RGB constructor with hue in [0, 360).
    // - to_linear / to_srgb: gamma helpers using the IEC 61966-2-1
    //   piecewise sRGB transfer function (matches WGSL's textureLoad
    //   linear-storage convention so visual block colors will round-trip
    //   bit-identical with their CPU samples in Phase 9 session 10).
    //   Alpha is passed through untouched — alpha is always linear.
    // - lerp: sRGB-space (perceptual) component lerp; this is what
    //   the existing `mix(c, c, t)` already does, so it's named
    //   `color.lerp` for surface symmetry with the documented §7.5
    //   stdlib reference.
    // - lerp_linear: gamma-correct lerp (to_linear → mix → to_srgb)
    //   — physically accurate but loses some saturation through
    //   the midtones; offered alongside lerp because both have
    //   legitimate use cases (perceptual gradients vs. physical
    //   blending of light).
    fields.insert(
        "from_hex".to_string(),
        Value::from_builtin("color.from_hex", &["s"], color_from_hex),
    );
    fields.insert(
        "hsv".to_string(),
        Value::from_builtin("color.hsv", &["h", "s", "v"], color_hsv),
    );
    fields.insert(
        "to_linear".to_string(),
        Value::from_builtin("color.to_linear", &["c"], color_to_linear),
    );
    fields.insert(
        "to_srgb".to_string(),
        Value::from_builtin("color.to_srgb", &["c"], color_to_srgb),
    );
    fields.insert(
        "lerp".to_string(),
        Value::from_builtin("color.lerp", &["a", "b", "t"], color_lerp),
    );
    fields.insert(
        "lerp_linear".to_string(),
        Value::from_builtin("color.lerp_linear", &["a", "b", "t"], color_lerp_linear),
    );
    env.set(
        "color".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields,
            kind: "module",
        }))),
    );
}

// IEC 61966-2-1 sRGB → linear (alpha untouched). The 0.04045 cutoff
// + 1/12.92 below + ((c + 0.055) / 1.055) ^ 2.4 above is the standard
// piecewise-defined transfer function (matches WGSL's storage-format
// behavior so colors round-trip CPU↔shader unchanged).
fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn color_to_linear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "color.to_linear")?;
    let (r, g, b, a) = rgba(&args[0], "color.to_linear")?;
    Ok(make_color(
        srgb_to_linear(r),
        srgb_to_linear(g),
        srgb_to_linear(b),
        a,
    ))
}

fn color_to_srgb(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "color.to_srgb")?;
    let (r, g, b, a) = rgba(&args[0], "color.to_srgb")?;
    Ok(make_color(
        linear_to_srgb(r),
        linear_to_srgb(g),
        linear_to_srgb(b),
        a,
    ))
}

fn color_lerp(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "color.lerp")?;
    let (ar, ag, ab, aa) = rgba(&args[0], "color.lerp.a")?;
    let (br, bg, bb, ba) = rgba(&args[1], "color.lerp.b")?;
    let t = as_f64(&args[2], "color.lerp.t")?;
    Ok(make_color(
        ar * (1.0 - t) + br * t,
        ag * (1.0 - t) + bg * t,
        ab * (1.0 - t) + bb * t,
        aa * (1.0 - t) + ba * t,
    ))
}

fn color_lerp_linear(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "color.lerp_linear")?;
    let (ar, ag, ab, aa) = rgba(&args[0], "color.lerp_linear.a")?;
    let (br, bg, bb, ba) = rgba(&args[1], "color.lerp_linear.b")?;
    let t = as_f64(&args[2], "color.lerp_linear.t")?;
    let mix = |x: f64, y: f64| {
        let lx = srgb_to_linear(x);
        let ly = srgb_to_linear(y);
        linear_to_srgb(lx * (1.0 - t) + ly * t)
    };
    Ok(make_color(
        mix(ar, br),
        mix(ag, bg),
        mix(ab, bb),
        // Alpha lerps in straight space — it isn't gamma-encoded.
        aa * (1.0 - t) + ba * t,
    ))
}

fn color_from_hex(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "color.from_hex")?;
    let raw = {
        let t = &args[0];
        if !t.is_str() {
            let other = *t;
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "color.from_hex expected a string, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `color.from_hex(\"#ff8800\")`".to_string()),
            });
        }
        t.as_string().clone()
    };
    let s = raw.strip_prefix('#').unwrap_or(&raw);
    let parse_byte = |span: &str| -> Result<f64, RuntimeError> {
        u8::from_str_radix(span, 16)
            .map(|b| b as f64 / 255.0)
            .map_err(|_| RuntimeError {
                line: 0,
                col: 0,
                message: format!("color.from_hex: invalid hex byte `{span}`"),
                help: Some(
                    "expected `#rrggbb` or `#rrggbbaa` (case-insensitive, '#' optional)"
                        .to_string(),
                ),
            })
    };
    let (r, g, b, a) = match s.len() {
        6 => (
            parse_byte(&s[0..2])?,
            parse_byte(&s[2..4])?,
            parse_byte(&s[4..6])?,
            1.0,
        ),
        8 => (
            parse_byte(&s[0..2])?,
            parse_byte(&s[2..4])?,
            parse_byte(&s[4..6])?,
            parse_byte(&s[6..8])?,
        ),
        _ => {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "color.from_hex: expected 6 or 8 hex digits (rgb / rgba), got {}",
                    s.len()
                ),
                help: Some(
                    "e.g. `color.from_hex(\"#ff8800\")` or `color.from_hex(\"#ff8800cc\")`"
                        .to_string(),
                ),
            });
        }
    };
    Ok(make_color(r, g, b, a))
}

fn color_hsv(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "color.hsv")?;
    let h = as_f64(&args[0], "color.hsv.h")?;
    let s = as_f64(&args[1], "color.hsv.s")?.clamp(0.0, 1.0);
    let v = as_f64(&args[2], "color.hsv.v")?.clamp(0.0, 1.0);
    // Standard HSV→RGB. Hue is reduced into [0, 360); negative hues
    // wrap so `color.hsv(-30, 1, 1)` is equivalent to `color.hsv(330, 1, 1)`.
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Ok(make_color(r1 + m, g1 + m, b1 + m, 1.0))
}

/// Pull (r, g, b, a) out of a 3- or 4-tuple, defaulting alpha to 1.0
/// for the 3-tuple form. Centralises the unpack so every color helper
/// errors with the same message shape.
fn rgba(v: &Value, what: &str) -> Result<(f64, f64, f64, f64), RuntimeError> {
    if !v.is_tuple() {
        let other = *v;
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects an (r, g, b[, a]) tuple, got {}",
                other.type_name()
            ),
            help: Some(
                "use `color.red` etc. or build with `(r, g, b, a)` floats in [0, 1]".to_string(),
            ),
        });
    }
    let elems = v.as_tuple();
    if elems.len() < 3 || elems.len() > 4 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what} expects an (r, g, b[, a]) tuple, got a {}-tuple",
                elems.len()
            ),
            help: None,
        });
    }
    let r = number(&elems[0], what)?;
    let g = number(&elems[1], what)?;
    let b = number(&elems[2], what)?;
    let a = if elems.len() == 4 {
        number(&elems[3], what)?
    } else {
        1.0
    };
    Ok((r, g, b, a))
}

fn make_color(r: f64, g: f64, b: f64, a: f64) -> Value {
    Value::from_tuple(Rc::new(vec![
        Value::from_float(r),
        Value::from_float(g),
        Value::from_float(b),
        Value::from_float(a),
    ]))
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
    // Phase 27 session 4: AABB-vs-tile collision queries. Each
    // samples the four corners of an axis-aligned box at pixel
    // coords (x, y) extending (w, h) and returns true if any
    // corner satisfies the inner predicate (`solid` trait /
    // matching tile name). Replaces the 4-corner sample pattern
    // examples/platformer.twe was repeating three times.
    env.set(
        "tilemap_solid_aabb".to_string(),
        Value::from_builtin(
            "tilemap_solid_aabb",
            &["map", "x", "y", "w", "h"],
            tilemap_solid_aabb,
        ),
    );
    env.set(
        "tilemap_aabb_touches".to_string(),
        Value::from_builtin(
            "tilemap_aabb_touches",
            &["map", "x", "y", "w", "h", "name"],
            tilemap_aabb_touches,
        ),
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

// Phase 27 session 4: shared trait-lookup for the AABB helpers.
// Returns true if the tile under pixel (x, y) carries the named
// trait. Returns false on out-of-bounds, missing tile spec, or
// missing traits list. Mirrors the inline lookup in
// `tilemap_solid_at` but is parameterised on the trait name.
fn tile_has_trait(map: &Rc<RefCell<Object>>, x: f32, y: f32, trait_name: &str) -> bool {
    let name = tilemap_name_at(map, x, y);
    if name.is_empty() {
        return false;
    }
    let m = map.borrow();
    let tile_specs = match m.get_field("tiles") {
        Some(t) if t.is_object() => t.as_object(),
        _ => return false,
    };
    let specs = tile_specs.borrow();
    let tile = match specs.get_field(&name) {
        Some(t) if t.is_object() => t.as_object(),
        _ => return false,
    };
    let traits = match tile.borrow().get_field("traits") {
        Some(t) if t.is_list() => t.as_list().clone(),
        _ => return false,
    };
    let result = traits
        .borrow()
        .iter()
        .any(|v| v.is_str() && v.as_string() == trait_name);
    result
}

fn tilemap_solid_aabb(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "tilemap_solid_aabb")?;
    let map = expect_tilemap(&args[0], "tilemap_solid_aabb.map")?;
    let x = number(&args[1], "tilemap_solid_aabb.x")? as f32;
    let y = number(&args[2], "tilemap_solid_aabb.y")? as f32;
    let w = number(&args[3], "tilemap_solid_aabb.w")? as f32;
    let h = number(&args[4], "tilemap_solid_aabb.h")? as f32;
    let corners = [
        (x, y),
        (x + w - 1.0, y),
        (x, y + h - 1.0),
        (x + w - 1.0, y + h - 1.0),
    ];
    for (cx, cy) in corners {
        if tile_has_trait(&map, cx, cy, "solid") {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::FALSE)
}

fn tilemap_aabb_touches(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 6, "tilemap_aabb_touches")?;
    let map = expect_tilemap(&args[0], "tilemap_aabb_touches.map")?;
    let x = number(&args[1], "tilemap_aabb_touches.x")? as f32;
    let y = number(&args[2], "tilemap_aabb_touches.y")? as f32;
    let w = number(&args[3], "tilemap_aabb_touches.w")? as f32;
    let h = number(&args[4], "tilemap_aabb_touches.h")? as f32;
    let name = string_arg(&args[5], "tilemap_aabb_touches", "name")?;
    let corners = [
        (x, y),
        (x + w - 1.0, y),
        (x, y + h - 1.0),
        (x + w - 1.0, y + h - 1.0),
    ];
    for (cx, cy) in corners {
        if tilemap_name_at(&map, cx, cy) == name {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::FALSE)
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
        "circle_outline".to_string(),
        Value::from_builtin(
            "circle_outline",
            &["at", "radius", "thickness", "color"],
            draw_circle_outline,
        ),
    );
    env.set(
        "line".to_string(),
        Value::from_builtin("line", &["from", "to", "width", "color"], draw_line),
    );
    env.set(
        "text".to_string(),
        Value::from_builtin("text", &["content", "at", "size", "color"], draw_text),
    );
    // Phase 9 session 4: text rendering with a custom TTF/OTF font.
    env.set(
        "text_with_font".to_string(),
        Value::from_builtin(
            "text_with_font",
            &["content", "at", "size", "color", "font"],
            draw_text_with_font,
        ),
    );
    // Phase 10 session 1: immediate-mode `button` widget. Reads
    // ambient mouse state (`mouse.x` / `mouse.y` / `mouse_press.left` /
    // `mouse_held.left`) so scripts don't have to thread it through
    // each call. Returns `true` on the frame the user clicks the
    // button (mouse-press edge inside the rect). Hover/active styling
    // is handled internally; theming knobs are deferred to a later
    // session if real users push back.
    env.set(
        "button".to_string(),
        Value::from_builtin("button", &["at", "size", "label"], draw_button),
    );
    // Phase 10 session 2: stateless display widgets. `label` renders
    // text centered inside a (w, h) box — same call shape as `button`
    // so they line up cleanly under a future layout primitive.
    // `progress_bar` renders a 0..1 fill inside an outlined frame —
    // useful for HP bars, loading screens, settings volume meters.
    env.set(
        "label".to_string(),
        Value::from_builtin("label", &["at", "size", "text"], draw_label),
    );
    env.set(
        "progress_bar".to_string(),
        Value::from_builtin("progress_bar", &["at", "size", "value"], draw_progress_bar),
    );
    // Phase 10 session 3: `slider(at:, size:, value:, min:, max:) -> float`.
    // Drag-state widget: the user click-and-drags the knob; the builtin
    // returns the updated value each frame. Only one slider can be
    // dragging at a time — tracked via `UI_STATE.active_slider` so a
    // second slider in the same scene doesn't fight for the cursor.
    env.set(
        "slider".to_string(),
        Value::from_builtin(
            "slider",
            &["at", "size", "value", "min", "max"],
            draw_slider,
        ),
    );
    // Phase 10 session 4: selection widgets. `checkbox` is stateless
    // (its boolean is owned by the script's `var`); `dropdown` carries
    // open/closed state in `UI_STATE.open_dropdown` since the second
    // frame after click needs to know which dropdown to expand.
    env.set(
        "checkbox".to_string(),
        Value::from_builtin("checkbox", &["at", "size", "value"], draw_checkbox),
    );
    env.set(
        "dropdown".to_string(),
        Value::from_builtin(
            "dropdown",
            &["at", "size", "options", "selected"],
            draw_dropdown,
        ),
    );
    // Phase 10 session 5: `text_input(at:, size:, value:) -> string`.
    // Click to focus, type characters to append, backspace to delete.
    // Returns the (possibly edited) string each frame so the script
    // does `name = text_input(at:, size:, value: name)`. Clipboard
    // (`os.clipboard.read/write`) is a separate session 5b follow-on
    // since it needs an OS-clipboard dependency.
    env.set(
        "text_input".to_string(),
        Value::from_builtin("text_input", &["at", "size", "value"], draw_text_input),
    );
    // Phase 10 session 11: `key_input(at:, size:, value:) -> string`.
    // The keybind capture widget. Click to focus; the next key pressed
    // becomes the binding. `value` is the current binding name (e.g.
    // "right"); the widget returns the new binding next frame after a
    // key is pressed, otherwise echoes the input unchanged. Driven off
    // the existing `key_press` ambient — no separate input plumbing.
    env.set(
        "key_input".to_string(),
        Value::from_builtin("key_input", &["at", "size", "value"], draw_key_input),
    );
    // Phase 10 session 6: layout primitives. `panel` is a UI-themed
    // background rect (the visual frame for grouped widgets).
    // `stack` and `flex` are positioning helpers — they don't draw,
    // they return a {at, size} object naming the rect of the i-th
    // slot in a vertical (`stack`) or horizontal (`flex`) layout.
    // The script then passes `slot.at` and `slot.size` as the `at:`
    // and `size:` of a child widget. Same shape as the `mouse.pos`
    // / `mouse.x` / `mouse.y` ambient pattern: layout returns
    // structured data, the script destructures via field access.
    env.set(
        "panel".to_string(),
        Value::from_builtin("panel", &["at", "size"], draw_panel),
    );
    env.set(
        "stack".to_string(),
        Value::from_builtin(
            "stack",
            &["at", "size", "count", "index", "gap"],
            layout_stack,
        ),
    );
    env.set(
        "flex".to_string(),
        Value::from_builtin(
            "flex",
            &["at", "size", "count", "index", "gap"],
            layout_flex,
        ),
    );
    // Phase 10 session 7: 2D layout (`grid`) and stateful clipping
    // (`scroll`). `grid` is row-major: index 0 = top-left, index
    // (cols-1) = top-right, index cols = next row's leftmost.
    // `scroll` keeps a per-rect scroll-y state in `UI_STATE.scroll_y`,
    // updates from `mouse.wheel` while hovered, and returns a
    // `{at, size, scroll_y}` object so child widgets can apply the
    // offset themselves (no implicit GL scissor in v1 — scripts
    // place the children inside the visible band manually).
    env.set(
        "grid".to_string(),
        Value::from_builtin(
            "grid",
            &["at", "size", "cols", "rows", "index", "gap"],
            layout_grid,
        ),
    );
    env.set(
        "scroll".to_string(),
        Value::from_builtin("scroll", &["at", "size", "content_height"], layout_scroll),
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
            // Phase 12 session 3: bundle-first lookup, filesystem fallback.
            let bytes = crate::bundle::read_asset_bytes(&path).map_err(|e| RuntimeError {
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
            // Phase 12 session 3: bundle-first lookup, filesystem fallback.
            let bytes = crate::bundle::read_asset_bytes(path).map_err(|e| RuntimeError {
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

fn draw_circle_outline(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "circle_outline")?;
    arity(args, 4, "circle_outline")?;
    let (x, y) = xy_of(&args[0], "circle_outline.at")?;
    let radius = number(&args[1], "circle_outline.radius")? as f32;
    let thickness = number(&args[2], "circle_outline.thickness")? as f32;
    let color = color_of(&args[3], "circle_outline.color")?;
    macroquad::shapes::draw_circle_lines(x as f32, y as f32, radius, thickness, color);
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

/// `text_with_font(content, at, size, color, font)` — same as `text`
/// but renders with a custom TTF/OTF loaded via `load_font`. Phase 9
/// session 4.
fn draw_text_with_font(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "text_with_font")?;
    arity(args, 5, "text_with_font")?;
    let content = {
        let t = &args[0];
        if t.is_str() {
            t.as_string().clone()
        } else {
            (*t).display()
        }
    };
    let (x, y) = xy_of(&args[1], "text_with_font.at")?;
    let size = number(&args[2], "text_with_font.size")? as f32;
    let color = color_of(&args[3], "text_with_font.color")?;
    let path = font_handle_path(&args[4], "text_with_font")?;
    // Lazy-decode the Font on first use. The bytes were validated +
    // cached by `load_font` (headless-safe); the actual macroquad
    // parse needs THREAD_ID, which only exists once we're inside the
    // render frame — here.
    ensure_font_decoded(&path)?;
    FONT_CACHE.with(|cache| -> Result<(), RuntimeError> {
        let c = cache.borrow();
        let font = c.get(&path).ok_or_else(|| RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "text_with_font: internal — font '{path}' lazy-decode failed silently"
            ),
            help: None,
        })?;
        macroquad::text::draw_text_ex(
            &content,
            x as f32,
            y as f32,
            macroquad::text::TextParams {
                font: Some(font),
                font_size: size as u16,
                color,
                ..Default::default()
            },
        );
        Ok(())
    })?;
    Ok(Value::NIL)
}

/// Phase 10 session 1: immediate-mode `button(at:, size:, label:)` widget.
///
/// Reads ambient `mouse.x` / `mouse.y` / `mouse_press.left` / `mouse_held.left`,
/// hit-tests against the button rect, draws state-styled background +
/// centered label, and returns `true` on the frame the user clicks
/// inside the rect. Idle / hover / active backgrounds and a hover
/// border are baked in for v1; theming knobs (custom colors, fonts,
/// padding) are deferred until the session-1 surface meets a real game.
fn draw_button(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "button")?;
    arity(args, 3, "button")?;
    let (x, y) = xy_of(&args[0], "button.at")?;
    let (w, h) = xy_of(&args[1], "button.size")?;
    let label = {
        let t = &args[2];
        if t.is_str() {
            t.as_string().clone()
        } else {
            (*t).display()
        }
    };

    let (mx, my) = read_mouse_xy(env);
    let pressed_now = read_mouse_button(env, "mouse_press", "left");
    let held_now = read_mouse_button(env, "mouse_held", "left");
    let hovered = point_in_rect(mx, my, x, y, w, h);

    let bg = if hovered && held_now {
        macroquad::color::Color::new(0.45, 0.45, 0.50, 1.0)
    } else if hovered {
        macroquad::color::Color::new(0.30, 0.30, 0.35, 1.0)
    } else {
        macroquad::color::Color::new(0.18, 0.18, 0.22, 1.0)
    };
    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, bg);
    if hovered {
        macroquad::shapes::draw_rectangle_lines(
            x as f32,
            y as f32,
            w as f32,
            h as f32,
            2.0,
            macroquad::color::Color::new(1.0, 1.0, 1.0, 1.0),
        );
    }

    let font_size: f32 = 18.0;
    let dim = macroquad::text::measure_text(&label, None, font_size as u16, 1.0);
    let tx = x as f32 + (w as f32 - dim.width) / 2.0;
    // measure_text reports `height` as the glyph height above the
    // baseline; macroquad's draw_text takes the baseline y, so center
    // by adding half the glyph height to the rect midline.
    let ty = y as f32 + (h as f32 + dim.height) / 2.0;
    macroquad::text::draw_text(
        &label,
        tx,
        ty,
        font_size,
        macroquad::color::Color::new(1.0, 1.0, 1.0, 1.0),
    );

    Ok(if hovered && pressed_now {
        Value::TRUE
    } else {
        Value::FALSE
    })
}

/// Inclusive-on-top-left, exclusive-on-bottom-right point-in-rect test.
/// Pure helper kept separate so it's unit-testable without a render
/// context; matches the semantics used by `draw_button`'s hit test.
pub(crate) fn point_in_rect(px: f64, py: f64, rx: f64, ry: f64, rw: f64, rh: f64) -> bool {
    px >= rx && px < rx + rw && py >= ry && py < ry + rh
}

/// Phase 10 session 2: `label(at:, size:, text:)` — render text
/// centered inside a (w, h) box. No background, no border. Same call
/// shape as `button` so the two compose visually under any future
/// layout primitive.
fn draw_label(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "label")?;
    arity(args, 3, "label")?;
    let (x, y) = xy_of(&args[0], "label.at")?;
    let (w, h) = xy_of(&args[1], "label.size")?;
    let text = {
        let t = &args[2];
        if t.is_str() {
            t.as_string().clone()
        } else {
            (*t).display()
        }
    };
    let font_size: f32 = 18.0;
    let dim = macroquad::text::measure_text(&text, None, font_size as u16, 1.0);
    let tx = x as f32 + (w as f32 - dim.width) / 2.0;
    let ty = y as f32 + (h as f32 + dim.height) / 2.0;
    macroquad::text::draw_text(
        &text,
        tx,
        ty,
        font_size,
        macroquad::color::Color::new(1.0, 1.0, 1.0, 1.0),
    );
    Ok(Value::NIL)
}

/// Phase 10 session 2: `progress_bar(at:, size:, value:)` — render a
/// horizontal 0..1 fill inside an outlined frame. Values clamp to
/// [0, 1] silently rather than erroring, since `progress_bar(value: hp / max_hp)`
/// drifting slightly out of range due to float rounding shouldn't crash.
fn draw_progress_bar(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "progress_bar")?;
    arity(args, 3, "progress_bar")?;
    let (x, y) = xy_of(&args[0], "progress_bar.at")?;
    let (w, h) = xy_of(&args[1], "progress_bar.size")?;
    let raw = number(&args[2], "progress_bar.value")?;
    let value = raw.clamp(0.0, 1.0);

    let frame_color = macroquad::color::Color::new(0.18, 0.18, 0.22, 1.0);
    let fill_color = macroquad::color::Color::new(0.30, 0.65, 0.85, 1.0);
    let border_color = macroquad::color::Color::new(0.85, 0.85, 0.90, 1.0);

    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, frame_color);
    let fill_w = (w * value) as f32;
    if fill_w > 0.0 {
        macroquad::shapes::draw_rectangle(x as f32, y as f32, fill_w, h as f32, fill_color);
    }
    macroquad::shapes::draw_rectangle_lines(
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        2.0,
        border_color,
    );
    Ok(Value::NIL)
}

/// Phase 10 session 3: `slider(at:, size:, value:, min:, max:) -> float`.
/// Drag-state widget — click and drag the knob to scrub the value.
/// While dragging, the value updates each frame from the cursor x
/// position relative to the slider rect; otherwise the input value
/// passes through unchanged (so the widget is *driven* by the script's
/// `var` and stays in sync after, say, a "reset" button).
fn draw_slider(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "slider")?;
    arity(args, 5, "slider")?;
    let (x, y) = xy_of(&args[0], "slider.at")?;
    let (w, h) = xy_of(&args[1], "slider.size")?;
    let value_in = number(&args[2], "slider.value")?;
    let min = number(&args[3], "slider.min")?;
    let max = number(&args[4], "slider.max")?;

    let id = rect_id(x, y, w, h);
    let (mx, my) = read_mouse_xy(env);
    let pressed_now = read_mouse_button(env, "mouse_press", "left");
    let held_now = read_mouse_button(env, "mouse_held", "left");
    let hovered = point_in_rect(mx, my, x, y, w, h);

    // Drag-state transitions.
    let active = UI_STATE.with(|s| s.borrow().active_slider == Some(id));
    if hovered && pressed_now {
        UI_STATE.with(|s| s.borrow_mut().active_slider = Some(id));
    }
    if !held_now && active {
        UI_STATE.with(|s| s.borrow_mut().active_slider = None);
    }
    let active = UI_STATE.with(|s| s.borrow().active_slider == Some(id));

    // Compute current value. While dragging, project mouse.x to
    // [min, max]; otherwise pass the input through.
    let value = if active && (max - min).abs() > f64::EPSILON {
        let t = ((mx - x) / w).clamp(0.0, 1.0);
        min + t * (max - min)
    } else {
        value_in
    };

    let track_color = macroquad::color::Color::new(0.18, 0.18, 0.22, 1.0);
    let knob_color = if active {
        macroquad::color::Color::new(0.85, 0.85, 0.95, 1.0)
    } else if hovered {
        macroquad::color::Color::new(0.65, 0.65, 0.80, 1.0)
    } else {
        macroquad::color::Color::new(0.50, 0.50, 0.60, 1.0)
    };
    let border_color = macroquad::color::Color::new(0.85, 0.85, 0.90, 1.0);

    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, track_color);
    let t = if (max - min).abs() > f64::EPSILON {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let knob_w = 12.0_f32;
    let knob_x = x as f32 + (t as f32) * (w as f32 - knob_w);
    macroquad::shapes::draw_rectangle(knob_x, y as f32, knob_w, h as f32, knob_color);
    macroquad::shapes::draw_rectangle_lines(
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        2.0,
        border_color,
    );

    Ok(Value::from_float(value))
}

/// Phase 10 session 4: `checkbox(at:, size:, value:) -> bool`. Returns
/// the toggled value on the click frame; otherwise passes input
/// through. The check mark is two short lines drawn inside the box
/// when value is true — keeps the visual recognizable without bringing
/// in glyph rendering.
fn draw_checkbox(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "checkbox")?;
    arity(args, 3, "checkbox")?;
    let (x, y) = xy_of(&args[0], "checkbox.at")?;
    let (w, h) = xy_of(&args[1], "checkbox.size")?;
    let value_in = {
        let v = &args[2];
        if v.is_bool() {
            v.as_bool()
        } else {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("checkbox.value expects a bool, got {}", (*v).type_name()),
                help: Some("pass a `var` you toggle via the return value".to_string()),
            });
        }
    };

    let (mx, my) = read_mouse_xy(env);
    let pressed_now = read_mouse_button(env, "mouse_press", "left");
    let held_now = read_mouse_button(env, "mouse_held", "left");
    let hovered = point_in_rect(mx, my, x, y, w, h);
    let value = if hovered && pressed_now {
        !value_in
    } else {
        value_in
    };

    let bg = if hovered && held_now {
        macroquad::color::Color::new(0.45, 0.45, 0.50, 1.0)
    } else if hovered {
        macroquad::color::Color::new(0.30, 0.30, 0.35, 1.0)
    } else {
        macroquad::color::Color::new(0.18, 0.18, 0.22, 1.0)
    };
    let border = macroquad::color::Color::new(0.85, 0.85, 0.90, 1.0);
    let check = macroquad::color::Color::new(0.40, 0.85, 0.45, 1.0);

    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, bg);
    macroquad::shapes::draw_rectangle_lines(x as f32, y as f32, w as f32, h as f32, 2.0, border);
    if value {
        // Two-segment check mark inside the box, padded by 25% of
        // the smaller dimension so it visually fits any aspect.
        let pad = (w.min(h) * 0.25) as f32;
        let x0 = x as f32 + pad;
        let y0 = y as f32 + (h as f32) * 0.55;
        let x1 = x as f32 + (w as f32) * 0.42;
        let y1 = y as f32 + (h as f32) - pad;
        let x2 = x as f32 + (w as f32) - pad;
        let y2 = y as f32 + pad;
        macroquad::shapes::draw_line(x0, y0, x1, y1, 3.0, check);
        macroquad::shapes::draw_line(x1, y1, x2, y2, 3.0, check);
    }

    Ok(Value::from_bool(value))
}

/// Phase 10 session 4: `dropdown(at:, size:, options:, selected:) -> int`.
/// `options` is a list of strings, `selected` is the current 0-based
/// index. When closed (the default), shows just the current option +
/// a downward chevron. Click to open; click an option to select; click
/// outside or pick the same option to close. Returns the new selected
/// index. Stateful via `UI_STATE.open_dropdown` keyed by the rect.
fn draw_dropdown(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "dropdown")?;
    arity(args, 4, "dropdown")?;
    let (x, y) = xy_of(&args[0], "dropdown.at")?;
    let (w, h) = xy_of(&args[1], "dropdown.size")?;
    let options: Vec<String> = {
        let v = &args[2];
        if !v.is_list() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "dropdown.options expects a list of strings, got {}",
                    (*v).type_name()
                ),
                help: None,
            });
        }
        let rc = v.as_list();
        let borrowed = rc.borrow();
        borrowed
            .iter()
            .map(|item| {
                if item.is_str() {
                    item.as_string().clone()
                } else {
                    item.display()
                }
            })
            .collect()
    };
    let selected_in = {
        let v = &args[3];
        if !v.is_int_or_boxed_int() {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "dropdown.selected expects an int index, got {}",
                    (*v).type_name()
                ),
                help: None,
            });
        }
        v.as_int() as usize
    };
    let n = options.len();
    if n == 0 {
        return Ok(Value::from_int(0));
    }

    let id = rect_id(x, y, w, h);
    let (mx, my) = read_mouse_xy(env);
    let pressed_now = read_mouse_button(env, "mouse_press", "left");
    let header_hovered = point_in_rect(mx, my, x, y, w, h);
    let was_open = UI_STATE.with(|s| s.borrow().open_dropdown == Some(id));

    // Determine new selection + open state.
    let mut new_selected = selected_in.min(n - 1);
    let mut open_after = was_open;
    if header_hovered && pressed_now {
        open_after = !was_open;
    } else if was_open && pressed_now {
        // Check option-list clicks. Options stack downward below
        // the header at the same width and (h)-tall each.
        for (i, _) in options.iter().enumerate() {
            let oy = y + h + (i as f64) * h;
            if point_in_rect(mx, my, x, oy, w, h) {
                new_selected = i;
                open_after = false;
                break;
            }
        }
        if open_after {
            // Click landed outside both the header and any option —
            // dismiss the panel without changing selection.
            open_after = false;
        }
    }
    UI_STATE.with(|s| {
        let mut st = s.borrow_mut();
        if open_after {
            st.open_dropdown = Some(id);
        } else if was_open {
            st.open_dropdown = None;
        }
    });

    // Draw header.
    let bg = if header_hovered {
        macroquad::color::Color::new(0.30, 0.30, 0.35, 1.0)
    } else {
        macroquad::color::Color::new(0.18, 0.18, 0.22, 1.0)
    };
    let border = macroquad::color::Color::new(0.85, 0.85, 0.90, 1.0);
    let txt = macroquad::color::Color::new(1.0, 1.0, 1.0, 1.0);
    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, bg);
    macroquad::shapes::draw_rectangle_lines(x as f32, y as f32, w as f32, h as f32, 2.0, border);
    let label = &options[new_selected.min(n - 1)];
    let font_size: f32 = 18.0;
    let dim = macroquad::text::measure_text(label, None, font_size as u16, 1.0);
    macroquad::text::draw_text(
        label,
        x as f32 + 8.0,
        y as f32 + (h as f32 + dim.height) / 2.0,
        font_size,
        txt,
    );
    // Down-chevron at right edge.
    let cx = x as f32 + w as f32 - 14.0;
    let cy = y as f32 + h as f32 / 2.0 - 2.0;
    macroquad::shapes::draw_line(cx, cy, cx + 4.0, cy + 5.0, 2.0, border);
    macroquad::shapes::draw_line(cx + 4.0, cy + 5.0, cx + 8.0, cy, 2.0, border);

    // Draw open list, if open.
    if open_after {
        for (i, opt) in options.iter().enumerate() {
            let oy = y + h + (i as f64) * h;
            let row_hovered = point_in_rect(mx, my, x, oy, w, h);
            let row_bg = if row_hovered {
                macroquad::color::Color::new(0.30, 0.30, 0.45, 1.0)
            } else {
                macroquad::color::Color::new(0.22, 0.22, 0.28, 1.0)
            };
            macroquad::shapes::draw_rectangle(x as f32, oy as f32, w as f32, h as f32, row_bg);
            let dim = macroquad::text::measure_text(opt, None, font_size as u16, 1.0);
            macroquad::text::draw_text(
                opt,
                x as f32 + 8.0,
                oy as f32 + (h as f32 + dim.height) / 2.0,
                font_size,
                txt,
            );
        }
        // Outline the whole panel to keep visual unity.
        let panel_h = h as f32 + (n as f32) * h as f32;
        macroquad::shapes::draw_rectangle_lines(x as f32, y as f32, w as f32, panel_h, 2.0, border);
    }

    Ok(Value::from_int(new_selected as i64))
}

/// Phase 10 session 5: `text_input(at:, size:, value:) -> string`.
/// Single-line text entry. Click inside the rect to focus; click
/// elsewhere to unfocus. While focused, printable characters from
/// macroquad's `get_char_pressed` queue are appended to the value
/// and `Backspace` removes the last character. Returns the updated
/// string each frame.
fn draw_text_input(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "text_input")?;
    arity(args, 3, "text_input")?;
    let (x, y) = xy_of(&args[0], "text_input.at")?;
    let (w, h) = xy_of(&args[1], "text_input.size")?;
    let mut value = {
        let v = &args[2];
        if v.is_str() {
            v.as_string().clone()
        } else {
            (*v).display()
        }
    };

    let id = rect_id(x, y, w, h);
    let (mx, my) = read_mouse_xy(env);
    let pressed_now = read_mouse_button(env, "mouse_press", "left");
    let hovered = point_in_rect(mx, my, x, y, w, h);

    // Focus transitions. Click inside focuses. Click outside (anywhere
    // else on the screen) unfocuses. Multiple text_inputs share the
    // single focus slot, so clicking from one to another swaps cleanly.
    let was_focused = UI_STATE.with(|s| s.borrow().focused_text_input == Some(id));
    if pressed_now {
        UI_STATE.with(|s| {
            let mut st = s.borrow_mut();
            if hovered {
                st.focused_text_input = Some(id);
            } else if st.focused_text_input == Some(id) {
                st.focused_text_input = None;
            }
        });
    }
    let focused = UI_STATE.with(|s| s.borrow().focused_text_input == Some(id));

    // Consume input while focused. macroquad queues characters until
    // drained; if no text_input is ever focused they'll just sit there
    // until something focuses, which is acceptable (no infinite-growth
    // bug since macroquad caps the queue internally).
    if focused {
        // Ctrl+V paste lands first so it doesn't get swallowed by the
        // char-press loop (some platforms also emit a `\u{16}` SYN
        // char on Ctrl+V which the loop's is_control filter would
        // drop, but we explicitly read the OS clipboard here for the
        // human-readable text). Phase 10 session 5b.
        let ctrl_held = macroquad::input::is_key_down(macroquad::input::KeyCode::LeftControl)
            || macroquad::input::is_key_down(macroquad::input::KeyCode::RightControl);
        if ctrl_held && macroquad::input::is_key_pressed(macroquad::input::KeyCode::V) {
            #[cfg(not(target_arch = "wasm32"))]
            if let Ok(mut c) = arboard::Clipboard::new() {
                if let Ok(s) = c.get_text() {
                    for ch in s.chars() {
                        if !ch.is_control() {
                            value.push(ch);
                        }
                    }
                }
            }
        }
        loop {
            match macroquad::input::get_char_pressed() {
                Some(ch) if !ch.is_control() => value.push(ch),
                Some(_) => {}
                None => break,
            }
        }
        if macroquad::input::is_key_pressed(macroquad::input::KeyCode::Backspace) {
            value.pop();
        }
    }
    let _ = was_focused;

    let bg = if focused {
        macroquad::color::Color::new(0.10, 0.10, 0.14, 1.0)
    } else if hovered {
        macroquad::color::Color::new(0.22, 0.22, 0.28, 1.0)
    } else {
        macroquad::color::Color::new(0.18, 0.18, 0.22, 1.0)
    };
    let border_color = if focused {
        macroquad::color::Color::new(0.40, 0.85, 0.45, 1.0)
    } else {
        macroquad::color::Color::new(0.85, 0.85, 0.90, 1.0)
    };
    let txt_color = macroquad::color::Color::new(1.0, 1.0, 1.0, 1.0);

    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, bg);
    macroquad::shapes::draw_rectangle_lines(
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        2.0,
        border_color,
    );

    let font_size: f32 = 18.0;
    let dim = macroquad::text::measure_text(&value, None, font_size as u16, 1.0);
    let baseline_y = y as f32 + (h as f32 + dim.height) / 2.0;
    macroquad::text::draw_text(&value, x as f32 + 8.0, baseline_y, font_size, txt_color);

    if focused {
        // 1Hz blink — `time::Instant` is overkill for a UI cursor.
        // Use `macroquad::time::get_time()` (seconds since start).
        let t = macroquad::time::get_time();
        if t.fract() < 0.5 {
            let cursor_x = x as f32 + 8.0 + dim.width + 1.0;
            macroquad::shapes::draw_line(
                cursor_x,
                y as f32 + 4.0,
                cursor_x,
                y as f32 + h as f32 - 4.0,
                2.0,
                txt_color,
            );
        }
    }

    Ok(Value::from_string(value))
}

/// Phase 10 session 11: `key_input(at:, size:, value:) -> string`
/// — the keybind capture widget. Click the rect to focus, then
/// press any key in the `key_press` ambient's name set; the field
/// name is returned as the new binding and focus is released.
/// Clicks outside the rect cancel without rebinding. Used together
/// with `key_held(name)` / `key_pressed(name)` so games can persist
/// bindings via `settings` and read them back at runtime.
fn draw_key_input(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "key_input")?;
    arity(args, 3, "key_input")?;
    let (x, y) = xy_of(&args[0], "key_input.at")?;
    let (w, h) = xy_of(&args[1], "key_input.size")?;
    let mut value = {
        let v = &args[2];
        if v.is_str() {
            v.as_string().clone()
        } else {
            (*v).display()
        }
    };

    let id = rect_id(x, y, w, h);
    let (mx, my) = read_mouse_xy(env);
    let pressed_now = read_mouse_button(env, "mouse_press", "left");
    let hovered = point_in_rect(mx, my, x, y, w, h);

    if pressed_now {
        UI_STATE.with(|s| {
            let mut st = s.borrow_mut();
            if hovered {
                st.focused_key_input = Some(id);
            } else if st.focused_key_input == Some(id) {
                st.focused_key_input = None;
            }
        });
    }
    let focused = UI_STATE.with(|s| s.borrow().focused_key_input == Some(id));

    if focused {
        // Walk the `key_press` ambient looking for the first field
        // that's true this frame. Iteration order is
        // HashMap-arbitrary; the user is pressing one key at a time
        // in practice, so collisions are rare. Skip "escape" so it
        // can act as cancel.
        let mut captured: Option<String> = None;
        if let Some(kp) = env.get("key_press") {
            if kp.is_object() {
                let rc = kp.as_object();
                let o = rc.borrow();
                for (k, v) in &o.fields {
                    if k == "escape" {
                        continue;
                    }
                    if v.is_bool() && v.as_bool() {
                        captured = Some(k.clone());
                        break;
                    }
                }
            }
        }
        if let Some(name) = captured {
            value = name;
            UI_STATE.with(|s| s.borrow_mut().focused_key_input = None);
        } else {
            // Cancel on Escape so a user can back out of capture.
            let escape_pressed = env
                .get("key_press")
                .filter(|v| v.is_object())
                .and_then(|v| {
                    let rc = v.as_object();
                    let o = rc.borrow();
                    o.get_field("escape")
                })
                .map(|v| v.is_bool() && v.as_bool())
                .unwrap_or(false);
            if escape_pressed {
                UI_STATE.with(|s| s.borrow_mut().focused_key_input = None);
            }
        }
    }

    let bg = if focused {
        macroquad::color::Color::new(0.10, 0.10, 0.14, 1.0)
    } else if hovered {
        macroquad::color::Color::new(0.22, 0.22, 0.28, 1.0)
    } else {
        macroquad::color::Color::new(0.18, 0.18, 0.22, 1.0)
    };
    let border_color = if focused {
        macroquad::color::Color::new(0.95, 0.75, 0.30, 1.0)
    } else {
        macroquad::color::Color::new(0.85, 0.85, 0.90, 1.0)
    };
    let txt_color = macroquad::color::Color::new(1.0, 1.0, 1.0, 1.0);

    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, bg);
    macroquad::shapes::draw_rectangle_lines(
        x as f32,
        y as f32,
        w as f32,
        h as f32,
        2.0,
        border_color,
    );

    let display = if focused {
        "press a key...".to_string()
    } else {
        value.clone()
    };
    let font_size: f32 = 18.0;
    let dim = macroquad::text::measure_text(&display, None, font_size as u16, 1.0);
    let tx = x as f32 + (w as f32 - dim.width) / 2.0;
    let ty = y as f32 + (h as f32 + dim.height) / 2.0;
    macroquad::text::draw_text(&display, tx, ty, font_size, txt_color);

    Ok(Value::from_string(value))
}

/// Phase 10 session 6: `panel(at:, size:)` — UI-themed background
/// rect with border. Visual frame for grouped widgets (settings
/// panels, dialog boxes, info cards). Slightly darker than the
/// page background, with a thin neutral border. No state.
fn draw_panel(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "panel")?;
    arity(args, 2, "panel")?;
    let (x, y) = xy_of(&args[0], "panel.at")?;
    let (w, h) = xy_of(&args[1], "panel.size")?;
    let bg = macroquad::color::Color::new(0.14, 0.14, 0.18, 1.0);
    let border = macroquad::color::Color::new(0.65, 0.65, 0.72, 1.0);
    macroquad::shapes::draw_rectangle(x as f32, y as f32, w as f32, h as f32, bg);
    macroquad::shapes::draw_rectangle_lines(x as f32, y as f32, w as f32, h as f32, 2.0, border);
    Ok(Value::NIL)
}

/// Build a layout-result Object with `.at` (2-tuple) and `.size`
/// (2-tuple) fields. Used by `stack`, `flex`, `grid` so scripts can
/// destructure with `slot.at` / `slot.size` — same access shape as
/// the existing `mouse.pos` ambient.
fn layout_slot(at_x: f64, at_y: f64, sz_w: f64, sz_h: f64) -> Value {
    let at = Value::from_tuple(Rc::new(vec![
        Value::from_float(at_x),
        Value::from_float(at_y),
    ]));
    let size = Value::from_tuple(Rc::new(vec![
        Value::from_float(sz_w),
        Value::from_float(sz_h),
    ]));
    let mut fields = HashMap::new();
    fields.insert("at".to_string(), at);
    fields.insert("size".to_string(), size);
    Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "layout",
    })))
}

/// Pure helper: compute the rect of slot `index` in a stack of
/// `count` evenly-spaced rows. Negative or out-of-range `index`
/// is clamped — mirrors `progress_bar`'s value-clamp policy:
/// avoid panics on float-rounding edge cases.
pub(crate) fn stack_slot(
    at_x: f64,
    at_y: f64,
    sz_w: f64,
    sz_h: f64,
    count: i64,
    index: i64,
    gap: f64,
) -> (f64, f64, f64, f64) {
    let n = count.max(1) as f64;
    let i = index.clamp(0, count.max(1) - 1) as f64;
    let total_gap = gap * (n - 1.0).max(0.0);
    let slot_h = ((sz_h - total_gap) / n).max(0.0);
    let slot_y = at_y + i * (slot_h + gap);
    (at_x, slot_y, sz_w, slot_h)
}

pub(crate) fn flex_slot(
    at_x: f64,
    at_y: f64,
    sz_w: f64,
    sz_h: f64,
    count: i64,
    index: i64,
    gap: f64,
) -> (f64, f64, f64, f64) {
    let n = count.max(1) as f64;
    let i = index.clamp(0, count.max(1) - 1) as f64;
    let total_gap = gap * (n - 1.0).max(0.0);
    let slot_w = ((sz_w - total_gap) / n).max(0.0);
    let slot_x = at_x + i * (slot_w + gap);
    (slot_x, at_y, slot_w, sz_h)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn grid_slot(
    at_x: f64,
    at_y: f64,
    sz_w: f64,
    sz_h: f64,
    cols: i64,
    rows: i64,
    index: i64,
    gap: f64,
) -> (f64, f64, f64, f64) {
    let c = cols.max(1);
    let r = rows.max(1);
    let i = index.clamp(0, c * r - 1);
    let col = (i % c) as f64;
    let row = (i / c) as f64;
    let cf = c as f64;
    let rf = r as f64;
    let total_gap_x = gap * (cf - 1.0).max(0.0);
    let total_gap_y = gap * (rf - 1.0).max(0.0);
    let slot_w = ((sz_w - total_gap_x) / cf).max(0.0);
    let slot_h = ((sz_h - total_gap_y) / rf).max(0.0);
    let slot_x = at_x + col * (slot_w + gap);
    let slot_y = at_y + row * (slot_h + gap);
    (slot_x, slot_y, slot_w, slot_h)
}

fn layout_stack(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "stack")?;
    let (x, y) = xy_of(&args[0], "stack.at")?;
    let (w, h) = xy_of(&args[1], "stack.size")?;
    let count = number(&args[2], "stack.count")? as i64;
    let index = number(&args[3], "stack.index")? as i64;
    let gap = number(&args[4], "stack.gap")?;
    let (sx, sy, sw, sh) = stack_slot(x, y, w, h, count, index, gap);
    Ok(layout_slot(sx, sy, sw, sh))
}

fn layout_flex(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 5, "flex")?;
    let (x, y) = xy_of(&args[0], "flex.at")?;
    let (w, h) = xy_of(&args[1], "flex.size")?;
    let count = number(&args[2], "flex.count")? as i64;
    let index = number(&args[3], "flex.index")? as i64;
    let gap = number(&args[4], "flex.gap")?;
    let (sx, sy, sw, sh) = flex_slot(x, y, w, h, count, index, gap);
    Ok(layout_slot(sx, sy, sw, sh))
}

fn layout_grid(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 6, "grid")?;
    let (x, y) = xy_of(&args[0], "grid.at")?;
    let (w, h) = xy_of(&args[1], "grid.size")?;
    let cols = number(&args[2], "grid.cols")? as i64;
    let rows = number(&args[3], "grid.rows")? as i64;
    let index = number(&args[4], "grid.index")? as i64;
    let gap = number(&args[5], "grid.gap")?;
    let (sx, sy, sw, sh) = grid_slot(x, y, w, h, cols, rows, index, gap);
    Ok(layout_slot(sx, sy, sw, sh))
}

/// Phase 10 session 7: `scroll(at:, size:, content_height:) -> {at, size, scroll_y}`.
/// Stateful — keeps the current scroll-y per rect in `UI_STATE.scroll_y`.
/// Reads `mouse.wheel` while hovered to scroll, clamping to
/// `[0, content_height - size.y]`. Returns the scroll-y so children
/// can draw at `slot.at.y - scroll_y` to appear scrolled. v1 doesn't
/// clip with `set_scissor` — children that overflow the viewport
/// just draw past the rect; scripts position them inside manually.
fn layout_scroll(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "scroll")?;
    let (x, y) = xy_of(&args[0], "scroll.at")?;
    let (w, h) = xy_of(&args[1], "scroll.size")?;
    let content_height = number(&args[2], "scroll.content_height")?;
    let id = rect_id(x, y, w, h);

    let (mx, my) = read_mouse_xy(env);
    let hovered = point_in_rect(mx, my, x, y, w, h);
    let wheel = if hovered { read_mouse_wheel(env) } else { 0.0 };

    let max_scroll = (content_height - h).max(0.0);
    let new_scroll = UI_STATE.with(|s| {
        let mut st = s.borrow_mut();
        let cur = st.scroll_y.get(&id).copied().unwrap_or(0.0);
        // macroquad reports +y up for the wheel; "scroll content
        // down" reads as a negative wheel delta, hence the subtract.
        let next = (cur - wheel * 24.0).clamp(0.0, max_scroll);
        st.scroll_y.insert(id, next);
        next
    });

    let mut fields = HashMap::new();
    fields.insert(
        "at".to_string(),
        Value::from_tuple(Rc::new(vec![Value::from_float(x), Value::from_float(y)])),
    );
    fields.insert(
        "size".to_string(),
        Value::from_tuple(Rc::new(vec![Value::from_float(w), Value::from_float(h)])),
    );
    fields.insert("scroll_y".to_string(), Value::from_float(new_scroll));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "layout",
    }))))
}

/// Read `mouse.wheel` from the env's `mouse` ambient. Returns 0.0
/// if the field is missing or not a number. Used by `layout_scroll`.
fn read_mouse_wheel(env: &Env) -> f64 {
    let Some(m) = env.get("mouse") else {
        return 0.0;
    };
    if !m.is_object() {
        return 0.0;
    }
    let rc = m.as_object();
    let o = rc.borrow();
    match o.get_field("wheel") {
        Some(v) => number(&v, "mouse.wheel").unwrap_or(0.0),
        None => 0.0,
    }
}

/// Read `mouse.x` / `mouse.y` from the env's `mouse` ambient. Returns
/// (0, 0) if `mouse` is missing or shaped wrong (e.g. headless run
/// after `install` but before the first `play` frame writes it). Used
/// by `draw_button`'s hit test.
fn read_mouse_xy(env: &Env) -> (f64, f64) {
    let Some(m) = env.get("mouse") else {
        return (0.0, 0.0);
    };
    if !m.is_object() {
        return (0.0, 0.0);
    }
    let rc = m.as_object();
    let o = rc.borrow();
    let x = match o.get_field("x") {
        Some(v) => number(&v, "mouse.x").unwrap_or(0.0),
        None => 0.0,
    };
    let y = match o.get_field("y") {
        Some(v) => number(&v, "mouse.y").unwrap_or(0.0),
        None => 0.0,
    };
    (x, y)
}

/// Read a single boolean field off one of the `mouse_press` /
/// `mouse_held` ambients. Returns `false` if either the ambient or
/// the field is missing or not a bool.
fn read_mouse_button(env: &Env, ambient: &str, field: &str) -> bool {
    let Some(m) = env.get(ambient) else {
        return false;
    };
    if !m.is_object() {
        return false;
    }
    let rc = m.as_object();
    let o = rc.borrow();
    match o.get_field(field) {
        Some(v) if v.is_bool() => v.as_bool(),
        _ => false,
    }
}

/// Decode the cached TTF bytes for `path` into a macroquad `Font`,
/// caching the result. Returns Ok if the font is already cached or
/// the parse succeeds. Errors if `load_font` was never called for
/// this path or if the parse fails.
fn ensure_font_decoded(path: &str) -> Result<(), RuntimeError> {
    let already = FONT_CACHE.with(|c| c.borrow().contains_key(path));
    if already {
        return Ok(());
    }
    let bytes = FONT_BYTES_CACHE.with(|c| c.borrow().get(path).cloned());
    let bytes = bytes.ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!(
            "text_with_font: font '{path}' was not loaded (call `load_font(...)` first)"
        ),
        help: Some(
            "store the handle from `load_font(...)` and reuse it across draw calls".to_string(),
        ),
    })?;
    let font = macroquad::text::load_ttf_font_from_bytes(&bytes).map_err(|e| RuntimeError {
        line: 0,
        col: 0,
        message: format!("text_with_font: failed to decode font '{path}': {e}"),
        help: None,
    })?;
    FONT_CACHE.with(|c| {
        c.borrow_mut().insert(path.to_string(), font);
    });
    Ok(())
}

/// Pull the cache key off a font handle. Atlas/sprite/sound handles
/// won't pass — this is the type-check for `text_with_font`'s last
/// arg.
fn font_handle_path(v: &Value, callee: &str) -> Result<String, RuntimeError> {
    if !v.is_object() {
        let other = *v;
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{callee} expects a font handle from `load_font(...)`, got {}",
                other.type_name()
            ),
            help: None,
        });
    }
    let rc = v.as_object();
    let o = rc.borrow();
    if o.kind != "font" {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{callee} expects a font handle from `load_font(...)`, got a `{}` handle",
                o.kind
            ),
            help: None,
        });
    }
    match o.get_field("path") {
        Some(v) if v.is_str() => Ok(v.as_string().clone()),
        _ => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{callee}: font handle is missing a `path` field"),
            help: None,
        }),
    }
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
        "texture".to_string(),
        Value::from_builtin("texture", &["path"], texture_impl),
    );

    // Phase 22: typed save helpers — `save.vec3` / `save.f32` /
    // `save.get_vec3` / `save.get_f32` etc. Wrap an in-memory
    // HashMap that can flush to disk via save.write(path) and
    // restore via save.read(path). Reuses save_to / load_from
    // from Phase 8 — these are typed sugar for "save the player's
    // 3D position" without manually constructing nested objects.
    let mut save_fields = HashMap::new();
    save_fields.insert(
        "vec3".to_string(),
        Value::from_builtin("save.vec3", &["key", "v"], save_vec3_impl),
    );
    save_fields.insert(
        "f32".to_string(),
        Value::from_builtin("save.f32", &["key", "v"], save_f32_impl),
    );
    save_fields.insert(
        "int".to_string(),
        Value::from_builtin("save.int", &["key", "v"], save_int_impl),
    );
    save_fields.insert(
        "string".to_string(),
        Value::from_builtin("save.string", &["key", "v"], save_string_impl),
    );
    save_fields.insert(
        "get_vec3".to_string(),
        Value::from_builtin("save.get_vec3", &["key"], save_get_vec3_impl),
    );
    save_fields.insert(
        "get_f32".to_string(),
        Value::from_builtin("save.get_f32", &["key"], save_get_f32_impl),
    );
    save_fields.insert(
        "get_int".to_string(),
        Value::from_builtin("save.get_int", &["key"], save_get_int_impl),
    );
    save_fields.insert(
        "get_string".to_string(),
        Value::from_builtin("save.get_string", &["key"], save_get_string_impl),
    );
    save_fields.insert(
        "has".to_string(),
        Value::from_builtin("save.has", &["key"], save_has_impl),
    );
    save_fields.insert(
        "remove".to_string(),
        Value::from_builtin("save.remove", &["key"], save_remove_impl),
    );
    save_fields.insert(
        "clear".to_string(),
        Value::from_builtin("save.clear", &[], save_clear_impl),
    );
    save_fields.insert(
        "write".to_string(),
        Value::from_builtin("save.write", &["path"], save_write_impl),
    );
    save_fields.insert(
        "read".to_string(),
        Value::from_builtin("save.read", &["path"], save_read_impl),
    );
    save_fields.insert(
        "try_read".to_string(),
        Value::from_builtin("save.try_read", &["path"], save_try_read_impl),
    );
    env.set(
        "save".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: save_fields,
            kind: "module",
        }))),
    );

    // Phase 21: animation API surface. mesh.play(handle, clip)
    // selects which animation clip plays for a textured mesh
    // handle. v0.1 of this surface stores the active clip + time
    // but does not yet apply skinning to the rendered vertices —
    // GPU skinning is a follow-on. The clip-name + time state
    // can be queried for game logic (e.g. "is the attack
    // animation finished?") even before visual skinning lands.
    let mut mesh_anim = HashMap::new();
    mesh_anim.insert(
        "play".to_string(),
        Value::from_builtin("mesh.play", &["handle", "clip", "looping"], mesh_play_impl),
    );
    mesh_anim.insert(
        "stop".to_string(),
        Value::from_builtin("mesh.stop", &["handle"], mesh_stop_impl),
    );
    mesh_anim.insert(
        "blend".to_string(),
        Value::from_builtin(
            "mesh.blend",
            &["handle", "clip_a", "clip_b", "t"],
            mesh_blend_impl,
        ),
    );
    mesh_anim.insert(
        "current".to_string(),
        Value::from_builtin("mesh.current", &["handle"], mesh_current_impl),
    );
    mesh_anim.insert(
        "advance".to_string(),
        Value::from_builtin("mesh.advance", &["dt"], mesh_advance_impl),
    );
    env.set(
        "mesh_anim".to_string(),
        Value::from_object(Rc::new(RefCell::new(Object {
            fields: mesh_anim,
            kind: "module",
        }))),
    );
    env.set(
        "cube_textured".to_string(),
        Value::from_builtin(
            "cube_textured",
            &["at", "color", "size", "texture"],
            cube_textured_impl,
        ),
    );
    env.set(
        "mesh_textured".to_string(),
        Value::from_builtin(
            "mesh_textured",
            &["path", "at", "color", "size", "texture"],
            mesh_textured_impl,
        ),
    );
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
        Value::from_tuple(Rc::new(vec![Value::from_float(nx), Value::from_float(ny)])),
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
        texture: 0,
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
        texture: 0,
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
        texture: 0,
    });
    Ok(Value::NIL)
}

/// Phase 17 session 3: textured mesh draw. Mirrors `mesh()` but
/// takes a 5th texture-handle argument (the value returned by
/// `texture("path.png")`). Same split pattern as
/// `sound.play` / `sound.play_at` — Twe's calling convention
/// requires every kwarg supplied, so the texture variant is its
/// own name rather than an optional arg.
fn mesh_textured_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "mesh_textured")?;
    arity(args, 5, "mesh_textured")?;
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
                    "mesh_textured.path expects a string, got {}",
                    other.type_name()
                ),
                help: Some("e.g. `mesh_textured(\"crate.glb\", at, color, size, tex)`".to_string()),
            });
        }
    };
    let at = xyz_of(&args[1], "mesh_textured.at")?;
    let color = rgba_of(&args[2], "mesh_textured.color")?;
    let size = number(&args[3], "mesh_textured.size")? as f32;
    let tex_id = texture_handle_id(&args[4], "mesh_textured.texture")?;
    let id = env.intern_mesh_path(&path);
    env.render_queue3d.push(crate::value::DrawCall3d {
        primitive: crate::value::Primitive::Mesh(id),
        at,
        color,
        size,
        texture: tex_id,
    });
    Ok(Value::NIL)
}

/// Phase 17 session 3: textured cube. Same pattern.
fn cube_textured_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    require_render(env, "cube_textured")?;
    arity(args, 4, "cube_textured")?;
    let at = xyz_of(&args[0], "cube_textured.at")?;
    let color = rgba_of(&args[1], "cube_textured.color")?;
    let size = number(&args[2], "cube_textured.size")? as f32;
    let tex_id = texture_handle_id(&args[3], "cube_textured.texture")?;
    env.render_queue3d.push(crate::value::DrawCall3d {
        primitive: crate::value::Primitive::Cube,
        at,
        color,
        size,
        texture: tex_id,
    });
    Ok(Value::NIL)
}

/// Phase 17 session 3: `texture(path)` builtin. Returns a handle
/// `{ id, path, kind: "texture" }` whose `id` is the interned
/// texture-path identifier. Path existence is checked here so
/// typos fail fast rather than at render time.
fn texture_impl(env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "texture")?;
    let path = string_arg(&args[0], "texture", "path")?;
    if !std::path::Path::new(&path).exists() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("texture: file not found: {path}"),
            help: Some("path is resolved relative to the working directory".to_string()),
        });
    }
    let id = env.intern_texture_path(&path);
    let mut fields = HashMap::new();
    fields.insert("id".to_string(), Value::from_int(id as i64));
    fields.insert("path".to_string(), Value::from_string(path));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "texture",
    }))))
}

// ---------- Phase 20: lighting builtins ----------

fn light_add_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "light.add")?;
    let at = xyz_of(&args[0], "light.add.at")?;
    let color = rgba_of(&args[1], "light.add.color")?;
    let radius = number(&args[2], "light.add.radius")? as f32;
    if radius <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "light.add: radius must be > 0".to_string(),
            help: Some("a 0-radius light is disabled by definition".to_string()),
        });
    }
    let mut handle = 0;
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        for (i, slot) in st.point_lights.iter_mut().enumerate() {
            if slot.color_radius[3] <= 0.0 {
                slot.pos = [at[0], at[1], at[2], 0.0];
                slot.color_radius = [color[0], color[1], color[2], radius];
                handle = (i + 1) as i64;
                return;
            }
        }
    });
    if handle == 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "light.add: all 8 light slots full".to_string(),
            help: Some("call light.remove(h) to free a slot".to_string()),
        });
    }
    Ok(Value::from_int(handle))
}

fn light_remove_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "light.remove")?;
    let handle = handle_int(&args[0], "light.remove")?;
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        if let Some(slot) = st.point_lights.get_mut((handle - 1) as usize) {
            slot.pos = [0.0; 4];
            slot.color_radius = [0.0; 4];
        }
    });
    Ok(Value::NIL)
}

fn light_ambient_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "light.ambient")?;
    let color = rgba_of(&args[0], "light.ambient.color")?;
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.ambient = [color[0], color[1], color[2], 0.0];
    });
    Ok(Value::NIL)
}

fn light_set_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "light.set")?;
    let handle = handle_int(&args[0], "light.set")?;
    let at = xyz_of(&args[1], "light.set.at")?;
    let color = rgba_of(&args[2], "light.set.color")?;
    let radius = number(&args[3], "light.set.radius")? as f32;
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        if let Some(slot) = st.point_lights.get_mut((handle - 1) as usize) {
            slot.pos = [at[0], at[1], at[2], 0.0];
            slot.color_radius = [color[0], color[1], color[2], radius];
        }
    });
    Ok(Value::NIL)
}

fn light_set_radius_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "light.set_radius")?;
    let handle = handle_int(&args[0], "light.set_radius")?;
    let radius = number(&args[1], "light.set_radius.radius")? as f32;
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        if let Some(slot) = st.point_lights.get_mut((handle - 1) as usize) {
            slot.color_radius[3] = radius;
        }
    });
    Ok(Value::NIL)
}

fn light_clear_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        for slot in st.point_lights.iter_mut() {
            slot.pos = [0.0; 4];
            slot.color_radius = [0.0; 4];
        }
    });
    Ok(Value::NIL)
}

fn sun_direction_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sun.direction")?;
    let v = xyz_of(&args[0], "sun.direction.v")?;
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        // Normalize so the shader doesn't have to. Zero-length
        // vector falls back to a sensible up-vector.
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len > 1e-6 {
            st.sun_dir[0] = v[0] / len;
            st.sun_dir[1] = v[1] / len;
            st.sun_dir[2] = v[2] / len;
        } else {
            st.sun_dir[0] = 0.0;
            st.sun_dir[1] = 1.0;
            st.sun_dir[2] = 0.0;
        }
    });
    Ok(Value::NIL)
}

fn sun_intensity_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sun.intensity")?;
    let i = number(&args[0], "sun.intensity.i")? as f32;
    LIGHTS_STATE.with(|s| {
        s.borrow_mut().sun_dir[3] = i.max(0.0);
    });
    Ok(Value::NIL)
}

fn sun_shadow_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sun.shadow")?;
    let on = if args[0].is_bool() {
        args[0].as_bool()
    } else {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "sun.shadow expects a bool (sun.shadow(true) or sun.shadow(false))"
                .to_string(),
            help: None,
        });
    };
    SHADOW_ENABLED.with(|s| *s.borrow_mut() = on);
    Ok(Value::NIL)
}

fn sun_shadow_extent_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sun.shadow_extent")?;
    let r = number(&args[0], "sun.shadow_extent.radius")? as f32;
    if r > 0.0 {
        SHADOW_EXTENT.with(|s| *s.borrow_mut() = r);
    }
    Ok(Value::NIL)
}

fn postfx_tonemap_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.tonemap")?;
    let on = if args[0].is_bool() {
        args[0].as_bool()
    } else {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "postfx.tonemap expects a bool".to_string(),
            help: None,
        });
    };
    TONEMAP_ENABLED.with(|s| *s.borrow_mut() = on);
    Ok(Value::NIL)
}

fn postfx_vignette_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.vignette")?;
    let s = (number(&args[0], "postfx.vignette.strength")? as f32).clamp(0.0, 1.0);
    VIGNETTE_STRENGTH.with(|st| *st.borrow_mut() = s);
    Ok(Value::NIL)
}

// Phase 28 session 4: vignette tint color. Accepts an (r, g, b)
// or (r, g, b, a) tuple — alpha is ignored. Components clamp to
// [0, 1] so any color literal or color.* constant is valid input.
fn postfx_vignette_color_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.vignette_color")?;
    let (r, g, b, _a) = rgba(&args[0], "postfx.vignette_color.color")?;
    VIGNETTE_COLOR.with(|c| {
        *c.borrow_mut() = [
            (r as f32).clamp(0.0, 1.0),
            (g as f32).clamp(0.0, 1.0),
            (b as f32).clamp(0.0, 1.0),
        ]
    });
    Ok(Value::NIL)
}

// Phase 28 session 3: bloom intensity. 0 disables (default). 1.0
// is a strong-but-not-overpowering value for typical scenes.
fn postfx_bloom_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.bloom")?;
    let s = (number(&args[0], "postfx.bloom.intensity")? as f32).max(0.0);
    BLOOM_INTENSITY.with(|st| *st.borrow_mut() = s);
    Ok(Value::NIL)
}

// Phase 28 session 3: bloom threshold. HDR luminance below this
// value contributes nothing; above it, the excess blooms.
fn postfx_bloom_threshold_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.bloom_threshold")?;
    let t = (number(&args[0], "postfx.bloom_threshold.threshold")? as f32).max(0.0);
    BLOOM_THRESHOLD.with(|st| *st.borrow_mut() = t);
    Ok(Value::NIL)
}

fn postfx_frustum_cull_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.frustum_cull")?;
    let on = if args[0].is_bool() {
        args[0].as_bool()
    } else {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "postfx.frustum_cull expects a bool".to_string(),
            help: None,
        });
    };
    FRUSTUM_CULL_ENABLED.with(|s| *s.borrow_mut() = on);
    Ok(Value::NIL)
}

// ---------- Phase 18: physics builtins ----------

fn handle_int(v: &Value, what: &str) -> Result<u32, RuntimeError> {
    if !v.is_int() {
        let other = *v;
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: expected integer handle, got {}", other.type_name()),
            help: None,
        });
    }
    let i = v.as_int();
    if i <= 0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: invalid handle {i}"),
            help: None,
        });
    }
    Ok(i as u32)
}

fn physics_body_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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

fn physics_static_box_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.static_box")?;
    let at = xyz_of(&args[0], "physics.static_box.at")?;
    let size = xyz_of(&args[1], "physics.static_box.size")?;
    Ok(Value::from_int(
        crate::physics3d::static_box(at, size) as i64
    ))
}

fn physics_static_sphere_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.static_sphere")?;
    let at = xyz_of(&args[0], "physics.static_sphere.at")?;
    let radius = number(&args[1], "physics.static_sphere.radius")? as f32;
    Ok(Value::from_int(
        crate::physics3d::static_sphere(at, radius) as i64
    ))
}

fn physics_position_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "physics.position")?;
    let handle = handle_int(&args[0], "physics.position")?;
    let pos = crate::physics3d::position(handle).ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!("physics.position: unknown handle {handle}"),
        help: None,
    })?;
    Ok(Value::from_tuple(Rc::new(vec![
        Value::from_float(pos[0] as f64),
        Value::from_float(pos[1] as f64),
        Value::from_float(pos[2] as f64),
    ])))
}

fn physics_velocity_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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

fn physics_impulse_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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

fn physics_gravity_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "physics.gravity")?;
    let v = xyz_of(&args[0], "physics.gravity.v")?;
    crate::physics3d::set_gravity(v);
    Ok(Value::NIL)
}

fn physics_character_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "physics.character")?;
    let at = xyz_of(&args[0], "physics.character.at")?;
    let height = number(&args[1], "physics.character.height")? as f32;
    let radius = number(&args[2], "physics.character.radius")? as f32;
    Ok(Value::from_int(
        crate::physics3d::character(at, height, radius) as i64,
    ))
}

fn physics_character_move_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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
        Value::from_tuple(Rc::new(vec![
            Value::from_float(translation[0] as f64),
            Value::from_float(translation[1] as f64),
            Value::from_float(translation[2] as f64),
        ])),
    );
    fields.insert("grounded".to_string(), Value::from_bool(grounded));
    Ok(Value::from_object(Rc::new(RefCell::new(Object {
        fields,
        kind: "character_move",
    }))))
}

fn physics_collisions_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
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

fn physics_despawn_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "physics.despawn")?;
    let handle = handle_int(&args[0], "physics.despawn")?;
    Ok(Value::from_bool(crate::physics3d::despawn(handle)))
}

fn physics_reset_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    crate::physics3d::reset();
    Ok(Value::NIL)
}

/// Phase 18 finish: load a `.glb` and add its first primitive as a
/// static trimesh collider. The translation `at` positions the
/// whole mesh in world space; the mesh's own internal node
/// transforms are flattened into the positions at load time.
/// Returns the body handle (mostly for record-keeping; static
/// bodies rarely need post-creation lookup).
fn physics_static_mesh_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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
fn physics_raycast_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
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
                Value::from_tuple(Rc::new(vec![
                    Value::from_float(point[0] as f64),
                    Value::from_float(point[1] as f64),
                    Value::from_float(point[2] as f64),
                ])),
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

/// Pull a texture id out of a value returned by `texture(...)`.
/// Accepts either the handle Object or a bare integer id (for
/// scripts that want to pass the id around as a primitive).
fn texture_handle_id(v: &Value, what: &str) -> Result<u32, RuntimeError> {
    if v.is_int() {
        let i = v.as_int();
        if i < 0 {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!("{what}: texture id can't be negative ({i})"),
                help: None,
            });
        }
        return Ok(i as u32);
    }
    if v.is_object() {
        let rc = v.as_object();
        let o = rc.borrow();
        if o.kind != "texture" {
            return Err(RuntimeError {
                line: 0,
                col: 0,
                message: format!(
                    "{what} expects a texture handle (from `texture(\"...\")`), got an object of kind '{}'",
                    o.kind
                ),
                help: None,
            });
        }
        if let Some(v) = o.get_field("id") {
            if v.is_int() {
                return Ok(v.as_int() as u32);
            }
        }
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("{what}: texture handle missing id field"),
            help: None,
        });
    }
    let other = *v;
    Err(RuntimeError {
        line: 0,
        col: 0,
        message: format!("{what} expects a texture handle, got {}", other.type_name()),
        help: Some("create one with `texture(\"path.png\")`".to_string()),
    })
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

#[cfg(test)]
mod tests {
    use super::point_in_rect;

    #[test]
    fn point_in_rect_inside_returns_true() {
        assert!(point_in_rect(50.0, 50.0, 0.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn point_in_rect_top_left_is_inclusive() {
        assert!(point_in_rect(0.0, 0.0, 0.0, 0.0, 10.0, 10.0));
    }

    #[test]
    fn point_in_rect_bottom_right_is_exclusive() {
        // x = rx + rw and y = ry + rh sit on the boundary and count as outside,
        // matching the standard half-open-rect convention used by most GUI
        // toolkits. Two adjacent buttons sharing an edge don't both register
        // the same hover/click frame.
        assert!(!point_in_rect(10.0, 10.0, 0.0, 0.0, 10.0, 10.0));
    }

    #[test]
    fn point_in_rect_outside_in_each_direction_returns_false() {
        assert!(!point_in_rect(-1.0, 5.0, 0.0, 0.0, 10.0, 10.0)); // left
        assert!(!point_in_rect(11.0, 5.0, 0.0, 0.0, 10.0, 10.0)); // right
        assert!(!point_in_rect(5.0, -1.0, 0.0, 0.0, 10.0, 10.0)); // above
        assert!(!point_in_rect(5.0, 11.0, 0.0, 0.0, 10.0, 10.0)); // below
    }

    #[test]
    fn point_in_rect_handles_negative_origin() {
        // Buttons placed in negative coordinates (e.g. a HUD panel anchored
        // off-screen and slid in) still hit-test correctly.
        assert!(point_in_rect(-50.0, -50.0, -100.0, -100.0, 60.0, 60.0));
        assert!(!point_in_rect(0.0, 0.0, -100.0, -100.0, 60.0, 60.0));
    }
}

// ---------------------------------------------------------------------------
// Phase 33 session 3: stdlib JSON manifest
//
// One spec per callable, with category, params, and (where available)
// doc string. Built by introspecting a freshly-installed `Env` — no
// hand-maintained parallel list, so drift between `install()` and
// `manifest()` is structurally impossible. The LLM grounding contract
// is "every callable name in this manifest is callable; every callable
// not in this manifest is not callable."
// ---------------------------------------------------------------------------

/// One callable in the Twe standard library. Emitted by [`manifest`]
/// and serialized by [`manifest_to_json`]. Stable schema; consumers
/// should treat additional fields as additive (forward compatible).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinSpec {
    /// Fully-qualified callable name as called from Twe (e.g.
    /// `"math.sqrt"`, `"rect"`, `"physics.body"`). This is the
    /// string the grammar's IDENT/postfix sequence resolves to.
    pub name: String,
    /// Coarse category derived from the dotted prefix where one
    /// exists, else from a small lookup of well-known top-level
    /// names (`"rect"` → `"draw"`, `"button"` → `"ui"`, etc.).
    pub category: String,
    /// Parameter names in declaration order. Empty for nullary
    /// builtins (`print`, `math.identity`). v0.x stdlib uses
    /// positional + keyword bindings; the names are the kwarg keys.
    pub params: Vec<String>,
    /// Free-text doc string. Currently always `None` — populated by
    /// a follow-on session that lifts the inline comments above
    /// each `env.set` block into a side table. Schema reserves the
    /// field so adding it later isn't a breaking change.
    pub doc: Option<String>,
    /// Version this builtin first shipped in. Currently always
    /// `None` — populated by a follow-on. Kept in the schema for
    /// `--since` filtering and changelog drilling.
    pub since: Option<String>,
    /// True if the builtin is `@deprecated`. Currently always
    /// `false`; the deprecation path lives in language-level
    /// declarations today (Phase 13 session 9), not in stdlib.
    pub deprecated: bool,
}

/// Build the canonical stdlib manifest by walking a freshly-installed
/// `Env`. Every callable that `install()` registers — both top-level
/// dotted names and namespace-Object members — appears in the result.
/// Sorted by `name` for stable output.
pub fn manifest() -> Vec<BuiltinSpec> {
    let mut env = Env::new();
    install(&mut env);
    let mut out = Vec::new();
    for (binding_name, value) in env.iter_bindings() {
        if value.is_builtin() {
            let (canonical, params, _) = value.as_builtin();
            push_spec(&mut out, canonical, params);
            // Also catch the case where the binding name differs
            // from the canonical builtin name. In practice these
            // always match, but a defensive check keeps the
            // manifest honest.
            let _ = binding_name;
        } else if value.is_object() {
            let rc = value.as_object();
            let obj = rc.borrow();
            if obj.kind == "module" {
                for v in obj.fields.values() {
                    if v.is_builtin() {
                        let (canonical, params, _) = v.as_builtin();
                        push_spec(&mut out, canonical, params);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    // De-dupe: a builtin can in principle be reachable via both
    // top-level dotted name *and* a namespace object's field. The
    // manifest contract is one entry per canonical name.
    out.dedup_by(|a, b| a.name == b.name);
    out
}

fn push_spec(out: &mut Vec<BuiltinSpec>, name: &str, params: &[&str]) {
    out.push(BuiltinSpec {
        name: name.to_string(),
        category: derive_category(name),
        params: params.iter().map(|s| s.to_string()).collect(),
        doc: None,
        since: None,
        deprecated: false,
    });
}

/// Coarse category from the canonical name. Dotted names use the
/// prefix (`math.sqrt` → `"math"`); flat names look up a small
/// well-known table; everything else falls into `"core"`.
fn derive_category(name: &str) -> String {
    if let Some(dot) = name.find('.') {
        return name[..dot].to_string();
    }
    match name {
        // Drawing primitives.
        "rect" | "circle" | "circle_outline" | "line" | "text"
        | "text_with_font" | "sprite" | "sprite_frame" | "sprite_frame_at" => "draw".into(),
        // Immediate-mode UI widgets and layout helpers.
        "button" | "label" | "progress_bar" | "slider" | "checkbox"
        | "dropdown" | "text_input" | "key_input" | "panel" | "stack"
        | "flex" | "grid" | "scroll" => "ui".into(),
        // Asset loaders.
        "load" | "load_atlas" | "load_font" => "asset".into(),
        // Storage primitives (Phase 8 session 4 bottom layer).
        "save_to" | "load_from" => "storage".into(),
        // Input / lifecycle.
        "key_held" | "key_pressed" => "input".into(),
        "pause" | "is_paused" | "auto_pause_on_blur" => "lifecycle".into(),
        "screenshot" => "tooling".into(),
        // 3D atoms.
        "vec3" | "cube" | "sphere" | "texture" | "mesh" => "render3d".into(),
        // Tilemap helpers exposed at the top level.
        "tilemap" | "tilemap_render" | "tilemap_at" | "tilemap_solid_at"
        | "tilemap_solid_aabb" | "tilemap_aabb_touches" => "tilemap".into(),
        // Plain `print` and the math-module shorthands re-exported
        // at the top level.
        "print" => "io".into(),
        "smoothstep" | "mix" | "noise" => "math".into(),
        _ => "core".into(),
    }
}

/// Render a manifest as JSON. Hand-rolled to match the rest of the
/// project's no-serde pattern. Versioned via `tool` + `version` so
/// downstream tools can reject unknown shapes cleanly.
pub fn manifest_to_json(specs: &[&BuiltinSpec]) -> String {
    let mut s = String::with_capacity(256 + specs.len() * 96);
    s.push('{');
    s.push_str("\"tool\":\"twec-stdlib\",\"version\":1");
    s.push_str(",\"count\":");
    s.push_str(&specs.len().to_string());
    s.push_str(",\"builtins\":[");
    for (i, spec) in specs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');
        s.push_str("\"name\":");
        json_string(&mut s, &spec.name);
        s.push_str(",\"category\":");
        json_string(&mut s, &spec.category);
        s.push_str(",\"params\":[");
        for (j, p) in spec.params.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            json_string(&mut s, p);
        }
        s.push(']');
        s.push_str(",\"doc\":");
        match &spec.doc {
            Some(d) => json_string(&mut s, d),
            None => s.push_str("null"),
        }
        s.push_str(",\"since\":");
        match &spec.since {
            Some(v) => json_string(&mut s, v),
            None => s.push_str("null"),
        }
        s.push_str(",\"deprecated\":");
        s.push_str(if spec.deprecated { "true" } else { "false" });
        s.push('}');
    }
    s.push_str("]}");
    s
}

fn json_string(s: &mut String, value: &str) {
    s.push('"');
    for ch in value.chars() {
        match ch {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                s.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => s.push(ch),
        }
    }
    s.push('"');
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    #[test]
    fn manifest_is_non_empty() {
        let m = manifest();
        assert!(
            m.len() >= 200,
            "manifest unexpectedly small ({}); install + walk drift?",
            m.len()
        );
    }

    #[test]
    fn manifest_is_sorted_and_deduped() {
        let m = manifest();
        for win in m.windows(2) {
            assert!(win[0].name < win[1].name, "manifest not sorted at {:?}", win);
        }
    }

    #[test]
    fn manifest_includes_canonical_names() {
        // Sanity check: a sampling of builtins from each category
        // must show up. If install() drops one, this catches it.
        let m = manifest();
        let names: std::collections::HashSet<&str> =
            m.iter().map(|s| s.name.as_str()).collect();
        for expected in &[
            "print",
            "rect",
            "math.sqrt",
            "math.sin",
            "color.from_hex",
            "random.int",
            "save.write",
            "physics.body",
            "world.spatial_clear",
            "net.host",
            "button",
            "slider",
            "vec3",
            "mesh",
        ] {
            assert!(
                names.contains(expected),
                "manifest missing canonical builtin `{expected}`"
            );
        }
    }

    #[test]
    fn category_derivation_handles_dotted_and_flat() {
        assert_eq!(derive_category("math.sqrt"), "math");
        assert_eq!(derive_category("world.spatial_clear"), "world");
        assert_eq!(derive_category("rect"), "draw");
        assert_eq!(derive_category("button"), "ui");
        assert_eq!(derive_category("print"), "io");
        // Unknown flat name falls to `core` rather than panicking.
        assert_eq!(derive_category("unknown_thing"), "core");
    }

    #[test]
    fn json_output_is_balanced_and_versioned() {
        let m = manifest();
        let refs: Vec<&BuiltinSpec> = m.iter().collect();
        let json = manifest_to_json(&refs);
        assert!(json.contains("\"tool\":\"twec-stdlib\""));
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"count\":"));
        assert!(json.contains("\"builtins\":["));
        // Brace + bracket balance — catches accidents in the
        // hand-rolled emitter.
        assert_eq!(
            json.matches('{').count(),
            json.matches('}').count(),
            "unbalanced braces"
        );
        assert_eq!(
            json.matches('[').count(),
            json.matches(']').count(),
            "unbalanced brackets"
        );
    }
}
