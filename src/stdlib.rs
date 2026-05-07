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
    "a", "b", "x", "y", "lb", "rb", "lt", "rt", "start", "select",
    "dup", "ddown", "dleft", "dright",
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

    // Phase 18: 3D physics surface. All builtins forward to the
    // thread-local PhysicsWorld in src/physics3d.rs. The play3d
    // loop steps the world before each Twe `on update(dt)` so
    // scripts read authoritative positions.
    let mut physics_fields = HashMap::new();
    physics_fields.insert(
        "body".to_string(),
        Value::from_builtin(
            "physics.body",
            &["shape", "at", "mass"],
            physics_body_impl,
        ),
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
        Value::from_builtin(
            "physics.velocity",
            &["handle", "v"],
            physics_velocity_impl,
        ),
    );
    physics_fields.insert(
        "impulse".to_string(),
        Value::from_builtin(
            "physics.impulse",
            &["handle", "v"],
            physics_impulse_impl,
        ),
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
        Value::from_builtin("achievement.unlock", &["name"], crate::steam::achievement_unlock),
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
        Value::from_builtin("cloud.save", &["filename", "data"], crate::steam::cloud_save),
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
    let s = arboard::Clipboard::new()
        .and_then(|mut c| c.get_text())
        .unwrap_or_default();
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
    let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text));
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
    let loaded = crate::save::load_from_path(p)
        .map_err(|m| crate::save::to_runtime_error(m, 0, 0))?;
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
    lang.insert(
        "active".to_string(),
        Value::from_string("en".to_string()),
    );
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
    let bundles_v = lang.borrow().get_field("bundles").ok_or_else(|| RuntimeError {
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
            message: format!(
                "{what} expects a tuple, got {}",
                other.type_name()
            ),
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
    Ok(make_color(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a))
}

fn color_to_srgb(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "color.to_srgb")?;
    let (r, g, b, a) = rgba(&args[0], "color.to_srgb")?;
    Ok(make_color(linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b), a))
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
                    "expected `#rrggbb` or `#rrggbbaa` (case-insensitive, '#' optional)".to_string(),
                ),
            })
    };
    let (r, g, b, a) = match s.len() {
        6 => (parse_byte(&s[0..2])?, parse_byte(&s[2..4])?, parse_byte(&s[4..6])?, 1.0),
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
        Value::from_builtin(
            "progress_bar",
            &["at", "size", "value"],
            draw_progress_bar,
        ),
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
        Value::from_builtin(
            "text_input",
            &["at", "size", "value"],
            draw_text_input,
        ),
    );
    // Phase 10 session 11: `key_input(at:, size:, value:) -> string`.
    // The keybind capture widget. Click to focus; the next key pressed
    // becomes the binding. `value` is the current binding name (e.g.
    // "right"); the widget returns the new binding next frame after a
    // key is pressed, otherwise echoes the input unchanged. Driven off
    // the existing `key_press` ambient — no separate input plumbing.
    env.set(
        "key_input".to_string(),
        Value::from_builtin(
            "key_input",
            &["at", "size", "value"],
            draw_key_input,
        ),
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
        Value::from_builtin(
            "scroll",
            &["at", "size", "content_height"],
            layout_scroll,
        ),
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
                message: format!(
                    "checkbox.value expects a bool, got {}",
                    (*v).type_name()
                ),
                help: Some("pass a `var` you toggle via the return value".to_string()),
            });
        }
    };

    let (mx, my) = read_mouse_xy(env);
    let pressed_now = read_mouse_button(env, "mouse_press", "left");
    let held_now = read_mouse_button(env, "mouse_held", "left");
    let hovered = point_in_rect(mx, my, x, y, w, h);
    let value = if hovered && pressed_now { !value_in } else { value_in };

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

// ---------- Phase 18: physics builtins ----------

fn handle_int(v: &Value, what: &str) -> Result<u32, RuntimeError> {
    if !v.is_int() {
        let other = *v;
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "{what}: expected integer handle, got {}",
                other.type_name()
            ),
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
    Ok(Value::from_int(crate::physics3d::static_box(at, size) as i64))
}

fn physics_static_sphere_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "physics.static_sphere")?;
    let at = xyz_of(&args[0], "physics.static_sphere.at")?;
    let radius = number(&args[1], "physics.static_sphere.radius")? as f32;
    Ok(Value::from_int(
        crate::physics3d::static_sphere(at, radius) as i64,
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
    let id =
        crate::physics3d::static_trimesh(at, &verts, &tris).map_err(|e| RuntimeError {
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
        message: format!(
            "{what} expects a texture handle, got {}",
            other.type_name()
        ),
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
