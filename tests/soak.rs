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
        assert!(r.deaths >= 1, "a 10-minute session should see deaths and restarts");
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
