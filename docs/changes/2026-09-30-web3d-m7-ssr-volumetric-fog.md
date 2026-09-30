# web3d-M7 session 14: screen-space reflections, volumetric fog

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [GPU particles](2026-09-30-web3d-m7-gpu-particles.md))
**Code:**
- `src/kernel/ssr.rs` (new): the reflection trace and composite;
- `src/kernel/volumetric.rs` (new): the froxel volume;
- `src/kernel/render.rs`: the surface record, the pipeline variants, the four frame passes;
- `src/stdlib.rs`: `postfx.ssr`, `light.volumetric`.

**Example:** [`examples/light_shafts_3d.twe`](../../examples/light_shafts_3d.twe), with a mirror floor asset (`examples/assets/mirror_floor.glb`).

This is also the design note for the two new builtins (M7 plan rule 3).

## What changed

1. **Screen-space reflections: `postfx.ssr(strength)`**, 0 (off, the default) to 1.
   - **The surface record.** With reflections on, the main passes write a second, multisampled target per pixel: the world normal (octahedral), roughness, and the weight the environment's specular reflection has in the pixel. The weight is the split-sum specular albedo × occlusion × the environment's intensity, as `shade` computes it. Translucent surfaces and the backdrop leave the record alone.
   - **Two pipeline sets.** Frames without reflections use lit-surface pipelines whose second target slot is empty, so they don't pay for the extra target. The set that writes it (and each `visual` material's variant) is built on the first frame that needs it.
   - **The trace** (fullscreen) marches each reflective pixel's reflected ray through the depth buffer:
     - 48 steps, growing with distance and staggered per pixel and frame by interleaved gradient noise;
     - a hit is refined by bisection.

     It writes `weight · confidence · (hit colour − environment)` into a reflection target. The confidence fades toward the screen edge, with distance and roughness, and for rays turning back toward the camera.
   - **The composite** adds that onto the frame. So a hit **replaces** the environment's reflection rather than adding to it, and a miss leaves the frame exactly as it was.
   - **Pixels skipped:** roughness ≥ 0.7, weight below 5% (a matte dielectric seen head-on), and the backdrop.
2. **Volumetric fog: `light.volumetric(true)`.** The fog from `light.fog` is lit per point in a froxel volume (Wronski 2014; Hillaire 2015):
   - **The volume.** 160 × 90 screen tiles × 64 depth slices, exponential from 0.5 m to the far plane. The volume passes reuse the frame and shadow bind groups (lights, clusters, the sun's cascades), now visible to compute. Their textures stay fragment-only, keeping compute within WebGPU's 16 sampled textures per stage.
   - **Scatter** (compute): the height fog's density at each froxel, and its light:
     - a quarter sky light, which no shadow blocks;
     - three quarters sunlight × one lookup in the sun's shadow cascades, plus the forward-scattering glow toward the sun;
     - the froxel's cluster's point and spot lights with the surfaces' falloff.
   - **Integrate** (compute): front to back per tile, with Hillaire's energy-conserving step: a slice of constant light and density adds `L·(1 − e^(−σΔ))`.
   - **Apply:** each pixel looks up the volume at its depth and blends `frame · T + S` in place. The closed-form fog in the surface and sky shaders is switched off meanwhile.
   - **The split** keeps unshadowed fog equal to the closed form. A test checks the two images' mean brightness agrees within 3 levels.

## Numbers

**Image benchmark** (`bench/graphics`, mean ꟻLIP against Cycles, lower is better).
- **The score.** With reflections on, Twe goes from 0.1800 to **0.1773**. Three.js is 0.1857, and the best golden (the glTF Sample Viewer) 0.1799. Twe now leads the whole table.
- **Every scene that changed improved:** TransmissionTest 0.2796 → 0.2628, DamagedHelmet 0.0824 → 0.0798, FlightHelmet 0.0600 → 0.0573, Sponza 0.7988 → 0.7963, and small gains elsewhere.
- **Harness setting.** The harness now runs Twe with SSR on (`TWE_BENCH_SSR`, default 1), as it does ambient occlusion. The results page says so: Three.js as the harness sets it up uses neither (three has GTAOPass and SSRPass addons it doesn't use). The session-17 comparison should close that gap.

**Frame cost** (`tests/render_bench.rs`, `render_ssr_and_fog`): 300 cubes, a floor and 100 point lights, top-down, 1280×720, ms per frame.

| | Intel UHD | RTX 3050 Ti |
|---|---:|---:|
| plain | 7.2 | 0.86 |
| `postfx.ssr(1)` | 9.6 | 1.24 |
| `light.fog` | 8.0 | 0.80 |
| `light.fog` + `light.volumetric(true)` | 9.3 | 0.97 |

- **Frames with both off cost what they did before** (4.65 ms on the integrated GPU for the particle bench's empty scene).
- **One measurement was wrong at first.** Early in the session a run on the integrated GPU showed 9.6 ms for that scene and was blamed on the second target. Re-running the unchanged previous commit gave 11.6 ms: the chip was throttled. The pipeline variants were kept anyway (a frame shouldn't write a target nobody reads), and numbers were taken once it was back to normal.

## Limits

- **Only what's on screen reflects.** Off-screen objects and the hidden sides of objects can't be reflected; those rays fade to the environment.
- **Rough reflections aren't blurred.** They fade out above roughness 0.35 instead (the frame has no mip chain).
- **Reflections are tinted by a single reflectance.** The record stores one reflectance, so a coloured metal's reflection of the scene isn't tinted by its colour (the environment part it replaces is).
- **Translucent surfaces:**
  - they don't reflect the scene;
  - they show the reflection of the surface behind them;
  - they are fogged as the opaque surface behind them.
- **The fog volume:**
  - point-light shadows don't cut it;
  - its 64 slices show as soft banding at hard shadow edges;
  - one shadow tap per froxel (no filtering or temporal reprojection yet).
- **Particles are drawn after the fog** and aren't fogged.

## Verification

- `cargo test`: 1028 pass (1021 before, +7):
  - **`screen_space_reflections_show_the_scene`:** a mirror floor under a red block. With `postfx.ssr(1)` over 800 floor pixels turn red (the block's reflection), and the backdrop is byte-identical to the frame without.
  - **`volumetric_fog_matches_and_casts_shafts`:**
    - unshadowed, the volume matches the closed-form fog (mean within 3 levels);
    - with a wall shadowing the sun, over 2% of the pixels are darker in the volume.
  - **Shader validation** (naga) for the trace, composite, scatter, integrate and apply shaders, and uniform layouts.
  - **`ssr_and_volumetric_setters`:** the builtins, clamping and errors.
- **Clippy** (`-D warnings`) is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:**
  - `examples/light_shafts_3d.twe` built for the web shows sun shafts between the pillars, the lamp's halo, and the pillars reflected in the floor, with no WebGPU errors or warnings (the first storage-texture writes and 3D textures on the web);
  - `survive3d` still renders.
