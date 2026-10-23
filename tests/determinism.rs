//! web3d Track D (multiplayer) — cross-instance determinism guard.
//!
//! P2P rollback netcode rests on one property: **two independent
//! simulations fed the same inputs produce the same state.** A peer
//! re-simulates another peer's frames locally, so any divergence —
//! RNG drift, entity-iteration order leaking through serialization,
//! allocation-address-dependent behaviour, time-dependent code — breaks
//! rollback even between two instances on one machine. These tests lock
//! that property in.
//!
//! Scope note: this is the *same-machine, same-binary* guard. It catches
//! accidental nondeterminism (the common rollback killer). It does NOT
//! prove *cross-machine* float determinism across different CPUs / SIMD —
//! that needs rapier's `enhanced-determinism` feature and a second
//! machine, and is tracked as a maintainer step in the plan.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use twec::{eval, lexer, parser};

fn fnv1a(s: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn run_n_frames(src: &str, frames: u32, dt: f64) -> String {
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    eval::run_with_frames(&program, frames, dt).expect("eval")
}

/// Two fresh runs of a program that leans on every common source of
/// nondeterminism — RNG (`random.int` / `random.float`), several
/// entities updated each tick, and scene state — must be byte-identical.
#[test]
fn two_fresh_sims_with_rng_and_entities_match() {
    // `random.*` draws from the Env's RNG, which starts from a fixed
    // default seed; entities update in spawn order. If either leaked a
    // run-to-run difference, the digests below would diverge.
    let src = r#"
entity Mote:
    var pos = (0.0, 0.0)

    function update(dt):
        let a = random.float()
        pos = (pos.x + a, pos.y)
        print(pos.x)

scene Sim:
    var n = 0

    initial: tick

    state tick:
        on update(dt):
            n += 1
            let r = random.int(0..<1000)
            print(n)
            print(r)

spawn Mote at (0.0, 0.0)
spawn Mote at (2.0, 0.0)
"#;
    let a = run_n_frames(src, 500, 1.0 / 60.0);
    let b = run_n_frames(src, 500, 1.0 / 60.0);
    assert_eq!(
        fnv1a(a.as_bytes()),
        fnv1a(b.as_bytes()),
        "two fresh sims diverged — RNG or entity order is not deterministic (breaks rollback)"
    );
    // Guard against the digest being trivially empty.
    assert!(
        a.lines().count() >= 500,
        "expected the sim to actually run, got {} lines",
        a.lines().count()
    );
}

/// The rollback property itself: feed two independent instances the
/// identical per-tick input stream and the observed state must match.
#[test]
fn identical_input_stream_yields_identical_state() {
    let src = r#"
scene Move:
    var x = 0.0
    var y = 0.0

    initial: a

    state a:
        on update(dt):
            if key.left:
                x -= 1.0
            if key.right:
                x += 1.0
            if key.up:
                y -= 1.0
            if key.down:
                y += 1.0
            print(x)
            print(y)
"#;
    // A deterministic but non-trivial input script: each tick holds a
    // key chosen from the tick index, so the path wanders.
    let keys = ["left", "right", "up", "down"];
    let script: Vec<&str> = (0..300).map(|i| keys[(i * 7 + 3) % keys.len()]).collect();

    let run = || -> String {
        let tokens = lexer::lex(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let mut env = twec::value::Env::new();
        twec::stdlib::install(&mut env);
        eval::run_top_level(&mut env, &program).expect("top");
        let mut out = String::new();
        for held in &script {
            inject_key(&mut env, held);
            eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
            out.push_str(&env.out);
            env.out.clear();
        }
        out
    };

    let a = run();
    let b = run();
    assert_eq!(
        fnv1a(a.as_bytes()),
        fnv1a(b.as_bytes()),
        "same input stream produced different state across two instances (breaks rollback)"
    );
    assert!(a.contains('\n'), "the input-driven sim produced no output");
}

/// Set the `key` ambient directly, the way the windowed host would from
/// real input. Every key the script reads must be present (the real
/// ambient defines held and un-held keys alike), so define all four and
/// mark only `held` true.
fn inject_key(env: &mut twec::value::Env, held: &str) {
    let mut fields = HashMap::new();
    for k in ["left", "right", "up", "down"] {
        fields.insert(k.to_string(), twec::value::Value::from_bool(k == held));
    }
    let obj = twec::value::Value::from_object(Rc::new(RefCell::new(twec::value::Object {
        fields,
        kind: "input",
    })));
    env.set("key".to_string(), obj);
}
