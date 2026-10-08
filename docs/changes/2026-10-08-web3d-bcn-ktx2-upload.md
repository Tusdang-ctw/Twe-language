# web3d: BCn KTX2 runtime upload path

**Date:** 2026-10-08
**Line:** web3d engineering-health backlog (closes an M9 deferral)
**Status:** closed — a BCn `.ktx2` uploads compressed to the GPU, verified on
real hardware.

## What this closes

M9 ([closeout](2026-10-04-web3d-m9-closeout.md)) deferred "a GPU-compressed
BCn upload path for KTX2": the build-time `twec asset transcode` turned
Basis/KTX2 into RGBA8 the runtime loaded, but a `.ktx2` whose payload is
already block-compressed (BC1–BC7) still had to be expanded to RGBA8, paying
the decode and ~4–8× the VRAM. This adds the direct path: on a GPU that
advertises `TEXTURE_COMPRESSION_BC` (desktop wgpu), such a file uploads
straight to VRAM, compressed, with no CPU decode.

## What shipped

- **`src/kernel/ktx2_bc.rs`** — `is_ktx2` (identifier sniff), `vk_bc_to_wgpu`
  (the Vulkan BC `VkFormat` → wgpu `TextureFormat` map, BC1–BC7 incl. sRGB and
  BC4/5 signed and BC6H float), `parse_ktx2_bc` (via the pure-Rust `ktx2`
  crate: rejects supercompressed / universal / non-BC files with a
  `twec asset transcode` pointer), and `upload_ktx2_bc` (one compressed
  texture, each mip written with 4×4-block-aligned `bytes_per_row`).
- **`upload_texture_bytes`** (the runtime `texture(path)` loader in
  `kernel/render.rs`) now sniffs the KTX2 identifier and takes the BC path;
  everything else still decodes through the `image` crate.
- **The device requests `TEXTURE_COMPRESSION_BC`** when the adapter has it
  (`bc_device_feature`, folded into `required_features`). Where BC is absent
  (most WebGPU-in-browser, software adapters), `upload_ktx2_bc` returns an
  explanatory error rather than a crash.
- **`ktx2` moved from an `asset-tools`-only optional native dep to a normal
  all-target dependency.** It is a pure-Rust parser (no C), so this honours
  M9's "the runtime stays pure-Rust / wasm-safe" rule; it was unused in code
  before (the transcoder reads KTX2 through `basisu`).

## Verification

`cargo test --lib ktx2_bc` — 5 tests, all green:
- `detects_the_ktx2_identifier`, `maps_the_bc_vk_formats`,
  `rejects_a_non_bc_ktx2_with_guidance` — pure unit tests.
- `parses_a_minimal_bc1_ktx2` — builds a minimal, spec-valid single-level
  BC1 KTX2 in-test (the `ktx2` crate accepts a bare 4-byte DFD) and checks the
  format / dimensions / block bytes round-trip.
- **`uploads_a_bc1_texture_on_a_real_gpu`** — on this machine's RTX it created
  a BC-capable device, uploaded a 4×4 BC1 block inside a validation error
  scope, and asserted **no validation error** + the right format/size. It
  skips cleanly where no BC adapter exists (CI / software).

Also: `cargo clippy -- -D warnings` native (`--all-targets`) + `-p twe-web
--target wasm32-unknown-unknown` both clean (the module compiles for the web
too; the compressed upload is simply gated off there at runtime).

## Deferred (honestly)

- **meshopt** geometry — still blocked upstream (the `meshopt` 0.6 crate has
  no inverse vertex filters; decoding filtered data would corrupt it).
- **A visual/sampled** confirmation of a real-world BC7 asset in a running
  scene is a GPU + content step (the test proves the upload mechanics, not a
  specific image); ETC2/ASTC (the mobile/WebGPU compressed families) are a
  separate future path.

## Doc edits applied

- `src/kernel/ktx2_bc.rs`, `src/kernel/mod.rs`, `src/kernel/render.rs`,
  `Cargo.toml` — the code + dep.
- `docs/changes/2026-10-04-web3d-m9-closeout.md` — inline `UPDATE` on the
  deferral (history preserved).
- `CHANGELOG.md`, `docs/journey.md` — entries added.
