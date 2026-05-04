//! `twec play` — interactive macroquad backend.
//!
//! Owns the real game loop: reads keyboard input each frame, ticks the
//! interpreter's active scene, runs `on render():` handlers inside a
//! macroquad draw frame. The headless `twec run` path remains unchanged
//! and is still the test entry point.
//!
//! Hot reload: the file's mtime is polled at the top of every frame.
//! If it changed, the script is re-lexed, re-parsed, and a fresh `Env`
//! initialised from scratch. Phase-2 simplification: scene fields are
//! NOT preserved across reloads. Iteration is fast; tweaking values
//! and saving brings the change in immediately, but in-flight game
//! state is lost.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::SystemTime;

use macroquad::prelude::*;

use crate::value::{Env, Object, Value};

const KEYS: &[(&str, KeyCode)] = &[
    ("right", KeyCode::Right),
    ("left", KeyCode::Left),
    ("up", KeyCode::Up),
    ("down", KeyCode::Down),
    ("space", KeyCode::Space),
    ("escape", KeyCode::Escape),
    ("enter", KeyCode::Enter),
    ("r", KeyCode::R),
    ("w", KeyCode::W),
    ("a", KeyCode::A),
    ("s", KeyCode::S),
    ("d", KeyCode::D),
];

const MOUSE_BUTTONS: &[(&str, MouseButton)] = &[
    ("left", MouseButton::Left),
    ("middle", MouseButton::Middle),
    ("right", MouseButton::Right),
];

/// Phase 9 session 5: Twe field name → gilrs::Button mapping. Order
/// must mirror the `GAMEPAD_BUTTON_NAMES` const in stdlib (since
/// scripts read fields by name, the Rust list is the source of truth
/// for which buttons we poll).
const GAMEPAD_BUTTONS: &[(&str, gilrs::Button)] = &[
    ("a", gilrs::Button::South),
    ("b", gilrs::Button::East),
    ("x", gilrs::Button::West),
    ("y", gilrs::Button::North),
    ("lb", gilrs::Button::LeftTrigger),
    ("rb", gilrs::Button::RightTrigger),
    ("lt", gilrs::Button::LeftTrigger2),
    ("rt", gilrs::Button::RightTrigger2),
    ("start", gilrs::Button::Start),
    ("select", gilrs::Button::Select),
    ("dup", gilrs::Button::DPadUp),
    ("ddown", gilrs::Button::DPadDown),
    ("dleft", gilrs::Button::DPadLeft),
    ("dright", gilrs::Button::DPadRight),
];

/// Twe axis name → gilrs::Axis mapping. Triggers `lt` / `rt` use
/// `LeftZ` / `RightZ` (gilrs's analog trigger axes); the `lt` / `rt`
/// fields on `gamepad` and `gamepad_press` are the thresholded
/// boolean variants.
const GAMEPAD_AXES: &[(&str, gilrs::Axis)] = &[
    ("lx", gilrs::Axis::LeftStickX),
    ("ly", gilrs::Axis::LeftStickY),
    ("rx", gilrs::Axis::RightStickX),
    ("ry", gilrs::Axis::RightStickY),
    ("lt", gilrs::Axis::LeftZ),
    ("rt", gilrs::Axis::RightZ),
];

thread_local! {
    /// Lazily-initialised gilrs context. None means "tried and failed
    /// to initialise" — we log the error once and continue without
    /// gamepad input. Re-initialisation is not supported (matches
    /// gilrs's lifecycle expectations).
    static GILRS: RefCell<GilrsState> = const { RefCell::new(GilrsState::Uninit) };
    /// Previous-frame button states for edge-triggered detection.
    /// Cleared on hot reload by `clear_gamepad_state`.
    static PREV_GAMEPAD: RefCell<[bool; 14]> = const { RefCell::new([false; 14]) };
}

enum GilrsState {
    Uninit,
    // Boxed: gilrs::Gilrs is ~230 bytes (XInput buffers + connection
    // table); without the Box, clippy's large_enum_variant fires
    // because Uninit / Failed carry no data.
    Active(Box<gilrs::Gilrs>),
    Failed,
}

/// Reset the previous-frame gamepad state so a hot reload doesn't
/// produce phantom press events.
pub fn clear_gamepad_state() {
    PREV_GAMEPAD.with(|p| *p.borrow_mut() = [false; 14]);
}

pub fn launch(path: String) -> i32 {
    let conf = window_conf();
    macroquad::Window::from_config(conf, run_loop(path));
    0
}

/// `twec play --vm bytecode <file>` entry. Mirrors `launch` but
/// drives the bytecode VM (`vm.tick(dt)` + `vm.render()`) instead
/// of `eval::tick_frame` + `eval::render_frame`. Hot reload still
/// rebuilds from scratch on file change. The macroquad input /
/// screen-size pumps go through the same Rc-shared Objects the
/// stdlib installs, so they reach both interpreters identically.
pub fn launch_bytecode(path: String) -> i32 {
    let conf = window_conf();
    macroquad::Window::from_config(conf, run_loop_bytecode(path));
    0
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Twec play".to_string(),
        window_width: 640,
        window_height: 480,
        high_dpi: true,
        fullscreen: false,
        sample_count: 1,
        window_resizable: true,
        platform: Default::default(),
        icon: None,
    }
}

async fn run_loop(path: String) {
    let path_ref = path.clone();
    let mut env = match initialize(&path_ref) {
        Ok(e) => e,
        Err(()) => return,
    };
    let mut last_mtime = current_mtime(&path_ref);
    flush_output(&mut env);

    loop {
        if is_key_pressed(KeyCode::Escape) {
            break;
        }

        // Hot reload: poll mtime, reload on change.
        let cur_mtime = current_mtime(&path_ref);
        if cur_mtime.is_some() && cur_mtime != last_mtime {
            match initialize(&path_ref) {
                Ok(new_env) => {
                    eprintln!("[twec] hot reload: {path_ref}");
                    crate::stdlib::clear_asset_caches();
                    clear_gamepad_state();
                    env = new_env;
                    flush_output(&mut env);
                }
                Err(()) => {
                    // Init failed — keep running with the old env so the
                    // window doesn't close on a transient parse error.
                }
            }
            last_mtime = cur_mtime;
        }

        update_key_state(&mut env);
        let dt = get_frame_time() as f64;
        // Phase 10 session 8: when paused, skip `tick_frame` so no
        // fibers advance and no every-clocks fire, but keep the
        // render path live so a "PAUSED" overlay or settings menu
        // can draw. Render-side `button` / `slider` etc. read mouse
        // state directly so the pause menu still interacts.
        if !crate::stdlib::is_paused() {
            if let Err(e) = crate::eval::tick_frame(&mut env, dt) {
                eprintln!("{path_ref}: runtime error: {e}");
                break;
            }
        }
        flush_output(&mut env);

        clear_background(BLACK);
        env.in_render = true;
        // Phase 9 session 2: drive a macroquad Camera2D from the Twe
        // `camera` ambient (pos / zoom) plus the runtime shake offset.
        // Backward-compat carve-out: when pos == (0, 0), zoom == 1.0,
        // and shake is silent we leave macroquad's default camera in
        // place — every existing example that draws at pixel coords
        // (origin top-left, +y down) keeps working unchanged.
        // Convention when opted in: camera.pos is the world-space
        // coordinate that ends up at the screen center.
        crate::stdlib::camera_tick(dt);
        let ((px, py), zoom) = crate::stdlib::camera_view(&env);
        let (sx, sy) = crate::stdlib::camera_shake_offset(&mut env);
        let cam_active = px != 0.0 || py != 0.0 || zoom != 1.0 || sx != 0.0 || sy != 0.0;
        if cam_active {
            let cam = build_camera2d(px + sx, py + sy, zoom);
            set_camera(&cam);
        }
        let render_result = crate::eval::render_frame(&mut env);
        if cam_active {
            set_default_camera();
        }
        if let Err(e) = render_result {
            eprintln!("{path_ref}: runtime error: {e}");
            env.in_render = false;
            break;
        }
        env.in_render = false;
        flush_output(&mut env);

        next_frame().await;
    }
}

/// Build a macroquad `Camera2D` that puts world-coord `(cx, cy)` at
/// the screen center, with `zoom > 1.0` zooming in and `< 1.0` zooming
/// out. Y axis stays inverted (+y down) so call-site coordinates keep
/// matching the screen's pixel orientation.
fn build_camera2d(cx: f64, cy: f64, zoom: f64) -> Camera2D {
    let w = screen_width();
    let h = screen_height();
    let z = zoom as f32;
    Camera2D {
        target: vec2(cx as f32, cy as f32),
        // zoom.x positive: world +x → screen +x.
        // zoom.y negative: world +y → screen +y (orthographic flip
        // would otherwise put +y up, which contradicts the rest of
        // the runtime's pixel-coord convention).
        zoom: vec2(2.0 / w * z, -2.0 / h * z),
        offset: vec2(0.0, 0.0),
        rotation: 0.0,
        render_target: None,
        viewport: None,
    }
}

async fn run_loop_bytecode(path: String) {
    let path_ref = path.clone();
    let mut vm = match initialize_bytecode(&path_ref) {
        Ok(v) => v,
        Err(()) => return,
    };
    let mut last_mtime = current_mtime(&path_ref);
    flush_vm_output(&mut vm);

    loop {
        if is_key_pressed(KeyCode::Escape) {
            break;
        }

        // Hot reload: poll mtime, reload on change.
        let cur_mtime = current_mtime(&path_ref);
        if cur_mtime.is_some() && cur_mtime != last_mtime {
            match initialize_bytecode(&path_ref) {
                Ok(new_vm) => {
                    eprintln!("[twec] hot reload: {path_ref}");
                    crate::stdlib::clear_asset_caches();
                    clear_gamepad_state();
                    vm = new_vm;
                    flush_vm_output(&mut vm);
                }
                Err(()) => {
                    // Init failed — keep running with the old VM so the
                    // window doesn't close on a transient parse error.
                }
            }
            last_mtime = cur_mtime;
        }

        update_vm_input(&vm);
        let dt = get_frame_time() as f64;
        if let Err(e) = vm.tick(dt) {
            eprintln!("{path_ref}: runtime error: {e}");
            break;
        }
        flush_vm_output(&mut vm);

        clear_background(BLACK);
        if let Err(e) = vm.render() {
            eprintln!("{path_ref}: runtime error: {e}");
            break;
        }
        flush_vm_output(&mut vm);

        next_frame().await;
    }
}

fn initialize_bytecode(path: &str) -> Result<crate::vm::VM, ()> {
    let src = match std::fs::read_to_string(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return Err(());
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return Err(());
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return Err(());
        }
    };
    let chunk = match crate::compiler::compile_program(&program) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{path}: compile error: {e}");
            return Err(());
        }
    };
    let mut vm = crate::vm::VM::new();
    if let Err(e) = vm.run(&chunk) {
        eprintln!("{path}: runtime error: {e}");
        return Err(());
    }
    Ok(vm)
}

fn flush_vm_output(vm: &mut crate::vm::VM) {
    let s = vm.take_out();
    if !s.is_empty() {
        print!("{s}");
    }
}

/// Mirror of `update_key_state` for the bytecode VM. The `key`,
/// `key_press`, `mouse`, `mouse_held`, `mouse_press`, and `screen`
/// Objects are the same `Rc<RefCell<Object>>` instances the
/// stdlib installs, so writes via `.borrow_mut()` here reach the
/// running scene/state code through their globals.
fn update_vm_input(vm: &crate::vm::VM) {
    if let Some(__t) = (vm.get_global("key")).as_ref() {
        if __t.is_object() {
            let rc = __t.as_object();
            let mut o = rc.borrow_mut();
            for (name, code) in KEYS {
                o.insert_field(*name, Value::from_bool(is_key_down(*code)));
            }
        }
    }
    if let Some(__t) = (vm.get_global("key_press")).as_ref() {
        if __t.is_object() {
            let rc = __t.as_object();
            let mut o = rc.borrow_mut();
            for (name, code) in KEYS {
                o.insert_field(*name, Value::from_bool(is_key_pressed(*code)));
            }
        }
    }
    write_mouse_object(vm.get_global("mouse"));
    write_mouse_buttons(vm.get_global("mouse_held"), is_mouse_button_down);
    write_mouse_buttons(vm.get_global("mouse_press"), is_mouse_button_pressed);
    if let Some(__t) = (vm.get_global("screen")).as_ref() {
        if __t.is_object() {
            let rc = __t.as_object();
            let mut o = rc.borrow_mut();
            let w = screen_width() as f64;
            let h = screen_height() as f64;
            o.insert_field(
                "size".to_string(),
                Value::from_tuple(Rc::new(vec![Value::from_float(w), Value::from_float(h)])),
            );
            o.insert_field(
                "center".to_string(),
                Value::from_tuple(Rc::new(vec![
                    Value::from_float(w / 2.0),
                    Value::from_float(h / 2.0),
                ])),
            );
        }
    }
}

/// Write current mouse position + accumulated wheel delta into the
/// `mouse` ambient. v0.2 session 3.
fn write_mouse_object(mouse: Option<Value>) {
    let Some(t) = mouse else {
        return;
    };
    if !t.is_object() {
        return;
    }
    let rc = t.as_object();
    let (mx, my) = mouse_position();
    let (_wx, wy) = mouse_wheel();
    let mut o = rc.borrow_mut();
    o.insert_field("x", Value::from_float(mx as f64));
    o.insert_field("y", Value::from_float(my as f64));
    o.insert_field(
        "pos",
        Value::from_tuple(Rc::new(vec![
            Value::from_float(mx as f64),
            Value::from_float(my as f64),
        ])),
    );
    // y-axis wheel delta is the canonical "scroll" reading; macroquad
    // resets `mouse_wheel()` between frames so the value here is the
    // accumulated delta this frame.
    o.insert_field("wheel", Value::from_float(wy as f64));
}

/// Write per-button state into `mouse_held` or `mouse_press`. The
/// caller passes the button-state predicate (`is_mouse_button_down`
/// for `mouse_held`, `is_mouse_button_pressed` for edge-triggered
/// `mouse_press`). v0.2 session 3.
fn write_mouse_buttons(target: Option<Value>, mut pred: impl FnMut(MouseButton) -> bool) {
    let Some(t) = target else {
        return;
    };
    if !t.is_object() {
        return;
    }
    let rc = t.as_object();
    let mut o = rc.borrow_mut();
    for (name, btn) in MOUSE_BUTTONS {
        o.insert_field((*name).to_string(), Value::from_bool(pred(*btn)));
    }
}

fn current_mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(Path::new(path))
        .ok()
        .and_then(|m| m.modified().ok())
}

fn initialize(path: &str) -> Result<Env, ()> {
    let src = match std::fs::read_to_string(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return Err(());
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return Err(());
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return Err(());
        }
    };
    let mut env = Env::new();
    crate::stdlib::install(&mut env);
    if let Err(e) = crate::eval::run_top_level(&mut env, &program) {
        eprintln!("{path}: runtime error: {e}");
        return Err(());
    }
    Ok(env)
}

fn flush_output(env: &mut Env) {
    if !env.out.is_empty() {
        print!("{}", env.out);
        env.out.clear();
    }
}

fn update_key_state(env: &mut Env) {
    if let Some(t) = env.get("key") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            for (name, code) in KEYS {
                o.insert_field(*name, Value::from_bool(is_key_down(*code)));
            }
        }
    }
    let kp = env.get("key_press");
    if kp.as_ref().is_some_and(|t| t.is_object()) {
        let rc = kp.unwrap().as_object();
        let mut o = rc.borrow_mut();
        for (name, code) in KEYS {
            o.insert_field(*name, Value::from_bool(is_key_pressed(*code)));
        }
    } else {
        // Lazily install key_press as a sibling object next to key.
        let mut press = Object {
            fields: HashMap::new(),
            kind: "input",
        };
        for (name, code) in KEYS {
            press.insert_field(*name, Value::from_bool(is_key_pressed(*code)));
        }
        env.set(
            "key_press".to_string(),
            Value::from_object(Rc::new(RefCell::new(press))),
        );
    }
    if let Some(t) = env.get("screen") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            let w = screen_width() as f64;
            let h = screen_height() as f64;
            o.insert_field(
                "size".to_string(),
                Value::from_tuple(Rc::new(vec![Value::from_float(w), Value::from_float(h)])),
            );
            o.insert_field(
                "center".to_string(),
                Value::from_tuple(Rc::new(vec![
                    Value::from_float(w / 2.0),
                    Value::from_float(h / 2.0),
                ])),
            );
        }
    }
    write_mouse_object(env.get("mouse"));
    write_mouse_buttons(env.get("mouse_held"), is_mouse_button_down);
    write_mouse_buttons(env.get("mouse_press"), is_mouse_button_pressed);
    poll_gamepad(env);
}

/// Phase 9 session 5: poll gilrs and write button + axis state to
/// the `gamepad`, `gamepad_press`, and `gamepad_axis` ambients. The
/// first connected gamepad wins; multi-gamepad routing is a follow-on.
/// `gamepad_press` is derived by diffing `PREV_GAMEPAD` against the
/// current state (gilrs's event stream is drained but we don't use
/// it directly — diffing keeps the surface symmetric with `key_press`).
fn poll_gamepad(env: &mut Env) {
    GILRS.with(|g| {
        // Lazy init the first time we're called inside the macroquad
        // window context. gilrs init failures (e.g. no input
        // subsystem) produce a single warning and disable polling.
        let mut state = g.borrow_mut();
        if matches!(*state, GilrsState::Uninit) {
            *state = match gilrs::Gilrs::new() {
                Ok(g) => GilrsState::Active(Box::new(g)),
                Err(e) => {
                    eprintln!("[twec] gamepad disabled: {e}");
                    GilrsState::Failed
                }
            };
        }
        let GilrsState::Active(gilrs) = &mut *state else {
            return;
        };
        let gilrs = gilrs.as_mut();
        // Drain pending events so gilrs's internal connection /
        // disconnection state stays current. We don't act on the
        // event payloads — buttons are sampled below by polling.
        while gilrs.next_event().is_some() {}

        let pad = gilrs.gamepads().next().map(|(_id, p)| p);
        let connected = pad.is_some();

        // Continuous + edge-triggered booleans.
        let cur: [bool; 14] = match pad {
            Some(p) => {
                let mut buf = [false; 14];
                for (i, (_name, btn)) in GAMEPAD_BUTTONS.iter().enumerate() {
                    buf[i] = p.is_pressed(*btn);
                }
                buf
            }
            None => [false; 14],
        };
        let prev = PREV_GAMEPAD.with(|p| *p.borrow());
        if let Some(t) = env.get("gamepad") {
            if t.is_object() {
                let rc = t.as_object();
                let mut o = rc.borrow_mut();
                for (i, (name, _)) in GAMEPAD_BUTTONS.iter().enumerate() {
                    o.insert_field(*name, Value::from_bool(cur[i]));
                }
                o.insert_field("connected", Value::from_bool(connected));
            }
        }
        if let Some(t) = env.get("gamepad_press") {
            if t.is_object() {
                let rc = t.as_object();
                let mut o = rc.borrow_mut();
                for (i, (name, _)) in GAMEPAD_BUTTONS.iter().enumerate() {
                    // Edge-trigger: true only on the frame the button
                    // transitions from up to down.
                    o.insert_field(*name, Value::from_bool(cur[i] && !prev[i]));
                }
            }
        }
        PREV_GAMEPAD.with(|p| *p.borrow_mut() = cur);

        // Analog axes.
        if let Some(t) = env.get("gamepad_axis") {
            if t.is_object() {
                let rc = t.as_object();
                let mut o = rc.borrow_mut();
                match pad {
                    Some(p) => {
                        for (name, axis) in GAMEPAD_AXES {
                            o.insert_field(*name, Value::from_float(p.value(*axis) as f64));
                        }
                    }
                    None => {
                        for (name, _) in GAMEPAD_AXES {
                            o.insert_field(*name, Value::from_float(0.0));
                        }
                    }
                }
            }
        }
    });
}
