# Low-end track G1–G3: measurements, quality tiers, render scale

**Date:** 2026-10-14
**Line:** post-M12 coding plan, Track G (low-end graphics), sessions 1–3
**Status:** G1–G3 shipped. G4 (cheap paths for the cost centres) and G5
(automatic tier + dynamic resolution) are next.

## Why

The integrated GPU is what most players have, in the browser above all.
Before this session:
- `examples/stress_3d.twe` ran at ~23 fps in Chrome on the Intel iGPU.
- Every GPU budget was a hard-coded constant: 4× MSAA, 2048² × 3 sun
  cascades, 4 shadowed point lights, and every particle a game emitted.
- No game or host could trade image quality for frame rate.
- The web canvas's backing store was a fixed 1280×960 whatever size it
  was shown at.

## G1 — where the integrated GPU's frame goes

There was no recorded per-pass table before; this is the first. Native,
Intel UHD (`TWE_GPU_POWER=low`), 1280×720, `TWE_GPU_PROFILE=1`, 60-frame
averages. Reproduce with `tests/render_bench.rs::render_stress_scene`;
`TWE_STRESS_SCRIPT` swaps the scene.

**`stress_3d`** (100k blocks, 500 lights, 1M particles) — GPU 42–47 ms:

| Pass | ms |
|---|---:|
| Main | 21.5 |
| ParticlesDraw | 8.7 |
| ParticlesSim | 5.1 |
| Clusters | 1.9 |
| CullLate | 1.8 |
| MainLate | 1.4 |
| Bloom | 0.7 |
| everything else | < 0.5 each |

**`survive3d`** — GPU 8.4–8.9 ms:

| Pass | ms |
|---|---:|
| Main | 4.7 |
| Ao | 2.5 |
| Bloom | 0.6 |
| Tonemap | 0.4 |
| Clusters | 0.2 |
| three shadow cascades together | 0.4 |

What the tables say:
- **Shadows are not a cost centre.** The three cascades cost 0.4 ms in
  total, so the tiers don't shrink the shadow map (the plan had a
  `shadow_size` field; it was dropped as speculative).
- **The stress scene's main pass is geometry-bound, not pixel-bound.**
  At 49 % of the pixels (`low`) it costs 16.1 ms against 22.5 ms. The
  cost is 100k instances' vertex work under clustered shading. That
  points G4 at instance and vertex cost, not resolution.
- **Particles are the second-largest cost:** 14 ms for 1M particles.
- **AO is survive3d's largest cost after the main pass:** 2.5 ms at half
  resolution.
- **Without the profiler, survive3d is GPU-bound:** 9.9 ms/frame natively
  on the iGPU, about 100 fps; 3.2 ms on the RTX 3050 Ti.

Deferred from G1: a `?profile=1` mode for the browser. The profiler
blocks on a buffer map, which WebGPU can't do on the main thread; the
browser keeps the `frame_stats` CPU breakdown it already had.

## G2 — quality tiers

**Kernel.** `src/kernel/quality.rs` defines `Tier { Low, Medium, High,
Ultra }` and a `Quality` budget. The budget sets:
- `render_scale`
- `particle_density`
- `point_shadows`
- which effects may run: AO, SSR, volumetric fog, TAA, depth of field,
  motion blur, bloom.

How the renderer uses it:
- `Renderer::set_quality` stores it.
- `render()` first takes a copy of the snapshot with
  `Quality::clamp(post, fog)` applied. `RenderSnapshot` is now `Copy`:
  every field already was, so the copy is cheap.
- The point-shadow budget takes `min(4, point_shadows)`.
- `Particles::prepare` thins each emission to `ceil(count × density)`,
  never to zero. Particles are cosmetic, so replays are unaffected.

**Rules.**
- A tier is a ceiling, never a request: `high` doesn't turn bloom on.
- `Ultra` is the renderer as it was, and it is the default. Every
  existing capture, the image harness (mean ꟻLIP 0.1655) and every test
  render exactly as before.

**Script API** (`src/stdlib/graphics.rs`, `docs/06 §7.7b`):
- `graphics.quality("low"|"medium"|"high"|"ultra")`
- `graphics.render_scale(0.25..1.0)`, which overrides the tier's scale
- `graphics.tier()`

Natively, `TWE_QUALITY=<tier>` applies to games that don't choose a tier.
`host3d::render_frame` hands the budget to the renderer every frame.

Unknown tiers and out-of-range scales are runtime errors that name the
allowed values. Dropping an effect is not an error.

**MSAA stays 4× in every tier.** The sample count is baked into every
pipeline at creation, and its cost is inside the Main pass's number
rather than measured on its own. A runtime MSAA switch (two pipeline
sets) is a G4 candidate only if a 1×-sample experiment shows it pays.

## G3 — render scale and device pixel ratio

**Render scale.**
- Inside `render()`, every scene pass sizes itself from
  `Quality::scene_size(window)`:
  - the graph's pool
  - TAA, clusters, volumetric fog, SSR, AO, DoF, motion blur
  - transmission, GPU culling, particles
- The 2D canvas, the camera's aspect ratio and the swapchain keep the
  window's size.
- The tonemap pass already sampled the HDR image by UV through a
  bilinear sampler, so it scales the scene up to the window with no new
  pass. The HUD still draws at full resolution, so text stays sharp.
- A CAS-style sharpen was in the plan. It is not added: the measured
  frames look right without it, and it would be a new pass on the
  critical path of the weakest GPUs. It can come back if a playtest asks
  for it.

**Web shell** (`crates/twe-web`).
- `fit_canvas` runs each frame and sizes the canvas's backing store to
  its CSS size × `devicePixelRatio` (capped at 2).
- It resizes the renderer when that changes.
- `?capture=N` keeps the fixed 1280×960, so browser captures still
  compare with native frames.
- `bench/graphics/fps.mjs` sets the viewport at a device pixel ratio of
  1, so its 1280×960 numbers are unchanged.

## Measured (native, Intel UHD, 1280×720, GPU ms/frame)

| Scene | ultra | high | medium | low |
|---|---:|---:|---:|---:|
| `stress_3d` | 47.0 | 43.6 | 37.9 | 26.5 |
| `survive3d` | 8.4 | — | 6.5 | 3.4 |

## Tests

- `tests/kernel_render.rs::low_tier_scales_the_scene_to_the_window`:
  hello_3d on `low` matches `ultra` within 8/255 on average, with the lit
  sphere still centred. That catches a scene drawn into a corner or a
  black border.
- `tests/eval.rs::graphics_quality_and_render_scale`: tier names, the
  getter, and the two errors.
- Unit tests in `kernel/quality.rs`: `ultra` is a no-op, `low` drops and
  scales, tiers never get worse going up, particle thinning.
- `tests/render_bench.rs`: `TWE_STRESS_SCRIPT` now runs from the
  script's own directory, so multi-file games like survive3d find their
  assets.

## Two pre-existing failures fixed on the way

- `tests/corpus.rs::every_difficulty_is_in_vocabulary`:
  `examples/action3d` declared `@difficulty: showcase`, which is not in
  the vocabulary. It is now `large`, like the other full games. A plain
  `cargo test` stopped at this binary, so every later test binary was
  silently skipped.
- `tests/debug.rs`: both tests wrote their program to the same
  `twe_dbg_<pid>/prog.twe` and run in parallel. The second write could
  replace the first test's program mid-run, so the breakpoint was never
  hit. Each call now uses its own file. The debugger itself was fine.

## Next

- **G4:** the measured cost centres:
  - stress main pass: geometry-bound, so instance/vertex cost;
  - particles: size floor / LOD;
  - quarter-resolution AO on `medium`;
  - culled shadow casters.
- **G5:** pick the tier from the adapter, plus a dynamic-resolution
  governor. Exit: survive3d ≥ 60 fps and stress ≥ 30 fps on the iGPU in
  Chrome on `auto`.
