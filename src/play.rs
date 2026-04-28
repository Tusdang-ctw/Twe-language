//! `twec play` — interactive macroquad backend.
//!
//! Owns the real game loop: reads keyboard input each frame, ticks the
//! interpreter's active scene, runs `on render():` handlers inside a
//! macroquad draw frame. The headless `twec run` path remains unchanged
//! and is still the test entry point.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

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
    let src = match std::fs::read_to_string(Path::new(&path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return;
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return;
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return;
        }
    };

    let mut env = Env::new();
    crate::stdlib::install(&mut env);
    env.in_render = false;
    if let Err(e) = crate::eval::run_top_level(&mut env, &program) {
        eprintln!("{path}: runtime error: {e}");
        return;
    }
    flush_output(&mut env);

    loop {
        if is_key_pressed(KeyCode::Escape) {
            break;
        }
        update_key_state(&mut env);
        let dt = get_frame_time() as f64;
        if let Err(e) = crate::eval::tick_frame(&mut env, dt) {
            eprintln!("{path}: runtime error: {e}");
            break;
        }
        flush_output(&mut env);

        clear_background(BLACK);
        env.in_render = true;
        if let Err(e) = crate::eval::render_frame(&mut env) {
            eprintln!("{path}: runtime error: {e}");
            env.in_render = false;
            break;
        }
        env.in_render = false;
        flush_output(&mut env);

        next_frame().await;
    }
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
        // Lazily install key_press as a sibling object next to key.
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
