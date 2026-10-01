# The Twe LLM benchmark

Does an LLM write working games in Twe more reliably than in Python? This benchmark measures it (web3d-M5; [plan](../docs/changes/2026-10-01-web3d-m5-plan.md)). The graphics comparison against Three.js lives separately in [`graphics/`](graphics/).

**Programs are graded by behaviour, not by their text or output.** Each program runs headless for a fixed number of 60 Hz ticks while a scripted player presses keys. Then the checks are evaluated on the running world, e.g. "after holding D for one second, the player is 5 units to the right".

## A task

```text
bench/tasks/<id>/
  task.md        the task, in words (the Python baseline gets the same text)
  twe.md         what a Twe program must name for the checks to find it
  task.toml      ticks, the input script, the checks
  solution.twe   a reference solution
  starter.twe    optional: the file the model starts from
  mutants/*.twe  optional: hand-written broken variants
```

`task.toml`:

```toml
ticks = 180                 # 60 Hz ticks to run

[[input]]                   # keys held over ticks 0..60 (end exclusive)
from = 0
to = 60
hold = ["d"]

[[input]]                   # a press on one tick (a pressed key is also held)
at = 90
press = ["space"]

[[input]]                   # the mouse: position over a span, buttons
from = 100
to = 101
mouse = [320, 240]
click = ["left"]            # or hold_mouse = ["left"] over a span

[[check]]
name = "D for 1 s moves 5 toward +x"
at = 60                     # after 60 ticks; default: after the last
expr = "math.abs(player.x - 5.0) < 0.01"
```

**Inputs** are key names as Twe scripts see them (`key.d`, `key_pressed("space")`).

**A check** is a Twe expression evaluated as top-level code after its tick, so it sees the program's globals, classes and the stdlib (`entities.count(Coin) == 0`). A check passes only if it evaluates to `true`.

**Naming:** names a program must define are listed in `twe.md`. They must not be stdlib names: a probe on a missing `label` would find the UI builtin of that name.

## Commands

```sh
twec bench grade bench/tasks/move_player my_attempt.twe     # one program; --json for the record
twec bench check --all                                      # validate every task
```

**Grading runs in a child process** (`twec bench grade <task> - --json`, with the program on stdin) under a wall-clock limit. A program that loops forever fails its task (stage `timeout`) instead of stopping the run, and no engine state carries from one program to the next.

**A grade records how far the program got:**

| Stage | Meaning |
|---|---|
| `lex`, `parse` | The program didn't lex or parse. |
| `load` | Its top-level code failed, including the scope check. |
| `run` | It raised a runtime error on some tick. |
| `checks` | It ran every tick; the checks decide. |
| `timeout` | It was killed at the wall-clock limit. |

Each check reports its value or why it has none.

## Validating a task

`twec bench check` accepts a task only if:
1. **The reference solution passes.**
2. **The starter fails**, if there is one.
3. **Every check is caught by some mutant of the solution that runs every tick.**

Mutants are made mechanically: one statement line deleted, one operator flipped (`<` → `>=`, `+` → `-`, `and` → `or`, `true` → `false`, …) or one number changed. Hand-written ones in `mutants/` are added to them.

Rule 3 is what keeps a check from passing vacuously. A mutant that crashes doesn't count, since a crash says nothing about whether the check tests behaviour. When no mechanical mutant can reach a check (string values, say), add a hand-written one.

`tests/bench.rs` runs the validation on every committed task.
