//! web3d-M4: soak `examples/survive3d` with the scripted player in
//! `twec::soak` (the same driver the wasm soak runs under Node).
//!
//! The smoke soak runs with the suite. The long ones are opt-in:
//!
//!     cargo test --release --test soak -- --ignored --nocapture
//!
//! `soak_ten_ten_minute_runs` is M4's exit soak (10 consecutive
//! 10-minute sessions); `soak_under_gc_stress` repeats a shorter one
//! with the GC collecting at every safepoint.

use twec::soak::{run, SoakReport};

const MINUTE: u64 = 60 * 60;

fn soak(ticks: u64, variant: u64) -> SoakReport {
    twec::bundle::set_asset_root(Some("examples/survive3d".into()));
    // Best-run saves land in a scratch data dir, not the user's.
    std::env::set_var(
        "TWE_DATA_DIR",
        std::env::temp_dir().join(format!("twe-soak-{}", std::process::id())),
    );
    let src = std::fs::read_to_string("examples/survive3d/main.twe").expect("read game");
    run(&src, ticks, variant).unwrap_or_else(|e| panic!("soak failed: {e}"))
}

/// Heap after each restart stays within a band: a leak across runs
/// would grow it run over run.
fn assert_no_growth(r: &SoakReport) {
    if let (Some(first), Some(max)) = (r.heap_at_restart.first(), r.heap_at_restart.iter().max()) {
        assert!(
            *max <= first + first / 2 + 64 * 1024,
            "heap after restarts grew: {:?}",
            r.heap_at_restart
        );
    }
}

#[test]
fn soak_smoke_one_minute() {
    let r = soak(MINUTE, 0);
    assert_eq!(r.ticks, MINUTE);
    assert!(r.peak_entities > 10, "{r:?}");
}

#[test]
#[ignore = "long: run with --release -- --ignored"]
fn soak_ten_ten_minute_runs() {
    for i in 0..10 {
        let r = soak(10 * MINUTE, i);
        eprintln!("soak {}: {}", i + 1, r.to_json());
        assert!(
            r.deaths >= 1,
            "a 10-minute session should see deaths and restarts"
        );
        assert_no_growth(&r);
    }
}

#[test]
#[ignore = "long: run with --release -- --ignored"]
fn soak_under_gc_stress() {
    twec::heap::gc_set_stress(true);
    let r = soak(2 * MINUTE, 7);
    twec::heap::gc_set_stress(false);
    eprintln!("stress soak: {}", r.to_json());
    assert!(r.level_ups >= 1, "{r:?}");
}

/// Lanternfall G8: load the multi-file battle royale the way `twec
/// play3d` does and soak it with the Lanternfall pilot. Variants are
/// 1-based to match `node web/soak.mjs <build> M N --pilot lanternfall`,
/// so native and wasm reports can be compared run for run.
fn soak_lanternfall(ticks: u64, variant: u64) -> SoakReport {
    twec::bundle::set_asset_root(Some("examples/lanternfall".into()));
    std::env::set_var(
        "TWE_DATA_DIR",
        std::env::temp_dir().join(format!("twe-soak-lf-{}-{variant}", std::process::id())),
    );
    let main = std::path::Path::new("examples/lanternfall/main.twe");
    let src = std::fs::read_to_string(main).expect("read game");
    let env = twec::module::prepare_entry(main, &src).expect("load the game");
    twec::soak::run_env(env, ticks, variant, twec::soak::Pilot::Lanternfall)
        .unwrap_or_else(|e| panic!("soak failed: {e}"))
}

#[test]
#[ignore = "long: run with --release -- --ignored"]
fn soak_lanternfall_ten_ten_minute_runs() {
    for i in 1..=10 {
        let r = soak_lanternfall(10 * MINUTE, i);
        eprintln!("lanternfall soak {i}: {}", r.to_json());
        assert!(r.rounds >= 1, "ten minutes should finish a round: {r:?}");
        assert_no_growth(&r);
    }
}

#[test]
#[ignore = "long: run with --release -- --ignored"]
fn soak_lanternfall_under_gc_stress() {
    twec::heap::gc_set_stress(true);
    let r = soak_lanternfall(2 * MINUTE, 3);
    twec::heap::gc_set_stress(false);
    eprintln!("lanternfall stress soak: {}", r.to_json());
    assert!(r.peak_entities > 100, "{r:?}");
}
