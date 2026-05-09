//! Phase 29 session 4: replay determinism + log round-trip tests.
//!
//! The replay primitive's correctness rests on two invariants:
//!
//! 1. **Replay determinism.** Running the same Twe program with the
//!    same input log twice produces byte-identical output. The first
//!    test exercises this with a 1000-tick deterministic counter
//!    program — pure simulation, no real input dependency, but the
//!    failure mode (any non-determinism in eval / GC / RNG order)
//!    would break this just as readily.
//!
//! 2. **Frame-log round-trip.** A captured input frame applied via
//!    `apply_frame` must be observable from inside the script as
//!    the same ambient values that were captured. The second test
//!    drives this end-to-end: write a synthetic 3-frame log, replay
//!    it through a Twe program that prints `key.left` per frame,
//!    and assert the output matches the synthetic input.

use std::fs;
use std::path::Path;

use twec::{eval, lexer, parser, replay};

fn run_n_frames(src: &str, frames: u32, dt: f64) -> String {
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    eval::run_with_frames(&program, frames, dt).expect("eval")
}

fn fnv1a(s: &[u8]) -> u64 {
    // Tiny dependency-free hash. Stable across platforms; that's
    // all the test needs.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[test]
fn deterministic_program_produces_identical_output_across_runs() {
    // 1000-frame counter scene. No external input — the only thing
    // that could vary between runs is non-deterministic eval state
    // (HashMap ordering observed through serialization, allocator
    // returning different addresses, etc.). Output is the printed
    // counter at each frame.
    let src = r#"
scene Counter:
    var n = 0

    initial: tick

    state tick:
        on update(dt):
            n += 1
            print(n)
"#;
    let a = run_n_frames(src, 1000, 1.0 / 60.0);
    let b = run_n_frames(src, 1000, 1.0 / 60.0);
    assert_eq!(
        fnv1a(a.as_bytes()),
        fnv1a(b.as_bytes()),
        "1000-frame deterministic run produced different output across two invocations"
    );
    // Sanity: the program actually ran 1000 frames.
    let lines = a.lines().count();
    assert_eq!(lines, 1000, "expected 1000 lines of counter output, got {lines}");
}

#[test]
fn replay_log_round_trips_keyboard_state_into_ambient() {
    // Synthesize a 3-frame log: frame 0 has `left` held; frame 1
    // has `right` held; frame 2 has nothing. Replay it through a
    // tree-walker run that prints whether `key.left` is true each
    // tick, and assert the printed sequence matches.
    let path = std::env::temp_dir().join("twec-replay-roundtrip.log");
    let path_str = path.to_str().unwrap();

    let log = "TWE-REPLAY v1\n\
               left||0|0||\n\
               right||0|0||\n\
               ||0|0||\n";
    fs::write(&path, log).unwrap();

    // Prime the player. Subsequent `replay::tick(env)` calls pull
    // frames out of this log and overwrite ambients.
    replay::start_playing(path_str).expect("start_playing");

    // Tree-walker drive loop that calls `replay::tick` between each
    // tick_frame, mirroring the play loop integration. We can't go
    // through `eval::run_with_frames` because that helper doesn't
    // expose a per-tick hook; instead we rebuild the loop manually.
    let src = r#"
scene S:
    initial: a
    state a:
        on update(dt):
            if key.left:
                print("L")
            else:
                print("-")
"#;
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    eval::run_top_level(&mut env, &program).expect("run_top_level");

    let mut output = String::new();
    for _ in 0..3 {
        replay::tick(&mut env);
        eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick_frame");
        output.push_str(&env.out);
        env.out.clear();
    }
    replay::stop();
    let _ = fs::remove_file(&path);

    // Frame 0: left held → "L". Frame 1: only right → "-". Frame 2:
    // nothing → "-".
    assert_eq!(output, "L\n-\n-\n", "got {output:?}");
}

#[test]
fn record_then_play_reproduces_the_same_input_stream() {
    // End-to-end: record a synthetic input stream by mutating the
    // env directly, then replay the recorded log into a fresh env
    // and check the script observes the same per-frame values.
    let path = std::env::temp_dir().join("twec-replay-record-then-play.log");
    let path_str = path.to_str().unwrap();

    // ---- Record phase ----
    {
        let src = r#"
scene S:
    var n = 0
    initial: a
    state a:
        on update(dt):
            n += 1
"#;
        let tokens = lexer::lex(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let mut env = twec::value::Env::new();
        twec::stdlib::install(&mut env);
        eval::run_top_level(&mut env, &program).expect("top");
        replay::start_recording(path_str).expect("start_recording");
        for i in 0..5 {
            // Synthesize: even frame holds `up`, odd frame holds `down`.
            inject_held_keys(
                &mut env,
                if i % 2 == 0 { &["up"] } else { &["down"] },
            );
            replay::tick(&mut env);
            eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
        }
        replay::stop();
        assert!(Path::new(path_str).exists(), "log not written");
    }

    // ---- Playback phase ----
    let src = r#"
scene S:
    initial: a
    state a:
        on update(dt):
            if key.up:
                print("U")
            elif key.down:
                print("D")
            else:
                print("?")
"#;
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    eval::run_top_level(&mut env, &program).expect("top");
    replay::start_playing(path_str).expect("start_playing");

    let mut output = String::new();
    for _ in 0..5 {
        replay::tick(&mut env);
        eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
        output.push_str(&env.out);
        env.out.clear();
    }
    replay::stop();
    let _ = fs::remove_file(path_str);

    assert_eq!(output, "U\nD\nU\nD\nU\n", "got {output:?}");
}

/// Test helper: synthesize "these keys are held" by mutating the
/// `key` ambient directly, the way the macroquad-side
/// `update_key_state` would — except in tests there's no window so
/// we have to fill it ourselves.
fn inject_held_keys(env: &mut twec::value::Env, keys: &[&str]) {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    let mut fields = HashMap::new();
    for k in keys {
        fields.insert(k.to_string(), twec::value::Value::from_bool(true));
    }
    let obj = twec::value::Value::from_object(Rc::new(RefCell::new(twec::value::Object {
        fields,
        kind: "input",
    })));
    env.set("key".to_string(), obj);
}
