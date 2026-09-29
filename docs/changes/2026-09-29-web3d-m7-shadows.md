# web3d-M7 session 6: shadows (fitted cascades, PCSS, point-light cubes)

**Date:** 2026-09-29
**Milestone:** web3d-M7 ([scope](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md), previous: [anti-aliasing](2026-09-29-web3d-m7-aa.md))
**Code:** cascade fitting, point-shadow cubes and the shadow shader in `src/kernel/render.rs`; `light.shadow` in `src/stdlib.rs`

## What changed

1. **Sun cascades fitted to the camera.** Before, the three cascades were fixed-size squares around the camera target, whatever the view. Now:
   - the view frustum from the near plane to `4 × sun.shadow_extent` is split into three slices, using the "practical" split scheme (Zhang et al. 2006: 75% logarithmic, 25% uniform);
   - each slice is wrapped in a bounding sphere, so a cascade's size doesn't change as the camera turns;
   - each cascade's window is snapped to whole shadow-map texels in a fixed light view, so shadow edges don't crawl or shimmer as the camera moves;
   - each cascade's depth range reaches `sun.shadow_extent` beyond its slice toward the sun, so casters outside the view still cast into it.
2. **Soft sun shadows (PCSS**, Fernando 2005):
   - a 16-tap Poisson blocker search finds the average depth of whatever shadows a point;
   - the gap between blocker and receiver sets the penumbra width, so shadows are sharp where objects touch the ground and soften with distance (contact hardening);
   - a 16-tap PCF filters over that width;
   - the sample disk is rotated per pixel (interleaved gradient noise), which turns banding into fine noise that TAA averages away.

   The sun's apparent size is fixed at a tangent of 0.04 (about 2.3°), a little softer than the real sun, which reads better at game scale. It is a shader constant, not script surface; it becomes a setting if a game asks for one.
3. **Point-light shadows.** `light.shadow(handle, true)` makes a point light cast shadows:
   - up to 4 lights per frame, each a 512² depth cube (6 faces), all in one `texture_depth_cube_array`;
   - each shadowed light adds six render-graph passes, one per face, so a frame with no shadowed lights pays nothing;
   - the cube uses the same face table as the environment maps, and a unit test checks that each face's camera and the cube sampler agree texel for texel;
   - lookups are 5-tap PCF with a normal offset that grows with distance.

   More than 4 shadowed lights: the first 4 in slot order cast shadows and the rest light without them.
4. **Normal-offset biasing throughout.** Receivers step off the surface by about a texel along the normal before the lookup, which removes acne without the peter-panning that a large depth bias causes.

## Scores

Unchanged: mean ꟻLIP 0.196 against Three.js 0.186. The harness scenes are lit by the environment only (the glTF reference renders have no punctual lights), so shadows don't reach the scoreboard. They are judged by the tests and by eye.

**Not done: shadows from IBL.** The environment casts no shadows. That needs ambient occlusion (screen-space, session 7) or baked AO maps, which glTF materials already carry.

## Limits

- **No spot lights.** The plan listed spot-light shadows, but Twe has no spot lights yet. They'll arrive with clustered lighting (session 12), and their shadows are a single face of the point-light path.
- **Point-light cubes re-render every frame,** even when nothing near the light moved. Caching static casters is a GPU-driven-rendering item (session 11).
- **Cascades don't blend at their boundaries.** A seam can show where the filter width jumps between cascades. Dithered cascade blending is a small follow-up if it shows in `survive3d`.
- **`sun.shadow_extent` changed meaning slightly.** It used to be the radius of the shadowed square around the target. Now shadows reach `4 × extent` from the camera, and casters up to `extent` outside the view are included. Existing scripts keep working, and their shadows reach further.

## Verification

- `cargo test`: 985 pass. New tests:
  - `cascades_are_fitted_to_the_view`: the cascade windows contain their frustum slices;
  - `cascades_move_in_whole_texels`: a small camera move shifts a cascade window by whole texels only;
  - `point_shadow_faces_match_cube_sampling`: each face's projection agrees with cube-map sampling;
  - `point_lights_cast_shadows`: a lamp beside a block darkens the floor behind it (at least 1,500 pixels), and brightens nothing;
  - `sun_shadows_are_soft`: a block 4 m above the ground casts a shadow with a penumbra of more than 150 pixels (204 measured). The penumbra grows with the sun's size, which confirms PCSS works: 96 px at a tangent of 0.02, 971 − 546 = 425 px at 0.1.
- `cargo clippy -- -D warnings`: clean for native, `wasm32-unknown-unknown` and `-p twe-web`.
- **Chrome:** a scratch web build with PCSS sun shadows, a shadow-casting point light, TAA and an orbiting camera ran without WebGPU errors. The cube-array depth texture and the PCSS shader compile under Tint. `survive3d` still loads clean.
