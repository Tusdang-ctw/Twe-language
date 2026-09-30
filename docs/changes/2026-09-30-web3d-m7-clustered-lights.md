# web3d-M7 session 12: clustered lighting, spot lights, a shadow budget

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [GPU-driven culling](2026-09-30-web3d-m7-gpu-driven.md))
**Code:**
- `src/kernel/clusters.rs` (new): the grid and its build pass;
- `src/render3d_types.rs`: the light list type;
- `src/kernel/render.rs`: the shader's clustered loop, the shadow budget;
- `src/stdlib.rs`: `light.*`, `light.cone`.

## What changed

1. **Clustered forward lighting** (Olsson, Billeter & Assarsson 2012; the exponential depth slicing of Doom 2016 and Filament):
   - **The grid.** The view is a grid of 16 × 9 screen tiles × 24 depth slices; each slice spans the same ratio of depths.
   - **The build pass.** A compute pass lists, for each of the 3456 clusters, the lights whose range sphere reaches the cluster's view-space box (up to 128 per cluster).
   - **Shading.** Each surface looks up its cluster from its pixel and view depth, and loops over that list only. Shared by glTF, plain and `visual` surfaces, opaque and translucent.
   - **Storage.** The light list, the grid and the parameters are storage and uniform buffers in the frame group. A frame with no point or spot lights skips the build and the loop.
2. **Up to 1024 point and spot lights,** from 8. The script side keeps them in a list (`light.add` returns index + 1 and reuses removed slots). The snapshot hands the kernel a slice (`RenderSnapshot::point_lights`). `LightsUniform` keeps only the ambient and the sun.
3. **Spot lights:** `light.cone(handle, direction, angle)`.
   - **Why a cone, not a new kind of light.** One way to make a light (`light.add`), with a cone as an attribute of it, like its shadow (Principle 2).
   - **The shape.** The cone lights `angle` degrees either side of its direction, fading over its outer fifth, and angle ≥ 180 is a point light again.
   - **Clustering.** Clusters list a spot by its range sphere; the cone test happens when shading.
   - **Shadows.** A shadowed spot uses a shadow cube like a point light (one face would do; see Limits).
4. **A shadow budget.** Session 6 gave the first four shadow-casting lights *in slot order* their cube maps. Now the four nearest the camera (distance minus radius) get them, so the budget follows the player through a level of torches.

## Numbers

`tests/render_bench.rs`: 300 cubes and a floor, top-down, 1280×720, 4× MSAA, ms per frame.

| Lights | Intel UHD | RTX 3050 Ti |
|---|---:|---:|
| none | 4.6 (7.0 in session 11) | 0.55 |
| 100 (radius 3) | 7.1 | 0.86 |
| 1000 (radius 3) | 30.5 | 3.3 |

- **The no-light frame got faster** (7.0 → 4.6 ms on the integrated GPU): every pixel used to walk 8 empty light slots.
- **The 1000-light scene is deliberately dense.** Radius-3 lights packed into a 20-unit disk overlap about 22 deep, so every pixel really shades about 22 lights. The cost is the lighting itself, not the bookkeeping.
- **The image benchmark is unchanged (0.1801).** Its scenes have no point lights.

## Limits

- **A shadowed spot light renders a full cube (6 faces).** One face covering the cone would be a sixth of the cost. The budget counts lights, not faces.
- **Clusters list by range sphere.** A narrow spot is listed in clusters outside its cone and rejected per pixel.
- **128 lights per cluster.** Past that a cluster drops the rest. That needs over 128 lights overlapping one small region of the view.
- **Light colours are linear gains, not sRGB,** as before (`light.add`'s colour is multiplied straight in). Changing that would change every existing scene's lights.

## Verification

- `cargo test`: 1011 pass (1007 before, +4):
  - `hundreds_of_lights_all_shine`: 300 lights, each lighting only its own patch of floor; every patch is lit, and the floor between them stays dark;
  - `spot_lights_light_their_cone`: lit inside the cone, dark outside it within the same radius;
  - `many_lights_and_cones` (`tests/eval.rs`): 40 lights from a script, `light.cone`'s maths and errors, `light.clear`;
  - `slices_are_exponential`, and shader validation for the build pass.
- Every existing render test passes on the clustered path, including point-light shadows.
- `cargo clippy -- -D warnings`: clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:** 200 coloured point lights and a shadowed spot light over a floor with blocks, under an orbiting camera, rendered without WebGPU errors. This is the first use of storage buffers in the main fragment shader on the web. `survive3d` still plays.
