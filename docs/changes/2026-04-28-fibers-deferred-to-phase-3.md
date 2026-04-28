# 2026-04-28 — Fibers deferred from Phase 2 to Phase 3

## Status: design-change note (process only — no language semantics change)

## Background

`docs/05-roadmap.md` Phase 2 component list includes:

> 2. **Coroutines / fibers** in the interpreter (the design from
>    `docs/03-runtime.md`).

`CLAUDE.md` "What is locked" reaffirms:

> **Concurrency: Wren-style cooperative fibers.** Single-threaded VM.
> No `async`/`await` distinction visible to the user.

The locked decision is the *design* (cooperative, single-threaded, no
async coloring). Phase 2 was supposed to ship the *tree-walker
implementation* of that design. We're deferring that implementation
work to Phase 3.

## Decision

`wait <duration>`, fiber-backed `every <duration>:`, and the rest of
the fiber-driven surface stay unimplemented in v0.1's tree-walker.
They land in Phase 3 alongside the bytecode VM, where the call-stack
serialisation that fibers need is naturally part of the bytecode
design (per *Crafting Interpreters* Chapter 30 + Wren's
`wren_vm.c`).

The user-visible API contract from `docs/01-examples.md` and
`docs/03-runtime.md` is unchanged. Programs that currently work
continue to work.

## Why now

1. **No shipping example pressures fibers.** The four playable
   programs that exit Phase 2 — `examples/snake.twe`,
   `examples/survive.twe`, `examples/hero.twe`,
   `examples/sprite_demo.twe`, plus `examples/particles_demo.twe` —
   contain zero `wait` statements. The only examples in
   `docs/01-examples.md` that *do* use `wait` are Examples 3
   (dialogue) and 5 (AI state machine), both of which the roadmap
   places in Phase 5.

2. **The Phase 2 frustration list is silent on fibers.** That list
   (`docs/changes/2026-04-28-phase-2-frustration-list.md`) catalogues
   every place the language design hurt during the build of Snake,
   Hero, and Survive. Fifteen items in three buckets, none of which
   are "I needed `wait` and didn't have it." The implementer never
   reached for the feature.

3. **`every <duration>:` already works without a fiber.** It's
   implemented as a per-state accumulator (`every_timers` /
   `every_intervals_secs` in `Instance`), ticked by `tick_scene`
   in `src/eval.rs`. That's the only fiber-shaped construct any
   shipped Phase 2 program touches, and it doesn't need a fiber to
   work — it needs an accumulator, which it has.

4. **The implementation work doesn't transfer.** A tree-walker fiber
   needs either:

   - **Rust generators** (unstable; would force nightly toolchain;
     CLAUDE.md says "buildable from `cargo build` with no special
     tooling").
   - **Threads** (forbidden — `docs/03-runtime.md`: "Single-threaded
     VM. No threads in v0.1.").
   - **Whole-block CPS rewrite at parse time** so every block
     containing `wait` becomes an explicit state machine. This is
     significant infrastructure that gets thrown away when the
     bytecode VM ships, because bytecode coroutines are a different
     mechanism (heap-allocate the call frame, suspend the IP,
     resume by restoring).

   Spending Phase 2 weeks on option 3 would burn hours that would
   immediately be redone in Phase 3.

5. **Principle-aligned.** CLAUDE.md anti-patterns: *"Featuritis.
   Adding a feature is easy; removing one is impossible. If it's not
   required by an example, it doesn't ship."* No Phase 2 example
   requires `wait`. The principle wins.

## What this changes in the docs

- `docs/05-roadmap.md` Phase 2 §"Components added in this phase"
  item 2 (Coroutines / fibers) is moved to Phase 3 §"Bytecode VM"
  alongside the other lifecycle / scheduling work.
- `notes/future-phases.md` adds a Phase 3 entry for the `wait`
  statement and the fiber-backed `every` rewrite.
- This change note is the canonical record. CLAUDE.md is **not**
  edited because the language-level locked decision (single-threaded
  cooperative fibers, no async coloring) is unchanged.

## Exit criteria for Phase 2 — restated

With this descope, the four-bullet Phase 2 exit-criteria checklist
in `docs/05-roadmap.md` reads:

- [x] A playable Vampire Survivors clone runs from a Twe source
      file. (`examples/survive.twe` — bullets now hurt monsters per
      F3 closure, sound on hit.)
- [x] Saving the source file hot-reloads the running game.
- [x] Total Twe code for the game is under 500 lines.
      (Survive ≈ 120 lines.)
- [x] The implementer has a list of language frustrations (15
      items, this directory).

Phase 2's six-component checklist now reads:

- [x] macroquad bindings — sprite drawing (commit phase-2 sprites),
      input (commit phase-2 macroquad backend), audio (commit
      phase-2 audio bindings).
- [~] Coroutines / fibers — **deferred to Phase 3** (this note).
- [x] Basic ECS world with simple queries — `entities.of(Class)` /
      `entities.count(Class)` (commit phase-2 entities).
- [x] `particles` block — commit phase-2 particles.
- [x] Hot reload — commit `795fab3`.
- [x] Game stdlib — `key.*`, `screen.*`, `time.dt`, `load`,
      `sprite()`, `sound.*`. `time.now`, `time.frame`, `mouse`,
      `gamepad` remain stdlib §7 unchecked items.

Five of six components shipped. The one descoped item is documented
here, with a clear next-phase home and a clear reason.

## Reopening

If a Phase 3 game wants `wait` *before* the bytecode VM is up, this
note should be revisited and a CPS-rewrite tree-walker
implementation considered. The expected cost is roughly a week of
parser + eval work; the cost of doing it twice (now in tree-walker,
again in bytecode) is roughly two weeks. Defer until forced.
