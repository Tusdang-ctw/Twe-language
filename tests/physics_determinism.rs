//! web3d Track D — physics determinism gate for P2P rollback.
//!
//! Rollback requires every peer's physics simulation to produce
//! bit-identical results from identical inputs. rapier's
//! `enhanced-determinism` feature (enabled in Cargo.toml) makes that hold
//! across different CPUs / SIMD paths, *assuming the surrounding code is
//! deterministic*. This harness is the cross-machine check.
//!
//! The scene is a ragged cluster of mixed-shape bodies settling on a
//! floor — the classic amplifier of floating-point divergence, since a
//! sub-ULP difference in one contact cascades through the whole stack.
//! It is stepped a fixed number of ticks and the final state is hashed.
//!
//! - **Same machine** (asserted here): the two runs must match.
//! - **Cross machine** (the real go/no-go, your step): run `cargo test
//!   --test physics_determinism -- --nocapture` on two different machines
//!   and compare the printed `PHYS-HASH` line. Identical → P2P rollback is
//!   safe to build on rapier. Different → constrain the networked physics
//!   (or pick a model that tolerates divergence) before investing in the
//!   matchbox/ggrs stack.

#![cfg(not(target_arch = "wasm32"))]

use twec::physics3d;

const DT: f32 = 1.0 / 60.0;
const TICKS: usize = 600;
const BODIES: u32 = 24;

fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Build the scene, step it `TICKS` times, and hash every body's final
/// position (exact f32 bit patterns, so any divergence shows).
fn simulate() -> u64 {
    physics3d::reset();
    // A floor to collide against.
    physics3d::static_box([0.0, 0.0, 0.0], [20.0, 1.0, 20.0]);
    // A ragged cluster of falling bodies: varied shapes, positions and
    // masses so the settle is chaotic.
    let mut handles = Vec::new();
    for i in 0..BODIES {
        let fi = i as f32;
        let x = (fi * 0.37).sin() * 3.0;
        let z = (fi * 0.53).cos() * 3.0;
        let y = 3.0 + fi * 0.45;
        let shape = if i % 2 == 0 { "sphere" } else { "box" };
        let mass = 1.0 + (i % 3) as f32 * 0.5;
        handles.push(physics3d::body(shape, [x, y, z], mass).expect("body"));
    }
    for _ in 0..TICKS {
        physics3d::step(DT);
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for h in &handles {
        let p = physics3d::position(*h).expect("position");
        for c in p {
            hash = fnv1a(hash, &c.to_bits().to_le_bytes());
        }
    }
    physics3d::reset();
    hash
}

#[test]
fn physics_is_deterministic_on_this_machine() {
    let a = simulate();
    let b = simulate();
    assert_eq!(
        a, b,
        "physics diverged between two runs on the same machine — a determinism \
         bug that breaks rollback regardless of CPU"
    );
    // Printed for the cross-machine gate: run with `--nocapture` on two
    // different machines and compare this line.
    println!("PHYS-HASH ticks={TICKS} bodies={BODIES} hash={a:#018x}");
}
