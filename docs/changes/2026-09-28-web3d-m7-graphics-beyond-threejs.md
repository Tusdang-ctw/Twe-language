# Design change: M7, graphics beyond Three.js

**Date:** 2026-09-28
**Decided by:** the maintainer: *"This language should be better and more advanced than Three.js … I don't care how long it will take."*
**Amends:** [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md): the v1.0 definition, the milestone table, and the "out of scope this year" list. The 12-month capacity constraint no longer bounds this line of work.

## Why

The pivot plan was scoped for performance and one playable slice. None of its exit criteria measured image quality, and on quality Twe's renderer is today **behind** Three.js.

| Area | Twe (end of M3) | Three.js |
|---|---|---|
| Materials | Lambert / Blinn-Phong colour + texture; procedural `visual` materials | physically based (metal-rough, clearcoat, sheen, transmission, iridescence …) |
| Environment light | ambient colour | HDR image-based lighting (prefiltered) |
| Surface maps | base colour only | normal, occlusion, emissive, roughness / metalness |
| Anti-aliasing | none | MSAA, FXAA / SMAA, TAA |
| Shadows | 3-cascade maps, PCF | cascaded / PCF-soft / VSM; point and spot shadows |
| Post | HDR, ACES, single-pass bloom, vignette | effect composer: bloom, SSAO, SSR, depth of field, … |
| Transparency, fog | alpha cut-out only; none | both |

Three.js also ships a WebGPU renderer, so "we use WebGPU" is not by itself an advantage. The claim has to be earned and measured.

## Decision

1. **A new milestone, M7: graphics beyond Three.js**, sequenced **right after M4** and **before M6**.
   - M4 keeps its exit (a playable `survive3d` run on itch.io); that is now an alpha.
   - M6 (retire macroquad) follows M7.
2. **v1.0 is redefined:** `survive3d` at 60 fps in Chrome and natively **with M7's exit criteria met**. The public v1.0 release waits for M7.
3. **Language surface grows only through examples** (the project's standing rule). Every new author-facing key or block, such as more `look:` material keys or light declarations, lands with a design note, an example that needs it, `docs/06` updates, and the full tool integration that `look:` had (parser → printer → verify → grammar export → editors → primer). Engine work that needs no new syntax needs no note.

## "Better than Three.js", made testable

Three.js is a library; how its output looks depends on assets and effort. So the claim is defined against **pinned references**:
- **Three.js** at the release current when M7 opens, recorded in the M7 closeout, using its best comparable settings (WebGPU renderer where it helps, physical materials, PMREM environment lighting, its post-processing).
- **Ground truth:** the Khronos glTF Sample Viewer's reference renders and path-traced references where available.

**`bench/graphics/`** holds the reference scene suite, rendered by Twe and by Three.js with identical assets, cameras, environments and exposure:
- glTF Sample Assets: DamagedHelmet, FlightHelmet, Sponza, and the extension tests (ClearCoatTest, TransmissionTest, SheenChair, IridescenceLamp, AnisotropyBarnLamp, EmissiveStrengthTest, TextureTransformTest, AlphaBlendModeTest);
- a Twe-specific stress scene: a `survive3d` arena with 100k instances, 500 lights and 1M particles.

A headless harness renders each scene in both engines, stores the images, and computes **ꟻLIP** error against the ground truth. Frame times come from Chrome on the reference laptop (2023 mid-range, per the pivot note).

## Scope

### Tier 1: parity (Twe matches the best of Three.js)

- **Physically based shading:** glTF 2.0 metallic-roughness (Cook-Torrance GGX, energy-conserving multiscatter), with normal, occlusion, emissive and metal-rough maps, texture transforms, and vertex colours.
- **glTF extensions:** `KHR_materials_clearcoat`, `_transmission`, `_volume`, `_sheen`, `_iridescence`, `_anisotropy`, `_emissive_strength`, `_ior`, `_specular`, `KHR_texture_transform`; plus `KHR_texture_basisu` / KTX2 and meshopt compression for assets.
- **Image-based lighting:** HDR environment maps (`.hdr` / KTX2), GPU-prefiltered specular mips, spherical-harmonic irradiance, and a BRDF lookup table; sky and environment as a scene setting.
- **Anti-aliasing:** 4× MSAA, plus temporal AA (TAA) with motion vectors.
- **Shadows:** cascade fitting to the view frustum, soft contact-hardening shadows (PCSS), and point and spot light shadows.
- **Post-processing:**
  - ambient occlusion (GTAO);
  - physically based multi-level bloom;
  - exposure (manual and automatic);
  - tonemapping (ACES, AgX, Khronos PBR Neutral);
  - colour-grading LUTs;
  - depth of field;
  - motion blur.
- **Transparency and atmosphere:** order-correct blended transparency (sorted, with weighted-blended OIT as the fallback) and exponential height fog.

### Tier 2: beyond (what Three.js does not make first-class)

- **GPU-driven rendering:** compute culling (frustum plus hierarchical-Z occlusion) and indirect draws, so draw calls stop growing with instance count.
  - Target: 100k animated instances at 60 fps on the reference laptop, in a scene where Three.js (instanced) falls below 60.
- **Clustered forward lighting:** hundreds of dynamic lights with shadows budgeted.
- **GPU compute particles:** 1M particles with collision against the depth buffer, driven from `particles` blocks (a core Twe construct).
- **Screen-space reflections, and volumetric fog with light shafts.**
- **Procedural materials as a first-class language feature:** `visual` blocks gain the full surface model:
  - albedo, normal, roughness, metalness and emission outputs;
  - vertex displacement;
  - time and world-space inputs.

  So a game's look can be written in code, with no texture assets. This is Twe's headline differentiator (use case 3 in the README) and has no Three.js equivalent at the language level.
- **Stretch, only once the rest lands:** meshlet / visibility-buffer rendering, and Gaussian-splat scenes.

## Exit criteria (all required)

1. **Parity:** every Tier 1 feature ships, and each glTF test scene renders with **ꟻLIP error at or below Three.js's** against the same ground truth, or equal within noise, recorded per scene.
2. **Beyond:** every Tier 2 feature (not the stretch items) ships. The 100k-instance, 500-light and 1M-particle stress scene holds **60 fps in Chrome** on the reference laptop, and the equivalent Three.js scene is measured and published alongside.
3. **The game uses it:** `survive3d` ships at 60 fps with physically based materials, image-based lighting, anti-aliasing, AO and bloom on. At least one of its looks is authored purely as a procedural `visual` material.
4. **Native parity:** the same scenes render identically on native, with a headless render test per feature in `tests/kernel_render.rs`.
5. **Published:** the comparison images, error numbers and frame times are published with the M7 closeout. If Twe loses a comparison, the note says so.

## Consequences for the plan

- **Milestone order:** M4 → **M7** → M6, with M5 still running in parallel.
- **"Out of scope this year":** hierarchical-Z culling moves **in** (M7 Tier 2). Meshlets, the visibility buffer and Gaussian splats become M7 stretch goals. Networking, MMO, consoles and mobile remain out.
- **Pivot constraints:** the "GPU-driven pieces only when a profile demands it" constraint is lifted for M7, and the 512 MB resident-VRAM cap still applies to `survive3d`.
- **Renderer structure:** it will need a render graph (passes declared with their resources, so the pass count stays manageable). That's the first task when M7 opens, with its own design note.
