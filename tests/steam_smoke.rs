//! Phase 35 sub-deliverable: Steam SDK end-to-end smoke test scaffold.
//!
//! Phase 15 session 3 shipped the optional Steam SDK integration
//! (`--features steam`). The Phase 15 closeout left the
//! "end-to-end Steam SDK test on a live AppID" criterion *pending
//! external action* — the test requires a Steam account, a real
//! AppID, and a running Steam client. This test scaffold closes
//! the loop on what we *can* automate:
//!
//! 1. The no-feature build compiles + runs the stubs without
//!    panicking. (Default `cargo test`.)
//! 2. The `--features steam` build links against `steamworks` and
//!    can attempt initialisation. (`cargo test --features steam`.)
//!
//! What's still operator-action:
//!
//! - End-to-end achievement-unlock-round-trip on a real Steam AppID.
//!   Requires `steam_appid.txt` next to the test binary with a valid
//!   AppID (480 = Spacewar, Steam's public test app), the Steam
//!   client running, and a Steam account that owns Spacewar.
//! - Cloud save round-trip. Same requirements.
//! - Stat commit + leaderboard read. Same requirements.
//!
//! When operator-action conditions are met, run:
//!
//!   cargo test --features steam --test steam_smoke -- --nocapture
//!
//! and inspect the printed status.
//!
//! ## Phase 35 contract
//!
//! This file proves the Steam build path stays buildable. A future
//! contributor with a Steam AppID can run the gated portion to
//! produce the end-to-end criterion's evidence; the result goes in
//! a `docs/changes/<date>-phase-35-steam-validation.md` note.

use twec::value::Env;

#[test]
fn no_feature_steam_stubs_compile_and_dont_panic() {
    // Default build: no `--features steam`. The stubs in src/steam.rs
    // should compile, return Nil/false on every call, and never
    // attempt to load `steam_api.dll` (the redistributable isn't
    // present in the dev workspace).
    twec::steam::init();
    assert!(
        !twec::steam::is_available(),
        "is_available() must be false in the no-feature build"
    );

    let mut env = Env::new();

    // Each builtin should accept ≥ 0 args and return Nil without
    // panicking. We pass a string arg where required and an empty
    // arg list otherwise; the no-feature stubs ignore args.
    let name = twec::value::Value::from_string(String::from("FIRST_KILL"));
    let _ = twec::steam::achievement_unlock(&mut env, std::slice::from_ref(&name));

    let stat_name = twec::value::Value::from_string(String::from("KILLS_TOTAL"));
    let stat_val = twec::value::Value::from_int(1000);
    let _ = twec::steam::stat_set(&mut env, &[stat_name, stat_val]);
    let _ = twec::steam::stat_get(&mut env, std::slice::from_ref(&stat_name));
    let _ = twec::steam::stat_commit(&mut env, &[]);

    let file = twec::value::Value::from_string(String::from("slot1.json"));
    let payload = twec::value::Value::from_string(String::from("{}"));
    let _ = twec::steam::cloud_save(&mut env, &[file, payload]);
    let _ = twec::steam::cloud_load(&mut env, std::slice::from_ref(&file));
}

#[cfg(feature = "steam")]
#[test]
#[ignore = "requires running Steam client + steam_appid.txt with a valid AppID; run with --ignored"]
fn feature_steam_initialises_when_steam_is_running() {
    // This test requires:
    //   1. A `steam_appid.txt` next to the cargo test binary, e.g.
    //      target/debug/deps/steam_smoke-XXXX.exe; the easy path is
    //      to write `480` to that file before running the test.
    //   2. The Steam client running and signed in.
    //   3. The Steam account owning Spacewar (free, AppID 480).
    //
    // When all three are satisfied, init() should produce a live
    // client and is_available() should return true.
    twec::steam::init();
    assert!(
        twec::steam::is_available(),
        "Steam client did not initialise. Check that Steam is running, \
         steam_appid.txt is present next to the test binary, and \
         the signed-in account owns the AppID. See test source for setup steps."
    );
    println!("[steam smoke] is_available = true");
}

#[cfg(feature = "steam")]
#[test]
#[ignore = "requires running Steam + valid AppID; run with --ignored after Steam test"]
fn feature_steam_achievement_round_trip() {
    twec::steam::init();
    if !twec::steam::is_available() {
        println!("[steam smoke] Steam not available; skipping achievement test");
        return;
    }
    let mut env = Env::new();
    // Spacewar (AppID 480) ships with achievement IDs ACH_TRAVEL_FAR_ACCUM,
    // ACH_TRAVEL_FAR_SINGLE, etc. We attempt unlocking one and rely on
    // Steam's idempotent unlock to make repeated runs harmless.
    let name = twec::value::Value::from_string(String::from("ACH_TRAVEL_FAR_ACCUM"));
    let _ = twec::steam::achievement_unlock(&mut env, std::slice::from_ref(&name));
    println!("[steam smoke] Achievement unlock attempted (Spacewar ACH_TRAVEL_FAR_ACCUM)");
}
