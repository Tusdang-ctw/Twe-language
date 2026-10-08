//! web3d-M11 follow-on: physics on the shared tick path.
//!
//! The rapier3d world used to be stepped only by the native-only
//! `play3d` loop, so browser 3D games never advanced physics. The step
//! now lives in `host3d::sim_tick`, the one tick every 3D shell runs —
//! native `play3d`, the browser shell, the soak harness. These tests
//! pin that: a body falls when driven through `sim_tick` (never calling
//! `physics3d::step` directly), and an empty world steps as a no-op so
//! non-physics 3D scenes pay nothing for the shared step.
//!
//! Native-gated like the other physics tests: the integration-test
//! harness builds for the host, and the wasm *build* check
//! (`cargo check -p twe-web --target wasm32-unknown-unknown`) is what
//! proves the same path compiles for the browser.

#![cfg(not(target_arch = "wasm32"))]

use twec::host3d::{sim_tick, InputState};
use twec::physics3d;

const DT: f64 = 1.0 / 60.0;

/// Build an `Env` from a minimal 3D script that never touches physics,
/// so any motion in the world must come from `sim_tick` stepping it.
fn env_from(src: &str) -> twec::value::Env {
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    env
}

#[test]
fn sim_tick_steps_the_physics_world() {
    physics3d::reset();
    // A free body high up. The script below never calls `physics.*`, so
    // the only thing that can make it fall is `sim_tick`'s own step.
    let ball = physics3d::body("sphere", [0.0, 10.0, 0.0], 1.0).expect("body");
    let y0 = physics3d::position(ball).expect("pos")[1];

    let mut env = env_from("var ticks = 0\n\non update(dt):\n    ticks = ticks + 1\n");
    let mut input = InputState::default();
    for _ in 0..90 {
        sim_tick(&mut env, &mut input, DT).expect("tick");
    }

    let y1 = physics3d::position(ball).expect("pos")[1];
    assert!(
        y1 < y0 - 1.0,
        "sim_tick advanced the physics world under gravity (y {y0} -> {y1})"
    );
    physics3d::reset();
}

#[test]
fn sim_tick_on_an_empty_world_is_a_harmless_noop() {
    // Guards the "non-physics 3D scenes pay nothing" claim: with no
    // bodies spawned, ticking the shared path many times must not error
    // or panic — it is the path every ordinary 3D example runs.
    physics3d::reset();
    let mut env = env_from("var n = 0\n\non update(dt):\n    n = n + 1\n");
    let mut input = InputState::default();
    for _ in 0..120 {
        sim_tick(&mut env, &mut input, DT).expect("empty-world tick must not error");
    }
    physics3d::reset();
}
