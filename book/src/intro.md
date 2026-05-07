# Twe

> Ship a 2D or 3D game in a language built around `entity`, `state`,
> `scene`, and `dialogue`. No engine dependency — `twec build`
> produces a single self-extracting `.exe`.

```twe
entity Slime:
    var pos = (0.0, 0.0)

    function update(dt):
        let dx = player_x - pos.x
        let dy = player_y - pos.y
        let d = math.sqrt(dx * dx + dy * dy)
        pos = (pos.x + dx / d * 40.0 * dt,
               pos.y + dy / d * 40.0 * dt)

    function render():
        rect(at: (pos.x - 10, pos.y - 10), size: (20, 20), color: color.cyan)
```

Game concepts (`entity`, `state`, `visual`, `dialogue`, `particles`,
`scene`) are first-class language constructs, not library calls, so
a Vampire-Survivors-class 2D game ships in ~1300 lines and a 3D
crystal-collection prototype with shadows + ACES tone mapping ships
in ~250.

## What ships in v1.0

- **2D runtime** (`twec play`) — macroquad-backed, full UI widget
  set, particle systems, settings + localization, pause stack,
  rebindable keys.
- **3D runtime** (`twec play3d`) — wgpu pipeline, rapier3d physics
  with character controller, glTF 2.0 multi-node scenes, GPU
  skinning + animation, 8 point lights + Blinn-Phong, dynamic
  shadow maps with 3×3 PCF, HDR linear lighting + ACES filmic
  tone mapping, frustum culling.
- **Visual runtime** (`twec play_visual`) — `visual` blocks
  compile to WGSL fragment shaders for procedural pixel art.
- **Build pipeline** (`twec build`) — produces a self-extracting
  Windows `.exe` (macOS `.app` + Linux AppDir scaffolds shipped;
  per-target binaries via cargo-dist in Phase 7).
- **Module system** + **strict mode** + **verified-mode JSON
  diagnostics** for LLM tool use.

## Five principles

1. **Game concepts are first-class.** `entity`, `state`,
   `visual`, `dialogue`, `particles`, `scene` are language
   constructs.
2. **One obvious way per concept.** Single inheritance, one
   method-call syntax. Regularity helps humans and LLMs.
3. **No silent footguns.** 0-indexed, only `false` is falsy,
   dimensional units enforced, errors that suggest fixes.
4. **AI-legible by design.** Predictable LL(1)-ish grammar,
   structured JSON diagnostics, round-trippable AST.
5. **Engine-native.** The Twe runtime *is* the engine.

## Where to next

- New here? **[Install + first program](./install.md)** then
  **[Tutorial](./tutorial.md)**.
- Looking up a builtin or syntax form?
  **[Reference](./reference.md)**.
- Curious about the 3D pipeline?
  **[3D pipeline](./3d-pipeline.md)**.
- Want to contribute?
  **[Contributing](./contributing.md)**.
