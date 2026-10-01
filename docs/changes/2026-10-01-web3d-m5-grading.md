# web3d-M5 session 2: behavioural grading

**Date:** 2026-10-01
**Milestone:** web3d-M5 ([plan](2026-10-01-web3d-m5-plan.md); previous: [`twe-llm`](2026-10-01-web3d-m5-twe-llm.md))
**Code:**
- `src/bench.rs` (new);
- `src/cli.rs` (`twec bench grade` / `check`);
- `tests/bench.rs` (new);
- `bench/tasks/move_player/` (the first task);
- `bench/README.md`.

## Why

`twec eval` (Phase 33) graded a generated program by its stdout after N frames, with no input. That rewards a program that prints the expected words. It can't test a game that responds to keys, and a check can pass for the wrong reason. The benchmark needs grades that mean "the game does what was asked".

## What shipped

**Tasks.** A directory under `bench/tasks/` with:
- the task in words, shared with the coming Python baseline;
- the Twe interface (the names checks read);
- `task.toml`: ticks, an input script, checks;
- a reference solution;
- optionally a starter and hand-written mutants.

**Input scripts:**
- keys held over tick spans, or pressed on one tick (a pressed key is also held that tick);
- the mouse position over a span;
- mouse buttons held or clicked.

They compile to one `InputCommand` per tick, the M4 replay type.

**Grading** (`bench::grade`) runs the program headless through the same calls as the 3D shells and the soak harness: `host3d::apply_command`, `eval::tick_frame`, `eval::render_frame3d`.

**Checks** are Twe expressions evaluated after their tick. Each runs as a top-level `let` in the running world, so it sees globals, classes and the stdlib through the normal resolver and evaluator; nothing special-cased. A check passes only on `true`. Failing checks report their value, or why they have none.

**The grade's stage** says how far the program got: `lex`, `parse`, `load`, `run` (with the tick), `checks`, `timeout`, `crash`.

**Child processes.** `twec bench grade <task> <file|-> [--json]` grades one program. The benchmark grades through `bench::grade_in_child`:
- a fresh `twec` process per program, killed at a wall-clock limit;
- its stdout drained on a thread, so a chatty program can't stall the pipe;
- a program that loops forever fails its task instead of hanging the run;
- no thread-local engine state (the GC heap, the replay mode, the pause flag) carries between programs.

**Task validation** (`twec bench check`) rejects a task unless:
1. its solution passes;
2. its starter fails;
3. **every check is failed by some mutant of the solution that runs every tick.**

Mutants come mechanically from the solution's text, and only those that still parse are kept:
- one statement line deleted;
- one operator flipped, outside strings and comments, with `->` left alone;
- one number doubled.

Hand-written ones in `mutants/` are added. Rule 3 guards against the benchmark's worst failure mode, a check that passes whatever the program does. Crashing mutants don't count toward it: a crash says nothing about what the check tests.

**The first task, `move_player`** (WASD on the ground plane): 37 mutants, 33 of which run. Each of its 5 checks is caught by 3–22 of them. Validation takes under a second.

## Verification

- **`src/bench.rs` unit tests** (6):
  - input spans compile to per-tick commands;
  - task errors name their entry;
  - grading feeds input and checks at the right ticks (holding a key one tick short is caught);
  - every failure stage;
  - grade JSON round-trips;
  - mutants leave strings, comments and `->` alone.
- **`tests/bench.rs`** (5, through the real binary):
  - every committed task validates;
  - an infinite loop times out in about 2 s;
  - child grades equal in-process grades, and a flipped direction fails the right check;
  - a vacuous check and a failing solution / passing starter are reported;
  - a check only a hand-written mutant can reach is rejected, then accepted once one exists.
- **A finding:** names a task asks a program to define must not be stdlib names. A probe on a deleted `label` found the UI builtin `label` and ran. `bench/README.md` says so.
- **Clippy** (`-D warnings`) is clean for native (all targets), wasm32 and `twe-web`.

## Not done here

**`twec eval` and its three stdout suites in `eval/`** stay as they are. Session 4 decides whether `twec bench run` replaces them (Principle 2: one way to grade).
