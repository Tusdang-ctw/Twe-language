# Phase 28 closeout — 3D commercial polish

**Status:** codebase-closed 2026-05-09. All six sessions shipped. Visual playtest of `examples/crystal_hunter.twe` is the remaining manual step before treating the phase as fully shipped rather than codebase-closed.

The second phase of the post-v1.0 plan from `docs/05-roadmap.md` "Post-v1.0 — Phases 27–32." Closes the deferrals carried from `docs/changes/2026-05-07-phase-17-closeout.md` (mipmaps + anisotropic) and `docs/changes/2026-05-07-phase-24-26-closeout.md` (bloom, DoF, cascaded shadows, async preload). The DoF deferral remains open — see "Honest deferrals" below.

## Sessions shipped

| # | Surface | Commit |
|---|---------|--------|
| 1 | Mipmap pyramid + anisotropic filtering on game textures | `969966c` |
| 2 | Cascaded shadow maps (3 cascades, PCF, view-z cascade selection) | `033264a` |
| 3 | Inline 12-tap bloom (`postfx.bloom` + `postfx.bloom_threshold`) | `033264a` |
| 4 | Vignette tint color (`postfx.vignette_color`) | `033264a` |
| 4b | Sessions 2–4 closure: stdlib tests + `crystal_hunter.twe` showcase | this commit's pair |
| 5 | Async `.glb` parse via background worker thread + main-thread upload | this commit's pair |
| 6 | This closeout note | this commit |

## Exit criteria

Per the Phase 28 entry in `docs/05-roadmap.md`:

- **`examples/crystal_hunter.twe` runs at 60fps with bloom + cascades on a 4-year-old GPU.** Pending visual playtest. The crystal_hunter scene now drives both surfaces (`postfx.bloom(0.55)` + `postfx.vignette_color((0.04, 0.02, 0.10))`); cascade rendering rides the existing `sun.shadow(true)` path.
- **`survive_beta.exe` shows zero regression on the Phase 11 bench harness (`benches/vm.rs`).** Bench command is `cargo bench`; not snapshotted in this closeout (criterion is the canonical command). The 2D path doesn't go through `play3d`, so the bench surface is unchanged by Phase 28.
- **No new language surface.** Met for sessions 1, 2, 5. Sessions 3 + 4 ship four new `postfx.*` builtins under the existing `postfx` namespace — no new keywords, no parser changes.

## Stdlib delta

Four new `postfx.*` builtins shipped this phase:

| Function | Default | Effect |
|----------|---------|--------|
| `postfx.bloom(intensity)` | 0.0 (off) | Inline 12-tap bright-pixel kernel scaled by intensity. Added to HDR before ACES. |
| `postfx.bloom_threshold(t)` | 1.0 | HDR luminance above which a sample contributes to bloom. |
| `postfx.vignette_color(c)` | `(0, 0, 0)` (black) | Tint the vignette lerps toward at the corners. |
| (vignette strength + tonemap toggle existed before) | | |

`docs/06-design-document.md` gains a new §7.7b "3D post-processing" subsection enumerating the full `postfx.*` surface.

Three new tests in `tests/eval.rs` pin the state-setter behaviour:

- `postfx_bloom_setters` — both `postfx.bloom` and `postfx.bloom_threshold` round-trip through the thread-local store.
- `postfx_vignette_color_setter` — RGB tuple parses and clamps.
- `postfx_bloom_clamps_negative_intensity_to_zero` — negative intensities don't sneak through.

These are state-setter tests, not visual tests. The render path is exercised by `twec play3d`, which a terminal can't drive — the visual side is the user's manual step.

## Code-side audit

**Session 1 — Mipmaps + anisotropy** (`969966c`):
- `upload_texture_with_mips` helper builds the mip chain CPU-side via `image::imageops::resize` with Triangle filter.
- `default_sampler` upgraded to `mipmap_filter: Linear` + `anisotropy_clamp: 16`.
- Two upload sites swapped to the helper: `texture("path")` builtin loader + auto-extracted glb base color.
- Render targets (white 1×1 fallback, shadow map, depth target, HDR offscreen) keep `mip_level_count = 1` — they don't sample at varying mip levels.
- Honest caveat: Triangle resampling on `Rgba8UnormSrgb` resamples in sRGB space, not linear. Slight darkening at small mips. Linear-space resize is a follow-on if it's ever pressured.

**Session 2 — Cascaded shadow maps** (`033264a`):
- `ShadowUniform` now holds `[mat4; CASCADE_COUNT]` + `split_distances: vec4` + `flags: vec4`. `CASCADE_COUNT = 3`.
- New `ShadowPassUniform` (just one mat4) is rebound between three depth passes; small per-pass uniform buffers (`shadow_pass_buffers: [Buffer; 3]` + `shadow_pass_bgs: [BindGroup; 3]`) hold the active cascade matrix.
- Shadow texture is 2D array, three layers @ 2048² each (~48 MB depth memory total). Three depth passes per frame, one per layer.
- `compute_shadow_uniform` iterates `CASCADE_SCALES = [0.25, 1.0, 4.0]`, building one ortho per cascade centered on the camera target. Split distances are camera-space forward depths (cascade thresholds).
- WGSL shadow lookup picks a cascade by `view_z`, samples `texture_depth_2d_array` with `textureSampleCompare(t, s, uv, layer, ref)`. PCF bias scales 4× per cascade step.
- VertexOutput gains `view_z: f32` derived from `clip.w` (positive forward depth under the codebase's reverse-Z perspective).
- **Compromise**: each cascade is target-centered, not view-frustum-corners-fitted. Works for third-person cameras (target ≈ player); free cameras far from target lose cascade-0 resolution. View-frustum-corner CSM is a deferred follow-on.

**Session 3 — Inline bloom** (`033264a`):
- Tonemap shader gains a `bloom_inline(uv, threshold)` helper that samples 12 offsets in two concentric rings (6 px + 12 px) around the current pixel, threshold-subtracts, averages, and is added to the HDR base before ACES.
- Tonemap params buffer extended from 16 B → 32 B to carry bloom intensity, bloom threshold, and the new vignette tint color alongside the existing ACES + vignette flags.
- Small radius — multi-tier downsample chain for wide haloes is a deferred follow-on.

**Session 4 — Vignette tint color** (`033264a`):
- Tonemap pass now lerps the LDR color toward the configured tint at corners instead of always darkening to black. Default tint = black so the prior vignette behaviour is preserved.

**Session 4b — Closure** (this commit):
- New stdlib tests pin postfx state-setter behaviour.
- `examples/crystal_hunter.twe` drives the new surface as the canonical demo (`postfx.bloom(0.55)` + `postfx.bloom_threshold(1.2)` + tinted vignette).

**Session 5 — Async asset preload** (this commit):
- `load_and_upload_mesh` split into `load_glb` (CPU only, threadable) + `upload_loaded_glb` (main-thread GPU work).
- `RenderState` gains `mesh_load_jobs: HashMap<u32, JoinHandle<Result<LoadedGlb, String>>>`.
- Render loop spawns a worker thread per uncached referenced id (named `twec-glb-load-<id>` for diagnostic clarity), polls `is_finished()` non-blocking the next frame, and `join()`s + uploads to GPU only when the worker has exited.
- Hot reload clears `mesh_load_jobs` alongside `mesh_cache` and `mesh_load_failures` so a reloaded env starts fresh.
- The first frame a mesh is referenced spawns a worker but draws nothing; the upload typically lands one or two frames later. For interactive scenes this means loading a complex `.glb` no longer freezes the frame.
- The synchronous `load_and_upload_mesh` is retained `#[allow(dead_code)]` for tests + the texture-only path; not used by the render loop anymore.

## Honest deferrals

- **Depth of field.** Listed in the Phase 28 plan; not shipped. Real DoF needs a focus-distance API + blur radius derivation from the depth buffer + at least two-pass separable Gaussian — its own session of work and tuning. Deferred to a follow-on; `postfx.dof` would slot into the same `postfx.*` namespace when it lands.
- **View-frustum-corners CSM.** Each cascade is target-centered rather than tightly bounded to the cascade's slice of the view frustum. For third-person cameras this is fine; for free-cameras this loses cascade-0 quality. Re-entry: pressured if a contributor ships a free-camera 3D demo and notices the gap.
- **Multi-tier bloom downsample chain.** The inline 12-tap kernel has a small radius (~12 pixels). Wide haloes need a downsample/upsample mip chain. Re-entry: pressured if a content scene needs Hollywood bloom rather than subtle highlight glow.
- **Linear-space mip resampling.** `image::imageops::resize` on `Rgba8UnormSrgb` byte values resamples in sRGB space — small but real darkening on aggressive mip downsamples. Re-entry: visible if textures show a noticeable luminance shift at distance.
- **Auto cascade tuning for `sun.shadow_extent`.** The cascade scales (0.25× / 1.0× / 4.0×) are hard-coded. Real engines auto-fit to view distance.

## Visual playtest checklist

Code is not visually verified from this terminal. Run `twec play3d examples/crystal_hunter.twe` and watch for:

- **Mipmaps**: distant ground textures should NOT show shimmering / aliasing as the camera moves. With the sampler set to Linear/Linear/Linear + 16× anisotropy, surfaces seen at sharp grazing angles (e.g., looking down a long corridor) should stay sharp instead of blurring.
- **Cascaded shadows**: shadows under the player should be sharp; shadows in the distance should still be visible (not cut off at the old single-cascade ortho boundary). Watch for "peter-panning" (shadow detached from caster) — if visible, cascade-2 bias may be too high.
- **Bloom**: the crystal highlights and torch cores should glow softly. Bloom too strong → reduce `postfx.bloom(0.55)` to `0.3`. Bloom never visible → drop `postfx.bloom_threshold(1.2)` to `0.7`.
- **Vignette tint**: corners should fade toward a deep blue-purple instead of pure black. Wrong tint → adjust the `postfx.vignette_color` tuple.
- **Async load**: first-frame visual hitch loading a `.glb` should be gone (the freeze is now distributed across one or two frames as the worker finishes mid-frame).

## Doc updates

- `docs/06-design-document.md` §7.7b — new "3D post-processing" subsection.
- `examples/crystal_hunter.twe` — postfx surface showcase.
- `CLAUDE.md` "Post-v0.1 the canonical plan is..." line — Phase 28 marked codebase-closed.
- `docs/05-roadmap.md` Phase 28 section — status note + size-table row updated.
- `README.md` — test count refresh.

## Test delta

`cargo test --release` reports **745 passing** (was 742 at Phase 27 close on 2026-05-06; +3 from sessions 2–4 closure tests, no new tests for sessions 1 or 5 because the changes are render-path only and not driveable from a terminal). `cargo clippy --release --all-targets -- -D warnings` clean.

## What this enables

- The Phase 24–26 deferrals named in `docs/changes/2026-05-07-phase-24-26-closeout.md` are partially closed: bloom + cascaded shadows + mipmaps + async preload all ship; DoF is the only one still pending.
- `examples/crystal_hunter.twe` becomes the canonical "shipped on Twe 3D" demo, exercising every post-v1.0 3D feature in one scene.
- Phase 29 (determinism layer) can open against a 3D commercial-tier baseline rather than the v0.6 minimum-viable 3D.

## What does not change

- No grammar change. No new keyword. No type-system change. Four new `postfx.*` builtins fit the existing `postfx.tonemap` / `postfx.vignette` / `postfx.frustum_cull` surface.
- No regression on the v1.0 surface. All previous examples continue to parse + type-check + run.
- Phase 29 (determinism layer) entry remains where it was: the unresolved 3× bytecode-VM speedup gap from Phase 8.5 + the input replay primitive + bounded GC pauses + sample-accurate audio.
