# web3d-M7 closeout: graphics beyond Three.js

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([scope and exit criteria](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md))
**Status:** **all 17 sessions done; not closed.** Exit criterion 1 (per-scene image parity with Three.js) is not met on 6 of 12 scenes. Criterion 2 is met on the discrete GPU only, and the Three.js stress scene is faster. Criterion 3 is met. The rule is that nothing is called closed until its exit criteria are met; the choice of what happens next is at the end.

**Published with this note:**
- [`bench/graphics/results.md`](../../bench/graphics/results.md): the ꟻLIP table, including a like-for-like column;
- [`bench/graphics/comparison.jpg`](../../bench/graphics/comparison.jpg): every scene rendered by Cycles, Twe and Three.js;
- the frame times below, from `bench/graphics/fps.mjs`.

## Exit criteria

### 1. Parity: every Tier 1 feature ships, and every scene scores at or below Three.js — **not met**

**Every Tier 1 feature ships:**
- physically based materials with the glTF extensions (sessions 3, 10);
- image-based lighting (4);
- MSAA and TAA (5);
- cascaded soft sun shadows and point-light shadows (6);
- GTAO, bloom, exposure and tone curves (7);
- depth of field, motion blur and colour grading (8);
- sorted translucency and height fog (9).

**The scores** (mean ꟻLIP against Blender Cycles, lower is better; Three.js r186 as the harness sets it up):

| | Mean (12 scenes) | Scenes at or below Three.js |
|---|---:|---:|
| Twe, as the harness runs it (AO and SSR on) | **0.1774** | 6 of 12 |
| Twe with AO and SSR off (like for like) | 0.1911 | 3 of 12 |
| Three.js | 0.1857 | — |
| best golden (glTF Sample Viewer) | 0.1799 | — |

**The mean** is the best in the table, ahead of every renderer's golden. That lead depends on ambient occlusion and reflections, which the harness gives Twe and Three.js doesn't use; like for like, Twe's mean is worse than Three.js's.

**Where Twe loses** (AO and SSR on), with what the contact sheet shows:

| Scene | Twe | Three.js | What differs |
|---|---:|---:|---|
| DamagedHelmet | 0.0800 | 0.0615 | the largest gap, present since session 3 and never isolated |
| TextureTransformTest | 0.1177 | 0.1063 | |
| IridescenceLamp | 0.1023 | 0.0952 | Twe tints the glass globe; Three.js keeps it clear |
| AlphaBlendModeTest | 0.1113 | 0.1071 | |
| FlightHelmet | 0.0573 | 0.0548 | |
| EmissiveStrengthTest | 0.2580 | 0.2557 | within noise; both wash out the emissive panels |

**Where Twe wins:**
- Sponza, ClearCoatTest, TransmissionTest and AnisotropyBarnLamp by clear margins.
- SheenChair and MetalRoughSpheres narrowly.
- Anisotropy isn't implemented (deferred in session 10); its win comes from how the other terms land. The contact sheet shows a magenta rim artefact on that lamp that the score doesn't punish much.

### 2. Beyond: every Tier 2 feature ships, and the stress scene holds 60 fps in Chrome — **met on the discrete GPU only**

**Every Tier 2 feature (not the stretch items) ships:**
- GPU frustum + hi-Z occlusion culling with indirect draws (11);
- clustered forward lighting with 1024 lights and a shadow budget (12);
- a million compute particles from `particles` blocks, with depth collision (13);
- screen-space reflections and volumetric fog (14);
- procedural surface materials with displacement, as a language feature (15).

**The stress scene** is [`examples/stress_3d.twe`](../../examples/stress_3d.twe):
- 100,000 blocks animated by a `displace` material;
- 500 point lights moved every tick;
- 1,000,000 GPU particles.

**Its Three.js equivalent** is [`bench/graphics/three/stress.html`](../../bench/graphics/three/stress.html), written the way Three.js does this best:
- `WebGPURenderer`;
- one `InstancedMesh` displaced in the vertex shader;
- `ClusteredLighting` for the 500 lights;
- a compute-shader particle system;
- MSAA, ACES and the same bloom.

Measured in Chrome at 1280×720, throughput (vsync off):

| | Twe | Three.js r186 |
|---|---:|---:|
| RTX 3050 Ti (discrete) | 12.2 ms, **82 fps** | 4.2 ms, **239 fps** |
| Intel UHD (integrated; Chrome's default on this laptop) | 81 ms, 12 fps | 49 ms, 20 fps |

**Twe holds 60 fps on the discrete GPU and loses to Three.js on both GPUs.**
- **On the RTX, Twe is CPU-bound.** Per frame in the browser:
  - 6.6 ms of interpreter time moving 500 lights with `light.set`;
  - 2.9 ms gathering looks;
  - 4.1 ms of kernel CPU re-uploading 100k instances.

  Three.js does all of this on the GPU except the light positions, which cost it little in JavaScript.
- **On the integrated GPU both are GPU-bound,** and Twe's frame costs more.
- **What the scope hoped for** ("a scene where Three.js falls below 60") didn't happen. Three.js done its best way holds 239 fps on the discrete GPU.

**Getting the discrete GPU:** Chrome on Windows ignores WebGPU's `powerPreference`, so a player gets the discrete GPU by choosing it for Chrome in Windows' graphics settings.

### 3. The game uses it — **met**

`survive3d` runs with:
- physically based shading and image-based lighting from a computed sky;
- 4× MSAA, ambient occlusion and bloom;
- soft sun shadows, clustered torch lights and GPU particles.

Its floor, slimes, gems, bolts and flames are procedural `visual` materials; the floor is authored purely in code. It runs at **67 fps on the integrated GPU** in Chrome (14.8 ms, late game) and far above that on the discrete one. TAA is off: it cost 2.7 ms on the integrated GPU for no visible gain (session 16).

## What M7 built, by session

| # | Session | Note |
|---|---|---|
| 1 | Render graph | [note](2026-09-29-web3d-m7-render-graph.md) |
| 2 | Comparison harness, baseline 0.348 vs Three.js 0.186 | [baseline](2026-09-29-web3d-m7-baseline.md) |
| 3 | PBR per glTF primitive, 0.295 | [note](2026-09-29-web3d-m7-pbr.md) |
| 4 | Image-based lighting, 0.198 | [note](2026-09-29-web3d-m7-ibl.md) |
| 5 | MSAA + TAA, 0.196 | [note](2026-09-29-web3d-m7-aa.md) |
| 6 | Cascaded PCSS and point shadows | [note](2026-09-29-web3d-m7-shadows.md) |
| 7 | GTAO, bloom, exposure, AgX / Neutral, 0.185 | [note](2026-09-29-web3d-m7-post1.md) |
| 8 | Depth of field, motion blur, LUTs | [note](2026-09-29-web3d-m7-post2.md) |
| 9 | Translucency and height fog, 0.184 | [note](2026-09-29-web3d-m7-transparency-fog.md) |
| 10 | glTF material extensions, 0.180 | [note](2026-09-29-web3d-m7-gltf-extensions.md) |
| 11 | GPU-driven culling | [note](2026-09-30-web3d-m7-gpu-driven.md) |
| 12 | Clustered lighting | [note](2026-09-30-web3d-m7-clustered-lights.md) |
| 13 | GPU particles | [note](2026-09-30-web3d-m7-gpu-particles.md) |
| 14 | SSR and volumetric fog, 0.177 | [note](2026-09-30-web3d-m7-ssr-volumetric-fog.md) |
| 15 | Procedural surface materials | [note](2026-09-30-web3d-m7-procedural-materials.md) |
| 16 | `survive3d` uses it all; the stress scene | [note](2026-09-30-web3d-m7-survive3d-stress.md) |
| 17 | Publish and close | this note |

**Script surface M7 added:**
- `postfx.*`: TAA, AO, exposure, curves, depth of field, motion blur, LUTs, SSR;
- `light.*`: `cone`, `shadow`, `fog`, `volumetric`, `environment`;
- `camera.far`;
- `visual` blocks' `surface` / `displace` with `material(...)`;
- GPU `particles` with `collide`.

Each came with an example and a design note.

## Session 17's changes

- **`three/stress.html`:** the Three.js stress scene.
- **`contact.py`:** the comparison contact sheet.
- **The like-for-like column:** `TWE_BENCH_OUT=twe-plain` in `tests/graphics_bench.rs`, plus `score.py` support. The results table now also names the scenes where Twe loses.
- **The harness README** documents `fps.mjs`, the like-for-like column, and the stress comparison commands.
- **The table was re-run** after session 16's renderer changes (half-resolution AO, `fs_script`): the mean moved from 0.1773 to 0.1774, no per-scene change above 0.002.

## What would close it

Two follow-ups would meet the criteria as written:
1. **Per-scene parity.** Isolate DamagedHelmet first: the largest gap, and long-standing. Then the iridescent glass tint, texture transforms and alpha blending. Each is a scene where the reference and Three.js agree and Twe doesn't, which makes it a debugging job, not new features. Probably 2–4 sessions.
2. **The stress scene on the CPU side.**
   - Lights attached to entities, moved by the kernel instead of 500 `light.set` calls a tick.
   - A persistent instance buffer, re-uploaded only when looks change (the look cache already knows when).

   These would bring Twe's RTX frame near the GPU cost. The integrated GPU's 81 ms needs GPU-side work: the particle composite and MSAA bandwidth are the likely costs.

The alternative is to accept M7 with these results recorded here and move to M6 (one engine, retiring macroquad), carrying the follow-ups as a backlog. That is the maintainer's call.

## Verification

- **`cargo test`:** 1051 pass.
- **Clippy** (`-D warnings`) is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **The graphics suite** was re-rendered twice (defaults and like-for-like) and scored; `results.md`, `results.json` and `comparison.jpg` are regenerated.
- **Both stress scenes** were measured on both GPUs in Chrome, with screenshots checked to show the scene (session 16's first measurement had timed an empty frame).
