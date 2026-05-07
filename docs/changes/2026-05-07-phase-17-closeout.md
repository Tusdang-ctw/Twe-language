# Phase 17 closeout — UV Textures + Mouse Look (post-v1.0)

**Date:** 2026-05-07.
**Status:** codebase-closed (MVP).
**Roadmap reference:** `docs/3d-roadmap.md` §"Phase 17".

---

## What shipped

Phase 17 ran in two commits:

| # | Session | Surface |
|---|---------|---------|
| 1 | vec3 math + mouse delta + cursor lock | `math.dot`/`cross`/`length`/`normalize`. `mouse.dx`/`mouse.dy` from `DeviceEvent::MouseMotion`. `cursor.lock()`/`cursor.unlock()` writes a pending flag drained by play3d each frame. |
| 2 | UV slot + texture pipeline | `Vertex` gains `uv: [f32; 2]` at attribute location 4. WGSL adds `@group(1)` for `texture_2d` + sampler. Fragment shader multiplies tint by `textureSample`. White 1×1 fallback ensures untextured meshes still render. New stdlib: `texture(path)` returns handle; `cube_textured`/`mesh_textured` accept a texture handle. `glb` loader reads `TEXCOORD_0`. Render flow groups draws by `(primitive, texture_id)` and binds the right group per draw. |

---

## Test count

Pre-phase: 732. Post-phase: 732. No new tests — Phase 17 changes
are GPU-side and require a real adapter to validate. Headless runs
still pass; the texture pipeline is exercised manually via
`twec play3d examples/physics_demo.twe`.

---

## What slipped (deferred to follow-on)

- **glTF material extraction.** The loader reads `TEXCOORD_0` but
  not the material's `baseColorTexture` URI. Scripts must call
  `texture("path.png")` explicitly. Real Blender export materials
  defer to a Phase 19 follow-on (full material binding alongside
  multi-primitive scenes).
- **Per-instance textures.** All instances within a draw-call
  group share one texture. Scripts wanting multiple textures per
  mesh class must split into separate `mesh_textured` calls (which
  the grouping already handles correctly — same mesh + different
  textures = two draw calls).
- **Mipmaps and anisotropic filtering.** v0.1 ships single-level
  textures with linear filtering. Mipmap generation + anisotropy
  would land alongside the eventual asset-pipeline pass.
