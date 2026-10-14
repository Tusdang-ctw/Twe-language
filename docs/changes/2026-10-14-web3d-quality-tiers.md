# Low-end track (G1–G5): measurements, quality tiers, render scale, auto quality

**Date:** 2026-10-14
**Line:** post-M12 coding plan, Track G (low-end graphics), sessions 1–5
**Status:** **Track G closed.** G1–G3 shipped first (commit `893ba87`);
G4 and G5 are below. Both exit numbers are met in steady state. One
Chrome transient is open; see "Open" at the end.

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

## G4 — cheap paths for the measured cost centres

Each candidate was measured on the iGPU. Only what paid was kept.

| Candidate | Result | Kept |
|---|---|---|
| Quarter-resolution AO on `medium` (`low` has no AO) | survive3d AO 1.73 → 0.74 ms; frame 6.5 → 5.7 ms | yes |
| Sub-pixel particle cull: skip particles below 0.35–0.5 px radius on lower tiers | `ultra` particle draw got *slower* (12–13 ms vs 8.7–11.6, the vertex stage gained a second projection), `medium` didn't improve | **no, reverted** |
| Culled shadow casters | sun shadows cost 0.4 ms in total (G1) | not attempted |
| Re-render point-shadow cubes only when something moves | no measured scene pays for point shadows | not attempted |
| 1× MSAA tier | the main pass always resolves a multisampled target; a 1× path is a second pipeline set | deferred |

**How quarter-resolution AO works.**
- `AoFrame::downsample` (2 or 4) sizes the AO textures.
- It reaches the GTAO and blur shaders as `ao.params.z`.
- The main shader maps a pixel to its AO texel by the texture's size
  relative to `clusters.screen`, instead of a hard-coded `* 0.5`.
- `kernel_render.rs::quarter_resolution_ao_still_darkens_contact` checks
  the result.

**Noise.** Run-to-run noise on this iGPU is large: `ultra` stress frames
ranged 43–56 ms across back-to-back runs. Only repeated, interleaved
runs count.

## G5 — automatic quality

**The governor** is `kernel::quality::AutoQuality`. It reads only the
interval between frames, the one signal every host has: a browser can't
read GPU timestamps without stalling.
- **Start.** Discrete GPUs start on `ultra`; integrated and unknown GPUs
  on `high` (WebGPU doesn't report the kind); CPU rasterisers on `low`.
- **Down, fast.** Over 1.2× the target for 0.5 s takes 0.1 off the render
  scale. Below half the tier's own scale it steps down a tier.
- **Up, slowly.** After 3 s on target it probes +0.05, never within 10 s
  of a drop and never above the tier it started on.
- **Ignored.** Frames over 0.25 s are hitches.
- **Tests:** unit tests in `quality.rs` (holds, steps down, recovers,
  ignores hitches, starting tiers).

**Hosts.** `host3d::govern_quality` runs it per live frame in `play3d`
and in the web shell's 3D frame.
- The target is 60 fps, or the monitor's refresh natively if lower.
- The budget reaches `render_frame` through
  `stdlib::set_auto_quality`, so `graphics_quality()` resolves in this
  order: the script's tier, then `TWE_QUALITY`, then the governor, then
  `Ultra`.
- Any explicit choice switches the governor off.
  `graphics.quality("auto")` gives the choice back.
- Tests, benchmarks and `?capture` never run it.
- Changes are logged: `twe: quality <tier> at <n>% scale` in the browser
  console (`fps.mjs` prints them), and with `TWE_QUALITY_LOG=1`
  natively.

### Three bugs the governor exposed

1. **GPU memory was never freed in the browser.** In wgpu's web backend,
   dropping a `Texture` or `Buffer` is a no-op; GPU memory waits for a
   JavaScript garbage collection. Every resize and every quality change
   replaces all screen-sized targets.
   - Now every replacement destroys the old resource:
     `kernel::release_view` / `destroy()` in the graph's texture pool,
     bloom, DoF, motion blur, transmission, AO, TAA history, the GPU
     culler's buffers and hi-Z, the particle pool, the instance buffer
     and the headless target.
   - Natively the renderer now also does a non-blocking `device.poll`
     each frame.
   - `kernel_render.rs::quality_changes_at_runtime_render_cleanly`
     steps through 15 budgets and two resizes with AO, TAA, bloom, DoF,
     motion blur, SSR, fog and particles on. A destroyed texture used
     again is a native validation panic, so the test catches that.
2. **The particle pool only grew, and costs its whole capacity.**
   Simulation and drawing both cover every slot. A pool grown under a
   denser tier kept that cost after the governor lowered the density, so
   a density drop now starts the pool over small. Particles are
   cosmetic, so replays are unaffected.
3. **The GPU-culling probe could be fooled by a budget change.** It
   times 60 frames culled against 60 unculled and keeps the winner for
   720 frames. A budget change inside its windows credited culling with
   the cheaper tier's saving, so a budget change now restarts the probe
   (`GpuCull::reprobe`).

`TWE_STRESS_RAMP=1` on `render_bench::render_stress_scene` replays the
governor's descent natively (syncing per step, as a presenting host's
frame pacing would) and then measures. `TWE_STRESS_FRAMES` measures
longer.

### Exit numbers

Chrome, Intel UHD (gen-12lp), 1280×960, vsync off, `auto`:

| Scene | Before tiers | `auto`, steady state |
|---|---:|---:|
| `survive3d` | 69 fps | 64 fps (stays on `high`) |
| `stress_3d` | 23 fps | **56–68 fps**, settled on `low` at 35–42 % scale |

Forced tiers on `stress_3d`, same setup:

| Tier | fps |
|---|---:|
| `ultra` | 21.8 |
| `low` | 33.4 |
| `low` at 35 % | 47.3 |

The plan's exit was survive3d ≥ 60 and stress ≥ 30 on `auto`: both met
in steady state. The ultra image harness is unchanged: `ultra` is the
code path as before. All 1135 tests pass, and clippy is clean natively
and for wasm32.

## Open: a Chrome-only slow period after a large reallocation

`fps.mjs --timeline` (new: frames per second from load) shows the issue.
After the first big budget change, Chrome runs at about 20 fps for
~18 s, then jumps to the new tier's rate.
- It happens with the governor's descent, and with a single scripted
  `high` → `low` switch at 4 s, which recovers at about 22 s.
- Natively the same switch takes effect on the next frame (GPU profile
  38 → 11.5 ms).
- It is not GPU culling: it happens with culling off.
- It is not leaked memory: it persists with every replaced resource
  destroyed.
- It is not the new tier's cost: that tier from the start runs at full
  speed.

The governor reads the slow period as real and descends to its floor.
It then climbs back once frames recover, but slowly. Likely suspects are
on Chrome's side, such as residency management after a large free or a
deferred-deletion queue. The next step is a `chrome://tracing` capture
across a switch. Until then, a game that knows its budget can set
`graphics.quality`.

## Next

Track W, the WebGL2 fallback, builds on the `low` tier: `Quality::compat`
will be `low` with every compute pass replaced.
