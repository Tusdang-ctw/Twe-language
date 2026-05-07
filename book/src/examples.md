# Examples gallery

The `examples/` directory ships 27 single-file programs and 7
multi-file projects. Run any of them with `twec play <path>`
(2D / UI / visual) or `twec play3d <path>` (3D).

## 3D

| File | What it shows |
|------|--------------|
| `examples/crystal_hunter.twe` | Full 3D showcase — physics, sun shadows, HDR + ACES, point lights, save state |
| `examples/fps_demo.twe` | First-person physics, mouse look, raycasts, collision events |
| `examples/hello_3d.twe` | 3D primitives + orbit camera |
| `examples/hello_glb.twe` | glTF mesh import |
| `examples/physics_demo.twe` | Falling boxes onto a static floor |

## 2D games

| File | What it shows |
|------|--------------|
| `examples/pong.twe` | Paddles, ball physics, AI opponent, state machine |
| `examples/snake.twe` | Classic Snake — entity-less, scene-driven |
| `examples/survive_beta/` | Full Vampire-Survivors clone (~1300 lines) |
| `examples/rpg_demo/` | Dialogue, rooms, pickups, save / load |
| `examples/dialogue_demo.twe` | Branching dialogue script |

## Visual / shaders

| File | What it shows |
|------|--------------|
| `examples/visual_fire.twe` | Procedural fire shader compiled from a `visual` block to WGSL |
| `examples/particles_demo.twe` | Particle systems with attraction + gravity |

## UI

| File | What it shows |
|------|--------------|
| `examples/widgets_demo.twe` | Full widget gallery — buttons, sliders, dropdowns, panels |
| `examples/pause_menu_demo.twe` | Pause stack with settings + save round-trip |
| `examples/keybind_demo.twe` | Live key rebinding UI |
| `examples/layout_demo.twe` | Stack / flex / grid layouts |

## Input

| File | What it shows |
|------|--------------|
| `examples/mouse_demo.twe` | Mouse position + buttons + wheel |
| `examples/gamepad_demo.twe` | Gamepad axes, buttons, edge detection |

## Asset pipelines

| File | What it shows |
|------|--------------|
| `examples/atlas_demo.twe` | Spritesheet animation |
| `examples/font_demo.twe` | TTF rendering with size + color |
| `examples/audio_demo.twe` | SFX + music with crossfade |
| `examples/sprite_demo.twe` | Per-frame sprite advancement |
| `examples/walk_demo.twe` | Generated walk-cycle from procedural sheet |
| `examples/save_demo.twe` | Disk save / load round-trip |
| `examples/tilemap_demo.twe` | Tile rendering + collision |

## Modules

| Project | What it shows |
|---------|--------------|
| `examples/modular_math_demo/` | Multi-file `import`-based program |
| `examples/modular_audio_demo/` | Module split with audio asset re-use |
