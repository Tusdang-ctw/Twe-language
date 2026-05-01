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
        if let Err(e) = crate::eval::tick_frame(&mut env, dt) {
            eprintln!("{path_ref}: runtime error: {e}");
            break;
        }
        flush_output(&mut env);

        clear_background(BLACK);
        env.in_render = true;
        if let Err(e) = crate::eval::render_frame(&mut env) {
            eprintln!("{path_ref}: runtime error: {e}");
            env.in_render = false;
            break;
        }
        env.in_render = false;
        flush_output(&mut env);

        next_frame().await;
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
}
