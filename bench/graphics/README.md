# Graphics comparison harness (web3d-M7)

This measures Twe's renderer against Three.js and other engines, with numbers. It is how M7's claim, "graphics better than Three.js", gets tested ([M7 plan](../../docs/changes/2026-09-28-web3d-m7-graphics-beyond-threejs.md)).

## What it compares

**Scenes** come from the [Khronos glTF Render Fidelity](https://github.com/KhronosGroup/glTF-Render-Fidelity-Generator) project, at a pinned commit (`suite.json`). Each scene fixes:
- a glTF sample model;
- an HDR environment;
- an orbit camera, field of view and image size.

**The reference** is that project's **Blender Cycles** render of each scene: a path tracer, standing in for ground truth. Cycles output is tone-mapped with the same ACES curve Three.js uses, at exposure 1.

**Contestants:**
- **Twe:** the kernel renderer, headless (`tests/graphics_bench.rs`).
- **Three.js:** `three@0.186.1`, rendered by this harness in headless Chrome, set up the way the fidelity project sets up model-viewer (PMREM environment lighting, ACES, sRGB output).
- **For context,** the goldens that model-viewer, the glTF Sample Viewer, Filament and Babylon.js contributed to the fidelity project, scored the same way.

**The score** is NVIDIA's [ꟻLIP](https://github.com/NVlabs/flip) (LDR, 67 pixels per degree): the mean perceptual difference from the reference, where 0 is identical and lower is better. Transparent backgrounds are composited over black first.

## Running it

From `bench/graphics/`:

```sh
npm install                                          # once: three, playwright-core, gltf-pipeline
pip install --user flip-evaluator==1.7 numpy pillow  # once
node run.mjs                                         # fetch → Three.js → Twe → score
node run.mjs khronos-DamagedHelmet                   # one scene
```

Requirements:
- Google Chrome installed (the Three.js renders use it; no browser download);
- a GPU;
- about 330 MB for the cache.

**Outputs** (all but `results.*` are git-ignored):

| Path | What |
|---|---|
| `cache/<scene>/` | model (multi-file glTFs packed into one `.glb`, so both engines load identical bytes), `scene.json`, goldens |
| `out/twe/`, `out/three/` | the two renders |
| `out/flip/` | ꟻLIP error maps for Twe and Three.js |
| `results.md`, `results.json` | the table (committed, so progress shows in history) |

**The steps separately:**
- `node fetch.mjs`
- `node render-three.mjs [scene …]`
- `cargo test --release --test graphics_bench -- --ignored --nocapture` (from the repo root). Environment variables:
  - `TWE_BENCH_SCENES=a,b` limits it to those scenes;
  - `TWE_BENCH_TAA=0` turns TAA off;
  - `TWE_BENCH_AO=0` turns ambient occlusion off;
  - `TWE_BENCH_AO_RADIUS` sets the AO radius as a fraction of the model's bounding radius.
  - `TWE_BENCH_SSR=0` turns screen-space reflections off (default 1, web3d-M7 session 14).
- `python score.py`
- `python contact.py`: `comparison.jpg`, the reference, Twe and Three.js side by side per scene with their scores (web3d-M7 session 17).

**The like-for-like column** (session 17). Three.js as this harness sets it up has no ambient occlusion or reflections. Render Twe the same way into `out/twe-plain/`:

```sh
TWE_BENCH_OUT=twe-plain TWE_BENCH_AO=0 TWE_BENCH_SSR=0 cargo test --release --test graphics_bench -- --ignored --nocapture
```

`score.py` then adds a "Twe, no AO/SSR" column.

## Frame rates in Chrome

`node fps.mjs <dir> [--page p.html] [--gpu high] [--seconds N] [--warmup N] [--shot out.png]` serves a folder, opens it in the installed Chrome, and prints frames per second and ms per frame:
- **Throttles and vsync are off,** so the result is throughput, independent of the window being visible.
- **Twe pages** also report the runtime's own split: script tick, script render, kernel CPU.
- **Which GPU:** Chrome uses the integrated GPU by default on a laptop with two; `--gpu high` forces the discrete one. WebGPU's `powerPreference` is ignored on Windows.

The M7 stress scene, both sides:

```sh
twec build --target web --out /tmp/stress examples/stress_3d.twe   # from the repo root
node fps.mjs /tmp/stress --warmup 12 [--gpu high]
node fps.mjs . --page three/stress.html --warmup 12 [--gpu high]    # the Three.js equivalent
bash compare.sh /tmp/stress [rounds]                                 # both, interleaved, medians per GPU
```

- **Canvas size:** both pages draw a 1280×960 canvas (Twe letterboxes 4:3); `fps.mjs` prints the window and canvas sizes so a mismatch shows.
- **Runtime stats:** Twe pages also print the share of frames the GPU culled; `FPS_STATS=1` prints the raw counters.
- **Interleaving:** `compare.sh` alternates the two sides, since a laptop's GPUs throttle and back-to-back runs of one side would see a different thermal state.
- **Natively,** `TWE_GPU_PROFILE=1` prints per-pass GPU times (timestamp queries) for any 3D run, e.g. `TWE_STRESS_SCRIPT=examples/stress_3d.twe cargo test --release --test render_bench -- --ignored`.

## Rules

- **Pins change only deliberately:** the generator commit, the sample-assets commit and the Three.js version. A pin change re-baselines every number and is noted in the results commit.
- **Harness setup must be identical for Twe and Three.js.** Neither engine gets scene-specific tuning.
- **A feature isn't "done" until it improves its scenes' rows.** The M7 closeout publishes the final table, including where Twe loses.
