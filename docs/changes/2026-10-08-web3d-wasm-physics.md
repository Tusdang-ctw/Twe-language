# web3d follow-on: physics in the browser (wasm rapier backend)

**Date:** 2026-10-08
**Line:** web3d creation-parity (M8–M12), M11 follow-on
**Status:** closed — browser 3D games now have the same physics as native.

## What this closes

The M11 closeout ([`2026-10-05-web3d-m11-closeout.md`](2026-10-05-web3d-m11-closeout.md))
deferred "the physics bridge on the web" with the note *"rapier isn't on
wasm … a wasm physics backend is out of scope."* That was the starkest
hole in the creation-parity thesis: **"the best way to build a *browser*
3D game" shipped with zero in-browser physics.** Any game using
`physics.*` had to guard every call behind `not assets.is_browser()`
(as `examples/action3d` did for its rope bridge).

The premise turned out to be wrong in our favour. rapier3d is pure Rust
and, with `default-features = false` (which keeps the `parallel`/rayon
feature off — already correct for the single-threaded VM), it compiles
to `wasm32-unknown-unknown` unchanged. The gating was a Phase-30
historical choice, not a real incompatibility (the Cargo.toml comment
said as much: *"Pure Rust — could compile to wasm32, but the 3D play
loop is native-only for Phase 30."*).

## What shipped

- **`rapier3d` moved from the native-only dependency block to the main
  `[dependencies]`** (`Cargo.toml`), so it cross-compiles for wasm.
- **`physics3d` is no longer `#[cfg(not(target_arch = "wasm32"))]`**
  (`src/lib.rs`). The module is pure Rust + the `gltf` crate (already a
  wasm dependency, used for `physics.static_mesh` trimeshes) + the
  all-targets `bundle` loader — nothing native-only.
- **`physics.*` installs on every target** (`src/stdlib.rs`): the module
  registration block and all 20 `physics_*_impl` builtins plus the
  `joint_result` helper lost their wasm gates.
- **The step moved to the shared tick** (`src/host3d.rs`): `sim_tick` —
  the one update every 3D shell runs (native `play3d`, the browser
  shell, the soak harness) — now calls `physics3d::step(dt)` before the
  script's `on update(dt)` body. It used to live in the native-only
  `play3d` loop, which is exactly why the browser never advanced
  physics. The duplicate step was removed from `play3d`
  (`step_simulation_3d`), so native ordering is unchanged
  (step → input → `tick_frame`).
- **`examples/action3d/main.twe`**: the three `not assets.is_browser()`
  guards around the rope bridge (build, shove-on-E, draw) and the
  "native-only" fallback text are gone — the jointed bridge now sways in
  the browser like it does natively.

## Why `sim_tick` is the right home for the step

`physics3d::step` on an empty world is a near-instant no-op (the body
set is empty), so putting it on the shared path costs nothing for the
many 3D scenes that never spawn a body. Native behaviour is identical
(the step simply moved a few lines, keeping the step → input →
`tick_frame` order). And there is now exactly one place physics
advances, for every shell, which is the same single-source-of-truth
shape `host3d` already gives input.

## Verification

- `cargo check` (native) — clean.
- `cargo check -p twe-web --target wasm32-unknown-unknown` — clean;
  rapier3d + parry + nalgebra cross-compile for the browser. **This is
  the deliverable: the physics surface compiles for web.**
- `cargo clippy -- -D warnings` (native) and
  `cargo clippy -p twe-web --target wasm32-unknown-unknown -- -D warnings`
  — both clean.
- New `tests/physics_web.rs`: `sim_tick_steps_the_physics_world` drops a
  body through `host3d::sim_tick` (the browser path, never calling
  `physics3d::step` directly) and asserts it falls under gravity;
  `sim_tick_on_an_empty_world_is_a_harmless_noop` guards the "non-physics
  scenes pay nothing" claim.
- Unbroken: `tests/physics_joints.rs` (2), `tests/survive3d.rs` (4 — a
  non-physics 3D game still ticks correctly with the added step),
  `tests/soak.rs` smoke (survive3d uses `physics.*` 0×, so the soak
  determinism path is a no-op step and its hash is unchanged),
  `tests/cli.rs`.

## Cost / notes

- The wasm bundle grows by rapier + parry + nalgebra. That is the
  intended price of browser physics; it is paid only by the 3D web
  build.
- A live in-browser run of `action3d`'s bridge is the maintainer's
  GPU/playtest step (same class as the other M11 visual deferrals).
- `physics.static_mesh` decodes a `.glb` via the `gltf` crate on wasm
  just as natively; assets still resolve through the bundle.

## Doc edits applied

- `Cargo.toml` — `rapier3d` moved + comment rewritten.
- `src/lib.rs`, `src/stdlib.rs`, `src/host3d.rs`, `src/play3d.rs` —
  comments updated to describe the shared-tick step.
- `CLAUDE.md` — the M11 line's rope-bridge and "wasm physics backend"
  deferral marked closed with a pointer here.
- `docs/changes/2026-10-05-web3d-m11-closeout.md` — inline `UPDATE`
  correction on the deferral (history preserved).
- `CHANGELOG.md`, `docs/journey.md` — entries added.
