# Twe 3D Commercial Game Roadmap — Phases 17–23

> **Goal:** Bring Twe from its current minimal 3D scaffolding to full commercial/production 3D game capability.
>
> **Status (2026-05-07):** **Phases 17–23 are all codebase-closed (MVP scope).** Each phase ships a real but partial subset of the full plan, with deferred items honestly tracked.
>
> - Phase 17 — UV + textures + mouse + cursor + vec3 math + glTF material auto-extraction. Closeout: `docs/changes/2026-05-07-phase-17-closeout.md`.
> - Phase 18 — full rapier3d physics + KCC + raycast + collision events. Closeout: `docs/changes/2026-05-07-phase-18-closeout.md`.
> - Phases 19–23 — multi-node glTF + mat4 + lighting + animation API + typed save + dynamic instances + spatial audio. Single combined closeout at `docs/changes/2026-05-07-phase-19-23-closeout.md`.
>
> **Major deferrals tracked in closeouts:** GPU skinning (Phase 21), shadow maps (Phase 20), frustum culling (Phase 23), post-processing (Phase 23), per-instance dynamic transforms (Phase 19). The Twe-facing API surface for these is stable where applicable; what lands later is the GPU pipeline implementation.

---

## Two milestones

| Milestone | End of | What you can ship |
|-----------|--------|-------------------|
| **A — Playable 3D demo** | Phase 18 | Textured level + physics-based movement + mouse-look camera |
| **B — Commercial-ready** | Phase 23 | Skeletal animation + shadows + scene management + 60fps on integrated GPU |

---

## Dependency order

```
Phase 17 (UV textures + mouse look)
    └─ Phase 18 (physics)                     ← Milestone A
        └─ Phase 19 (full glTF scene graph)
            ├─ Phase 20 (point lights + shadows)
            └─ Phase 21 (skeletal animation)
                └─ Phase 22 (scene management + streaming)
                    └─ Phase 23 (commercial polish + perf)  ← Milestone B
```

---

## Phase 17 — UV Textures + Mouse Look
**Size: M (2–3 weeks)**

The two most visible gaps for any 3D game.

### Sessions

| # | What | Files |
|---|------|-------|
| 1 | Add `uv: [f32; 2]` to `Vertex` struct. Update `VertexBufferLayout` + WGSL `VertexInput`. Cube/sphere get dummy `[0.0, 0.0]`. | `src/play3d.rs` |
| 2 | wgpu bind group 1: `texture_2d<f32>` + `sampler`. White 1×1 fallback. PNG/JPEG decode via `image` (already transitive). Wire `TEXCOORD_0` from glTF. | `src/play3d.rs` |
| 3 | Twe surface: `texture("char.png")` → opaque handle. `mesh(at:, path:, texture:, size:)` accepts it. | `src/stdlib.rs` |
| 4 | Mouse delta: `mouse.dx` / `mouse.dy` from `WindowEvent::CursorMoved`. `cursor.lock()` / `cursor.unlock()` via winit `set_cursor_grab(Locked)`. | `src/play3d.rs` |
| 5 | `vec3.*` math builtins: `add`, `sub`, `scale`, `dot`, `cross`, `normalize`, `length`, `lerp`. Return 3-tuples. No new Value variants. | `src/stdlib.rs` |

### New Twe surface
```twe
let tex = texture("assets/wall.png")
mesh(at: vec3(0, 0, 0), path: "level.glb", texture: tex, size: 1.0)

on update(dt):
    camera.yaw += mouse.dx * 0.002
    camera.pitch -= mouse.dy * 0.002
    cursor.lock()

let forward = vec3.normalize(vec3.sub(camera.target, camera.eye))
```

**New crates:** none  
**Runnable artifact:** First-person walk through a textured `.glb` level with WASD + mouse look.  
**Exit criteria:** Textured glb renders; first-person camera works; `vec3.*` usable in update logic.

---

## Phase 18 — Physics + Collision
**Size: L (4–6 weeks)**

Movement that feels solid. `rapier3d` is pure Rust, no native dependencies.

### Sessions

| # | What | Files |
|---|------|-------|
| 1 | `src/physics3d.rs`: wrap `rapier3d::PhysicsPipeline` + sets. Tick before Twe `on update(dt)`. | `src/physics3d.rs`, `src/play3d.rs` |
| 2 | `physics.body(shape: "capsule", at:, mass:)` → opaque handle. Shapes: `"box"`, `"sphere"`, `"capsule"`, `"trimesh"`. | `src/stdlib.rs` |
| 3 | `physics.velocity(h, v)`, `physics.impulse(h, v)`, `physics.position(h)`, `physics.character_move(h, dir, dt)`. | `src/stdlib.rs` |
| 4 | `physics.static_mesh(path)`: reads existing `parse_glb_bytes` positions/indices → rapier `TriMeshShape` + static body. | `src/physics3d.rs` |
| 5 | `physics.on_collision(h_a, h_b, callback)`: fires a Twe callback on overlap. Drives item pickup. | `src/stdlib.rs`, `src/eval.rs` |
| 6 | `physics.gravity(vec)`. Jump via vertical impulse. | `src/stdlib.rs` |

### New Twe surface
```twe
let player = physics.body(shape: "capsule", at: vec3(0, 2, 0), mass: 1.0)
let ground = physics.static_mesh("level.glb")
let coin   = physics.body(shape: "sphere", at: vec3(3, 1, 0), mass: 0.0)

physics.on_collision(player, coin, fn():
    score += 1
    despawn coin
)

on update(dt):
    let dir = vec3(dx, 0, dz)
    physics.character_move(player, dir, dt)

    let pos = physics.position(player)
    camera.eye = pos
```

**New crates:** `rapier3d = "0.22"`  
**Runnable artifact (Milestone A):** 3D platformer — textured level, capsule walks/jumps, coins collectible via collision.  
**Exit criteria:** No clip-through; collision callback fires; physics tick decoupled from render.

---

## Phase 19 — Full glTF Scene Graph + Node Transforms
**Size: M (2–3 weeks)**

Real `.glb` files from Blender are multi-node hierarchies. The current loader ignores all node transforms and reads only the first primitive.

### Sessions

| # | What | Files |
|---|------|-------|
| 1 | Rewrite `parse_glb_bytes`: walk `doc.scenes() → nodes → children` recursively. Flatten to `Vec<(model_matrix: [f32;16], verts, indices, tex_id)>`. | `src/play3d.rs` |
| 2 | Replace `pos_size: vec4` instance slot with `model_matrix: [f32;16]`. WGSL: `clip_pos = vp * model * vec4(pos, 1.0)`. | `src/play3d.rs` |
| 3 | Per-primitive draw partitioning: one wgpu draw per (mesh_path, primitive_idx, texture_id). A single `.glb` can have N materials. | `src/play3d.rs` |
| 4 | `mat4` Twe type: `mat4.identity()`, `mat4.translate(v)`, `mat4.rotate_y(a)`, `mat4.mul(a, b)`, `mat4.transform_vec3(m, v)`. Stored as 16-element tagged list. | `src/stdlib.rs` |

### New Twe surface
```twe
# Multi-object scene loads and places each node at its Blender transform
mesh(at: vec3(0, 0, 0), path: "dungeon_full.glb", size: 1.0)

# Manual transform for procedural placement
let m = mat4.mul(mat4.translate(vec3(5, 0, 3)), mat4.rotate_y(1.57))
mesh(at: vec3(0, 0, 0), path: "pillar.glb", transform: m, size: 1.0)
```

**Runnable artifact:** Multi-object Blender scene renders with correct node placements.  
**Exit criteria:** A multi-node `.glb` from Blender renders all primitives at correct world positions.

---

## Phase 20 — Point Lights + Shadow Maps
**Size: L (4–6 weeks)**

Visual quality step from toy to commercial. Required for any indoor/dungeon setting.

### Sessions

| # | What | Files |
|---|------|-------|
| 1 | `LightsUniform { ambient: vec4, lights: array<PointLight, 8> }` (pos, color, radius). Bind as group 2. | `src/play3d.rs` |
| 2 | Rewrite WGSL fragment shader: loop over 8 lights, Blinn-Phong attenuation `1/(d*d)`. Sun stays as slot 0. | `src/play3d.rs` |
| 3 | Shadow map render pass: 2048×2048 `Depth32Float`, depth-only, orthographic from sun direction. Runs before main pass. | `src/play3d.rs` |
| 4 | PCF shadow sampling: 3×3 tap with `textureSampleCompare` in main fragment shader. | `src/play3d.rs` |
| 5 | Twe builtins: `light.add(at:, color:, radius:)`, `light.remove(h)`, `light.ambient(c)`, `sun.direction(v)`, `sun.shadow(bool)`, `light.fog(density, color)`. | `src/stdlib.rs` |

### New Twe surface
```twe
sun.direction(vec3(0.3, -0.8, 0.2))
sun.shadow(true)
light.ambient(color.from_hex("#1a1a2e"))

let torch1 = light.add(at: vec3(2, 2, 0), color: color.orange, radius: 8.0)
let torch2 = light.add(at: vec3(-2, 2, 4), color: color.orange, radius: 8.0)

on update(dt):
    # Torch flicker
    light.set_radius(torch1, 8.0 + math.sin(time * 12.0) * 1.5)
```

**Runnable artifact:** Dungeon corridor with 3 colored point lights, soft shadows, torch flicker effect.  
**Exit criteria:** 8 simultaneous point lights with correct attenuation; PCF shadows visible on geometry.

---

## Phase 21 — Skeletal Animation
**Size: L (4–8 weeks)**

Characters that move. The largest single phase.

### Sessions

| # | What | Files |
|---|------|-------|
| 1 | Add `joints: [u16; 4]` + `weights: [f32; 4]` to `Vertex` (locations 4+5). Extract `JOINTS_0` / `WEIGHTS_0` from glTF. | `src/play3d.rs` |
| 2 | Load `Skin.inverse_bind_matrices()` per mesh → `SkinData { joints: Vec<usize>, ibms: Vec<[f32;16]> }`. | `src/play3d.rs` |
| 3 | Joint matrix UBO: `array<mat4x4<f32>, 128>` as group 4. WGSL: `skin_mat = w0*J[j0] + w1*J[j1] + w2*J[j2] + w3*J[j3]`. | `src/play3d.rs` |
| 4 | Animation sampler: load glTF `Animation` channels (TRS per node). `AnimClip { name, duration, channels }`. Linear keyframe interpolation. | `src/play3d.rs` |
| 5 | `AnimPlayer { clip_name, time, looping }`. Advances per frame, writes joint matrices to UBO. | `src/play3d.rs`, `src/stdlib.rs` |
| 6 | `quat` Twe type: `quat.slerp(a, b, t)`, `quat.from_axis_angle(axis, angle)`, `quat.to_mat4(q)`. Required for rotation interpolation. | `src/stdlib.rs` |
| 7 | Blend trees: `mesh.blend(h, clip_a, clip_b, t)` lerps joint matrices between two clips. | `src/stdlib.rs` |

### New Twe surface
```twe
let char = mesh.load("character.glb")

on update(dt):
    let speed = vec3.length(velocity)
    if speed > 0.1:
        mesh.blend(char, "idle", "walk", math.min(speed / 3.0, 1.0))
    else:
        mesh.play(char, clip: "idle", loop: true)

    if jumped:
        mesh.play(char, clip: "jump", loop: false)
```

**New crates:** none (gltf crate already exposes skins/animations)  
**Runnable artifact:** Humanoid character with idle/walk/run clips and blend-tree, running through Phase 18 level.  
**Exit criteria:** Mixamo-exported `.glb` plays all clips; blending produces no visual pops; up to 128 joints.

---

## Phase 22 — Scene Management + Asset Streaming
**Size: M (2–4 weeks)**

Multi-scene games need clean transitions. This is the difference between a tech demo and a game.

### Sessions

| # | What | Files |
|---|------|-------|
| 1 | `scene.enter(name)` for 3D: drop physics bodies, release GPU cache for exiting scene, call new scene startup. `on enter():` hook. | `src/eval.rs`, `src/play3d.rs` |
| 2 | Scene asset manifests: `scene MyLevel:` gains `load: ["level.glb", "char.glb"]`. Background prefetch via `std::thread` + channel. `on loading(progress):` hook. | `src/parser.rs`, `src/eval.rs` |
| 3 | LRU GPU cache: 256 MB budget. Unreferenced assets evicted 2 frames after scene exit. | `src/play3d.rs` |
| 4 | `on exit():` hook: fires before transition completes. | `src/eval.rs` |
| 5 | `save.vec3(key, v)` + `save.f32(key, v)`: extend Phase 8 save layer. Persist player 3D position across transitions. | `src/save.rs`, `src/stdlib.rs` |

### New Twe surface
```twe
scene Dungeon:
    load: ["dungeon.glb", "torch.png", "goblin.glb"]

    on enter():
        let saved_pos = save.vec3("player.pos")
        player.pos = saved_pos

    on exit():
        save.vec3("player.pos", player.pos)
        save.save("game.save")

    on loading(progress):
        rect(at: (100, 230), size: (440 * progress, 20), color: color.white)
        text("Loading... {math.floor(progress * 100)}%", at: (250, 210), size: 18, color: color.gray)
```

**Runnable artifact:** Two-room dungeon — player walks through a door, room loads with different lighting, position persists.  
**Exit criteria:** No GPU resource leak on transition; `on enter` fires exactly once; loading progress is monotonic.

---

## Phase 23 — Commercial Polish + Performance
**Size: M (2–4 weeks)**

The gap between "technically works" and "ships on Steam."

### Sessions

| # | What | Files |
|---|------|-------|
| 1 | Frustum culling: 6 frustum planes from view-projection. Per-mesh AABB baked at load time. Skip culled draw calls. Removes 4096 hard cap as a practical limit. | `src/play3d.rs` |
| 2 | Dynamic instance buffer: replace `MAX_INSTANCES = 4096` with `Vec<Instance>` grown via `queue.write_buffer`. `max_drawcalls` in `twe.toml`. | `src/play3d.rs`, `src/build.rs` |
| 3 | Post-processing pass: fullscreen blit with ACES filmic tone mapping, gamma correction (linear→sRGB), optional vignette. | `src/play3d.rs`, `src/stdlib.rs` |
| 4 | Spatial audio: `sound.play3d(handle, at: vec3, radius: f32)` — pan + attenuation from camera position, over existing Phase 9 audio layer. No new crate. | `src/stdlib.rs` |
| 5 | `twec bench3d <file>`: 1000 headless frames, report avg/p99 frame time. Target: ≥60fps with 2000 visible draw calls on Intel Iris Xe. | `src/cli.rs` |

### New Twe surface
```twe
postfx.tonemap(true)
postfx.vignette(0.4)

sound.play3d(footstep, at: player_pos, radius: 12.0)
sound.play3d(torch_crackle, at: vec3(2, 2, 0), radius: 5.0)
```

**Runnable artifact (Milestone B):** Two-scene action-adventure — textured animated character, 4 point lights + shadows, spatial audio, ACES tone mapping, 60fps on integrated GPU.  
**Exit criteria:** Frustum culling reduces draw calls ≥50% in an open scene; p99 ≤ 16.7ms on Intel Iris Xe; all Phase 17–22 criteria still pass.

---

## Files changed per phase

| File | Phases |
|------|--------|
| `src/play3d.rs` | 17, 18, 19, 20, 21, 22, 23 |
| `src/physics3d.rs` *(new)* | 18 |
| `src/stdlib.rs` | 17, 18, 19, 20, 21, 22, 23 |
| `src/eval.rs` | 18, 22 |
| `src/save.rs` | 22 |
| `src/cli.rs` | 23 |
| `Cargo.toml` | 18 (`rapier3d = "0.22"`) |

---

## Architecture decisions

**One Vertex layout.** UV + joints + weights on every vertex. A cube's extra 32 bytes/vertex is irrelevant; one pipeline with one layout is simpler to maintain than branching for skinned vs. unskinned meshes.

**Physics ticks before Twe `on update`.** rapier3d steps first; the script reads authoritative positions via `physics.position(handle)`. Scripts own *intent* (velocity, impulses); physics owns *truth* (positions, rotations). Prevents script mutations from fighting the integrator.

**Scene graph baked at load time.** `parse_glb_bytes` flattens all node transforms into per-primitive model matrices at load time. Animated nodes override their baked matrix each frame. No live scene graph to maintain.

**`mat4`/`quat` as tagged lists.** Stored as 16-element and 4-element Twe lists with a marker field. No new `Value` variants, no GC changes, no parser changes. Trades ergonomics for implementation simplicity — acceptable for a game scripting language.

---

## Timeline

| Phase | Theme | Size | Cumulative |
|-------|-------|------|-----------|
| 17 | UV textures + mouse look | M · 2–3 wk | 3 wk |
| 18 | Physics (rapier3d) | L · 4–6 wk | 9 wk |
| 19 | Full glTF scene graph | M · 2–3 wk | 12 wk |
| 20 | Point lights + shadows | L · 4–6 wk | 18 wk |
| 21 | Skeletal animation | L · 4–8 wk | 26 wk |
| 22 | Scene management | M · 2–4 wk | 30 wk |
| 23 | Commercial polish + perf | M · 2–4 wk | 34 wk |

**~7–8 months** of focused development to reach commercial 3D capability.  
One new crate across all 7 phases: `rapier3d`.
