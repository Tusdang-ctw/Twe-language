# Phase 17 closeout — UV Textures + Mouse Look (post-v1.0)

**Date:** 2026-05-07.
**Status:** **codebase-closed** (all session goals shipped except mipmap generation).
**Roadmap reference:** `docs/3d-roadmap.md` §"Phase 17".

---

## What shipped

Phase 17 ran in three sessions:

| # | Session | Surface |
|---|---------|---------|
| 1 | vec3 math + mouse delta + cursor lock | `math.dot`/`cross`/`length`/`normalize`. `mouse.dx`/`mouse.dy` from `DeviceEvent::MouseMotion` (raw delta, not absolute). `cursor.lock()`/`cursor.unlock()` writes a pending flag drained by play3d each frame. |
| 2 | UV slot + texture pipeline | `Vertex` gains `uv: [f32; 2]` at attribute location 4. WGSL adds `@group(1)` for `texture_2d` + sampler. Fragment shader multiplies tint by `textureSample`. White 1×1 fallback ensures untextured meshes still render. New stdlib: `texture(path)` returns handle; `cube_textured`/`mesh_textured` accept a texture handle. `glb` loader reads `TEXCOORD_0`. Render flow groups draws by `(primitive, texture_id)` and binds the right group per draw. |
| 3 | glTF material auto-extraction | `parse_glb_bytes` now also returns `Option<AutoTextureData>` with the first primitive's `pbr_metallic_roughness.baseColorTexture` pre-decoded to RGBA8. `GpuMesh` carries an `Option<wgpu::BindGroup>` auto_texture slot uploaded at load time. The render flow's bind selection prioritises script-supplied texture > mesh's auto_texture > white fallback, so plain `mesh()` calls on Blender-exported textured `.glb` files render the embedded material without any extra script work. |

---

## Test count

Pre-phase: 732. Post-phase: 732. No new tests — Phase 17 changes
are GPU-side and require a real adapter to validate. Headless runs
still pass; the texture pipeline is exercised via the `fps_demo.twe`
walkthrough and the `physics_demo.twe` smoke test.

---

## What slipped

- **Mipmap generation.** v0.1 ships single-level textures with
  linear filtering. Mipmap generation requires a blit/compute pass
  to build the mip chain at upload time; deferred to a follow-on
  whenever distant-texture aliasing pressures it.
- **Anisotropic filtering.** Coupled to mipmaps (no benefit
  without). The sampler config is one feature flag away once
  mipmap generation lands; deferred together.
- **Multi-material glTF primitives.** The auto-texture extractor
  reads `material[0]` only — a `.glb` with multiple primitives,
  each with its own material, only renders the first material's
  texture across all primitives. Defers to Phase 19's full scene
  graph + node transform work where multi-primitive draw
  partitioning lands.
