# 3D pipeline

Twe ships a complete 3D rendering pipeline driven from script.
Phase 17–26 layered the features — see
[3D roadmap](./3d-roadmap.md) for the original plan, and the
closeout notes in
[`docs/changes/`](https://github.com/Tusdang-ctw/Twe-language/tree/main/docs/changes)
for what shipped versus deferred per phase.

## Stack at a glance

| Layer | Crate | What it does |
|-------|-------|--------------|
| Window + raw input | winit 0.30 | Mouse delta, cursor lock, key events |
| GPU pipeline | wgpu 22 | wgpu-core + WGSL shaders |
| Geometry | gltf 1.4 | Multi-node scene flatten + skin + animation |
| Physics | rapier3d 0.22 | Rigid bodies, KinematicCharacterController, raycasts |
| Audio | macroquad-audio | Stereo + distance-attenuated 3D positional |

## Bind-group layout (5 groups)

```
@group(0) camera          mat4 view_proj
@group(1) texture         (texture_2d<f32>, sampler)
@group(2) lights          ambient + sun + 8 point lights
@group(3) joints          array<mat4, 128> for GPU skinning
@group(4) shadow          depth_2d + sampler_comparison + light_space_matrix
```

The main pipeline always renders to a `Rgba16Float` HDR offscreen;
a fullscreen tone-map pass applies ACES + optional vignette and
writes sRGB to the swapchain.

## Twe-side surface

```twe
# Camera
camera.eye = vec3(0, 1.6, 0)
camera.target = vec3(0, 1.6, -1)

# Lighting
sun.direction((0.4, 0.85, 0.35))
sun.intensity(0.65)
sun.shadow(true)
sun.shadow_extent(25.0)
light.add(at, color, radius)         # returns slot 1..=8
light.set(handle, at, color, radius)
light.ambient((r, g, b, a))

# Post-FX
postfx.tonemap(true)
postfx.vignette(0.30)
postfx.frustum_cull(true)            # default on

# Physics
let player = physics.character((0, 1, 0), 1.6, 0.4)
let move = physics.character_move(player, (vx, vy, vz), dt)  # vy is m/s
let cube = physics.body("box", at, mass)
let floor = physics.static_box(at, full_extents)             # full, not half!
let mesh = physics.static_mesh("level.glb", at)              # trimesh collider
let hit = physics.raycast(origin, dir, max_dist)             # nil or {handle, point, distance}
for ev in physics.collisions():                              # drained per call
    if ev.started: ...

# Animation (glTF)
mesh_anim.play(handle, "walk", true)
mesh_anim.blend(handle, "walk", "run", t)
mesh_anim.advance(dt)

# Math
mat4.translate(v) * mat4.rotate_y(angle) * mat4.scale(v)
quat.from_axis_angle(axis, angle)
quat.slerp(a, b, t)
```

## Showcase: `examples/crystal_hunter.twe`

Drives the full Phase 17–26 stack in ~250 lines:

- rapier3d character controller + collision events
- 5 active point lights (4 flickering torches + ambient)
- Sun-cast shadows at a 25 m extent
- HDR + ACES tone mapping + vignette tied to game state
- Persistent high score via `save.int`

```sh
twec play3d examples/crystal_hunter.twe
```

WASD + mouse + space to jump + R to restart. Goal: collect 10
crystals, dodge 4 sentinels, don't lose all 3 hearts.
