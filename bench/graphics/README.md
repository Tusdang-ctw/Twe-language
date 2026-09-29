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
- `cargo test --release --test graphics_bench -- --ignored --nocapture` (from the repo root; `TWE_BENCH_SCENES=a,b` limits it)
- `python score.py`

## Rules

- **Pins change only deliberately:** the generator commit, the sample-assets commit and the Three.js version. A pin change re-baselines every number and is noted in the results commit.
- **Harness setup must be identical for Twe and Three.js.** Neither engine gets scene-specific tuning.
- **A feature isn't "done" until it improves its scenes' rows.** The M7 closeout publishes the final table, including where Twe loses.
