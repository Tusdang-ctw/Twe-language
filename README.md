# Twe

> Ship a 2D or 3D game in a language built around `entity`, `state`, `scene`,
> and `dialogue`. No engine dependency — `twec build` produces a single
> self-extracting `.exe`.

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

**Twe in one sentence:** game concepts (`entity`, `state`, `visual`, `dialogue`,
`particles`, `scene`) are first-class language constructs, not library calls,
so a Vampire-Survivors-class 2D game ships in ~1300 lines and a 3D
crystal-collection prototype with shadows + ACES tone mapping ships in ~250.

**3D pipeline (v0.1):** rapier3d physics, glTF 2.0 scene graph + GPU skinning,
8 point lights + Blinn-Phong, 2K shadow maps with 3×3 PCF, HDR + ACES filmic
tone mapping, frustum culling. Try `twec play3d examples/crystal_hunter.twe`.

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
| 3D physics + character controller (rapier3d) | post-v1.0 |
| glTF 2.0 multi-node scenes + auto-textures | post-v1.0 |
| GPU skinning + glTF animation channels | post-v1.0 |
| Real-time directional shadows (2K depth + 3×3 PCF) | post-v1.0 |
| HDR pipeline + ACES filmic tone mapping + vignette | post-v1.0 |
| Frustum culling, dynamic instance buffer | post-v1.0 |
| `twec grammar` — GBNF / JSON-Schema / EBNF export for constrained LLM decoding | post-v1.0 |
| `twec verify` — JSON v2 with structured machine-applicable `fix` patches | post-v1.0 |
| `twec stdlib --json` — manifest of all 235 builtins (LLM grounding surface) | post-v1.0 |
| `twec llm-loop` — provider-agnostic prompt → generate → verify self-correction loop | post-v1.0 |
| `twec mcp` — stdio JSON-RPC MCP server (Claude Desktop, Cursor, future Studio) | post-v1.0 |
| `twec corpus` — labelled `@task` / `@expected` / `@category` examples manifest | post-v1.0 |
| `twec eval` — replay-based suite scorer for LLM-generated programs | post-v1.0 |
| `twec mutate` — auto-generated (broken, verify_json, fix) corpus for fine-tuning | post-v1.0 |
| Typed holes (`???`) — verify-time warning + runtime error for unfilled expressions | post-v1.0 |
| `fx.*` — procedural VFX library (hit-flash, screen-shake, hit-stop, damage numbers, death bursts, …) | v1.0.1 |
| `tween.*` — deterministic easing primitives (replay-safe, 14 curves) | v1.0.1 |
| `light2d.*` — additive multi-light pass + optional AABB shadow caster | v1.0.1 |
| Audio polish — `sound.pool` / `sound.duck` / `music.layer` / `music.crossfade` | v1.0.1 |
| `save SaveSlot:` block + `migration from N:` version migrations | v1.0.1 |
| Per-state pause opt-out (`pause: false` / `persistent`) | v1.0.1 |
| Nine-slice panels (`panel(... skin: nine_slice("path", border: N))`) | v1.0.1 |
| `camera2d.*` — `follow` / `zoom_to` / `cinematic_pan` / `bounds` | v1.0.1 |
| LSP cross-module find-references + rename | v1.0.1 |
| Replay-on-crash (always-on input ring + `twec replay` subcommand) | v1.0.1 |
| `twec perf-snapshot` / `twec perf-diff` + CI baseline + `perf.yml` workflow | v1.0.1 |
| Localization plurals (`lang.t_plural` with CLDR rules for 15 locales) | v1.0.1 |
| `twec doctor` triage diagnostic (`--json` for LLM-grounded support) | v1.0.1 |

**991 tests pass. `cargo clippy --release --all-targets -- -D warnings` clean.** **Phases 1–41 codebase-closed; v1.0.1 polish release closed 2026-05-18 (all 13 feature sessions + closeout shipped — see [`docs/changes/2026-05-18-v1.0.1-closeout.md`](docs/changes/2026-05-18-v1.0.1-closeout.md)).** The full post-v1.0 round 2 roadmap landed: 2D Steam-class, 3D action-RPG, lockstep LAN multiplayer, WASM 2D, open-world infrastructure, the LLM-differentiator surface (grammar export + structured-fix verify + stdlib manifest + llm-loop + MCP + corpus + eval + mutate + typed holes), cross-platform polish, external-validation scaffolding, internet multiplayer (Steam P2P primary + STUN fallback + lobbies + reconnect), rollback netcode (snapshot ring buffer + predicted-input policies), browser-3D scaffolding (`BuildTarget::Wasm32_3D` + audit), mobile (iOS + Android) scaffolding (touch + virtual joystick + safe-area insets + signing recipe), **console targets** (`console.*` abstract input + glyphs + service-trait stubs + `PARTNER.md` partner contribution guide), **and the MMO foundation** (sharded-zones architecture RFC + `mmo.*` API stubs + workshop traits + sandboxing pre-requirements). See [`LLMsPlan.md`](LLMsPlan.md) for the LLM-authoring strategy, [`docs/mobile-signing.md`](docs/mobile-signing.md) for the iOS / Android shipping recipe, and [`PARTNER.md`](PARTNER.md) for licensed studios working on console ports.

## Install

Build from source (requires Rust 1.74+):

```sh
git clone https://github.com/Tusdang-ctw/Twe-language
cd Twe-language
cargo build --release
./target/release/twec version
```

Or install directly:

```sh
cargo install --git https://github.com/Tusdang-ctw/Twe-language --bin twec
```

Pre-built binaries for Windows / macOS / Linux land in [Releases](https://github.com/Tusdang-ctw/Twe-language/releases) once cargo-dist is wired up (Phase 7).

## Quick start

2D — pick any:

```sh
twec play examples/pong.twe                # Pong (player vs AI)
twec play examples/survive_beta/main.twe   # Vampire-Survivors clone
twec play examples/rpg_demo/main.twe       # Dialogue-driven mini-RPG
twec play_visual examples/visual_fire.twe  # Procedural fire shader
```

3D — pick any:

```sh
twec play3d examples/crystal_hunter.twe    # FPS — collect crystals, dodge sentinels
twec play3d examples/fps_demo.twe          # Bare physics + KCC + raycast prototype
twec play3d examples/hello_3d.twe          # Cubes + spheres + orbit camera
```

Build a redistributable:

```sh
twec build examples/survive_beta
# → examples/survive_beta/dist/survive_beta.exe (self-extracting, no Twe required)
```

## Examples gallery

30 single-file examples + 7 multi-file projects, covering every major surface:

| File | What it shows |
|------|--------------|
| `examples/crystal_hunter.twe` | **3D showcase** — physics, shadows, HDR + ACES, 5 point lights, save state |
| `examples/fps_demo.twe` | First-person physics, mouse-look, raycasts, collision events |
| `examples/hello_3d.twe` / `hello_glb.twe` | 3D primitives + glTF mesh import |
| `examples/pong.twe` | Paddles, ball physics, AI, state machine |
| `examples/snake.twe` | Classic Snake — entity-less version |
| `examples/flappy.twe` | Flappy Bird — gravity + recycled obstacles + state machine |
| `examples/platformer.twe` | Coyote time, jump buffer, variable jump, AABB tile collision, one-way platforms |
| `examples/tetris.twe` | 7-bag, simplified SRS, line clears, DAS/ARR, ghost piece |
| `examples/cards.twe` | Solitaire-Lite — drag-and-drop, layered z-order, snap-back |
| `examples/survive_beta/` | Full Vampire-Survivors clone (~1300 lines) |
| `examples/rpg_demo/` | Dialogue, rooms, pickups, save / load |
| `examples/visual_fire.twe` | Procedural fire via `visual` → WGSL |
| `examples/particles_demo.twe` | Particle systems |
| `examples/pause_menu_demo.twe` | Pause stack + settings save |
| `examples/keybind_demo.twe` | Live key rebinding UI |
| `examples/dialogue_demo.twe` | Branching dialogue script |
| `examples/tilemap_demo.twe` | Tile rendering + collision |
| `examples/atlas_demo.twe` | Spritesheet animation |
| `examples/widgets_demo.twe` | Full widget gallery |
| `examples/modular_math_demo/` | Multi-file modules |
| `examples/pong_net.twe` | LAN lockstep multiplayer over UDP |
| `examples/pong_net_internet.twe` | Internet multiplayer — Steam Lobbies or STUN+rendezvous fallback |
| `examples/fighter_demo.twe` | Rollback netcode — 2-player fighting game with `rollback.snapshot` |
| `examples/crystal_hunter_web.twe` | Browser-3D placeholder using `assets.platform()` branching |
| `examples/survive_beta_mobile/` | Mobile touch controls — virtual joystick + safe-area-aware HUD |
| `examples/console_demo.twe` | Console controller + per-style glyph rendering (`console.*`) |
| `examples/mmo_demo/` | MMO API single-player demo — `mmo.*` replication + persist + broadcast |
| … and more | Audio, camera, gamepad, fonts, mouse, save, layout, physics |

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
| [Survive Beta](examples/survive_beta/) — Vampire-Survivors clone | First-party | v0.8 |
| [RPG Demo](examples/rpg_demo/) — Dialogue-driven adventure | First-party | v1.0 |
| [Crystal Hunter](examples/crystal_hunter.twe) — 3D FPS showcase | First-party | post-v1.0 |

*Ship a game with Twe? Open a PR to add it here.*

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The short version: one thing per PR, tests first, docs in the same commit. Read [`CLAUDE.md`](CLAUDE.md) before proposing anything — it records every locked decision.

## License

MIT — see [`LICENSE`](LICENSE).
