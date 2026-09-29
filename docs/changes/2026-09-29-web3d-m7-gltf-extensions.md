# web3d-M7 session 10: glTF material extensions

**Date:** 2026-09-29
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [translucency and fog](2026-09-29-web3d-m7-transparency-fog.md))
**Code:**
- `src/kernel/material.rs`: the extension parameters, texture roles and slots;
- `src/kernel/render.rs`: the layered `shade()`, the transmission split of the main pass;
- `src/kernel/post.rs`: the transmission source;
- `src/kernel/environment.rs`: the sheen albedo in the DFG table.

## What changed

1. **Seven layered-material extensions**, read from the glTF JSON (the `gltf` crate models few of them):
   - **`KHR_materials_ior` and `KHR_materials_specular`:** the dielectric lobe's f0 and f90, from the IOR, a specular weight and a specular colour (the split sum becomes f0·A + f90·B).
   - **`KHR_materials_clearcoat`:** a dielectric GGX layer (IOR 1.5) over everything, with its own roughness and normal map. What it reflects is taken from the layers below it, for lights, the environment and emission.
   - **`KHR_materials_sheen`:** the Charlie distribution (Estevez & Kulla 2017) with Neubelt's visibility term. The layer below is scaled by the sheen's directional albedo, precomputed into the DFG table's unused third channel. Environment sheen uses the irradiance, since the lobe is broad.
   - **`KHR_materials_iridescence`:** thin-film interference (Belcour & Barla 2017), evaluated as the Khronos sample viewer does: an air / film / base Fresnel with the phase integrated against the CIE sensitivity curves. It replaces the base layer's Fresnel, for lights and the environment, with the film thickness from its texture range.
   - **`KHR_materials_transmission` with `KHR_materials_volume`:**
     - transmitted light replaces diffuse reflection;
     - what's behind is refracted by the IOR through the volume's thickness (in mesh units, scaled by the instance);
     - it is read from a mipmapped copy of the opaque scene at a roughness-dependent level;
     - it is absorbed over the path (Beer–Lambert, attenuation colour and distance) and tinted by the base colour.
2. **Extension textures through four shared slots.**
   - Each extension texture a material uses (a *role*: clearcoat, clearcoat roughness, clearcoat normal, sheen colour, transmission, thickness, iridescence thickness, …) gets one of four extension slots. References to the same texels share one, and the uniform records the slot per role.
   - Why four: WebGPU allows 16 sampled textures per shader stage. The frame, shadow and material groups now use exactly 16 (the frame group gained the transmission source), so four extension slots is the budget.
   - Every Khronos sample in the suite fits (ClearCoatTest uses three). A material that needs more is reported and loses the extra texture.
3. **Transmission splits the main pass.** When a transmissive primitive is visible:
   - the opaque scene and backdrop draw first, keeping the multisampled targets;
   - the resolved frame is copied into a full mip chain (the *transmission source*);
   - a second pass loads the targets and draws the sorted transparent list, where transmissive primitives now also go.

   **Alpha as coverage.** With transmission on, the background clears with alpha 0, so after the MSAA resolve alpha is exactly geometry coverage. Where a refracted ray reaches no geometry, it sees the environment even when the camera's background is a flat colour, as a path tracer does. This is what made the barn lamp's bulb read as glass rather than black.
4. **Not in this session:**
   - **`KHR_materials_anisotropy`.** The barn lamp draws isotropically and already scores best in the table.
   - **KTX2 / Basis Universal textures and meshopt geometry.** Decoding either needs a C++ library: the Basis transcoder has no Rust implementation, and the meshopt crate binds the C++ original. That breaks `cargo build` on the wasm target without extra tooling, and no scene in the suite uses them. They return when a game needs them, most likely as a pure-Rust meshopt decoder first (the codec is small), with KTX2 behind a feature.

## Scores

Mean ꟻLIP against Cycles; lower is better.

| Scene | Session 9 | Session 10 | Three.js |
|---|---:|---:|---:|
| SheenChair | 0.0957 | **0.0752** | 0.0767 |
| ClearCoatTest | 0.0706 | **0.0684** | 0.0754 |
| TransmissionTest | 0.2934 | 0.2792 | 0.2780 |
| IridescenceLamp | 0.1070 | 0.1027 | 0.0952 |
| AnisotropyBarnLamp | 0.0609 | **0.0533** | 0.0917 |
| **Mean, 12 scenes** | 0.1839 | **0.1799** | 0.1857 |

- **Twe's mean now ties the best golden in the table,** the Khronos glTF Sample Viewer's 0.1799, and is 0.0058 below Three.js.
- **Twe is ahead of Three.js on 5 of 12 scenes:** MetalRoughSpheres, Sponza, ClearCoatTest, SheenChair and AnisotropyBarnLamp. TransmissionTest trails by 0.001. The AO caveat from session 7 stands: the harness gives Twe ambient occlusion and Three.js none.
- **Where it helped:**
  - SheenChair is sheen alone.
  - ClearCoatTest is clearcoat with its normal maps.
  - The barn lamp gained most from the environment behind its glass bulb.
  - IridescenceLamp's glass globe now reads as glass (it was opaque white, which session 10's first half made worse, since iridescence tinted it).
- **IridescenceLamp still trails Three.js.** Cycles doesn't render glTF iridescence at all (its lamp-shade lining is plain), so matching the spec costs error against this reference on that scene.

## Verification

- `cargo test`: 1006 pass (1003 before, +3). New tests:
  - `material_extensions_read_from_gltf`: a JSON-only glTF with seven extensions; the parameters, shared slots (clearcoat and its roughness on one texture), overflow (six roles over five textures drop exactly one) and the colour encoding of colour slots.
  - The uniform layout test, updated.
  - `gltf_material_extensions_shade`: under flat ambient light, sheen and clearcoat add light to a black base; a thin film colours a grey metal. (On a perfect white mirror a film reflects every wavelength and stays white, as physics says; the first version of the test learned that.)
  - `gltf_transmission_and_volume`: a red block shows through a clear quad and is absorbed by a blue volume.
  - Shader validation for the transmission copy (in the existing post-shader test).
- `cargo clippy -- -D warnings`: clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:** a scratch web project with the two test glTFs (extension quads, and transmission quads over a red block) under a sun, ambient light and TAA ran without WebGPU errors. The 16-texture material layout, the split main pass and the transmission mip chain all work under Tint. `survive3d` still loads clean.

## Found on the way: an intermittent crash in the parallel render tests

`tests/kernel_render.rs` crashes about one run in seven with `STATUS_HEAP_CORRUPTION` or `STATUS_ACCESS_VIOLATION`.

- **It isn't this session.** The session-9 commit does the same (1 in 15).
- **It needs concurrency.** There were 0 crashes in 12 runs with `--test-threads=1`.
- **The GPU layer alone is clean.** 12 threads rendering in parallel without the interpreter gave 0 crashes in 20 runs.

- **Neither half alone crashes.** The script-only tests and the asset-loading tests each ran 30 times in parallel without a crash. The GPU-only stress was also clean at 25 threads (25 runs), and the interpreter alone runs hundreds of parallel tests in `tests/eval.rs` cleanly.

So far it takes the interpreter and the renderer together, in many threads at once. That points at something shared between those paths when both are busy: the glTF loader threads, the HUD font rasteriser, a process-global the bisection didn't separate, or a genuine unsafe bug in the value layer that only this load exposes.

**Status: open.**
- **Next step:** run the suite under the Windows page heap (`gflags /p /enable`), which faults at the first bad access instead of at a later heap check.
- **Workaround:** `cargo test --test kernel_render -- --test-threads=1` runs clean.
- **Scope:** it has never reproduced in a game run (one interpreter, one renderer, one thread), nor in the 10 × 10-minute M4 soaks.
