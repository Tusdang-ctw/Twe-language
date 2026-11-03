# Lanternfall — closeout (E1–E6, G0–G8)

**Date:** 2026-11-03. **Plan:** the approved Lanternfall plan (engine enablers E1–E6, then game G0–G8). **Result:** `examples/lanternfall` is a complete, original top-down 3D voxel battle royale: title → barge drop → loot → fight 31 AI rivals while the Hush closes over five phases → results → play again. It plays natively, as a standalone Windows `.exe`, and in Chrome (WebGPU). Every model, the sky and every sound are generated in code.

## What shipped

### Engine (each with its own design note)

| | | Note |
|---|---|---|
| E1 | `camera.project` / `ray` / `ground_point` / `fov`, `screen.aspect`; the 3D HUD is 480 tall × aspect wide; replay format v3 | `2026-11-03-web3d-camera-project.md` |
| E5 | Multi-file games in web / `.exe` builds; `spawn module.Class` | `2026-11-03-web3d-multi-file-builds.md` |
| E3 | Look `rotation:`, per-axis `scale:`, `glow:`; 80-byte instances | `2026-11-03-web3d-look-transforms.md` |
| E4 | Per-entity skeletal animation through one joint palette | `2026-11-03-web3d-entity-animation.md` |
| E2 + E6 | `fx.*`, shake and hit-stop in 3D; `light.background`; shake-RNG and paused-physics fixes | `2026-11-03-web3d-fx-3d.md` |

Found and fixed while building the game:
- `spawn` is also an expression (G0).
- A module's `visual` / `particles` blocks are usable by the game; a `visual` local named like a WGSL keyword no longer breaks the shader; strings have `.length`; `TWE_CAPTURE` prints `on render()` errors (G0–G5).
- `time.set_scale` / `time.scale`: slow motion, deterministic (G6).
- **An entity starts in its `initial:` state** — before, only scenes did, so a `persistent` entity state never kept its `update` running under `pause(true)` (G7; `tests/programs/entity_persistent_pause.twe`).
- **Physics queries** no longer rebuild rapier's collider BVH on every `raycast` / `character_move` — only when colliders were added or removed since the last step (G8).
- **Quality tiers:** `medium` / `low` draw the sun shadow with a 4-tap filter instead of PCSS; the `"auto"` governor reacts to frames only a little late, steps a tier before blurring below 80 % scale, and backs off (doubling) from a tier that keeps failing (G8).
- The profiler names method scopes `Class.method`; `bench/graphics/fps.mjs --keys`; the soak harness takes a loaded env and a `Pilot` (G8).

### The game (`examples/lanternfall/`, 14 Twe files, ~3,900 lines)

- **G0–G1** — the island (five places from stretched-cube blocks with procedural materials, lantern posts with lights), the keeper (character controller, cursor aim, roll, per-entity animation), `tests/lanternfall.rs`.
- **G2** — six weapons (wrench, coil pistol, tack SMG, scatterhorn, longwick rifle, petal mortar), rarities, shields and cells, Starcall, tracers, damage numbers, training dummies.
- **G3** — 31 AI rivals (loot / roam / fight / heal, line of sight, skill-scaled aim), generated names, name tags, kill feed.
- **G4** — the round: lantern barge, leap and glide, chests and loot, the Hush (five phases), the vault opening at 3:00, banners, minimap, win / lose.
- **G5** — night sky (`night.hdr`), dressed places, shore foam, fireflies, a real minimap.
- **G6** — juice: guns in hand, hit squash, hit marker, damage-direction chevron, death bursts, ELIMINATED call-outs, casings, footstep dust, pickup sparkles, a low-health / Hush vignette, a slow-motion last moment.
- **G7** — title, pause (Esc / focus loss), settings (volumes, quality, shake, ten rebindable keys), results with best record; menus by mouse, keyboard or gamepad; settings and record persist; 30 synthesised effects and two music layers.
- **G8** — performance, soaks, builds (below).

Assets: 34 voxel `.glb` models (`tests/gen_lanternfall_models.rs`), `night.hdr` (`gen_lanternfall_sky.rs`), 32 WAVs (`gen_lanternfall_sounds.rs`) — 2.8 MB.

## G8: measured, then fixed

The player reported lag. Measuring first (`tests/lanternfall.rs` `perf_breakdown` / `perf_render`, `TWE_GPU_PROFILE`, `fps.mjs`):

| Finding | Fix | After |
|---|---|---|
| Hit-stop froze the *whole game* for any wrench hit or explosion anywhere: 36 frozen ticks in a round's first 50 s, from fights nowhere near the player | `combat.stop_if_yours`: the keeper's own hits, hits on it, blasts within reach | 0 |
| Chrome on the Intel iGPU was GPU-bound: 12–17 ms/frame on `high`; sun shadows 5 ms (PCSS 2.7 ms), AO 2.7 ms | `medium` / `low` use a 4-tap shadow filter | `medium`: 7.6 ms GPU; **~100 fps** in Chrome vs 50–65 on `high` |
| The governor shrank `high` to 50 % before trying `medium`, and ignored 55-fps judder (only reacted past 1.2× budget) | 1.08× for 2 s counts as slow; tier step below 80 %; doubling retry hold | 4 new governor tests |
| Rapier's query BVH was rebuilt (`clear_and_rebuild`) on every raycast and character move — ~45 times a tick | rebuild only when colliders changed since the last step | rivals' movement 0.63 → 0.37 ms/tick |

On the discrete GPU (RTX 3050 Ti, native) a mid-round frame is 5.5 ms including the tick — the native build was never GPU-bound; its "lag" was the global hit-stops.

What it costs now, mid-round with ~20 alive (native, release): tick ~2.1 ms (rivals ~1 ms of it), looks + HUD ~0.6 ms a frame. In Chrome on the iGPU: script ~1.9 ms a tick, frame building ~0.7 ms, renderer CPU ~0.9 ms.

## Soaks

The Lanternfall pilot (`twec::soak::Pilot::Lanternfall`) works the title and results menus, leaps, roams, aims and fires in bursts, presses E / R / Q, and pauses every 90 s. The same driver runs natively (`cargo test --release --test soak -- --ignored soak_lanternfall --nocapture`) and in WebAssembly under Node (`node web/soak.mjs examples/lanternfall/dist/web 10 10 --pilot lanternfall`), each run 10 simulated minutes (36,000 ticks).

**In progress when this note was committed** — runs finished so far, all clean:

| Run | Rounds (lost) | Peak entities | Heap after each restart (native / wasm, KB) |
|---:|---:|---:|---|
| 1 | 1 (1) | 695 | 301 / 199 |
| 2 | 3 (3) | 728 | 263, 265, 279 / 173, 175, 184 |
| 3 | 2 (2) | 726 | 253, 296, 300 / 167, 196, 199 |
| 4 (native) | 1 (1) | 676 | — (round still running at 10 min) |

- **Native and wasm play identical sessions:** rounds, deaths and peak entity counts match run for run (heap bytes differ by pointer size).
- **No growth:** the heap after a full collection at each restart stays in a band of 170–300 KB.
- The remaining runs (and the 2-minute GC-stress soak) are recorded in a follow-up commit.

## Builds

- `twec build examples/lanternfall` → `dist/lanternfall.exe` (19 MB: 17.2 MB runtime + 1.9 MB bundle). Copied to an unrelated folder and run, it boots from its embedded bundle (title captured with `TWE_CAPTURE`).
- `twec build --target web examples/lanternfall` → `dist/web` (7.6 MB), checked in headless Chrome repeatedly (title, a round in play). The browser build hides Quit.

## Exit criteria

1. **60 fps in Chrome on the Intel iGPU at `auto`, through a match** — met with the G8 changes when the governor settles on `medium` (~100 fps); `high` alone sits at 50–65. A full match was not measured end to end in Chrome (the measurement covers a round's first ~90 s of play); the soaks cover full matches without the GPU.
2. **The `.exe` plays the same** — it boots and renders standalone; a full interactive match on it is the maintainer's to play.
3. **10 soak matches native + wasm, clean, no heap growth, identical** — see Soaks.
4. **A first-time player can drop, loot, fight and win or lose from the on-screen prompts alone** — the prompts are all there (title controls line, barge "Space — Leap", chest / weapon "E" prompts, the Hush timer, results). Not tested with a real first-time player.

## Honest deferrals

- **Nobody has listened to the sounds.** They're checked as valid, seamlessly looping files only. The mix needs ears.
- **No live first-time-player test**, and no full-length interactive match in Chrome (only the soaks run full matches, without a GPU).
- **The static props' looks** are re-evaluated every frame (~0.35 ms native) — measured, small; left for a general engine look-cache.
- **`high` is still 50–65 fps on the Intel iGPU.** Cheaper AO or cascade culling would let it reach 60; the governor now covers it by stepping to `medium`.
- **Online multiplayer** stays out of scope (the separate browser multiplayer plan).
- **itch.io publish** is the maintainer's step.
