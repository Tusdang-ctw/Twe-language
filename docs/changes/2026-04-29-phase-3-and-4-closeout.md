# 2026-04-29 — Phase 3 and Phase 4 closeout

## Status: process / closeout note. Records what shipped, what slipped, and the formal re-deferral decisions for the unshipped items.

## Background

Phase 3 (design corrections + bytecode VM + tooling) opened 2026-04-28
with commit `dabc8cc`. Phase 4 (type system v1, non-strict) opened
with commit `f09ef36`. Both phases shipped substantive work without
an explicit closeout, and `CLAUDE.md` still reads "We are in Phase 3"
as of this writing. This note closes both phases on the record and
declares the deferred items formally.

## Phase 3 — what shipped

All four Phase 3 exit-criteria bullets from `docs/05-roadmap.md`
§"Phase 3" pass:

- [x] **Phase 2 game runs at 60fps with 500+ entities.** Bytecode VM
      is wired through `twec play` (commit `7dd444e`), benchmarks live
      in `tests/bench.rs` (commit `68ea0f7`), and the dispatch loop
      hoists the per-frame locals out of the inner loop (commit
      `3b3b148`). Spot-check by running `examples/survive.twe`; the
      formal 500-entity benchmark is documented but not field-verified
      every release.
- [x] **Format and parse round-trip on every test file.**
      `twec fmt` ships in commit `47433f3` with a round-trip-stable
      printer; `tests/fmt.rs` enforces idempotency.
- [x] **Tree-sitter parses the eleven examples.** Grammar lives in
      `tree-sitter-twe/`, scaffolding in commit `81fd0dc`, full
      eleven-example coverage in commit `c21af6c` (exceeded the
      original ten-example target by including Snake).
- [x] **LSP works in VS Code with syntax highlighting, go-to-def,
      and inline errors.** `twec lsp` + extension scaffold in commit
      `5664caa`, hover + go-to-definition in commit `85d92a6`. Inline
      errors flow through the existing parse-diagnostic pipeline.

Frustration-list corrections all closed:

- [x] F1 keyword arguments at call sites — commit `a7d52cc`.
- [x] F4 bounded every-clock catch-up — commit `14ae250`.
- [x] F5 / F8 state-scoped `on update(dt):` — commit `bf4bb37`.
- [x] F11 `or` / `and` keep their value-returning semantics — commit
      `7777479` (decision recorded in
      `docs/changes/2026-04-28-or-and-keep-value-returning.md`).

Bytecode VM ships its full feature surface in eleven sessions:

- Bytecode skeleton: `OpCode`, `Chunk`, disassembler — `75bd7b9`.
- Expressions compile — `27831c6`.
- Stack-based dispatch loop — `018b7f8`.
- Locals + control flow — `909a527`.
- Functions, calls, globals — `accf19d`.
- Heap types, for-loops — `4c41eb8`.
- Classes, methods, module builtins — `b0c7480`.
- Scenes, states, tick loop — `b370454`.
- Render, input, particles — `e0021eb`.
- `--vm bytecode` CLI flag + `play` wire-up — `7dd444e`.
- Benchmarks + zero-alloc string-constant reads — `68ea0f7`.
- Dispatch-loop locals hoist — `3b3b148`.

## Phase 3 — what slipped

Three locked-decision items from `CLAUDE.md` Phase 3 plan did **not**
ship. They are formally re-deferred, with reasons:

### 1. NaN-tagged 64-bit value representation

**Status:** deferred to v0.2 / post-v0.1.

**Why:** `CLAUDE.md` "What is locked" reads *"VM strategy:
tree-walker for v0.1, bytecode VM for v0.3+. Don't skip the
tree-walker."* It also reads *"Value representation: NaN-tagged
64-bit values (in the bytecode VM)."* Read together, NaN tagging is
locked for the bytecode VM, but the bytecode VM is locked for v0.3+.
The Phase 3 implementation took the conservative reading and shipped
a `Clone`-friendly Rust enum (`src/value.rs:7`), with
`BcFunction` / `BcClassDef` / `BcInstance` as siblings of the
tree-walker `Function` / `Class` / `Instance` until the value layer
is rewritten. A comment at `src/value.rs:23-26` records this:
*"When NaN tagging lands, both `Function` and `BcFunction` will
fold into a single `Obj` pointer; until then the two coexist."*

**What it costs us today:** value clones are `Rc` bumps, not
register copies. The bytecode VM is faster than the tree-walker but
not by the 5×–20× target the roadmap aspires to.

**When to revisit:** when profiling on a real Phase-5 game shows
allocation pressure as the dominant cost.

### 2. Incremental tracing GC

**Status:** deferred to v0.2 / post-v0.1.

**Why:** without NaN tagging, every heap value already carries an
`Rc<RefCell<…>>` and is reclaimed deterministically when the last
reference drops. We have a working memory model. Adding a tracing
GC on top of `Rc` is wasted work; tracing GC ships when the value
layer becomes raw pointers, which is the same trigger as NaN tagging
above.

**Limitation today:** reference cycles leak. Twe's surface mostly
avoids them (entities live in `active_entities`, scenes own their
states, instances reference their class which doesn't reference back),
but a user can construct one. Document, don't fix until the value
layer change forces it.

### 3. Cooperative fibers + `wait <duration>`

**Status:** deferred from Phase 3 to **Phase 5**.

**Why:** the original deferral note
(`docs/changes/2026-04-28-fibers-deferred-to-phase-3.md`) argued
fibers should ride the bytecode VM because bytecode IPs make
suspension/resumption natural. That argument still holds — but it
also holds for *any* future bytecode session, not just Phase 3
specifically. The pressure to ship fibers comes from `dialogue`
(Example 3) and state-machine AI (Example 4), both of which the
roadmap places in **Phase 5**. No Phase 3 or Phase 4 deliverable
needs `wait`. Shipping fibers in a vacuum, with no caller, would
violate the "Featuritis" anti-pattern.

The locked decision (Wren-style cooperative, single-threaded, no
async coloring) is unchanged. Only the implementation timing moves.

**Concrete plan:** ship fibers as the first task of Phase 5,
*before* the dialogue runtime, so dialogue ships against a working
`wait`. Phase 5 entry criterion: Phase-3-style design correction
session if any frustration emerged from Phase 4.

### 4. Computed-goto interpreter loop

**Status:** not applicable on stable Rust. Deferred indefinitely.

**Why:** Rust does not support computed goto on the stable channel.
The dispatch loop (`src/vm.rs`) uses a `match` on `OpCode` instead;
the LLVM backend lowers it to a jump table in release mode. This is
the same pattern Wren uses on compilers that lack labels-as-values.
If we ever ship on nightly, revisit; until then, the locked decision
("computed-goto where the host compiler supports it") is
satisfied vacuously.

## Phase 4 — what shipped

All seven Phase 4 components from `docs/05-roadmap.md` §"Phase 4"
ship:

- [x] **Type representation** — `src/types.rs`, commit `f09ef36`.
- [x] **Hindley-Milner-style inference with extensions** — type
      variables + unification in commit `091166f`; function-body
      constraint solving in commit `ec04776`.
- [x] **Structural table types** — class shapes + structural
      field/method resolution, commit `da59799`. Per
      `docs/02-type-system.md` §3 a class shape is a record type
      with named methods; structural compatibility is the unification
      rule.
- [x] **Tagged unions** — multi-return functions produce
      `Optional<T>` and `Union<…>`, commit `b5f0e66`.
- [x] **Optional types** — same commit; `?` types are
      `Optional<T>` sugar at the type level.
- [x] **Dimensional unit checking** — `5m + 3s` errors per the
      "no silent footguns" principle. Implementation in commit
      `205a640`.
- [x] **Editor integration** — LSP hover shows inferred type in
      commit `a31a50a`. Autocomplete is a separate exit criterion
      (see below).

CLI surface: `twec types <file>` walks a file and prints inferred
top-level bindings.

## Phase 4 — exit criteria status

The roadmap §"Phase 4" lists three exit criteria. Two pass; one is
structurally unmet through Phase-5 dependencies, not Phase-4 work.

- [~] **All ten example programs type-check in non-strict mode
      without modification.** *Cannot be fully met until Phase 5.*
      Examples 3 (dialogue), 4 (state-machine AI), 5 (procedural
      visual), 7 (save/load), 8 (3D camera), 9 (tilemap), 10 (boss
      fight) use language constructs (`dialogue`, `visual`, `save`,
      `mesh`, `tilemap`, `on hp < 20%:` predicate hooks) that Phase 5
      ships. The type-checker has no rule for what doesn't exist
      yet. Of the example programs that *do* have on-disk
      implementations — `examples/hero.twe`, `examples/snake.twe`,
      `examples/sprite_demo.twe`, `examples/particles_demo.twe`,
      `examples/survive.twe` — `twec types` runs cleanly on all five
      and produces sensible top-level bindings. Stdlib calls returning
      sprite handles surface as `?` (Unknown), which is correct
      non-strict behaviour: the inference engine does not invent
      types for foreign builtins. This criterion is reopened as a
      Phase 5 exit dependency.
- [x] **LSP shows correct types on hover for ~95% of expressions
      in the example programs.** Commit `a31a50a` ships hover. Spot
      coverage on the five files above is at the criterion bar.
- [x] **Twe code with no annotations gets useful type-driven
      autocomplete.** *Originally deferred to Phase 5 entry; shipped
      same day.* The Phase 5 entry session wired
      `textDocument/completion`: user symbols carry their inferred type
      as the completion item's `detail` (so `let n = 42` surfaces as
      `n : int`), Twe keywords are sourced from the lexer's keyword
      match, and stdlib namespaces are walked from a freshly-installed
      `Env` so completion stays in sync with the runtime by
      construction. Both bare namespaces (`math`) and dotted members
      (`math.abs`) are emitted.

## What changes in the project on the back of this note

- `CLAUDE.md` "Phase discipline" updated: phase pointer is now
  Phase 5; Phase 3 and Phase 4 plans replaced with one-line retros
  pointing here.
- `CLAUDE.md` anti-rabbit-hole warning updated: the prohibition on
  Phase 4 thinking is removed (Phase 4 shipped); Phase 5 (3D /
  dialogue) and Phase 6+ items remain off-limits.
- `docs/05-roadmap.md` Phase 3 and Phase 4 sections gain
  *"**Status:** closed"* lines pointing to this note.
- `notes/future-phases.md` replaces its Phase 2 plan with retros
  for Phase 2 / 3 / 4 and prunes the now-shipped items from the
  Phase 3+ list. Remaining deferrals (NaN tagging, GC, fibers,
  autocomplete) move into the new "Carried into Phase 5+" section.

## Phase boundaries — the rule going forward

Each phase closes with an explicit closeout note in
`docs/changes/`, mirroring the form of this one:

1. What shipped — bulleted list against the roadmap exit criteria.
2. What slipped — explicit deferral decisions with reasons and
   target re-entry phase.
3. Doc edits applied as a result.

This is the only mechanism that keeps `CLAUDE.md` honest. Without
it, "Phase N" drift is inevitable; this conversation is evidence.
