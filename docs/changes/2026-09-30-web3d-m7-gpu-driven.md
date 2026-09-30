# web3d-M7 session 11: GPU-driven culling and indirect draws

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [glTF extensions](2026-09-29-web3d-m7-gltf-extensions.md))
**Code:** `src/kernel/gpu_cull.rs` (new); the opaque draw list, the split main pass and the adapter choice in `src/kernel/render.rs`; `tests/render_bench.rs` (new)

## What changed

1. **Measured first.** `tests/render_bench.rs` renders 100 000 cubes headless at 1280×720 in scenes built to separate the costs:
   - **open:** everything visible;
   - **field:** a low camera over a wide grid, the frustum's case;
   - **occluded:** the field behind a wall, the occlusion case;
   - **behind:** everything behind the camera;
   - **small:** 300 cubes and a floor, survive3d's scale;
   - **empty:** nothing to draw.

   It reports `render()`'s CPU time and the time per frame (render + GPU, over 60 frames ended by a readback). Opt-in: `cargo test --release --test render_bench -- --ignored --nocapture`.
2. **Two-phase GPU occlusion culling** (Wihlidal 2015; as in Niagara and Nanite), replacing the CPU's per-instance frustum test:
   - **Early.** A compute pass keeps the instances inside the frustum that were visible last frame. It compacts each draw group's survivors into that group's region of an output buffer, and writes one indirect draw per draw entry (a shape, a mesh, or one glTF primitive of a mesh). The main pass draws them.
   - **Hierarchical Z.** A max-depth pyramid is built from that depth: level 0 is the farthest of each pixel's MSAA samples, and each level above is the farthest of 2×2 below, with the extra row or column an odd size leaves.
   - **Late.** Every frustum-visible instance's bounding sphere is projected. Its nearest depth is compared with the farthest the pyramid holds over its screen box, at the level where the box spans at most 2×2 texels. Visible instances not drawn early are drawn in a second opaque pass, and the result becomes next frame's early set.
   - **Correctness.** It doesn't depend on the early set, which only decides what draws first. An instance that becomes visible is drawn late in the same frame, so an immediate-mode draw list whose order shifts costs efficiency, never pixels.
   - **WebGPU fit.** There is no indirect `first_instance` without an optional feature, so each group's compacted instances are bound as their own vertex-buffer slice. The cull uses 7 storage buffers against WebGPU's limit of 8.
3. **Only when it pays.** The pyramid costs per pixel, not per object: on the 300-cube scene, culling cost the integrated GPU 1.1 ms and saved nothing. GPU culling therefore runs only above **4096 opaque instances** (`GPU_CULL_MIN_INSTANCES`). Below that, everything is drawn directly and the GPU clips what's outside the view. `postfx.frustum_cull(false)` still turns culling off.
4. **Instance colours decode on the GPU.** The sRGB → linear decode of each instance's tint (three `powf` per instance) moved from the CPU into the vertex shader.
5. **Unskinned geometry skips the joint blend.** Every vertex used to blend four joint matrices, identity for unskinned geometry. The instance now carries a skinned flag (`rot.w`), and the main, shadow, prepass and mask vertex shaders skip the blend without it.
6. **The discrete GPU by default (native).** wgpu's default power preference picks the *integrated* GPU on a machine with two. On this laptop, every 3D game had been running on the Intel UHD instead of the RTX 3050 Ti.
   - Native builds now ask for the high-performance adapter.
   - `TWE_GPU_POWER=low` asks for the integrated one, to benchmark the weakest target or save battery.
   - The web build doesn't ask: the browser picks, and Chrome ignores the hint on Windows and warns about it in the console (seen during the Chrome check, then removed).

## Numbers

ms per frame, 100 000 cubes at 1280×720, 4× MSAA, on this laptop.

**Session start** (then the default adapter, the Intel UHD; CPU culling): open 25.2 ms without culling / 27.4 ms with it, field 18.9 / 17.6. `render()` itself took 9–11 ms of CPU. The occluded scene had a bug (the camera stood inside the wall), so there is no baseline for it.

| Scene | Intel UHD, no cull | Intel UHD, GPU cull | RTX 3050 Ti, no cull | RTX 3050 Ti, GPU cull |
|---|---:|---:|---:|---:|
| open (all visible) | 25.6 | 27.9 | 3.45 | 3.86 |
| field (frustum) | 19.0 | 17.8 | 3.54 | 4.01 |
| **occluded** (behind a wall) | 23.1 | **8.69** | 3.66 | 4.53 |
| behind the camera | 7.78 | 5.23 | 3.57 | 3.76 |
| small (300 cubes, under the threshold) | 7.00 | 6.70 | 0.61 | 0.61 |

- **Occlusion culling is the win it should be on the weaker GPU:** 2.7× on the occluded scene, and the 100 000-cube frame drops from 23 to 8.7 ms.
- **The discrete GPU is CPU-bound at this size.** A frame is 3.5 ms, most of it building and uploading 100 000 instances (the 4.8 MB instance buffer). Culling adds 0.4–0.9 ms of CPU there: two phases' bind groups and a render pass per pyramid level, rebuilt each frame. A 100 000-instance frame at about 250 fps doesn't justify caching them yet.
- **When nothing can be culled** (the open scene), the pyramid is pure overhead: +2.3 ms on the integrated GPU.
- **CPU `render()` time** for 100 000 instances dropped from 9–11 ms to 3.2–5 ms (the sRGB decode, and no per-instance frustum test). In GPU-bound scenes the CPU figure also includes waiting for staging memory, so only the "behind" and dGPU rows measure the CPU alone.

**Image quality.** The benchmark now runs on the discrete GPU. Per-scene ꟻLIP moved by at most 0.001 (GPU precision), and the mean is 0.1801 (from 0.1799); none of its scenes reaches the culling threshold. Three.js scores 0.1857.

## Limits

- **Shadow cascades, point-light cubes and the AO prepass still draw every instance.** They have their own views; culling them per view is the obvious extension.
- **Transparent draws are sorted and drawn directly** (per instance, back to front), not culled.
- **Per-object motion vectors** were planned alongside this session, but need stable instance identity across frames. Immediate-mode draws don't carry it, and the renderer can't invent it. It returns with a host-side id (entities have one).
- **No multi-draw-indirect.** It isn't in WebGPU. There is one indirect draw per draw entry, which a game-sized material count keeps small.

## Verification

- `cargo test`: 1007 pass (1006 before, +1).
  - `gpu_culling_is_invisible_and_culls`: 6400 cubes behind a wall, four frames with and without GPU culling. **0 pixels differ.** With culling, 886 of 6401 instances are drawn (all in the early set once it settles), and the rest are hidden by the wall.
  - Shader validation for the cull and pyramid shaders.
  - Every existing render test (the skinned hero, survive3d's frames) passes on the new path.
- `cargo clippy -- -D warnings`: clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:**
  - A scratch page drawing 4901 cubes a frame around a large block, under an orbiting camera, rendered with GPU culling (compute passes, the pyramid, indirect draws) without WebGPU errors, and with no gaps around the occluder.
  - `survive3d` loads clean.
- **Still open from session 10:** the intermittent crash in parallel `kernel_render` runs. This session's runs used `--test-threads=8`, and none crashed.
