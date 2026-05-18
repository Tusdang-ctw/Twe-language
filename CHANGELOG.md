# Changelog

The deprecation log for the public Twe surface. See
`docs/05-roadmap.md` for the phase-by-phase development log; this
file is the user-facing record of what changed between releases.

The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/) once v1.0
ships. Until then, every minor (v0.x) release is permitted to break
the surface, with deprecations rather than removals where the
removal would be load-bearing.

## v1.0.1 — Polish release (closed 2026-05-18)

> Patch-tier release after v1.0 that hardens the Survivors-class
> path: game feel as one-call procedural effects, audio polish, 2D
> dynamic lighting, save migrations, contributor LSP, replay-on-crash,
> and CI perf snapshots. Full plan in [`docs/v1.0.1-plan.md`](docs/v1.0.1-plan.md);
> closeout at [`docs/changes/2026-05-18-v1.0.1-closeout.md`](docs/changes/2026-05-18-v1.0.1-closeout.md).
> No cloud-hosted assets — fully procedural fx/lighting libraries
> instead, for the determinism + offline-`.exe` + LLM-grounding
> reasons enumerated in the plan. Net **+41 tests; 979 passing.**

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

- **`tween.*` deterministic easing primitives** (Session 2,
  2026-05-13). Pure functions of `t` — replay-safe by construction:
  `tween.ease(name, t)`, `tween.lerp(a, b, t)`, `tween.lerp_eased`,
  `tween.bounce`, `tween.shake`, `tween.eases()` enumerates the
  fourteen supported curves.

- **`light2d.*` dynamic 2D lighting** (Session 3, 2026-05-14).
  Cheap additive multi-light pass with optional AABB shadow caster.
  `light2d.add(at, color, radius, flicker)`, `light2d.set_ambient`,
  `light2d.cast_shadows`, `light2d.clear`. 16-light budget per
  frame. Reference: `examples/dungeon_demo.twe`.

- **Audio polish — pooling + ducking + music layers** (Session 4,
  2026-05-14). `sound.pool("path", max_voices: N)` lifts the
  per-`play` voice limit; `sound.duck` ducks a channel while a
  triggered sound plays. New `music.*` namespace: `music.layer`
  (weighted blend), `music.crossfade`, `music.stop`. Reference:
  `examples/audio_demo.twe`.

- **`save SaveSlot:` block + version migrations** (Session 5,
  2026-05-14). Language-level save schema declaration with per-
  version migration clauses; closes the `docs/07-save-system.md`
  "What is open" item. `save SaveSlot:` declares versioned fields
  with defaults; `migration from N:` blocks run when an older
  save loads. Reference: `tests/programs/save_migrate.twe`.

- **Per-state pause opt-out** (Session 6, 2026-05-15). `state foo:`
  blocks accept `pause: false` (or the `persistent` alias) so debug
  overlays / pause menus / toast HUDs keep running while gameplay
  states are paused. Closes the second `CLAUDE.md` "What is open"
  item.

- **Nine-slice / nine-patch panels** (Session 7, 2026-05-15).
  `panel(at, size, skin: nine_slice("path", border: N))` lets the
  Phase 10 widget set render skinned panels. Solid-color fallback
  preserved.

- **`camera2d.*` follow + zoom + cinematic pan** (Session 8,
  2026-05-15). Survivors-class follow camera in a one-liner:
  `camera2d.follow(entity, lerp, deadzone)`, `camera2d.zoom_to`,
  `camera2d.cinematic_pan`, `camera2d.bounds`. `examples/survive_beta`
  rewrites its hand-rolled follow logic against the new API.

- **LSP cross-module find-references + rename** (Session 9,
  2026-05-16). Phase 13 modules + Phase 3/13 LSP now support
  cross-`import`-boundary go-to-definition, find-references, and
  rename refactor (multi-file safe; word-boundary scan skips
  strings + `#` comments).

- **Replay-on-crash + `twec replay`** (Session 10, 2026-05-16).
  Always-on input ring stores the last 30 seconds of frames; the
  crash reporter writes a sibling `twec-crash-<secs>-<pid>.replay`
  next to every `.log`. New CLI subcommand `twec replay <script>
  <replay-file>` re-runs the bug.

- **CI perf snapshot + `twec perf-snapshot` / `twec perf-diff`**
  (Session 11, 2026-05-16). New `.github/workflows/perf.yml` runs
  `cargo bench --bench vm` on push-to-main, scrapes criterion's
  `target/criterion/` into a deterministic JSON document, and
  diffs against the checked-in
  `docs/perf-snapshots/v1.0.1-baseline.json`. Default 5% regression
  threshold fails CI.

- **Localization plurals** (Session 12, 2026-05-18). CLDR-style
  cardinal plural rules for **en / es / de / ja / pl** plus ten more
  Steam-relevant locales (fr / it / nl / pt / sv / no / da / zh /
  ko / th / vi / ru / uk). `lang.t_plural(key, n, args)` selects a
  `<key>.<one|few|many|other>` template; `{n}` substitutes the
  count, `{0}+` substitutes positional args (same shape as
  `lang.tf`). `lang.plural_category(locale, n)` exposes the rule
  directly; `lang.set_plural_rule(locale, base_locale)` aliases
  long-tail locales onto a built-in rule (e.g. `pt-BR` → `es`).
  Closes the third `CLAUDE.md` "What is open" item.

- **`twec doctor`** (Session 13, 2026-05-18). Triage diagnostic
  command. Reports twec version + target triple + active feature
  flags + effective crash directory + last 3 crash logs + cache
  directory (via `$TWEC_CACHE_DIR`). `--json` for the
  LLM-grounded support workflow; `-o PATH` writes to a file.
  Always exits 0.

### Closeout (Session 14)

See [`docs/changes/2026-05-18-v1.0.1-closeout.md`](docs/changes/2026-05-18-v1.0.1-closeout.md).

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
