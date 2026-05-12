# Changelog

The deprecation log for the public Twe surface. See
`docs/05-roadmap.md` for the phase-by-phase development log; this
file is the user-facing record of what changed between releases.

The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/) once v1.0
ships. Until then, every minor (v0.x) release is permitted to break
the surface, with deprecations rather than removals where the
removal would be load-bearing.

## v1.0.1 — Polish release (in development)

> Patch-tier release after v1.0 that hardens the Survivors-class
> path: game feel as one-call procedural effects, audio polish, 2D
> dynamic lighting, save migrations, contributor LSP, replay-on-crash,
> and CI perf snapshots. Full plan in [`docs/v1.0.1-plan.md`](docs/v1.0.1-plan.md).
> No cloud-hosted assets — fully procedural fx/lighting libraries
> instead, for the determinism + offline-`.exe` + LLM-grounding
> reasons enumerated in the plan.

### Added

- **`fx.*` procedural VFX library** (Session 1, 2026-05-12).
  Twelve call-and-go effects covering the standard Survivors-class
  hit-feedback vocabulary, all procedural — no PNGs, no shaders,
  no asset CDN:
  - `fx.hit_flash(at, size, color, duration)` — tint flash over a sprite rect
  - `fx.screen_shake(amount, duration)` — canonical screen-shake (`camera.shake` kept as a back-compat alias; shares state)
  - `fx.hit_stop(duration)` — freeze gameplay for N seconds (counts in physics ticks, replay-safe)
  - `fx.damage_number(at, value, color)` — rising fading number
  - `fx.crit_text(at, value)` — bigger yellow crit text
  - `fx.death_burst(at, count, color)` — radial particle explosion
  - `fx.pickup_pop(at, color)` — expanding outlined circle
  - `fx.dash_trail(at, color)` — call per-frame to leave a streak
  - `fx.level_up_ring(at, color)` — expanding ring
  - `fx.blood_splat(at, dir, color)` — directional cone splatter
  - `fx.muzzle_flash(at, dir)` — gunfire flash
  - `fx.ground_shockwave(at, radius)` — white expanding ring

  Reference: [`examples/fx_demo.twe`](examples/fx_demo.twe). Documented in
  [`docs/06-design-document.md`](docs/06-design-document.md) §7.8b.
  All four play-loop variants wired (tree-walker `run_loop` /
  `run_loop_wasm` / `run_loop_embedded`, bytecode VM `run_loop_bytecode`).
  +4 unit tests. **942 tests pass.**

### Sessions pending

Sessions 2–14 per [`docs/v1.0.1-plan.md`](docs/v1.0.1-plan.md): tween,
light2d, audio pool/duck/music-layers, `save SaveSlot:` migrations,
per-state pause opt-out, nine-slice panels, camera2d follow/zoom/pan,
LSP cross-module rename, replay-on-crash, CI perf snapshot,
localization plurals, `twec doctor`, closeout.

---

## v0.1.0 — First public release (2026-05-07)

The first public-tagged release of Twe. Everything below the line
ships in this build; what's open and tracked in CLAUDE.md / the
roadmap is not in scope here.

### Highlights

- **2D runtime** (`twec play`) — macroquad-backed game loop, full
  UI widget set (button, slider, dropdown, panel, stack, flex,
  grid, scroll, text input, key-input rebind), pause stack,
  settings + localization, gamepad, particles, clipboard, hot
  reload, screenshot (F12), frame-time HUD (F3), crash reporter.
- **3D runtime** (`twec play3d`) — wgpu pipeline with rapier3d
  physics, glTF 2.0 multi-node scene flatten + GPU skinning +
  animation channel sampling, 8 point lights + Blinn-Phong, 2K
  shadow maps with 3×3 PCF, HDR linear lighting + ACES filmic
  tone mapping + vignette, frustum culling, dynamic instance
  buffer, distance-attenuated 3D audio, KinematicCharacterController
  with raycasts + collision events, typed `save.*` namespace.
- **Visual runtime** (`twec play_visual`) — `visual` blocks
  compile to WGSL fragment shaders.
- **Build pipeline** (`twec build`) — produces a self-extracting
  Windows `.exe` with the bundled game + Twe runtime; macOS
  `.app` and Linux `.AppDir` directory layouts also supported
  (per-target binaries via cargo-dist in this release).
- **Module system** + **strict mode v2** + **verified-mode JSON
  diagnostics** for LLM tool-use loops.
- **Tooling** — `twec fmt` (trivia-preserving since Phase 27),
  `twec verify`, `twec types`, `twec profile` (Chrome trace),
  `twec info`, `twec bench` (criterion-based), tree-sitter
  grammar, LSP with hover + completion + go-to-definition.
- **737 tests pass** across lib + 12 integration binaries.
- **Two reference games**: `survive_beta` (Vampire-Survivors
  clone, ~1300 lines) and `crystal_hunter` (3D FPS, ~250 lines).

### Released artifacts

Cross-platform binaries attached to the GitHub Release for:

- `x86_64-pc-windows-msvc` (`.zip`)
- `x86_64-unknown-linux-gnu` (`.tar.gz`)
- `x86_64-apple-darwin` (`.tar.gz`)
- `aarch64-apple-darwin` (`.tar.gz`)

Each archive contains the `twec` binary plus README, LICENSE,
and CHANGELOG.

### Known limitations carried into v0.1

- Bytecode VM is partial: rejects `on render():`, keyword
  arguments to builtin calls, dialogue, and non-literal field
  defaults. The tree-walker is the canonical execution path;
  `--vm bytecode` falls back with a clean compile error.
- The bytecode VM is currently 1.1×–1.8× *slower* than the
  pre-NaN-tag baseline on tight integer loops; the 3× target is
  unmet but the criterion harness (`benches/vm.rs`) is in place
  to drive it down.
- Auto-pause-on-window-blur ships only on Windows;
  macOS / X11 / Wayland focus paths stub `is_focused() = true`.
- Cross-compiled per-target twec runtimes for the macOS `.app`
  and Linux `.AppDir` layouts produce empty shells today; the
  cargo-dist release pipeline (this release) fills them in.

## v0.7 (Phase 13) — Modules + type-system stability

**Status:** in development.

This is the public-API freeze that v0.8+ depends on. Anything
flagged with `@deprecated("since v0.7")` here will keep working in
v0.7.x and v0.8 (a 12-month carry-over per
`docs/05-roadmap.md` §"Phase 13"), then be removed in v1.0.

### Added

- **Module / package system.** `import "<path>"` and
  `import "<path>" as Alias` bind a module value whose fields are
  the imported file's top-level names. Multi-file projects are
  supported out of the box; the importer's directory is the
  default search path.
- **`twe.toml [dependencies]`.** Each entry maps a logical name to
  a search path (table form) or a version pin (string form). The
  resolver consults dependency paths before the importer's directory.
- **Strict mode v2.** Structural-record subtyping (`{x: int, y: int}`)
  and Luau-style lax narrowing (a Union → variant assignment is
  accepted as an implicit narrowing assertion).
- **Verified mode (Tier 3).** `# verified` directive + the
  `twec verify <file>` subcommand emit a JSON document an LLM can
  sit in a self-correction loop with.
- **`@deprecated("since vX.Y")` annotations.** Attach to top-level
  function and type declarations. `twec verify --warn-deprecated`
  surfaces a `deprecation` warning per use site.

### Deprecated (since v0.7)

(None yet — first cycle. As the surface evolves through v0.7.x and
into v0.8, additions here document the 12-month-carry-over schedule
each retired symbol is on.)

### Changed

- The `# strict` directive's behavior: structural records and
  Union-to-variant lax narrowing are now part of the strict
  contract. Programs that relied on strict rejecting these will
  see fewer diagnostics. No source-level breakage — the change is
  purely "fewer errors in strict mode."

### Removed

(None.)

---

Earlier phases (v0.1 — v0.6) are tracked in
`docs/changes/` as per-session closeout notes; this file picks up at
v0.7 because the API-freeze contract is what users care about, and
that's a Phase 13 concern.
