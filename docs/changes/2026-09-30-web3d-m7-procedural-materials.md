# web3d-M7 session 15: procedural surface materials

**Date:** 2026-09-30
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [SSR and volumetric fog](2026-09-30-web3d-m7-ssr-volumetric-fog.md))
**Code:**
- `src/visual_check.rs`: the method set, `material(...)`, the new GPU-safe calls, types through the codegen;
- `src/visual_wgsl.rs`: type inference, `surface` / `displace` codegen, `if` expressions;
- `src/kernel/render.rs`: the material fragment and displacing vertex stages, and the displaced depth pipelines;
- `src/verify.rs`: `visual-error` diagnostics.

**Example:** [`examples/procedural_materials_3d.twe`](../../examples/procedural_materials_3d.twe), a courtyard with every surface written in code, plus a dense grid mesh for displacement (`examples/assets/grid_plane.glb`, made by `grid_plane.py` beside it).

This is a language change, so this note is also its design record (M7 plan rule 3).

## Why

Procedural materials are M7's Tier-2 headline: a game's look written in code, with no texture assets. That is Twe's use case 3 and Principle 1 (`visual` is a core block). Three.js has nothing equivalent at the language level.

Before this session a `visual` used as a material could set only its colour: a lit, rough, non-metal surface. Every other channel of the PBR model the renderer now has (sessions 3–10) was out of a script's reach.

## The design

A `visual` block gets two methods beside Example 5's `pixel` (docs/06 §4.9):

```twe
visual Lava:
    surface(uv, time, pos, normal) -> material:
        let crack = 1 - smoothstep(0.0, 0.07, math.abs(noise((pos.x, pos.z) * 3)))
        return material(albedo: (0.07, 0.06, 0.06), roughness: 0.9, emission: (1.0, 0.35, 0.05) * crack * 4)

    displace(uv, time, pos, normal) -> vec3:
        return normal * noise((pos.x, pos.z) * 4) * 0.06
```

### No new syntax

Methods, `-> type` annotations and named arguments all parse today. The change sits entirely in the visual checker and codegen, so:
- the parser, printer, AST JSON, tree-sitter and GBNF grammars are unchanged;
- no keyword is added (the open "keyword pruning" item is untouched);
- `material` is not a stdlib builtin. It exists only inside a `surface` body, like `color.red`'s inlining.

### Decisions, and the alternatives turned down

1. **One method returning every output, not one method per channel.**
   - **Rejected:** separate `roughness(uv, time)`, `normal(...)` and so on. Channels share work (one noise sample drives albedo, roughness and emission), and per-channel methods would repeat it or need common-subexpression elimination.
   - **Chosen:** `material(albedo:, normal:, roughness:, metalness:, emission:)`, with named arguments only and all optional. Each output is named at its use site, so an LLM can't swap roughness and metalness positionally (Principle 4). Unknown names get a did-you-mean.
2. **`pixel` or `surface`, not both.** Principle 2: a material's colour has one home.
   - `pixel` stays exactly as Example 5 fixes it (`pixel(uv, time) -> color`). It remains the fullscreen entry, and as a material it is shorthand for `material(albedo: <pixel>)`.
3. **`material(...)` only as `surface`'s return value**, never bound with `let`. This keeps the value out of the rest of the subset: no material arithmetic, no fields to read back.
4. **Inputs are positional and fixed in order, `(uv, time, pos, normal)`, and trailing ones may be left off.**
   - `surface(uv, time)` reads like `pixel`.
   - The order is the same for both new methods.
   - Leaving inputs off avoids forcing unused parameters.
5. **World space for `pos` and `normal`.**
   - World-space patterns tile seamlessly across meshes (floors, terrain, water) and are what the plan asked for.
   - Patterns that should travel with a moving mesh use `uv`. The docs say which to use for what.
   - Object space was considered; it would be a fifth input if an example ever needs it.
6. **Displacement is a world-space `vec3` offset**, not a scalar along the normal.
   - Waves displace in y whatever the mesh's normals; a scalar is `normal * h`.
   - One return type, so there's no second form.
7. **Colours are sRGB, as everywhere else in Twe.**
   - Before this session a material's `pixel` colour went to the lighting undecoded, while tints were decoded, so the same `(0.5, 0.5, 0.5)` came out brighter as a material than as a tint. It's fixed: albedo and emission are decoded like tints.
   - Emission may exceed 1. Its brightest channel above 1 is an intensity, and the colour under it is decoded, so `color.orange * 4` glows orange rather than shifting hue.
8. **Types are checked at verify time.** The WGSL codegen now infers types: numbers are `f32`, tuples `vecN`, calls follow the WGSL builtin. This lets it:
   - splat a number where WGSL wants a vector (`clamp(v3, 0, 1)`, `mix` branches, `if` expressions);
   - coerce outputs (`(r, g, b)` gains alpha 1; a number is a grey);
   - report a wrong shape as a visual error.

   `visual_check` runs the codegen once the structure checks pass, so these reach `twec verify` rather than first appearing when a look draws.

### Smaller language changes

- **More GPU-safe calls.** `math.clamp`, `mod`, `atan2`, `dot`, `cross`, `length` and `normalize` join the list. They are existing CPU builtins with the same meaning: `mod` is Euclidean (`a − |b|·floor(a/|b|)`), and `normalize` leaves a zero vector alone.
- **`if` expressions compile** (to nested `select`). The checker already allowed them; the codegen used to fail on them.
- **Unknown visual method names are an error.** They were silently ignored.
- **`twec verify` reports visual problems** as `visual-error`. `docs/06` §4.9a already claimed it did; it didn't. Before this, only running the program surfaced them.

## The renderer

- **Surface.** Every material now compiles to `twe_surface(uv, time, pos, normal) -> TweSurface`. A `pixel` visual is wrapped in one.
  - `fs_material` calls it with the geometric normal (flipped on back faces).
  - It cuts out albedo alpha below 0.5 and falls back to the mesh normal if the output normal is zero.
  - It feeds the same `surface_plain` → `shade` path as every other surface, so image-based lighting, shadows, clustered lights, AO, SSR and fog all apply.
- **Displacement in the main passes.** A displacing material gets `vs_material`, which calls the shared vertex body and moves the world position.
  - The normal is rebuilt by sampling `displace` at two points 1 cm away along the surface: (p + ε·a + D(p + ε·a)) − (p + D(p)), crossed with the same along b.
  - That tracks displacement that varies with `pos`. Displacement driven only by `uv` keeps the mesh's normal; that's a documented limit.
- **Displacement in the depth passes.** Sun cascades, point-light faces and the ambient-occlusion prepass get per-material depth pipelines (`vs_shadow_displaced`, same culling and bias as each plain one), built on first use beside the colour pipelines.
  - The shared depth-pass uniform gained `params.x` = simulation time, so animated displacement casts matching shadows.
  - A test proves it: the shadow of a cube displaced out of view leaves the floor under its old position lit. Turning the displaced depth pipelines off makes that test fail (floor 35 instead of 168).
- **The kernel contract.** The kernel still knows nothing of Twe syntax. The contract is WGSL: a material defines `twe_surface`, and `twe_displace` when it displaces (`material_displaces`).

## Numbers

**Frame cost** (`tests/render_bench.rs`, `render_procedural_materials`): 300 cubes and a floor, top-down, 1280×720, ms per frame. Every draw uses the given surface.

| | Intel UHD | RTX 3050 Ti |
|---|---:|---:|
| plain surface | 4.8 | 0.56 |
| `surface` material (tiles, grout, noise) | 2.3 | 0.28 |
| the same + `displace` (3 noise taps per vertex) | 2.3 | 0.44 |

The procedural surface is **cheaper** than the plain one. The plain surface is the glTF material path, which reads nine textures (1×1 white ones for a script cube), builds a tangent frame from derivatives and evaluates the extension lobes. A procedural material reads no textures.

The comparison repeated identically, with a second plain run after the others. It points at a follow-up: a texture-free variant of the plain pipeline for script-drawn primitives would roughly halve their cost on the integrated GPU.

**Image benchmark.** No change. Its glTF scenes don't use `visual` materials, and Three.js has no procedural-material equivalent to score against. The feature is covered by render tests instead (below).

## Limits

- **Culling uses undisplaced bounds.** Frustum and hi-Z culling use the mesh's own bounds, so a large displacement can pop out at the screen's edge.
- **Only vertices move.** Detail is limited by the mesh (the built-in sphere is 16 × 24 segments; a cube has 8 corners), and there is no tessellation.
- **Normals follow `pos`-driven displacement only.** A `uv`-driven one keeps the mesh's normals.
- **Cut-outs don't reach the depth passes.** A material's alpha cut-out isn't applied in shadows or AO (unchanged from M3).
- **No textures, loops or per-entity parameters inside a visual.**
  - A look can pick among visuals (`material: surface_field`) but can't pass a visual a value.
  - A material uniform for per-entity parameters is the natural next step if `survive3d` needs one (session 16).
- **Translucent materials.** A draw with a visual material stays opaque (unchanged).

## Verification

- **`cargo test`: 1046 pass** (1028 before, +18):
  - **Checker** (`tests/visual_check.rs`, +10): surface + displace accepted; trailing inputs optional; unknown output with its suggestion; `surface` must return `material(...)`; `material` elsewhere, positional or twice; `pixel` with `surface`; unknown methods; type errors via the codegen; named arguments elsewhere.
  - **Codegen** (`tests/visual_wgsl.rs`, +4):
    - kernel material and depth shaders validate in naga for a material using every construct (`if`/`elif` expressions, `mod`, `normalize`, `clamp`, coercions);
    - the Example 5 fire still compiles to an identical snapshot and validates as a material;
    - output coercion and defaults;
    - `math.mod` is Euclidean.
  - **Render** (`tests/kernel_render.rs`, +3):
    - **`procedural_surface_outputs`:** albedo colours, emission glows, a turned normal darkens.
    - **`displacement_moves_geometry_and_its_shadow`:** a displaced cube moves on screen, and its shadow and AO move with it. Mutation-checked, as above.
    - **`procedural_materials_example_renders`:** the example renders.
  - **Verify** (`tests/verify_v2.rs`, +1): visual problems are reported as `visual-error`, and the example is clean.
  - The in-crate test that compiles every example's visual now also validates displacing materials' depth shaders.
- **Clippy** (`-D warnings`) is clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:**
  - `examples/procedural_materials_3d.twe`, built for the web, shows the tiled floor, the rippling pool reflecting the crystal, the gold sphere reflecting the tiles, glowing lava cracks, and the spiked crystal with its matching shadow. There are no WebGPU errors or warnings.
  - `survive3d` still renders.
