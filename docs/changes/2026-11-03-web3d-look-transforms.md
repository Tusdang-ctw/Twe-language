# Look `rotation:`, per-axis `scale:` and `glow:` (Lanternfall E3)

**Date:** 2026-11-03
**Line:** Lanternfall engine enablers, E3 (third of E1–E6)
**Status:** shipped.

## Why

A look could only be placed, turned about +Y and scaled uniformly
(web3d-M3). The battle royale's effects need more:
- **Tracers:** thin beams pointed along the shot (pitch plus a
  non-uniform scale).
- **Debris:** voxel chunks that tumble (full rotation).
- **The zone:** a cylinder wall scaled `(r, h, r)`.
- **Juice:** squash and stretch on landing and on hits.
- **Hit flashes:** every character lights up when struck.

None of these could be expressed. A `visual` material can't read entity
state either, so a flash had to be a material swap.

## What ships

- **Three new keys in `look:`:**
  - `scale:` takes a number (uniform, as before) or a `vec3` (per axis).
  - `rotation:` takes a `quat`, composed after `facing`, so the transform
    is `yaw(facing) · rotation · (scale ⊙ v)`.
  - `glow:` takes a number (white) or an `(r, g, b)` colour, added as
    emission in the `emission:` convention: sRGB up to 1, HDR above (it
    blooms). Negative values clamp to 0.
- **Two quat builders.** `quat.from_euler(pitch, yaw, roll)` (yaw about +Y
  as `facing`, then pitch, then roll) and `quat.look_along(dir)` (turns +Z,
  a mesh's front, to `dir`, keeping +Y up). Both join `identity`,
  `from_axis_angle`, `mul`, `slerp` and `to_mat4`. `quat.*` is now
  documented in docs/06 §7.2 (it wasn't before).
- **Wider GPU instances, 48 → 80 bytes.**
  - Fields: `position + bounding scale | colour | orientation quaternion |
    per-axis scale + cull group | glow + skinned flag`.
  - `instance_of` builds it from a `DrawCall3d`. That struct gains
    `rotation` (`NO_ROTATION` default), `scale` and `glow`.
  - The two flags that lived in the yaw vector moved: the GPU-cull group to
    `scale.w`, the skinned flag to `glow.w`.
- **Every pass shares one transform.** `yaw_rotate` is replaced with
  `quat_rotate(rot, v · scale)`, and normals are divided by the scale.
  This applies in all three vertex shaders: the main `vertex_out` (so the
  material and displacement variants too), the shadow / prepass
  `world_vertex`, and the alpha-mask prepass.
- **CPU side.** Culling spheres use the largest axis, and the transparency
  sort's `world_point` uses the quaternion. The GPU-cull WGSL struct and
  `INSTANCE_BYTES` were updated; a compile-time assert ties the Rust struct
  to that size.
- **Glow is added once**, in the shared `shade()`, so every lit surface
  (glTF, plain script draws, `visual` materials) flashes the same way.
- **Errors name the fix.** For example, `rotation: 5` says to build a quat
  with `quat.from_euler` / `quat.look_along`.

## Cost

32 more bytes per instance uploaded, which is negligible for a game with
hundreds to low thousands of instances. The 100k-cube stress scene
uploads about 3.2 MB more per frame when nothing is retained; it hasn't
been re-measured here (it needs the GPU bench).

## Tests

- **`tests/kernel_render.rs` (GPU):**
  - `look_rotation_and_per_axis_scale`: a 6 × 0.4 bar lies along x;
    turned by `rotation:` it lies along z and matches a `facing:` quarter
    turn pixel for pixel.
  - `look_glow_lights_up_a_dark_mesh`: an unlit black cube glows orange.
  - The existing 45 renderer tests still pass, including GPU culling,
    skinning, shadows, materials and displacement.
- **`tests/look_transforms.rs` (no GPU):**
  - defaults;
  - vec3 versus number `scale:`;
  - `glow:` forms;
  - per-entity glow following each entity;
  - `from_euler` yaw equals `from_axis_angle`;
  - `look_along` points the front;
  - error help text.
