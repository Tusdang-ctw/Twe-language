# web3d follow-on: a 1-D animation blend space

**Date:** 2026-10-08
**Line:** web3d creation-parity (M8–M12), M11 follow-on
**Status:** closed — continuous locomotion blending ships; the declarative
anim state machine stays open (see below).

## What this closes

The M11 closeout deferred *"a true blend-space / anim state machine
(beyond crossfade + the 2-clip blend)."* This closes the **blend-space**
half. The reference game (`examples/action3d`) drove its idle↔walk↔run
locomotion by discrete speed thresholds — at 4.6 m/s it *switched* clip
and `play()` crossfaded over 0.15 s. That pops and double-counts: the
crossfade fights the threshold. A locomotion blend space is the right
primitive: one parameter (ground speed) continuously weights an ordered
set of clips.

## What shipped

- **`mesh_anim.blend_space(mesh, clips, stops, value)`** (`src/stdlib.rs`):
  `clips` are clip names at ascending `stops` (e.g. `["idle","walk","run"]`
  at `[0.0, 3.2, 6.5]`); `value` selects the two bracketing clips and the
  weight between them. Below the first stop it plays the first clip; above
  the last, the last. Call it each tick with the current `value`.
- **Zero renderer change.** A 1-D blend space at any parameter always
  blends exactly the two bracketing clips, so it reduces to the existing
  2-clip `AnimSnapshot` (`clip` ⊕ `blend_clip` by `blend_t`) that
  `compute_skinned_joint_matrices` already lerps/slerps. Same trick M9
  used for morph targets: reuse the established path, add no shader.
- **Continuous, phase-synced.** `blend_space` never resets the clock when
  `value` crosses a stop (unlike `play()`, which zeroes `time` on a clip
  change), and it sets the partner clip's `blend_time` to the primary's
  `time`, so the two active gaits share one clock and feet don't slide
  relative to each other. It also cancels any in-flight auto-crossfade —
  the blend space owns the weight.
- **`examples/action3d`** now drives locomotion with one
  `mesh_anim.blend_space(hero, ["idle","walk","run"], [0.0, walk_speed, run_speed], speed_now)`
  call, replacing ~10 lines of threshold + `play()` switching and the
  `loco` bookkeeping var. The attack clip still locks locomotion out while
  a swing plays (`play(hero, "attack", false)`).

## API shape

`blend_space` sits beside `blend` (the explicit 2-clip hold) and `play`
(a single clip with an auto-crossfade on change). One obvious way per job:
`play` for discrete state clips (attack, hit, jump), `blend_space` for a
parameterised continuum (locomotion). Bad input is reported, not
swallowed: unequal/short `clips`/`stops`, or non-ascending `stops`, raise
a `RuntimeError` with an example in the help.

## Verification

- `cargo test --lib mesh_anim` — `mesh_anim_blend_space_brackets_and_weights`
  (below-range → first clip; mid idle→walk weight 0.5; into walk→run span
  weight 0.5; above-range → last clip; the clock keeps running and the
  partner stays phase-synced across a parameter move; bad input errors) +
  the existing `mesh_anim_crossfade_ramps_then_clears`, both pass.
- `examples/action3d/main.twe` verifies clean (0/0).
- `cargo clippy -- -D warnings` native and
  `cargo clippy -p twe-web --target wasm32-unknown-unknown -- -D warnings`
  — both clean. (Builds on web too; it is plain stdlib + render3d_types,
  no native-only code.)

## Still open (tracked, not done)

- **A declarative animation state machine** — named states with transition
  conditions (so the script says *states + edges*, not *set the clip each
  tick*). `blend_space` + `play` cover the common cases today; the state
  machine is the next anim increment if a reference game needs it.
- **2-D / directional blend spaces** (strafe) — the N-way generalisation
  would need `AnimSnapshot` to carry >2 weighted clips (a renderer change);
  deferred until a game needs it.
- Visual confirmation of the blended gait is the maintainer's GPU step,
  like the other M11 visual deferrals.

## Doc edits applied

- `docs/06-design-document.md` §7 — `blend_space` documented in the
  `mesh_anim` block.
- `CHANGELOG.md`, `docs/journey.md` — entries added.
