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

pub fn launch(path: String) -> i32 {
    let conf = window_conf();
    macroquad::Window::from_config(conf, run_loop(path));
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
    if let Some(Value::Object(rc)) = env.get("key").cloned() {
        let mut o = rc.borrow_mut();
        for (name, code) in KEYS {
            o.fields
                .insert((*name).to_string(), Value::Bool(is_key_down(*code)));
        }
    }
    if let Some(Value::Object(rc)) = env.get("key_press").cloned() {
        let mut o = rc.borrow_mut();
        for (name, code) in KEYS {
            o.fields
                .insert((*name).to_string(), Value::Bool(is_key_pressed(*code)));
        }
    } else {
        // Lazily install key_press as a sibling object next to key. The
        // stdlib already installs it but legacy scripts loaded via
        // hot-reload may have a stale env without it.
        let mut fields = HashMap::new();
        for (name, code) in KEYS {
            fields.insert((*name).to_string(), Value::Bool(is_key_pressed(*code)));
        }
        env.set(
            "key_press".to_string(),
            Value::Object(Rc::new(RefCell::new(Object {
                fields,
                kind: "input",
            }))),
        );
    }
    if let Some(Value::Object(rc)) = env.get("screen").cloned() {
        let mut o = rc.borrow_mut();
        let w = screen_width() as f64;
        let h = screen_height() as f64;
        o.fields.insert(
            "size".to_string(),
            Value::Tuple(Rc::new(vec![Value::Float(w), Value::Float(h)])),
        );
        o.fields.insert(
            "center".to_string(),
            Value::Tuple(Rc::new(vec![Value::Float(w / 2.0), Value::Float(h / 2.0)])),
        );
    }
}
