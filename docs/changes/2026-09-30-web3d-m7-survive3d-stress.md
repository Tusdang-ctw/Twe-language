# web3d-M7 session 16: `survive3d` uses it all; the stress scene

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [procedural materials](2026-09-30-web3d-m7-procedural-materials.md))
**Examples:**
- `examples/survive3d/main.twe`: the v1.0 slice, now with every M7 feature on;
- `examples/stress_3d.twe` (new): the exit criterion's stress scene.

**Code:**
- `src/stdlib.rs` + `src/host3d.rs`: `light.environment`, `camera.far`;
- `src/eval.rs` + `src/value.rs`: the look cache;
- `src/kernel/render.rs`: per-material pipeline checks, `fs_script`;
- `src/kernel/ao.rs`: half-resolution GTAO;
- `src/verify.rs`: the `particles-cpu` warning;
- `bench/graphics/fps.mjs` (new): frame rate in Chrome.

This note is also the design record for two additions to the script surface (M7 plan rule 3): `light.environment` and `camera.far`.

## What `survive3d` now uses

- **Image-based lighting** from a computed dusk sky: `assets/sky.hdr`, written by `tests/gen_survive3d_sky.rs`, so the game still ships no downloaded art.
- **A shadowing sun** (PCSS cascades) and **16 torches**: point lights on the arena rim, flickering, through the clustered lighting.
- **Procedural materials** (session 15):
  - the whole arena floor is one `ArenaFloor` surface: flagstones, wear, and rune light running along the grout;
  - slimes wobble (`displace`);
  - gems, bolts and torch flames glow into the bloom.

  The floor used to be 144 script-drawn cubes a frame. Now it's one entity on a 64 × 64 grid mesh.
- **GPU particles:** deaths burst into sparks that bounce off the floor. A `particles` block replaced the `Spark` entities.
- **4× MSAA, ambient occlusion, bloom.** TAA is off; see the numbers below for why.

## Language and API additions

1. **`light.environment(path, intensity)`.**
   - Image-based lighting was implemented in session 4, but only the benchmark harness could turn it on: `host3d` always passed `None`. The plan's "IBL on" for the game needed a way in.
   - The shape follows `postfx.lut`: a path and a strength, 0 turns it off, a non-`.hdr` path is an error.
   - The backdrop stays the background colour; a script-visible sky backdrop wasn't needed by any example.
2. **`camera.far`**, a field on `camera` beside `eye` / `target` / `up`, default 100 (the old fixed value).
   - The stress scene's 632 m field was invisible from 420 m away: the first "97 fps" measurement was of an empty frame.
   - Any large scene needs this. It's a field, not a call, because the camera already is fields.
   - `near` and the field of view stay fixed until an example needs them.
3. **A `particles-cpu` warning in `twec verify`.** A `particles` block the GPU can't run falls back to the CPU interpreter silently. That's fine for a spark burst, ruinous for a million.
   - The stress scene's `Motes` read one script global, ran on the CPU, and the page did 0.5 fps with nothing said anywhere.
   - `verify` now names the reason (Principle 3: no silent footguns).

## Performance work (what the measurements found)

Measured in Chrome (`bench/graphics/fps.mjs`: throttles off, vsync off, so frame rate is throughput) at 1280×720.
- **The GPU.** Chrome gives WebGPU the Intel UHD iGPU by default on this laptop. `--gpu high` forces the RTX 3050 Ti.
- **Trying to request the fast GPU:** Chrome on Windows ignores WebGPU's `powerPreference` (crbug.com/369219127) and logs a warning, so the runtime doesn't ask. A player chooses the GPU in Windows' graphics settings.
- **Throttling:** the iGPU throttles under long runs, so comparisons are interleaved A/B runs with medians.

Changes:

1. **Material pipelines checked per material, not per draw** (kernel).
   - The loop that builds each `visual` material's pipeline looked every draw up in a map keyed by the material's WGSL source. That meant hashing a few KB per draw: 100k material draws cost **93 ms of CPU a frame** natively.
   - Deduplicating the material ids first: **4.4 ms**.
2. **Untextured script draws skip the glTF material path** (`fs_script`).
   - `cube` / `sphere` / a look without a material shaded through the glTF path: nine texture reads (of 1×1 white textures), a derivative tangent frame, and the extension lobes. `fs_script` shades the same surface without them; the render tests show identical results.
   - Integrated GPU, native, 300 cubes and a floor: **4.8 → 1.95 ms**.
   - The pre-session `survive3d` in Chrome on the iGPU: **11.3 → 4.3 ms**.
3. **Ambient occlusion at half resolution.**
   - Full-resolution GTAO cost `survive3d` about 18 ms a frame on the iGPU in Chrome (30 → 13 ms with AO off).
   - At half resolution it's ~4.5 ms. Of that, ~1.1 ms is the prepass and blur. Halving the samples saved only ~0.6 ms, so the rest isn't the sample loop; a follow-up should find it.
4. **The look cache** (`value::look_epoch`).
   - Gathering 100k looks cost 9.6 ms a frame: a visit to every entity, each a separate heap object.
   - A counter now moves on every instance-field write (bar runtime-internal `__` fields), instance creation and despawn. Vectors are immutable, so a position can't change without one of those.
   - While the counter stands still, positions gathered last frame are reused. Each class's shared look values (which may read globals, like `hero_tint`) are still evaluated every frame, and looks that read their own entity are evaluated as before.
   - Cost: **9.6 → 1.8 ms**.
   - A test moves, spawns and despawns a cached entity. It fails if field writes stop moving the counter.
5. **Headless runs don't simulate GPU particles** (`Env::particles_unseen`, set by `twec run` and `eval::run`).
   - A block that compiles for the GPU is pure (no globals, no output), so with no renderer its simulation is invisible. It now only ages its emitter, as on a 3D host.
   - Before, `twec run examples/stress_3d.twe` interpreted a million particles for minutes, and the example tests would never have finished.
   - `tests/gc_stress.rs` skips the stress scene: collecting at every one of its 100k spawn safepoints is quadratic.
6. **Tried and reverted:** an 8-tap PCSS blocker search. It shrank the penumbra measurably (the soft-shadow test caught it), and its saving couldn't be measured through the throttling.

## Numbers

**`survive3d`**, ms per frame in Chrome:

| | Intel UHD (Chrome's default) | RTX 3050 Ti |
|---|---:|---:|
| before this session (none of the above on) | 10.2 | — |
| every feature on, before the fixes | 30.6 | 2.5 |
| every feature on, AO at half resolution | 17.6 | — |
| **as shipped (TAA off), wave 1** | **14.9 (67 fps)** | — |
| **as shipped, late game** (100 s in, level-up menu over the field) | **14.8 (67.5 fps)** | — |

- **Why TAA is off:** on the iGPU it cost 2.7 ms (17.7 vs 14.9), and without it the frame showed no visible grain; the AO blur and MSAA carry it.
- **A dash** means not measured after that step; the RTX has headroom to spare throughout.

**The stress scene** (`examples/stress_3d.twe`):
- **Load:** 100,000 blocks (entities with constant looks, animated only by a `displace` material), 500 point lights moved every tick, 1,000,000 GPU particles.
- **On the RTX 3050 Ti: 12.2 ms, 82 fps.** The criterion's 60 fps is met.
- **On the Intel UHD: 81 ms, 12 fps.** GPU-bound; the criterion is not met there.
- **CPU per frame, in Chrome on the RTX:** 6.6 ms script tick at 60 Hz (the 500 `light.set` calls), 2.9 ms gathering looks, 4.1 ms kernel.
- **Natively:** tick 4.0 ms, looks 1.8 ms, kernel 4.6 ms.

**How the stress scene got there:** 0.5 fps (particles on the CPU) → 3.3 fps (material hashing) → 82 fps.

The Three.js equivalent of the stress scene is session 17's, measured the same way.

## Limits and follow-ups

- **Exit criterion 2 holds only on the discrete GPU.** On the laptop's integrated GPU, which is what Chrome uses unless told otherwise, the stress scene runs at 12 fps. `survive3d` holds 60 on both.
- **GTAO's remaining cost on the iGPU** isn't in its sample loop. XeGTAO-style prefiltered depth mips are the likely fix.
- **The look cache helps static scenes only.** A game whose entities all move each tick (`survive3d`'s swarm) rebuilds it every frame, at the old cost. SoA entity storage (deferred in M3) is the general fix.
- **500 `light.set` calls a tick cost 4–7 ms of interpreter time.** Lights attached to entities, moved by the kernel, would remove it.

## Verification

- **`cargo test`: 1051 pass** (1046 before, +5). New tests:
  - `light_environment_setter`;
  - `camera_far_sets_the_view_distance` (a block 150 m away: clipped at the default, drawn at `far = 300`);
  - `cached_looks_follow_moves_spawns_and_despawns` (mutation-checked);
  - `verify_warns_when_particles_fall_back_to_the_cpu` (and the three particle examples are clean);
  - `gen_survive3d_sky` (the committed sky matches its recipe);
  - `render_stress_scene` (an ignored benchmark in `tests/render_bench.rs`).

  Existing render, soak and `survive3d` tests pass with the new look.
- **Clippy** (`-D warnings`) is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:** `survive3d` and the stress scene built for the web render with no WebGPU errors or warnings in the console (`fps.mjs` reports any).
