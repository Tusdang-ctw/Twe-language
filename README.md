# Twe

> A game-first programming language designed for the AI-collaboration era.

Twe is a scripting language for 2D and 3D game development. Game concepts — `entity`, `state`, `visual`, `dialogue`, `particles`, `scene` — are first-class language constructs, not library calls. The runtime compiles Twe to bytecode and runs it; `twec build` bundles a game into a self-extracting Windows `.exe`.

```twe
entity Slime:
    var pos = (0.0, 0.0)

    function update(dt):
        let dx = player_x - pos.x
        let dy = player_y - pos.y
        let d = math.sqrt(dx*dx + dy*dy)
        pos = (pos.x + dx/d * 40.0 * dt,
               pos.y + dy/d * 40.0 * dt)

    function render():
        rect(at: (pos.x - 10, pos.y - 10), size: (20, 20), color: color.cyan)
```

## What ships with Twe

| Feature | Status |
|---------|--------|
| Hand-written recursive-descent parser | v0.1 |
| Tree-walking interpreter + bytecode VM | v0.1 / v0.2 |
| NaN-tagged 64-bit values + tracing GC | v0.2 |
| `twec play` (2D macroquad runtime) | v0.1 |
| `twec play3d` (3D wgpu runtime) | v0.1 |
| `twec play_visual` (procedural shader runtime) | v0.3 |
| `visual` block → WGSL compilation | v0.3 |
| UI widget set (button, slider, dropdown, …) | v0.4 |
| Settings, localization, pause, rebindable keys | v0.4 |
| Crash reporter + screenshot + profiler | v0.5 |
| `twec build` → self-extracting Windows `.exe` | v0.6 |
| Module system + `import` | v0.7 |
| Strict mode + verified mode (LLM JSON output) | v0.7 |
| `@deprecated` annotations + 12-month cycle | v0.7 |
| Survive Beta — Vampire-Survivors clone | v0.8 |
| Tutorial v2 (Pong → Survivors → RPG) | v0.8 |
| Steam SDK integration (`--features steam`) | v0.9 |
| RPG demo — second first-party game | v1.0 |

**732 tests pass. `cargo clippy -- -D warnings` clean.**

## Install

```
cargo install --git https://github.com/your-org/twe-language twec
```

Or clone and build:

```
git clone https://github.com/your-org/twe-language
cd twe-language
cargo build --release
./target/release/twec version
```

## Quick start

```
twec play examples/pong.twe          # Pong (player vs AI)
twec play examples/survive_beta/main.twe  # Vampire Survivors clone
twec play examples/rpg_demo/main.twe      # Dialogue RPG
twec play_visual examples/visual_fire.twe # Procedural fire shader
```

Build a redistributable:

```
twec build examples/survive_beta
# → examples/survive_beta/dist/survive_beta.exe (self-extracting, no Twe required)
```

## Examples gallery

28 runnable examples covering every major surface:

| File | What it shows |
|------|--------------|
| `examples/pong.twe` | Paddles, ball physics, AI, state machine |
| `examples/snake.twe` | Classic Snake — entity-less version |
| `examples/survive_beta/` | Full Vampire-Survivors clone (1300 lines) |
| `examples/rpg_demo/` | Dialogue, rooms, pickups, save/load |
| `examples/visual_fire.twe` | Procedural fire via `visual` → WGSL |
| `examples/particles_demo.twe` | Particle systems |
| `examples/pause_menu_demo.twe` | Pause stack + settings save |
| `examples/keybind_demo.twe` | Live key rebinding UI |
| `examples/dialogue_demo.twe` | Branching dialogue script |
| `examples/tilemap_demo.twe` | Tile rendering + collision |
| `examples/atlas_demo.twe` | Spritesheet animation |
| `examples/widgets_demo.twe` | Full widget gallery |
| `examples/modular_math_demo/` | Multi-file modules |
| … 15 more | Audio, camera, gamepad, fonts, mouse, save, layout, … |

## Language in 60 seconds

```twe
# Scenes hold state. States handle behavior.
scene Pong:
    var ball_x = 320.0
    var ball_vx = 300.0

    initial: playing

    state playing:
        on update(dt):
            ball_x += ball_vx * dt
            if ball_x < 0 or ball_x > 640:
                ball_vx = -ball_vx

        on render():
            rect(at: (ball_x - 5, 235), size: (10, 10), color: color.white)

# Entities have update + render lifecycle.
entity Bullet:
    var pos = (0.0, 0.0)
    var vel = (0.0, -400.0)

    function update(dt):
        pos = (pos.x + vel.x * dt, pos.y + vel.y * dt)
        if pos.y < 0:
            despawn self

    function render():
        rect(at: (pos.x - 3, pos.y - 3), size: (6, 6), color: color.yellow)

spawn Bullet at (320, 400)

# Dialogue is a first-class block.
dialogue Merchant:
    say "Looking to trade?"
    choice:
        "Yes": say "Here's what I have."
        "No":  say "Safe travels."

Merchant()
```

Key design choices: 0-indexed arrays, indentation-based syntax (no braces), only `false` is falsy, `let`/`var` immutable/mutable split, gradual typing (non-strict default → `# strict` → `# verified` for LLM JSON output).

## Design principles

In strict priority order:

1. **Game concepts are first-class.** `entity`, `state`, `visual`, `dialogue`, `particles`, `scene` are language constructs.
2. **One obvious way per concept.** Single inheritance, one method-call syntax. Regularity helps humans and LLMs equally.
3. **No silent footguns.** 0-indexed, only `false` is falsy, dimensional units enforced, errors suggest fixes.
4. **AI-legible by design.** Predictable LL(1)-ish grammar, structured JSON diagnostics, round-trippable AST.
5. **Engine-native.** The runtime *is* the engine. Engine objects are first-class Twe values.

## Documentation

| Doc | Contents |
|-----|----------|
| [`docs/tutorial.md`](docs/tutorial.md) | Hands-on tutorial: Pong from scratch, Survivors read-along, mini-RPG |
| [`docs/06-design-document.md`](docs/06-design-document.md) | Formal grammar, semantics, full stdlib reference |
| [`docs/02-type-system.md`](docs/02-type-system.md) | Gradual typing, strict mode, verified mode |
| [`docs/03-runtime.md`](docs/03-runtime.md) | Runtime architecture + explicit list of footguns avoided |
| [`docs/05-roadmap.md`](docs/05-roadmap.md) | Phase-by-phase plan from v0.1 through v1.0 |

## Shipped on Twe

| Game | Author | Version |
|------|--------|---------|
| [Survive Beta](examples/survive_beta/) — Vampire Survivors clone | First-party | v0.8 |
| [RPG Demo](examples/rpg_demo/) — Dialogue-driven adventure | First-party | v1.0 |

*Ship a game with Twe? Open a PR to add it here.*

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The short version: one thing per PR, tests first, docs in the same commit. Read [`CLAUDE.md`](CLAUDE.md) before proposing anything — it records every locked decision.

## License

MIT — see [`LICENSE`](LICENSE).
