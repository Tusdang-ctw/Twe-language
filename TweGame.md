# TweGame.md — Building real games on Twe

> Four-track plan for shipping production games on Twe across the categories that are codebase-closed and battle-tested. Companion to `docs/05-roadmap.md` (which tracks language phases). This file tracks *game-shipping phases* + *per-game sessions* using the same closeout discipline.

---

## Context

Phases 1–41 of the Twe language roadmap are codebase-closed. Out of that work, four game categories ship today as proven end-to-end paths: 2D Steam-class, 2D web demos, LAN multiplayer, and small 3D for Windows. This plan converts those capabilities into concrete game projects — one demo game per track — so the project has reference titles that exercise the full pipeline beyond the existing test-bed examples.

The four games are independent. Each phase ends with a closeout note describing what shipped. Sessions inside each phase follow the same per-commit shape Twe's language phases used: short, focused, runnable artifact each commit.

| Track | Game | Genre | Scope | Target |
|---|---|---|---|---|
| A | **GlyphRunner** | Survivors-class action / RPG | 600–1000 LOC | Steam Windows |
| B | **PixelPop** | Single-screen reflex arcade | 150–300 LOC | itch.io + GitHub Pages |
| C | **GridDuel** | 2-player LAN versus | 400–700 LOC | Direct download / LAN-party |
| D | **CrystalCaves** | First-person 3D collection | 500–800 LOC | Steam Windows |

Build targets, runtime paths, and library APIs all already exist in the codebase. The work here is the *game design + content + polish* on top.

---

## Source-of-truth references

| Resource | Why it matters |
|---|---|
| [examples/survive_beta/](examples/survive_beta) | Survivors-class template (Track A) |
| [examples/flappy.twe](examples/flappy.twe), [examples/snake.twe](examples/snake.twe) | Tiny-arcade templates (Track B) |
| [examples/pong_net.twe](examples/pong_net.twe) | Lockstep LAN template (Track C) |
| [examples/fps_demo.twe](examples/fps_demo.twe), [examples/crystal_hunter.twe](examples/crystal_hunter.twe) | 3D templates (Track D) |
| [src/build.rs](src/build.rs) | `twec build` for all targets |
| [src/net.rs](src/net.rs) | Lockstep UDP runner |
| [docs/tutorial.md](docs/tutorial.md) Part II | Pong → Survivors → Mini-RPG tutorial arc |
| [docs/playtest/cross-machine-lan-multiplayer.md](docs/playtest/cross-machine-lan-multiplayer.md) | LAN playtest recipe |
| [docs/3d-roadmap.md](docs/3d-roadmap.md) | 3D feature inventory |
| [.github/workflows/wasm-demo.yml](.github/workflows/wasm-demo.yml) | Pages deploy CI reference |

---

## Phase 0 — Shared dev environment

**Status:** complete. Twec already cargo-builds in this repo; `cargo run -- play examples/snake.twe` and `cargo run -- play3d examples/fps_demo.twe` both work. The 938-test suite passes (2 pre-existing CRLF cascade failures documented in Phase 33+ closeouts).

If a contributor runs `TweGame.md` from a fresh clone, they walk through `Phase 0` in the master plan at `C:\Users\Admin\.claude\plans\we-plan-covers-this-reflective-fiddle.md` to verify their environment matches.

---

# Phase A — GlyphRunner (Steam Survivors-class)

**Pitch:** A roguelite Survivors-class where the player is a glyph-mage. Runes auto-cast in waves; killing enemies drops glyph fragments; level-up lets the player upgrade a rune family (Flame / Frost / Lightning / Void). Boss every 60 seconds. Target play session: 8–12 minutes.

**Theme angle:** Differentiates from survive_beta (which is the generic reference) by leaning into *element-based weapon synergies* — three runes from the same family stack into a stronger combined effect.

## Session A1 — Project scaffold

Copy `examples/survive_beta/` directory shape: `examples/glyph_runner/{twe.toml, main.twe}`. Strip `main.twe` to a hello-world scene with a single state, a player rectangle, and an `on render()` that confirms `twec play examples/glyph_runner/` runs. Verify clean.

## Session A2 — Core loop: player + camera + enemy

- `entity Player:` with movement (WASD) + HP + camera follow.
- `entity Slime:` enemy with chase-the-player AI.
- Off-screen wave spawner.
- Verify the player can move and slimes chase.

## Session A3 — First rune (auto-cast projectile)

- `entity Rune:` projectile spawned every N seconds at the nearest enemy.
- Circle-vs-circle collision.
- Damage + enemy death + score increment.

## Session A4 — XP + level-up modal

- `entity XPGem:` drop on enemy death.
- Magnetic pickup within radius.
- Level-up modal: pick from 3 upgrade choices (more damage / faster cast / extra projectile).
- Reference: `examples/survive_beta/main.twe` state `upgrading:` (~line 879).

## Session A5 — Rune families + synergies

- Three rune families: Flame (DoT burn), Frost (slow), Lightning (chain).
- Level-up adds a new family-rune; three from the same family unlock a combined effect.
- Visual differentiation by color.

## Session A6 — Boss + game-over polish

- Boss spawns at 60s, 120s, 180s — escalating HP.
- Boss telegraphed attacks (3-second wind-up; player can dodge).
- Game-over: show score + glyph-kill-count + restart.
- Pause menu (Phase 10 primitives).

## Session A7 — Audio + particles

- `sound.play("sfx/cast.wav")` on rune cast (lazy load to avoid the survive_beta headless-audio caveat).
- `sound.schedule("music/loop.wav", 0)` for background music.
- Particles: spark on enemy hit, glyph trail on rune, screen-shake on boss hit.

## Session A8 — Build pipeline

- `twec build --target windows-x86_64 --config release examples/glyph_runner/`.
- Verify the self-extracting `.exe` on a clean Windows machine / VM.
- Iterate bundle size + asset compression.

## Session A9 — Steam release prep (operator-action)

- Steam Direct application ($100). 2–4 weeks for AppID.
- `twe.toml [steam]` section with real AppID + Depot ID.
- `--features steam` build for achievements / cloud saves.
- `tests/steam_smoke.rs` against the AppID (Phase 35).
- Steam Depot upload via `steamcmd`.
- Store page authoring (screenshots, GIFs, trailer).

## Session A10 — Closeout

- `docs/changes/<date>-glyph-runner-closeout.md`.
- What shipped, what slipped, sales tracking starts.

**Phase A exit criteria:** GlyphRunner playable on a clean Windows machine. Steam release operator-action items either completed or explicitly tracked.

---

# Phase B — PixelPop (web arcade)

**Pitch:** A single-screen reflex/tap arcade game. Colored bubbles drift onto the screen from random edges; tap (mouse or touch) to pop them before they reach the center. Combos increase the score multiplier; missing a bubble loses a life. Target play session: 60–120 seconds per run.

**Theme angle:** Web-first design — tight scope so it loads in 2 seconds and the run-to-leaderboard loop is 1 minute. Mobile-touch friendly out of the box (uses Phase 39 `touch.*` builtins with mouse fallback).

## Session B1 — Project scaffold

- `examples/pixel_pop.twe` single-file (no project directory — keeps the dist/web/ layout minimal).
- Hello-world scene: black canvas with the title text.
- Verify `twec play examples/pixel_pop.twe` runs.

## Session B2 — Bubble entities + spawn

- `entity Bubble:` with `x`, `y`, `vx`, `vy`, `radius`, `color`.
- Spawn one bubble per second from a random edge, vectoring toward the center.
- Render as filled circles.

## Session B3 — Input: mouse + touch unified

- Use `mouse.x` / `mouse.y` + `mouse_press.left` on desktop.
- Use `touch.x` / `touch.y` + `touch.is_active` (Phase 39) on mobile / touch displays.
- Tap-on-bubble = pop = +1 score.
- Tap-on-empty = miss = -1 combo.

## Session B4 — Scoring + combo + lives

- Combo multiplier increases per successful pop in a row; resets on miss.
- 3 lives; bubble reaching the center = lose a life.
- HUD: score / combo / lives.
- Use `safe_area.rect()` (Phase 39) so HUD avoids iPhone notches.

## Session B5 — Sound + particles + restart

- `sound.play("sfx/pop.wav")` on bubble pop (each color = different pitch).
- Burst particle on pop.
- Game-over screen with high score persisted via `save.save_to_path("score.json", ...)` — works in browser via Phase 30 localStorage reroute.

## Session B6 — WASM build + browser test

- `cargo run -- build --target wasm32 examples/pixel_pop.twe`.
- Serve `dist/web/` locally; test in Chrome + Firefox + Safari.
- AudioContext unlock verified (sound only after first click).

## Session B7 — itch.io + Pages deploy

- Zip `dist/web/`; upload to itch.io with "playable in browser" flag.
- Adapt `.github/workflows/wasm-demo.yml` for a separate repo.
- Pages deploy verified at `https://<user>.github.io/<repo>/`.

## Session B8 — Closeout

- `docs/changes/<date>-pixel-pop-closeout.md`.

**Phase B exit criteria:** PixelPop playable in Chrome + Firefox + Safari + mobile browsers. Deployed to itch.io and/or GH Pages.

---

# Phase C — GridDuel (LAN versus)

**Pitch:** Two players on the same LAN. Each controls a token on a shared 16×16 grid. Both players race to capture neutral flags; bumping into the opposing token bounces them off (no direct combat). First to 5 flags wins. Lockstep deterministic. Target play session: 3–5 minutes.

**Theme angle:** LAN-party-friendly — no installation, two-machine playtest is the goal. Avoids the rollback-needs-runner-engine issue by being lockstep-natural (token speed ~ 5 grid cells/sec is fine with 4-frame input delay).

## Session C1 — Project scaffold

- `examples/grid_duel.twe` single-file.
- Hello-world scene: render the 16×16 grid + two static tokens.
- Verify `twec play examples/grid_duel.twe` runs.

## Session C2 — Single-player baseline + determinism

- `entity Token:` with grid-cell position + facing.
- Player 1 controlled by WASD; Player 2 by arrow keys.
- Flag entities at fixed positions; capture on token-overlap.
- **Determinism prep:** seed RNG via `random.seed(N)` at scene init.
- Verify via `replay.record("test.log")` + `replay.play("test.log")` — final scene-hash must match.

## Session C3 — Network integration

- Lobby state: H to `net.host(7878, 2)`, J to `net.connect("127.0.0.1:7878")`.
- `state playing:`: `net.send_input(tick)` + `net.tick_ready(tick)` + `net.advance_tick(tick)`.
- Read inputs via `peer[0].key.w` / `peer[1].key.up` etc.
- State hash every 60 ticks via `net.send_state_hash`.

## Session C4 — Two-machine playtest

- Follow `docs/playtest/cross-machine-lan-multiplayer.md`.
- Both machines: `shasum -a 256 grid_duel.twe` matches.
- UDP firewall hole for 7878.
- Two-machine 5-minute playtest; zero desync events.

## Session C5 — Polish + lobby UX

- "Waiting for peer..." screen with peer-IP display.
- Capture/win/restart cycle.
- Reconnect handling: `net.peer_disconnected()` + 10-second grace period.

## Session C6 — Distribution

- `twec build --target windows-x86_64 --config release examples/grid_duel.twe` produces a single-file `.exe`.
- Same .exe distributed to both players (or via Steam — Track A's release path).
- Mention Steam Remote Play Together for non-LAN sales (Steam handles the network bridging).

## Session C7 — Closeout

- `docs/changes/<date>-grid-duel-closeout.md`.

**Phase C exit criteria:** GridDuel plays end-to-end on two machines on a LAN, no desync.

---

# Phase D — CrystalCaves (small 3D Windows)

**Pitch:** First-person cave exploration. Player navigates procedurally-arranged cave rooms with rapier3d physics, collects glowing crystals (Phase 20 point lights for crystal glow), avoids floor-trap pressure plates that drop spikes from the ceiling. Win condition: collect 10 crystals + escape via the exit portal. Target play session: 10–15 minutes.

**Theme angle:** Showcases the 3D stack with a tighter scope than crystal_hunter — three room types, one enemy mechanic (spike traps), one player ability (lantern toggle). Smaller asset budget; runs at 60fps on a 4-year-old GPU.

## Session D1 — Project scaffold

- `examples/crystal_caves/{twe.toml, main.twe}`.
- Hello-world 3D scene: flat ground, one cube, first-person camera. Verify `twec play3d examples/crystal_caves/` runs.

## Session D2 — Character controller + level geometry

- `physics.character(pos, radius=0.4, height=1.8)` kinematic capsule.
- `physics.character_move(handle, dx, dy, dz)` for WASD + gravity.
- Static-mesh level loaded from `cave.glb` (TODO: include a tiny placeholder .glb in `examples/crystal_caves/assets/`; doc a Blender export note).
- First-person camera follows the character handle.

## Session D3 — Crystals + collection

- `entity Crystal:` with `position` + `collected: bool`.
- Render as small mesh with a colored point-light (Phase 20).
- Raycast or proximity check on pickup.
- Counter in HUD; win condition at 10.

## Session D4 — Spike trap mechanic

- `entity Pressure_Plate:` static colliders on the floor.
- `entity Spike:` dropped from the ceiling on plate-trigger (uses `physics.collisions()` to detect plate contact).
- Hitting a spike = -1 HP; 3 HP total.

## Session D5 — Lighting + post-processing

- Sun via `sun.direction(...)` + Phase 28 cascaded shadow maps.
- HDR + ACES tone mapping (Phase 23+26 — automatic).
- `postfx.bloom(true)` for crystal glow.
- `postfx.vignette(0.4)` for cave atmosphere.

## Session D6 — Audio

- `sound.play3d("sfx/crystal_pickup.wav", x, y, z)` — distance-attenuated 3D audio (Phase 23).
- Footsteps via `sound.play` on each grid-cell change.
- Background ambience via `sound.schedule("music/cave_ambience.wav", 0)`.

## Session D7 — Save + load

- Phase 22 typed save namespace: `save.vec3("player_pos", ...)`, `save.int("crystals_collected", n)`, `save.write("slot1")`.
- Auto-save on every crystal pickup.
- Resume-from-save on next launch.

## Session D8 — Build + Steam release (operator-action overlaps Track A)

- `twec build --target windows-x86_64 --config release examples/crystal_caves/`.
- Same Steam release path as GlyphRunner (Track A Session A9).

## Session D9 — Closeout

- `docs/changes/<date>-crystal-caves-closeout.md`.

**Phase D exit criteria:** CrystalCaves runs at 60fps on a 4-year-old Windows GPU. Playable single .exe.

---

## Cross-cutting workflow

For every track, every session:

1. **Author with LLM assistance.** Wire `twec mcp` (Phase 33) into Claude Code / Cursor / Continue. The LLM has access to parse + verify + format + grammar + stdlib_list + stdlib_lookup + apply_patch tools.
2. **Per-commit verify.** `cargo run -- verify <your_game.twe>` before every commit. Phase 33 session 2 returns structured JSON with fix suggestions.
3. **Corpus header on every new file.** `@task / @inputs / @expected / @category / @difficulty` per Phase 33 session 6.
4. **Hot reload during dev.** `cargo run -- play your_game/` with `hot_reload = true` in twe.toml's `[build.dev]`.
5. **Build + smoke-test before each session closeout.** Don't ship a session that breaks `twec verify` or `cargo clippy --release --all-targets -- -D warnings`.

---

## Total scope estimate

| Phase | Sessions | Time (part-time) |
|---|---|---|
| A — GlyphRunner | 10 | 2–4 months |
| B — PixelPop | 8 | 2–3 weeks |
| C — GridDuel | 7 | 1–2 months |
| D — CrystalCaves | 9 | 2–3 months |
| **Total** | **34** | **6–9 months** |

These are honest part-time estimates. Full-time (40 hours/week) cuts them in half.

---

## Status tracker

| Track | Phase | Latest Session | Status |
|---|---|---|---|
| 0 | Shared env | — | complete |
| A | GlyphRunner | A1 — Project scaffold | **in flight** |
| B | PixelPop | B1 — Project scaffold | **in flight** |
| C | GridDuel | C1 — Project scaffold | **in flight** |
| D | CrystalCaves | D1 — Project scaffold | **in flight** |

This file is the canonical status. Update the table per session closeout.
