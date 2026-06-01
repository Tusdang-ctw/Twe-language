# Changelog

The deprecation log for the public Twe surface. See
`docs/05-roadmap.md` for the phase-by-phase development log; this
file is the user-facing record of what changed between releases.

The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/) once v1.0
ships. Until then, every minor (v0.x) release is permitted to break
the surface, with deprecations rather than removals where the
removal would be load-bearing.

## Unreleased

### Added
- `os.data_dir(app)` — returns the platform-correct, per-user, writable
  directory (`%APPDATA%\<app>` on Windows, `~/Library/Application Support/
  <app>` on macOS, `$XDG_DATA_HOME`/`~/.local/share/<app>` on Linux; `""`
  on WASM), creating it on first call. Gives a shipped game a safe place
  to write saves/settings when its bundle is mounted read-only. Std-only,
  no new dependency.

## v1.0.2 — Deferral-debt patch (closed 2026-05-26)

> Patch-tier release after v1.0.1 that closes the structural half
> of two long-open `What is open` items plus four cross-phase
> deferrals from Phases 13 / 32 / 37 / 39. Every retained session
> is pure parser sugar over existing builtins or one small additive
> runtime hook. Full plan in
> [`docs/v1.0.2-plan.md`](docs/v1.0.2-plan.md); closeout at
> [`docs/changes/2026-05-26-v1.0.2-closeout.md`](docs/changes/2026-05-26-v1.0.2-closeout.md).
> Session 3 (`entity X: lod` / `rollback` parser sugar) was cut at
> the planned spike — runtime targets aren't ready; defers to v1.1
> alongside Phase 32 wgpu render integration + Phase 37 eval-side
> rewind engine. **Net +13 tests; 1004 passing.** Zero new public
> builtins, zero new AST variants.

### Added

- **`save SaveSlot:` block + `migration from N:` clauses** (Session 1,
  Path B anchor-only). Pure parser sugar over the v1.0.1 stateless
  schema-version primitives. `version: N` declares the current
  schema; each `migration from M:` body runs when
  `save.loaded_version() ∈ {1..M}` and `M < N`. Closes the
  structural half of the v1.0.1 Session 5 deferral; typed-field
  Path A remains v1.1 work. See
  [`docs/07-save-system.md`](docs/07-save-system.md) §What v1.0.2
  implements.
- **`state X: pause: false` / `state X: persistent` parser sugar**
  (Session 2). Lowers to the v1.0.1 `persistent_state(name)`
  registry. `persistent` is an alias for `pause: false`; both forms
  inject a top-level `persistent_state("X")` call after the
  enclosing declaration. Closes the v1.0.1 Session 6 parser-sugar
  deferral.
- **`lang.set_plural_rule(locale, fn)` accepts Twe closures**
  (Session 4). Custom plural rules `(n: int) -> string` for the
  long-tail locales the CLDR built-ins don't cover. Closes the
  v1.0.1 Session 12 alias-only deferral. Exposes
  `eval::call_function` as `pub(crate)` for stdlib callback paths.
- **`twec run <dir>` auto-detects `main.twe`** (Session 6). Routes
  through the module loader; multi-file projects work without
  `/main.twe` on the command line. Closes the Phase 13 closeout
  deferral.
- **`touch.tap_count` play-loop hook** (Session 7). Sliding-window
  detector — taps held <250ms count, window is 500ms.
  Tap-press / tap-release diff runs once per frame against
  `macroquad::input::touches()` from the play loop. Closes Phase 39
  deferral #5.
- **MSG_HELLO mode-mismatch handshake check** (Session 8). 1-byte
  mode field in the MSG_HELLO payload; mismatched peers receive
  `MSG_BYE` from the host and a clean error from the client side.
  Closes Phase 37 deferral #4. Backward-compatible: pre-v1.0.2
  4-byte hello payloads still accept (assume same mode).

### Fixed

- **LSP cross-module rename no longer false-positives inside
  triple-quoted strings** (Session 5). `find_occurrences` now uses
  a lexer-driven scan instead of a per-line byte scan; identifier
  tokens come from the lexer, so string contents are never matched
  regardless of how many lines they span. The byte-scan path is
  retained as a fallback for documents the lexer rejects.

### Internal

- **VM-mirror parity test for `world.*` + `terrain.*` builtins**
  (Session 9). The 35 builtins were already reachable from the
  bytecode VM via `VM::new()` → `stdlib::install` → globals copy;
  the test pins the path so a future regression that shadows the
  `world` Object trips here rather than at release-tag time.
- **`docs/06-design-document.md` §7.20 writeup** for the
  `world.*` + `terrain.*` namespaces (Session 10) with worked
  examples from `examples/openworld_demo.twe`. Closes the Phase 32
  doc deferral.
- **EXIT GATE: `examples/survive_beta/main.twe` migrated onto
  `save SaveSlot:`** (Session 11). LOC delta +2 — the block-header
  fixed cost, accepted in the plan's honest exit-criterion
  revision. `tests/programs/v1_0_2_sugar.twe` exercises both
  shipped sugar paths end-to-end.

### Notes

- Session 3 (`entity X: lod = [...]` / `entity X: rollback = true`
  parser sugar) was cut at the planned 30-minute spike. Tuple-with-
  named-fields isn't real Twe syntax, the rollback runtime has no
  per-entity hook, and both halves are defer-on-defer over
  phase-sized runtime work. Re-enters in v1.1 alongside its
  respective runtime follow-on.
- No new keywords: `save`, `version`, `from`, `migration`,
  `persistent`, `pause` all stay as contextual idents recognized by
  the parser. The Phase-35 API stability snapshot
  (`docs/api-snapshots/2026-05-10-baseline.json`) sees only
  additive surface.

---

## v1.0.1 — Polish release (closed 2026-05-18)

> Patch-tier release after v1.0 that hardens the Survivors-class
> path: game feel as one-call procedural effects, audio polish, 2D
> dynamic lighting, save migrations, contributor LSP, replay-on-crash,
> and CI perf snapshots. Full plan in [`docs/v1.0.1-plan.md`](docs/v1.0.1-plan.md);
> closeout at [`docs/changes/2026-05-18-v1.0.1-closeout.md`](docs/changes/2026-05-18-v1.0.1-closeout.md).
> No cloud-hosted assets — fully procedural fx/lighting libraries
> instead, for the determinism + offline-`.exe` + LLM-grounding
> reasons enumerated in the plan. Net **+53 tests; 991 passing**
> (includes closing the 12 pre-existing CRLF-cascade failures via
> a one-line lexer fix).

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

- **Save schema versioning (MVP)** (Session 5, 2026-05-14). Three
  builtins on `save.*`: `set_schema_version(n)` stamps the in-memory
  store; `schema_version()` reads it; `loaded_version()` reads what
  the on-disk save was stamped with. Scripts branch on the loaded
  version to run their own migration logic. **Honest scope reduction:**
  the language-level `save SaveSlot:` block + `migration from N:`
  sub-blocks defer to v1.0.2 (needs lexer / parser / AST work).
  Reference: `tests/programs/save_schema_version.twe`.

- **Per-state pause opt-out (MVP)** (Session 6, 2026-05-15). Stdlib
  registry of "persistent" state names: `persistent_state(name)` /
  `clear_persistent_state(name)` / `clear_persistent_states()` /
  `is_persistent_state(name)`. The eval / VM pause filter walks the
  registry and keeps registered states ticking while the global
  pause flag is set, so debug overlays / pause menus / toast HUDs
  keep running. **Honest scope reduction:** parser-sugar form
  (`state X: pause: false` / `state X: persistent`) defers to v1.0.2.
  The MVP closes the *functional* `CLAUDE.md` "What is open" item.

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

### Fixed

- **CRLF blank-line indent tracker** ([src/lexer.rs](src/lexer.rs)).
  `handle_line_start` now treats a lone `\r` (Windows blank line)
  as a blank-line marker, alongside `\n` / `#` / EOF. Without this,
  40 of 53 `examples/*.twe` files on Windows checkouts tripped the
  parser with a phantom column-0 Indent token at the next non-blank
  line. Closes the 12 pre-existing CRLF-cascade test failures that
  had carried through v1.0.

### Changed

- **`sound.pool` accepts a string path** in addition to a loaded
  handle. The plan's documented call shape is `sound.pool("sfx/
  hit.wav", max_voices: 8)`; the previous implementation only
  accepted a `sound.load(...)` handle, which can't run at top level
  before macroquad initialises. Pool is voice-budget declaration —
  the asset doesn't need to exist yet.

- **`examples/survive_beta/main.twe` rewritten to use v1.0.1 polish
  APIs.** All four damage sites funnel through a `take_player_damage`
  helper that calls `fx.hit_flash` / `fx.screen_shake` /
  `fx.damage_number`; hand-rolled camera-clamp replaced with
  `camera2d.bounds` + `camera2d.follow(deadzone: (60, 40))`;
  boss-arrival shake + ground shockwave; `save.set_schema_version(1)`
  + `sound.pool(...)` declared at top level. **Net: 1300 → 1286 LOC
  (-14).** Closes Exit Criterion 5 in [`docs/v1.0.1-plan.md`](docs/v1.0.1-plan.md).

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
