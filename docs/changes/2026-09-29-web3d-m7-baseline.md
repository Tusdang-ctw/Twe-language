# web3d-M7 session 2: the comparison harness, and where Twe starts

**Date:** 2026-09-29
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md))
**Harness:** [`bench/graphics/`](../../bench/graphics/README.md); the table is [`bench/graphics/results.md`](../../bench/graphics/results.md)

## What was built

`node bench/graphics/run.mjs` fetches, renders and scores in one command:

1. **Fetch the suite** (`fetch.mjs`): 12 scenes from the Khronos glTF Render Fidelity generator, pinned at `deaaba0b6c`. Each includes:
   - the model, from glTF-Sample-Assets at the generator's pinned `cfbe2f9ac2`;
   - the HDR environment;
   - the camera, field of view and image size;
   - the goldens: Blender **Cycles** as the path-traced reference, plus model-viewer, the glTF Sample Viewer, Filament and Babylon.js for context.

   Multi-file glTFs are packed to `.glb`, so both engines load identical bytes.
2. **Render with Three.js** (`render-three.mjs`, `three/render.html`):
   - pinned `three@0.186.1`, in the installed Chrome via `playwright-core`;
   - PMREM environment lighting, ACES at exposure 1, sRGB output. This is the fidelity project's own setup and the same ACES curve applied to its Cycles goldens.
3. **Render with Twe** (`tests/graphics_bench.rs`, opt-in): the kernel renderer, headless, with the same camera, clip planes and size.
4. **Score** (`score.py`): mean ꟻLIP (NVIDIA's `flip-evaluator` 1.7, LDR, 67 ppd) of every image against Cycles. Writes `results.md` and `results.json`, and error maps for Twe and Three.js.

**Kernel support it needed:**
- `Camera3d` gained `fov_y`, `near` and `far`; `Camera3d::new` keeps the game camera's lens (60°, 0.1–100 m).
- `RenderSnapshot` gained `background`.
- `LoadedGlb::bounding_sphere`, for clip planes.
- The `gltf` crate's `KHR_texture_transform` feature, so models that *require* it load at all.

Game output is unchanged: the seven GPU render tests are still byte-identical.

## The baseline

Mean ꟻLIP against Cycles; lower is better.

| | Twe | Three.js | glTF Sample Viewer | Filament | Babylon.js |
|---|---:|---:|---:|---:|---:|
| Mean over 12 scenes | **0.348** | **0.186** | 0.180 | 0.194 | 0.196 |

Per scene (full table in `results.md`), Twe loses to Three.js everywhere except Sponza, and the Sponza "win" isn't real (see below). Its worst scenes are the ones where it lacks whole features:

| Scene | Twe | Three.js | What Twe lacks |
|---|---:|---:|---|
| IridescenceLamp | 0.757 | 0.095 | PBR, IBL, iridescence, transmission |
| MetalRoughSpheres | 0.671 | 0.152 | metallic-roughness shading, environment reflections |
| TextureTransformTest | 0.507 | 0.106 | `KHR_texture_transform` (loaded, not applied) |
| TransmissionTest | 0.394 | 0.278 | transmission |
| EmissiveStrengthTest | 0.323 | 0.256 | emissive, emissive strength |
| AlphaBlendModeTest | 0.297 | 0.107 | alpha blending and masking |
| SheenChair | 0.258 | 0.077 | sheen; per-primitive materials |
| DamagedHelmet | 0.215 | 0.062 | PBR maps, IBL, emissive |

So the gap is about 1.9× on the mean. Three.js at r186 is within noise of the best real-time engines in the fidelity project (0.186 against the glTF Sample Viewer's 0.180), so it's a strong target.

## What the harness found on its first run

- **Twe's loader applies one material to a whole model.** It flattens every primitive and textures all of them with the first material's base colour. On Sponza every wall, floor and curtain wears the same foliage texture. Fixed by session 3 (materials per glTF primitive).
- **Sponza isn't a meaningful comparison yet.** Its Cycles reference is lit only by sky through the courtyard opening, so it is very dark. Real-time engines without occlusion light the interior far more (0.86–0.88 for everyone else). Twe scores 0.37 only because its default lighting is dim. The note is carried in `suite.json` and printed under the table.
- **Two harness bugs, both fixed before the numbers above:**
  - `gltf-pipeline` dropped textures referenced only from material extensions (AnisotropyBarnLamp's anisotropy map) until run with `keepUnusedElements`.
  - A zero orbit radius (Sponza's camera stands at its target) needs the camera to look along the orbit direction, not "at the target". Both renderers now do.

## Next

Session 3, physically based shading, is scored against these rows:
- metallic-roughness (GGX with multiscatter);
- normal, occlusion, emissive and metal-rough maps;
- per-primitive materials;
- vertex colours and texture transforms.

The harness and pins stay fixed. A change to either re-baselines, and must say so.
