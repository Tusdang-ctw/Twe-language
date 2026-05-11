# Console targets RFC — NDA-bound model + partner contribution

**Status:** accepted 2026-05-11. Gate for Phase 40 sessions 2 onward.

**Parent:** `docs/05-roadmap.md` Phase 40.

## Question

Phase 40 of the roadmap lists Switch + PS5 + Xbox Series X|S as
**post-v1.0 console targets**. The roadmap entry already flags the
constraint:

> Cannot be shipped open-source. Sketched here for completeness; the
> actual work happens behind NDA with licensed dev kits, in a private
> fork or partner-maintained branch.

The hard question this RFC settles: **what CAN ship in the open-
source `twec` repository, and what CANNOT?** The answer drives Phase
40 sessions 2–7 — without it, every contributor risks accidentally
adding NDA-bound code to the public tree.

## Decision

The open-source `twec` ships **platform-agnostic abstractions** that
are useful on every target (PC, mobile, eventually console). The
console-specific implementations live in **partner-maintained
private forks**.

Concretely:

| Lives in open-source `twec` | Lives in partner private fork |
|---|---|
| Generic "console controller" input layer | Switch / PS / Xbox SDK input bindings |
| Per-button glyph rendering (A/B/X/Y vs △/○/×/□ vs Joy-Con) | First-party graphics API backends (NVN / GNM / GDK D3D12) |
| Achievement / cloud-save / friends *traits* (no impls) | First-party store API implementations |
| Controller-glyph asset format + lookup tables | Signing keys, code-signing pipelines |
| `examples/console_demo.twe` showing the abstractions | Real games using the platform SDKs |
| Partner contribution guide (`PARTNER.md`) | Per-platform certification submission machinery |

The partition is enforceable: every file in the public tree must
compile + run without any Nintendo / Sony / Microsoft SDK present.
PRs adding SDK code get rejected at review.

## Why this model

1. **NDA compliance is non-negotiable.** Nintendo's, Sony's, and
   Microsoft's platform agreements explicitly prohibit publishing
   SDK code, signing keys, or store-API client code on the open
   internet. Violating these terminates the developer agreement
   and the studio's right to ship on that platform. The model has
   to honor this from day one.
2. **Abstractions are independently valuable.** A generic
   "console controller" input layer — `console.controller(i).a` /
   `.b` / `.x` / `.y` / `.left_stick` / `.right_stick` — is useful
   on PC too. Many Steam-class indie games support Xbox controllers
   on PC; the same layer covers both cases. Shipping the
   abstraction in the open-source repo helps every PC + Steam Deck
   user, not just the partner studios.
3. **Partner-driven contribution path is realistic.** A licensed
   indie studio porting a Twe game to one console gets a private
   fork that contains the SDK code; they upstream the platform-
   agnostic abstractions to the public repo as a separate PR. This
   gives partner studios a clear contribution model + gives the
   project a way to grow the abstraction surface as new partners
   join.
4. **No false promise of support.** The roadmap is explicit: Phase
   40 exit criterion is "One first-party game on one console store"
   and it's "partner-driven, not implementer-driven." The open-
   source repo ships the abstractions; it does not commit to
   maintain the SDK ports.

## Abstraction design

### Console controller layer (Phase 40 session 2)

```twe
let pad = console.controller(0)        # first controller, 0-indexed
if pad.a:                              # face buttons by Xbox naming
    fire()
let move = pad.left_stick              # {x, y} normalized
let look = pad.right_stick
if pad.connected:                      # disconnect-aware
    draw_player()
```

The Xbox naming (A / B / X / Y) is the **wire-format** for the layer.
Per-platform glyphs translate it for the user (see session 3). This
mirrors how `Cargo.toml`'s `[features]` block names features in
implementation language while documenting them in user language —
the abstraction has a single canonical name.

`console.controller(i)` returns a record with:
- `connected: bool`
- `a` / `b` / `x` / `y` — face buttons (current frame held)
- `dpad_up` / `dpad_down` / `dpad_left` / `dpad_right`
- `left_shoulder` / `right_shoulder` (L1 / R1 / L / R / LB / RB)
- `left_trigger` / `right_trigger` — analog 0..1 (LT / RT / ZL / ZR)
- `left_stick` / `right_stick` — `{x, y}` records, normalized
- `left_stick_button` / `right_stick_button` (L3 / R3)
- `start` / `select` (Options / Share, +/-, Menu / View)
- `home` — system button (Xbox Guide, PS Home, Switch Home) — usually
  consumed by the platform OS; scripts cannot rely on it but the
  field exists for completeness

Implementation: today the layer wraps `gilrs` (Phase 9's gamepad
crate) for PC + Steam Deck. The `console.*` namespace is what
platform-specific bindings would replace inside a partner fork.

### Glyph rendering (Phase 40 session 3)

```twe
let g = console.glyph("a", style: "xbox")      # → "(A)" or asset key
glyph_text("Press {console.glyph("a", style: "auto")} to start", at: (8, 8))
```

`console.glyph(button, style)` returns a string that scripts can
embed in UI text. Styles:
- `"xbox"` — A / B / X / Y / LB / RB / LT / RT / LS / RS / DPad / Start / Select.
- `"playstation"` — × / ○ / □ / △ / L1 / R1 / L2 / R2 / L3 / R3.
- `"switch"` — Joy-Con button names.
- `"auto"` — detected from the connected controller; falls back to xbox.

Glyphs are returned as Unicode where possible (PS uses Unicode triangles;
Switch and Xbox use plain letters with parens). For pixel-accurate
glyph icons (e.g., the actual Xbox / PS glyph spritesheet) scripts use
`console.glyph_asset(button, style)` returning a sprite key the game
can render with `image()`.

### Platform-service traits (Phase 40 session 4)

`achievements.unlock(id)`, `cloud_save.save(slot, bytes)`,
`friends.list()` — trait stubs that route through `crate::steam::*`
(Phase 15) on Steam builds, **no-op on every other build**.

The traits are the contract. Partner forks provide implementations
that route through the platform's first-party APIs (NSO for Switch,
Trophy for PS, GamerScore for Xbox). The open-source repo ships:
- The trait definitions (in Rust).
- The Steam route (already implemented in `src/steam.rs`).
- No-op fallback so scripts written against the traits compile + run
  on every platform.

### Partner contribution guide (Phase 40 session 5)

`PARTNER.md` at the repo root, covering:
- The "abstractions in public, SDK in private" partition.
- How to set up a partner fork (`git clone` + `git remote add upstream`).
- PR shape for upstreaming abstraction extensions.
- Code-of-conduct + IP attribution boilerplate (partner code stays
  partner-owned; abstractions land under the project's MIT/Apache-2
  dual license once committed upstream).

## What this RFC does *not* settle

- **Specific platform certification requirements.** Each platform has
  its own TRC / TCR / XR ("technical requirements checklist") that
  changes with each SDK release. Out of scope for the open-source
  repo; partner forks are responsible.
- **Pricing + revenue sharing on platform stores.** Out of scope.
- **A list of partner studios.** None exist at Phase 40 closeout time.
  The path exists; partners come or don't.
- **The Steam Deck "console-class PC" question.** Steam Deck runs the
  PC Linux build; it's not a separate target. The `console.controller`
  abstraction already covers its built-in controller via `gilrs`.
- **VR / AR consoles** (PSVR2, Quest, Vision Pro). Out of scope for
  Phase 40. A future phase covers VR if it ever becomes a priority.

## Exit criteria for Phase 40

Per `docs/05-roadmap.md` Phase 40:

- One first-party game on one console store. **(Partner-driven; the
  open-source repo cannot deliver this directly.)**

Codebase-side exit (what this phase actually ships):

- `console.*` namespace with the abstract input + glyph builtins.
- `achievements` / `cloud_save` / `friends` trait stubs with Steam
  implementations + no-op fallbacks.
- `PARTNER.md` partner contribution guide.
- `examples/console_demo.twe` showing the abstractions in use.
- Closeout note enumerating the NDA partition.

## Implementation order (sessions 2–7)

| # | Deliverable | Output |
|---|-------------|--------|
| 1 | This RFC | merged 2026-05-11 |
| 2 | `console.controller(i)` abstraction over `gilrs` | `src/stdlib.rs` `console.*` namespace |
| 3 | `console.glyph` + `console.glyph_asset` Unicode + asset-key lookup | `src/stdlib.rs` |
| 4 | `achievements` / `cloud_save` / `friends` traits + Steam routes + no-op fallbacks | `src/stdlib.rs` |
| 5 | `PARTNER.md` | repo root |
| 6 | `examples/console_demo.twe` | examples |
| 7 | Closeout | `docs/changes/<date>-phase-40-closeout.md` |
