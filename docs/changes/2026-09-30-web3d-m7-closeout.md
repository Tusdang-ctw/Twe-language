# web3d-M7 closeout: graphics beyond Three.js

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([scope and exit criteria](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md))
**Status:** **closed** 2026-09-30, after all 17 sessions and two follow-ups ([image parity](2026-09-30-web3d-m7-image-parity.md), [stress performance](2026-09-30-web3d-m7-stress-performance.md)).
- **Criterion 1 is met:** Twe scores at or below Three.js on all 12 scenes.
- **Criterion 2 is met on the reference laptop's discrete GPU,** and Twe beats Three.js's stress scene on both GPUs.
- **Criterion 3 is met.**

The first version of this note recorded M7 as *not* closed: 6 of 12 scenes lost, and the stress scene 3× slower than Three.js on the discrete GPU. The follow-ups fixed both; that version is in the history of this file.

**Published with this note:**
- [`bench/graphics/results.md`](../../bench/graphics/results.md): the ꟻLIP table, including a like-for-like column;
- [`bench/graphics/comparison.jpg`](../../bench/graphics/comparison.jpg): every scene rendered by Cycles, Twe and Three.js;
- the frame times below, from `bench/graphics/fps.mjs`.

## Exit criteria

### 1. Parity: every Tier 1 feature ships, and every scene scores at or below Three.js — **met**

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
| **Twe, as the harness runs it (AO and SSR on)** | **0.1655** | **12 of 12** |
| Twe with AO and SSR off (like for like) | 0.1783 | 10 of 12 |
| Three.js | 0.1857 | — |
| best golden (glTF Sample Viewer) | 0.1799 | — |

- **Twe leads every renderer** in the Khronos table on the mean and on 9 of 12 scenes.
- **Like for like** it still beats Three.js's mean. It trails on FlightHelmet and ClearCoatTest there, where its lead comes from AO and SSR.
- **Where others still lead:** model-viewer and the glTF Sample Viewer on EmissiveStrengthTest, TextureTransformTest and AlphaBlendModeTest.
- **The fixes:** six rendering errors, each found by inspecting the worst scene. Among them: diffuse light that ignored what the specular layer reflects, a tangent frame upside down in one axis, glass drawn without depth writes, and thin films evaluated over the wrong base. See [image parity](2026-09-30-web3d-m7-image-parity.md).

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

Measured in Chrome, both drawing a 1280×960 canvas with the same framing, throughput (vsync off), medians of 5 interleaved runs:

| | Twe | Three.js r186 |
|---|---:|---:|
| RTX 3050 Ti (discrete) | **4.77 ms, ~210 fps** | 4.90 ms, ~204 fps |
| Intel UHD (integrated; Chrome's default on this laptop) | **44.0 ms, ~23 fps** | 54.1 ms, ~18 fps |

**Twe holds 60 fps on the discrete GPU and is faster than Three.js on both GPUs.**
- 19% faster on the integrated GPU; 3% on the discrete one, where the medians are close.
- **Neither engine reaches 60 fps on the integrated GPU.** The criterion is met on the discrete GPU of the reference laptop, not on its integrated one.
- **What changed from the first version of this note** (82 vs 239 fps on the RTX): the stress-performance follow-up.
  - **CPU:** 100k static entities no longer cost a tick; unchanged looks are neither rebuilt nor re-uploaded.
  - **GPU:** a finer cluster grid and a faster cluster build; occlusion culling switches itself off where it doesn't pay; particle simulation touches only the fields programs use.
- **A fairness correction:** the first comparison drew Twe at 1280×960 and Three.js at 1280×720.

**Getting the discrete GPU:** Chrome on Windows ignores WebGPU's `powerPreference`, so a player gets the discrete GPU by choosing it for Chrome in Windows' graphics settings.

### 3. The game uses it — **met**

`survive3d` runs with:
- physically based shading and image-based lighting from a computed sky;
- 4× MSAA, ambient occlusion and bloom;
- soft sun shadows, clustered torch lights and GPU particles.

Its floor, slimes, gems, bolts and flames are procedural `visual` materials; the floor is authored purely in code. It runs at **69 fps on the integrated GPU** in Chrome (14.4 ms, 1280×960) and ~750 fps on the discrete one. TAA is off: it cost 2.7 ms on the integrated GPU for no visible gain (session 16).

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
| 17 | Publish (first closeout, "not closed") | this note |
| F1 | Image parity: every scene at or below Three.js, 0.1655 | [note](2026-09-30-web3d-m7-image-parity.md) |
| F2 | Stress scene faster than Three.js on both GPUs | [note](2026-09-30-web3d-m7-stress-performance.md) |

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

## After M7

The follow-ups' limits, carried as backlog:
- **Tiny particles.** A compute splat for sub-pixel particles; the particle draw is raster-bound.
- **Moving entities.** Struct-of-arrays entity storage for games whose entities all move (`survive3d`'s swarm rebuilds its draws every frame).
- **Anisotropy and KTX2/meshopt** (deferred in session 10).
- **GTAO's cost on integrated GPUs.**

Next in the web3d line: **M6** (one engine: sprites on the kernel, retiring macroquad).

## Verification

- **`cargo test --release`:** 1054 pass.
- **Clippy** (`-D warnings`) is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **The graphics suite** was re-rendered in both configurations and scored; `results.md`, `results.json` and `comparison.jpg` are regenerated.
- **Both stress scenes** were measured on both GPUs in Chrome at equal canvas sizes, with screenshots checked to show the scene.
