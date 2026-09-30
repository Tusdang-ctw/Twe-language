# web3d-M7 follow-up 2: the stress scene, faster than Three.js

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([closeout](2026-09-30-web3d-m7-closeout.md); previous follow-up: [image parity](2026-09-30-web3d-m7-image-parity.md))
**Code:**
- `src/eval.rs` + `src/value.rs`: tickable entities, pruning only after a despawn, the look fingerprint;
- `src/host3d.rs`, `src/kernel/render.rs`: retained draws;
- `src/kernel/gpu_cull.rs`: the timing probe;
- `src/kernel/clusters.rs`: grid, shared-memory build;
- `src/kernel/particles.rs`: partial reads and writes;
- `src/kernel/gpu_profile.rs` (new): `TWE_GPU_PROFILE`.

**Tools:**
- `bench/graphics/compare.sh`: medians of interleaved Twe and Three.js runs per GPU;
- `fps.mjs` now reports the canvas size and the share of GPU-culled frames.

## Result

`examples/stress_3d.twe` (100,000 animated blocks, 500 moving lights, 1,000,000 particles) against `bench/graphics/three/stress.html` (Three.js r186 at its best: WebGPURenderer, instancing, ClusteredLighting, compute particles). Both in Chrome, both drawing a 1280×960 canvas with the same 4:3 framing; medians of 5 interleaved runs, ms per frame:

| | Twe before | Twe after | Three.js |
|---|---:|---:|---:|
| Intel UHD (Chrome's default GPU) | 81 | **44.0** | 54.1 |
| RTX 3050 Ti | 12.2 | **4.77** | 4.90 |

**Twe is faster on both GPUs:** by 19% on the integrated one and 3% on the discrete one, where the medians are close.

**Fairness correction.** The session-17 comparison drew Twe at 1280×960 (its page letterboxes 4:3) and Three.js at 1280×720, a third fewer pixels. The Three.js page now matches, and `fps.mjs` prints both canvases' sizes.

## What the time went to, and the fixes

Every step was measured first: headless timings, then per-pass GPU timestamps (`TWE_GPU_PROFILE=1`, new), then the same variants in Chrome, whose costs differ.

**CPU:**
1. **Ticking 100k entities with nothing to run** cost 1.9 ms a tick natively: a cache miss per entity, just to learn it had no `update`.
   - `Env::tickable_entities` lists only the entities with an `update` or that are particle emitters; the tick walks only those.
   - `prune_despawned` walks only after a despawn.
   - Tick: 4.0 → 0.9 ms (what's left is the 500 `light.set` calls).
2. **The 100k looks were rebuilt and re-uploaded every frame** although nothing changed. Now a frame's draws can be kept:
   - **The host:** a frame of nothing but looks that read nothing of their entities gets a fingerprint: the look epoch plus each class's shared values. If the renderer holds that fingerprint (`Renderer::retained_generation`), the host doesn't queue the draws at all.
   - **The renderer:** it keeps the draws and everything built from them (instance ranges, cull groups, the uploaded instance buffer). It rebuilds from its copy when an asset lands, and never keeps a frame with transparent draws, since those are sorted by the camera.
   - Kernel CPU: 4.2 → 1.5 ms. Looks: 2.0 → 0 ms.
   - Two tests pin it: one moves, spawns and despawns a cached entity; one loads a mesh after its draws were kept. Both fail if their invalidation is removed.
3. **CPU particle gathering visited every entity** each frame (1.3 ms). It now walks only emitters.

**GPU:**

4. **Clustered lighting:**
   - **The grid.** Seen from 400 m, each of the old 16×9×24 clusters spanned ~190 m of ground and listed dozens of lights, all shaded by every pixel in it. Now 32×18×64: iGPU 67 → 52 ms natively.
   - **The build.** It read all 500 lights per cluster and moved each to view space itself. Lights now pass through workgroup memory in batches of 64: 2.35 → 1.85 ms (iGPU), 0.34 → 0.22 ms (RTX).
5. **Culling that doesn't pay is switched off.** Two-phase occlusion culling splits the main pass and stores and reloads the multisampled targets.
   - In this scene, where everything is visible, that cost 18 ms a frame in Chrome on the iGPU (3.5 ms natively).
   - Culling now keeps itself on only while it measurably pays: every 720 frames it times 60 frames with and 60 without, keeping it if at least 5% faster.
   - A first version read the late pass's counts back; in a GPU-saturated Chrome page those `mapAsync` calls never resolved, so it timed frames instead.
   - The large-scene bench shows culling kept where it wins (objects behind a wall) and at parity elsewhere.
6. **Particle simulation** reads only the fields some program's update mentions (plus age, lifetime, program, seed) and writes back only the fields it assigns: 1.07 → 0.73 ms on the RTX.

**Tried and dropped:**
- a non-instanced particle draw (worse on the iGPU);
- fewer occlusion-culling slices or AO steps.

## Also measured

- **`survive3d`** in Chrome: 14.4 ms (69 fps) on the integrated GPU, 1.3 ms on the discrete one.
- **The image benchmark is unchanged:** mean 0.1655, every scene at or below Three.js. Its scenes use neither point lights nor particles, and every change here leaves pixels as they were.

## Limits

- **The stress scene doesn't hold 60 fps on the laptop's integrated GPU:** 44 ms, about 23 fps, where Three.js manages about 18. It does on the discrete GPU (4.8 ms, ~210 fps). Chrome uses the integrated GPU unless told otherwise.
- **The particle draw is bound by rasterizing a million sub-pixel quads.** A compute splat for tiny particles is the next step there.
- **Retained draws help worlds whose looks don't change.** A frame with any immediate-mode draw, any look that reads its entity, or any transparent draw is built as before.

## Verification

- **`cargo test --release`:** 1054 pass. New tests:
  - `probe_tests` (the culling probe keeps culling only when faster, re-probes each cycle, culls without a clock);
  - `kept_draws_rebuild_when_a_mesh_arrives`;
  - `cached_looks_follow_moves_spawns_and_despawns` now also exercises the kept path.
- **Clippy** (`-D warnings`) is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:**
  - the stress scene and `survive3d` render with no WebGPU errors or warnings;
  - screenshots confirm the scenes draw, the Three.js page's included.
