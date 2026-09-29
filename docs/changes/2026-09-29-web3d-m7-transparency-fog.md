# web3d-M7 session 9: translucency and height fog

**Date:** 2026-09-29
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [post, part 2](2026-09-29-web3d-m7-post2.md))
**Code:** the transparent pass, fog uniform and shader terms in `src/kernel/render.rs`; `light.fog` in `src/stdlib.rs`

## What changed

1. **Translucency.** Two things now draw translucent: a draw colour whose alpha is below 1, and glTF primitives with `alphaMode: BLEND`. Before, both drew opaque.
   - **No new syntax.** Script colours already carried an alpha, and 2D draws already honoured it. No shipped 3D script draws with alpha below 1, so no scene changes.
   - **Drawn last, sorted.** Translucent draws leave the opaque instance groups and become one draw each, after the opaque scene and the backdrop. They are sorted back to front by distance from the eye. A glTF primitive sorts by its own centroid, computed at load, so the panels of one model sort among themselves.
   - **Blending.** Alpha blending over the frame, depth tested but not written, inside the MSAA main pass. Double-sided materials draw back faces, then front faces.
   - **Where the alpha comes from.** A BLEND material's base-colour alpha; otherwise the draw's tint alpha. The whole shaded colour is blended, as Three.js and Cycles do.
   - **Kept out of the other passes.** Translucent draws skip the AO prepass. Translucent script draws also skip the shadow passes. BLEND primitives of opaque meshes still cast shadows, as in Three.js.
2. **Exponential height fog** (Quilez, "better fog"). `light.fog(density, falloff, color)`.
   - Density `a·e^(-b·y)` is integrated along each view ray in closed form, so a view up out of a valley clears while one across it doesn't. It is exact per pixel, not a depth ramp.
   - The fog is lit by its colour plus a forward-scattering glow toward the sun.
   - It is applied in the shared `shade()`, so opaque, translucent and `visual`-material surfaces all fog.
   - The backdrop fogs too: the environment map, or the background colour when there is none. It uses the same integral out to the far plane.
3. **Plan change: weighted-blended OIT moves to session 13.** The session plan listed order-independent transparency as the fallback for sorting. Its real use is particle clouds, where per-object sorting is hopeless, so it moves to session 13 (GPU particles). There, `particles` blocks will draw through it.

## Scores

Mean ꟻLIP against Cycles; lower is better.

| | Session 8 | Session 9 | Three.js |
|---|---:|---:|---:|
| AlphaBlendModeTest | 0.1248 | **0.1125** | 0.1071 |
| Mean, 12 scenes | 0.1850 | **0.1839** | 0.1857 |

- **AlphaBlendModeTest** now looks like Three.js's render.
- **What remains there is the reference's reading.** The Cycles golden blends even the panels whose glTF alpha mode is *mask*, a quirk of how Blender imports glTF alpha, so no engine that follows the spec matches it closely. Twe and Three.js agree with each other, not with the golden, on those panels.
- **Fog doesn't enter the score:** the references have none.

## Limits

- **Sorting is per surface, not per pixel.** Intersecting translucent surfaces blend in the wrong order where they overlap. Session 13's OIT is the answer for particles; for large intersecting glass, it is a known limit.
- **Translucent script draws cast no shadows** and don't occlude ambient light. Coloured (tinted) transmission shadows are out of scope until glTF transmission (session 10).
- **A draw with a `visual` material stays opaque** whatever its alpha. The material pipelines have no blended variant; add one if a game needs it.
- **Fog is anchored at y = 0** with a single colour. There is no volumetric scattering or light shafts (session 14).

## Verification

- `cargo test`: 1003 pass (999 before, +4). New tests:
  - `translucent_draws_blend_back_to_front`: green in front of red, drawn by the script in the wrong order, composites green over red. An opaque front block hides the back one, and beside it only the back block shows.
  - `gltf_blend_materials_are_translucent`: a half-transparent emissive blue BLEND quad lets the background through.
  - `height_fog_thickens_with_distance_and_depth`:
    - a near block is barely fogged;
    - a far block at ground level is fogged, and one high up much less;
    - the backdrop fogs fully looking down, and clears looking up.
  - `light_fog_setter` in `tests/eval.rs`.
- `cargo clippy -- -D warnings`: clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:** a scratch web build with translucent shapes, height fog with the sun's glow, and TAA, under an orbiting camera, ran without WebGPU errors. `survive3d` still loads clean.
