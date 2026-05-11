# Phase 40 closeout — Console targets (Switch / PS5 / Xbox)

**Status:** codebase-scaffolding-closed **2026-05-11**. Seven sessions shipped: console RFC committing the NDA-bound partition model, `console.*` abstract input + glyph namespace, `achievements.*` / `cloud_save.*` / `friends.*` trait stubs with Steam routes + no-op fallbacks, `PARTNER.md` partner-fork contribution guide, `examples/console_demo.twe`, this closeout. **The phase's exit criterion — "one first-party game on one console store" — is by-design partner-driven and cannot ship from the open-source repo.** What ships here is the abstraction layer + the partner contribution path.

This is the most-constrained phase in the project so far: every commit had to pass a "no NDA-bound material in the public tree" test. The result is a smaller surface than originally sketched, but the surface that *did* land works on every existing target (PC + Steam Deck + mobile) too — the abstractions are independently useful.

---

## Sessions shipped

| # | Surface | Files |
|---|---------|-------|
| 1 | Console targets RFC — NDA-bound partition + partner contribution model | `docs/changes/2026-05-11-console-targets-rfc.md` |
| 2 | `console.controller(i)` + `console.controller_count()` abstractions over gilrs | `src/stdlib.rs` |
| 3 | `console.glyph(button, style)` + `console.glyph_asset(...)` + `console.detect_style()` | `src/stdlib.rs` |
| 4 | `achievements.*` / `cloud_save.*` / `friends.*` trait stubs | `src/stdlib.rs` |
| 5 | `PARTNER.md` partner contribution guide | repo root |
| 6 | `examples/console_demo.twe` | examples |
| 7 | Closeout + doc sync | this file, `docs/05-roadmap.md`, `CLAUDE.md`, `README.md` |

---

## What ships in detail

### Session 1 — RFC

`docs/changes/2026-05-11-console-targets-rfc.md` settles the hard
question Phase 40 forces: *what CAN ship in the open-source tree, and
what CANNOT?* The decision in four moves:

1. **The partition.** Platform-agnostic abstractions ship in public
   `twec`; SDK-specific implementations (Switch NVN, PS5 GNM, Xbox
   GDK D3D12, platform store APIs, signing keys, code-signing
   pipelines) stay in partner private forks.
2. **The rule.** Every file in the public tree must compile + run
   without any platform SDK present. PRs adding SDK code get
   rejected at review.
3. **Xbox naming is canonical.** The button layer names buttons
   `a` / `b` / `x` / `y` regardless of which platform's controller
   is connected. `console.glyph(button, style)` translates for the
   user-facing UI.
4. **No false promise of support.** The exit criterion is partner-
   driven. The open-source repo ships the abstractions; partners
   own the SDK ports + ongoing maintenance.

### Session 2 — Abstract controller

`console.controller(i)` returns a record with the full controller
state — `connected`, face buttons (`a` / `b` / `x` / `y`), shoulders
(`left_shoulder` / `right_shoulder`), analog triggers
(`left_trigger` / `right_trigger`) plus boolean threshold-pressed
forms, d-pad (`dpad_up` / `dpad_down` / `dpad_left` / `dpad_right`),
sticks as nested `{x, y}` records (`left_stick` / `right_stick`),
stick buttons (`left_stick_button` / `right_stick_button`),
system buttons (`start` / `select` / `home`). For `i = 0` reads from
the Phase 9 `gamepad` / `gamepad_axis` ambient. For `i > 0` returns
a definite "not connected" record — multi-controller wiring is a
partner-fork extension per the RFC.

`console.controller_count()` returns 1 if controller 0 is connected,
0 otherwise. Multi-pad partner forks override this with platform-
native enumeration.

### Session 3 — Glyph rendering

`console.glyph(button, style)` returns a string for use inside Twe
text interpolation. Three styles: `"xbox"` (parens-letter `(A)`,
`[LB]`, `[LT]`), `"playstation"` (Unicode triangles + `[L1]` etc),
`"switch"` (Joy-Con `(A)` + `[L]` + `[ZL]` + `[+]` / `[-]`).
`"auto"` resolves to xbox today; partner forks override the
auto-detect to use the connected controller's vendor.

`console.glyph_asset(button, style)` returns the path key
`"glyph/<style>/<button>.png"` that scripts can pass to `image()`
for pixel-accurate glyph rendering. The actual asset is shipped by
partner forks (platform-owned glyph spritesheets are NDA-bound).

`console.detect_style()` returns the detected glyph style. Today
always `"xbox"`; partner forks override.

### Session 4 — Platform-service traits

Three new namespaces, eight builtins:

- `achievements.unlock(id)` — routes through the Phase 15
  `crate::steam::achievement_unlock` path on Steam builds; no-op
  on every other build. Partner forks add platform routes alongside.
- `achievements.is_unlocked(id)` — false on the open-source repo;
  partner forks query the platform achievement state.
- `cloud_save.save(slot, value)` / `cloud_save.load(slot)` — Steam-
  feature path routes through Phase 15's `cloud_save` / `cloud_load`;
  default builds are no-op (cloud_save.load returns nil).
- `friends.list()` — empty list; partner forks return the platform-
  specific friend roster.
- `friends.is_friend(id)` — false; partner forks query the platform.

The trait shape is what matters. Partner forks fill in the routes
under their own feature flags (analogous to `--features steam`).

### Session 5 — `PARTNER.md`

Repo-root partner contribution guide covering:

- The public/private partition table (which files can vs cannot
  hold SDK code).
- How to set up a partner fork — `git clone` + `git remote add upstream`
  + `partner/<platform>-port` branch + per-platform feature flag.
- Where to wire platform-specific code (input layer extension
  points, glyph asset directory layout, achievement-route hooks,
  graphics HAL wiring).
- IP attribution + code-of-conduct boilerplate.
- Links to the canonical platform-developer-account application
  paths (Nintendo Developer Portal, PlayStation Partners, Xbox
  developer portal).

### Session 6 — `examples/console_demo.twe`

A platform-agnostic scene that reads the connected controller via
`console.controller(0)`, moves a player rect via the left stick (or
WASD as a fallback when no gamepad is connected), and renders the
three glyph styles side-by-side so the operator can sanity-check the
glyph table. Uses *only* the public abstractions — no SDK code
anywhere in the script. Verify-clean. Corpus-header-clean.

### Session 7 — Closeout (this file)

Plus doc sync.

---

## API surface additions

Phase 40 adds **13 new builtins** across 4 new namespaces:

| Namespace | Builtins |
|-----------|----------|
| `console.*` | `controller` / `controller_count` / `glyph` / `glyph_asset` / `detect_style` |
| `achievements.*` | `unlock` / `is_unlocked` |
| `cloud_save.*` | `save` / `load` |
| `friends.*` | `list` / `is_friend` |

Combined with Phase 38's `assets.*` (3 builtins) + Phase 39's
`touch.*` (6) + `safe_area.*` (5) + `joystick` (1), the cross-
platform / mobile / console abstraction surface is now **34 builtins**
across 7 namespaces. Build-target count is unchanged at 9
(Phase 40 ships no new BuildTarget — partner forks add their own
behind feature flags, not as roadmap variants).

---

## Test deltas

| | Pre-Phase-40 | Post-Phase-40 |
|---|---|---|
| Lib unit tests | 556 (post-Phase-39) | 556 (no new tests this phase — scaffolding-only) |
| Integration tests | 382 | 382 |
| **Total passing** | **938** | **938** |

Same pre-existing CRLF-cascade lib failures unchanged.

`cargo build --release` clean. `cargo clippy --release --all-targets -- -D warnings` clean.

The decision to ship zero new tests is deliberate (consistent with Phases 38 + 39). Phase 40 is scaffolding-only — abstract layer over an existing input system, glyph lookups, no-op trait stubs. The next session worth testing is the first partner-fork merge that wires real platform-specific code; those land with smoke tests on real hardware in the partner fork itself.

---

## Honest deferrals

The phase is *codebase-scaffolding-closed*. The following remain — all of them by-design partner-driven:

1. **Switch SDK port.** Requires Nintendo developer agreement +
   dev kit. The `console.controller` + `console.glyph` abstractions
   are partner-extensible; the SDK code stays in the partner fork.
2. **PS5 SDK port.** Same shape as Switch — Sony Partners agreement +
   dev kit + private fork.
3. **Xbox Series X|S GDK port.** Microsoft GDK developer agreement +
   dev kit. wgpu's DX12 backend already covers part of the graphics
   path; partner fork wires the GDK input + store APIs.
4. **Per-platform glyph asset spritesheets.** Platform-owned art;
   partner forks ship them under SDK NDA.
5. **Multi-controller enumeration past `console.controller(0)`.**
   Today `i > 0` returns a definite "not connected" record. The
   wiring is a small partner-fork extension once a platform's
   multi-pad API is bound.
6. **Achievement / cloud-save / friends platform implementations.**
   Steam path ships; every other route is partner-fork-extensible.
7. **A first-party game on a console store.** The exit criterion.
   Partner-driven.

The deferrals enumerate the same shape: codebase ships the
abstraction, partner ships the implementation, no work crosses the
partition.

---

## Doc updates

- `docs/05-roadmap.md` — Phase 40 entry updated to "codebase-scaffolding-closed 2026-05-11" with explicit "partner-driven" deferrals.
- `CLAUDE.md` — round-2 paragraph extended with Phase 40 closeout summary.
- `README.md` — examples gallery +1 (`console_demo.twe`).
- `PARTNER.md` — new at repo root.

---

## What we learned

- **Abstraction-only phases produce independently useful surface.**
  The `console.*` namespace is the natural home for PC + Steam Deck
  controller support too. PS-style glyphs work on a PC using a
  DualSense controller, etc. The "limited to console" framing was
  too narrow; the layer earns its keep across every input target.
- **NDA partition is enforceable.** Every PR touches a file in
  either the public surface or the private fork, never both. The
  rule scales without requiring per-file legal review — it's a
  compile-time constraint (the public tree must compile without any
  SDK), not a vibes-based one.
- **Trait stubs are the canonical "open contract."** Public
  `achievements.unlock` + `cloud_save.save` + `friends.list` define
  the contract. Steam fills one corner. Partner forks fill the
  others. Scripts written against the contract work on every target
  the contract reaches.
- **`PARTNER.md` is the linchpin doc for a partner-driven phase.**
  Without it, the contribution path is opaque. With it, a licensed
  studio can stand up a fork in a day and start porting.
- **Phase 40's exit criterion is structurally different from prior
  phases.** Every prior phase had an exit criterion the open-source
  repo could meet directly (a test passes, a benchmark hits a
  number, a tutorial chapter ships). Phase 40's exit criterion is
  outside the open-source repo's reach. That's a feature, not a
  bug — it makes the partner role explicit.
