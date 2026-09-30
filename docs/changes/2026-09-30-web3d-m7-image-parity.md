# web3d-M7 follow-up 1: image parity with Three.js, scene by scene

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([closeout](2026-09-30-web3d-m7-closeout.md), which found exit criterion 1 unmet)
**Code:**
- `src/kernel/render.rs`: tangent frame, energy conservation, diffuse environment light, iridescence, transmissive depth;
- `src/kernel/environment.rs`: specular mip spacing and resolution;
- `src/kernel/ssr.rs`: hit thickness, LOD.

**Tools:** `bench/graphics/closeup.py` and `worst.py`. They crop a region of a scene and rank blocks by how much worse Twe's ꟻLIP error is than Three.js's, with the mean colours of the reference, Twe and Three.js in each.

## Result

Mean ꟻLIP against Blender Cycles, lower is better:

| | Before | After | Three.js |
|---|---:|---:|---:|
| Twe (AO and SSR on, as the harness runs it) | 0.1774 | **0.1655** | 0.1857 |
| Twe like for like (AO and SSR off) | 0.1911 | **0.1783** | 0.1857 |
| Scenes where Twe scores at or below Three.js | 6 of 12 | **12 of 12** | — |

**Like for like,** Twe is ahead on 10 of 12. It trails on FlightHelmet (0.0570 vs 0.0548) and ClearCoatTest (0.0790 vs 0.0754), where its lead comes from AO and SSR.

**Twe now leads every renderer** in the Khronos table on 9 of 12 scenes, and on the mean. model-viewer and the glTF Sample Viewer still lead on EmissiveStrength, TextureTransform and AlphaBlendMode.

## What was wrong

Each fix came from looking at the scene where Twe lost most, not from tuning. They're listed in the order found.

1. **Diffuse light ignored what the specular layer reflects.**
   - Environment and direct diffuse were added in full on top of specular, so every surface was a few percent too bright.
   - Diffuse is now scaled by `1 − max(specular albedo)` for environment light (as Three.js does) and by `1 − F` for direct light (glTF's `fresnel_mix`).
   - Mean 0.1776 → 0.1729. EmissiveStrengthTest and AlphaBlendModeTest moved ahead of Three.js.
2. **The derived tangent frame was upside down in one axis.** Models without vertex tangents build their tangent frame from screen derivatives.
   - WGSL's `dpdy` runs down the screen and GLSL's `dFdy` up, which turns the whole frame over.
   - glTF's v axis running down the texture turns the bitangent back (Three.js flips it for glTF, three.js issue 11438).
   - Net: the tangent must be negated. Without it, DamagedHelmet's visor reflections bent the wrong way.
   - DamagedHelmet 0.0809 → 0.0594, past Three.js's 0.0615. Flipping the bitangent instead, the obvious guess, made it worse (0.0826).
3. **Screen-space reflections accepted hits 25 cm behind the depth buffer.**
   - Rays passing behind a flight helmet's thin lens rims counted as hitting them: white streaks at silhouettes.
   - The allowance is now 2% of the view distance plus the step.
   - The environment SSR subtracts now uses the same LOD as the surface shader.
4. **A thin film was evaluated over the blended F0.**
   - For a half-metallic, iridescent material the film sat over an F0 around 0.5 and showed strong pink and green.
   - Now evaluated over the dielectric base and over the albedo, then blended by metalness, as Three.js does. The film's Fresnel reaches the split sum as an equivalent F0 (Schlick inverted).
   - IridescenceLamp 0.1036 → 0.0982.
5. **Diffuse environment light came from 9-coefficient spherical harmonics.**
   - The SH was checked and is correct Lambertian irradiance (projection, basis order, cosine-lobe band factors).
   - Cycles is darker than pure Lambert under these HDRIs, and the SH overshoots around bright softboxes.
   - Now the roughness-1 level of the prefiltered environment, as Three.js does. Every scene improved: mean 0.1695 → 0.1659, and TextureTransformTest moved ahead.
   - The SH stays for the volumetric fog's sky light.
6. **Transmissive glass was drawn blended, without writing depth.**
   - The lamp's globe is two shells. Both sort alike, so the far inner wall (a concave mirror, reflecting the room upside down) could draw over the near outer wall.
   - Transmissive, non-blended primitives already carry what's behind them, so they now draw like opaque surfaces, depth written.
   - IridescenceLamp 0.0967 → **0.0888**, the last scene behind Three.js.

## Also changed

- **The prefiltered specular environment** is 512 per face (was 256); mip 0 is a copy of the source (was a downsample).
- **Its 7 mips** hold roughness (i / 6)², read at `sqrt(roughness) · max_lod`. Linearly spaced, a roughness-0.1 surface blended the mirror half-and-half with the roughness-0.2 lobe; Three.js's PMREM likewise spends its levels on low roughness.
- **Measured and reverted:**
  - the specular dominant direction (`mix(r, n, roughness²)`): better helmets, worse transmission and iridescence;
  - Karis's analytic DFG in place of the integrated table: worse overall;
  - excluding the film from the transmitted light's attenuation: no change.

## Verification

- `cargo test`: 1051 pass. `tests/kernel_render.rs` (38 render tests) passes unchanged.
- Clippy (`-D warnings`) is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- The suite was re-rendered and scored in both configurations; `results.md`, `results.json` and `comparison.jpg` are regenerated.
