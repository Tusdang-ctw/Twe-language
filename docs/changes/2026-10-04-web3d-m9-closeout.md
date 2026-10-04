# web3d-M9 closeout — asset pipeline parity

**Closed:** 2026-10-04 (opened 2026-10-04). Plan: [`2026-10-04-web3d-m9-plan.md`](2026-10-04-web3d-m9-plan.md). Line: creation-parity (M8–M12), [`2026-10-04-creation-parity-plan.md`](2026-10-04-creation-parity-plan.md).

> **The milestone, in one line:** a glTF that Three.js's `GLTFLoader` loads, Twe loads too — compressed textures and geometry included.

## What shipped (against the exit criteria)

| Exit criterion | Status |
|---|---|
| Morph targets sampled | **Done** — CPU pre-blend, every pass, weights from glTF default + animation |
| Multi-skin | **Done** — all skins combined into one joint array (≤128 joints) |
| `KHR_materials_anisotropy` applied | **Done** — anisotropic GGX (direct) + IBL bent-normal, gated |
| KTX2/Basis textures render | **Done** — build-time transcode → RGBA8 PNG (`basisu`) |
| Draco geometry loads | **Done** — build-time transcode → plain glb (`draco-gltf`) |
| meshopt geometry loads | **Honest deferral** — the crate lacks the inverse filters (see below) |
| `twec asset info` + VRAM estimate | **Done** — session 1, + a build-time budget audit (session 8) |
| Studio `AssetBrowser` preview | **Out of this repo** — lands in `twe-engine`, consumes `asset info --json` |

Nine sessions; details and file anchors in the plan. The headline shape:

- **Sessions 1–4 (engine features, runtime, default+wasm):** the inspector, morph targets, multi-skin and anisotropy ship on the default runtime, each unit-tested + naga-validated where GPU-bound, with the skinned-hero headless-GPU render (`tests/survive3d.rs`, 4/4) and the main-shader naga validation as regression guards. Clippy clean native + wasm32.
- **Sessions 5–8 (compression, build-time):** per the maintainer's locked decision, the compression trio lands as **build-time transcode behind the `asset-tools` feature** — `twec asset transcode` emits a plain `.glb` the runtime loads unchanged. The decoder crates (draco-gltf, ktx2, basisu, meshopt) are referenced only from `src/asset_transcode.rs` and compile only under the feature, so **the default runtime and the wasm32 browser build stay pure-Rust** (verified: both build and clippy clean without the feature). Draco + KTX2 are fully decoded and verified against real Khronos samples; meshopt errors honestly.

## Verification

- **Draco:** `transcodes_draco_box_to_plain` decodes the committed Khronos **Box-Draco** and asserts the runtime-loaded vertex count equals the plain `Box.glb`, with the extension stripped.
- **KTX2:** `transcodes_ktx2_texture_to_png` embeds a real **32×32 Basis-UASTC KTX2** (CarConcept `Rib_N.ktx2`) in a minimal glb, transcodes it, and confirms a 32×32 PNG the runtime loads.
- **meshopt:** `meshopt_pass_errors_honestly` — the pass refuses rather than emit corrupt geometry.
- **Morph / multi-skin / anisotropy:** pure-logic unit tests (`blend_morph`, `sample_morph_weights`, `parse_glb_captures_morph_targets_and_track`, `parse_glb_combines_multiple_skins`, `anisotropy_extension_read_from_gltf`) + the `MaterialUniform` size assertion + `main_shader_parses_and_validates` (naga) + the survive3d headless-GPU render.
- **Build audit:** `audit_warns_about_draco_and_vram`.
- Committed fixtures: `examples/assets/morph_quad.glb` (generated), `examples/assets/compressed/{Box.glb, Box_draco.gltf, Box.bin, sample.ktx2}` (Khronos, CC0).

## The meshopt deferral (explicit)

`EXT_meshopt_compression` is **not** transcoded. The `meshopt` 0.6 crate exposes `decode_vertex_buffer` / `decode_index_buffer` but not the inverse vertex **filters** (octahedral / quaternion / exponential) or index-sequence decode. gltfpack routinely filters normals and tangents, so decoding without the filters would silently produce corrupt geometry — a Principle-3 ("no silent footguns") violation. The pass errors with that explanation instead. Re-entry: port the three inverse filters into a pure-Rust `asset-tools` module (small, well-specified), or adopt an upstream crate that exposes them. Draco covers the compressed-geometry need in the meantime.

## Honest deferrals (need the maintainer's GPU / tools)

These are code-complete and headlessly verified; what remains is a human with a GPU (and, for one, Blender + Three.js):

- **Visual confirmation** of the morph animation, a 2-skin scene, and the anisotropic highlight — code + naga + headless-render pass here, but pixels were not eyeballed.
- **The AnisotropyBarnLamp ꟻLIP re-score** — the anisotropic BRDF uses the standard Filament/Khronos formulation; exact match to the Khronos reference image is unmeasured here.
- **The session-9 parity artifact** — a Blender export with Draco + KTX2 rendered in Twe *side-by-side with Three.js*. The transcode is instead verified against real Khronos compressed samples (reproducible, stronger than a one-off export); the side-by-side render is the maintainer step.
- **meshopt** (above) and a **GPU-compressed BCn upload path** for KTX2 (RGBA8 PNG renders correctly today; BCn would save VRAM).
- **Per-instance / per-node morph routing** and **>128 combined joints** stay out of scope.

## Gates

`cargo build` + `cargo clippy --all-targets -- -D warnings` clean on: default, `--features asset-tools`, and `--target wasm32-unknown-unknown`. Default lib suite 559 pass; the new tests (asset_info, asset_transcode under the feature, gen_morph_quad, build audit, render-kernel morph/skin/anisotropy) green.

## Next

M9 closes. Per the creation-parity order (M8 → M9 ∥ M10 → M11 → M12), **M10 (Studio maturity)** is the sibling track — much of it in `twe-engine` (debugger, profiler, one-click publish, live editing), with the asset-info `--json` contract now in place for its `AssetBrowser`.
