# web3d-M11 closeout — game-feature depth

**Closed:** 2026-10-05 (opened 2026-10-05). Plan: [`2026-10-05-web3d-m11-plan.md`](2026-10-05-web3d-m11-plan.md). Line: creation-parity (M8–M12), [`2026-10-04-creation-parity-plan.md`](2026-10-04-creation-parity-plan.md).

> **The milestone, in one line:** the 3D features a real game needs that Twe still lacks — proven by a reference game that stands next to a comparable Three.js game.

## Reference game

A new small **3rd-person action slice** (Tunic-like): `examples/action3d/` (~237 lines of Twe) — a hero with blended locomotion + a sword attack, chasing enemies, and a physics rope bridge. Runs natively (`twec play3d`) and builds for the browser (`twec build --target web`, verified end-to-end).

## What shipped (against the exit criteria)

| Exit criterion | Status |
|---|---|
| Animation blending + state machines | **Done** — crossfade on clip change (idle↔walk↔run + a one-shot attack); a blend-space partner (`mesh_anim.blend`) remains available |
| Physics joints + CCD | **Done** — fixed / spherical / revolute / prismatic joints + `set_ccd` |
| Rollback rewind engine | **Not in scope** (the reference game isn't net-twitch — a locked decision) |
| A polished reference 3D game, in a browser, next to a Three.js game | **Built + browser-builds**; visual polish + the side-by-side are a maintainer/M12 step |

Six sessions; file anchors + commit hashes in the plan:

- **s1 — animation crossfade** (`806c9ff`): `mesh_anim.play()` crossfades (the old clip ramps out over `set_crossfade` seconds, default 0.15, via a new `blend_time`); also fixed a latent M10 bug (`debug::DEBUG_ACTIVE` made thread-local).
- **s2 — physics joints + CCD** (`5d008fd`): six builtins over the already-stepped rapier joint/CCD sets; `tests/physics_joints.rs`.
- **s3 — game scaffold** (`9a2328c`): the action-hero glb (4 clips), 3rd-person chase camera, crossfaded locomotion.
- **s4 — combat** (`06c28e1`): attack + 8 chasing enemies with health/death.
- **s5 — rope bridge** (`0b7e789`): 9 spherical-jointed planks between static posts, native-only (guarded on web).
- **s6 — polish + web** (this commit): IBL (`light.environment` + a copied sky), and a verified `twec build --target web` producing `dist/web` (index.html + content-hashed runtime + a 355 KB bundle).

## Verification

- Language features: `mesh_anim_crossfade_ramps_then_clears` (crossfade) + `tests/physics_joints.rs` (a spherical joint holds a body up vs free-fall; `remove_joint`; `set_ccd`). Native + wasm32 clippy clean; the suite stays green; survive3d headless-GPU 4/4 (anim path unchanged).
- The game: `twec verify` clean (0/0), ticks 120 frames headless, `examples_run` green, and `twec build --target web` succeeds (the web runtime built with `wasm-bindgen 0.2.120`).

## Twe vs Three.js (the short comparison)

The action slice is **~237 lines of Twe, one file, no dependencies**, built to a self-contained browser bundle with one command. A comparable Three.js game needs, by hand and across several files + npm deps: `GLTFLoader` + `SkeletonHelper`; an `AnimationMixer` with manual `crossFadeTo` wiring per transition (Twe: `play()` crossfades by default); `cannon-es`/`rapier` for the jointed bridge + its own draw-sync loop; `OrbitControls` or a hand-rolled chase camera; and per-material setup. Twe's wedges show here — **game concepts are first-class** (`entity` / `scene` / `look:` / `visual`), **one obvious way** (locomotion is three `play()` calls that crossfade themselves), **determinism**, and a **tiny self-contained build**. The full, measured side-by-side (same game in both, time/LOC/friction) is **M12's same-game case study**, with a GPU/visual pass on the slice as the maintainer step.

## Deferrals (maintainer / follow-on)

- **Visual polish + playtest** of the slice (combat feel, the bridge's swing, the crossfades) — needs a GPU.
- **The physics bridge on the web** — rapier isn't on wasm; the browser build ships locomotion + combat and shows a note. A wasm physics backend is out of scope. **UPDATE 2026-10-08:** this was closed by the wasm-physics follow-on ([`2026-10-08-web3d-wasm-physics.md`](2026-10-08-web3d-wasm-physics.md)) — rapier3d does compile to wasm, so `physics.*` now runs in the browser and the bridge's `not assets.is_browser()` guards are gone.
- **A true blend-space / anim state machine** (beyond crossfade + the 2-clip blend) and **per-instance morph/anim** — follow-ons.
- **The measured Three.js side-by-side** — M12.

## Gates

`cargo build` + `cargo clippy --all-targets -- -D warnings` clean on default + `wasm32`; suite green; docs updated. `dist/` is git-ignored (only sources commit).

## Next

M11 closes. **M12 (the measured proof)** is the capstone: run the parked M5 LLM live benchmark (needs the maintainer's API key + budget), a Twe-vs-Three.js same-game case study (building on this slice), and the kill-line verdict.
