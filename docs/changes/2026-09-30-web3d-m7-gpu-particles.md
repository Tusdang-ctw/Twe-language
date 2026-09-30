# web3d-M7 session 13: GPU particles

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [clustered lighting](2026-09-30-web3d-m7-clustered-lights.md))
**Code:**
- `src/particles_wgsl.rs` (new): `particles` blocks → WGSL;
- `src/kernel/particles.rs` (new): the pool, the compute passes, the draw and composite;
- `src/eval.rs`: emissions, the emitter countdown, the particle random stream;
- `src/kernel/render.rs`, `src/host3d.rs`: the three frame passes and the snapshot.

**Example:** [`examples/particles_3d.twe`](../../examples/particles_3d.twe).

This is also the design note the M7 plan requires: it changes *where* a `particles` block runs in 3D, and adds one field key, `collide`.

## What changed

1. **`particles` blocks run on the GPU in 3D.**
   - **Compiled.** At declaration, the block's `on_spawn(p)` and `on_update(p, dt)` are compiled to WGSL functions (`src/particles_wgsl.rs`).
   - **3D hosts use the GPU.** On a 3D host (`twec play3d`, the web build), `spawn Block at pos` records an emission — program, point, count, lifetime, seed — instead of creating particle objects. The kernel spawns, simulates and draws the particles.
   - **The emitter stays an entity.** It counts down its lifetime and despawns, as before.
   - **No new syntax.** The same block runs either way.
2. **A block the GPU can't run stays on the CPU,** automatically. This covers:
   - reading a global;
   - `print`;
   - a loop;
   - a `render()` override;
   - setting `age` / `lifetime`.

   CPU particles in 3D are handed to the kernel each frame and drawn by the same pipeline.
3. **The compiler checks types, so its WGSL is valid by construction.**
   - **What it checks.** It type-checks numbers, 2–4 vectors and booleans against the CPU's own rules: what the CPU rejects (component assignment, tuple × tuple, negating a tuple, scalar `math.*` on tuples) is refused.
   - **Integer division.** It tracks which numbers the CPU may hold as integers, and refuses a division that would truncate there (`p.size / 2` stays on the CPU; `p.size / 2.0` runs on the GPU).
   - **Why not a validator.** An earlier cut validated with naga at runtime; that made naga a runtime dependency and grew the web build from 3.56 to 4.85 MB. Type-checking instead costs 82 KB. naga stays a dev-dependency: tests validate compiled programs with it.
4. **The GPU pipeline, per frame, after the scene is drawn:**
   - **Update, then spawn** (compute): every live particle runs its program's update, then the runtime ages it as the CPU does (`age += dt`, `age_ratio`). New particles are initialised, run their spawn body, and are written into the pool.
   - **The pool.** It is a ring buffer sized to twice the live count (up to 2²⁰, 64 MB).
   - **One shader for every block.** All programs share one compute shader, dispatched by a `switch` on the particle's program.
   - **Draw:** a camera-facing soft disc per particle into an accumulation (`Rgba16Float`) and a revealage (`R16Float`) target, weighted blended OIT (McGuire & Bavoil 2013, their depth weight), with no sorting.
   - **Composite:** the weighted average over the HDR frame, before TAA, depth of field, motion blur, bloom and the tonemap. So particles glow and blur with the scene.
5. **`collide: true`:** after its update, a particle that has gone just behind the visible surface bounces off it:
   - the position goes back to where it was;
   - the velocity is reflected about the surface normal, rebuilt from the depth buffer, keeping half the normal speed.

   It is screen-space: only what the camera sees collides.
6. **Particles never change the simulation.**
   - **Their own random stream.** Particle bodies draw `random.float()` from their emitter's own stream (seeded by an emitter counter), on the CPU as well as the GPU. So spawning particles no longer advances the script's random numbers.
   - **The same entity lifetime.** A GPU emitter despawns on exactly the tick a CPU one would.
   - **Replays stay identical** whichever path a block takes. A test runs the same program both ways and compares every printed line.
7. **In 3D, `size` is a radius in world units,** defaulting to 0.1 (in 2D it stays 4 pixels).

## Numbers

`tests/render_bench.rs`, `render_particles`: the 300-cube scene, 1280×720, particles emitted once and then simulated (gravity, drag) and drawn every frame, ms per frame.

| | Intel UHD | RTX 3050 Ti |
|---|---:|---:|
| no particles | 4.7 | 0.46 |
| 100 000 | 8.8 | 1.3 |
| 1 000 000 | 30.9 | 4.8 |
| 1 000 000, `collide` | 31.2 | 5.4 |

- **The first cut drew into 4× multisampled targets:** 37 ms for 100 000 particles on the integrated GPU, 162 ms for a million.
- **The cost was blending, not simulation.** Skipping the simulation (every particle dead) or the draw each cut it to about 12 ms. Dense clusters overdraw, and every covered sample blends into two 4× targets.
- **Single-sampled WBOIT fixed it.** Particles are soft discs and need no MSAA, so the targets are now single-sampled and the depth test is done in the fragment shader against the scene's multisampled depth. That is 4× less blending and 37 MB less memory: 100 000 dropped to 8.8 ms, a million to 31 ms.
- **The image benchmark is unchanged (0.1801);** its scenes have no particles.

## Limits

- **Only what the camera sees collides** (screen-space). A particle hidden behind a wall does not collide with it.
- **The ring can recycle a long-lived particle early** when blocks with very different lifetimes share it under pressure. The pool holds twice the live count to make that rare; past 2²⁰ particles the oldest are recycled.
- **Dead slots still cost a vertex-shader read.** The draw covers the whole pool, so dead slots are rejected in the vertex shader. Compacting live particles into an indirect draw would save that; at a million live particles it wouldn't matter.
- **Particles draw after translucent surfaces.** A spark behind glass shows in front of it.
- **CPU-simulated particles don't collide.**
- **GPU numbers are `f32`.** The compiler refuses divisions that differ, but other float-vs-double rounding can differ slightly from the CPU. Only the visuals differ, never the simulation.

## Verification

- `cargo test`: 1021 pass (1011 before, +10):
  - **Compiler:** a spark fountain compiles, and its WGSL validates in naga; 16 refusals (unsupported statements, type errors, possible integer division, what the CPU can't do) each give their reason.
  - **Kernel:** the shaders validate, a bad program is refused, and the struct layouts match.
  - **`gpu_particles_draw_and_bounce`:** sparks draw; after 70 frames, colliding sparks rest on the floor, and non-colliding ones fall through it (at least 20× fewer visible).
  - **`cpu_particles_still_draw_in_3d`:** a block that reads a global runs on the CPU and draws.
  - **`a_million_particles`:** the pool reaches 2²⁰ and the cloud draws.
  - **`gpu_particles_leave_the_simulation_unchanged`:** CPU and GPU runs print identical lines, including random numbers and entity counts.
  - **`gpu_particle_blocks_also_run_on_the_cpu`:** the example's blocks compile for the GPU and run on the CPU.
- `cargo clippy -- -D warnings`: clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:** `examples/particles_3d.twe` built with `twec build --target web` shows bursts of sparks bouncing off a floor and blocks, over 60 000 drifting embers, with no WebGPU errors or warnings.
