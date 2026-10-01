# The Twe LLM benchmark

Does an LLM write working games in Twe more reliably than in Python? This benchmark measures it (web3d-M5; [plan](../docs/changes/2026-10-01-web3d-m5-plan.md)). The graphics comparison against Three.js lives separately in [`graphics/`](graphics/).

**Programs are graded by behaviour, not by their text or output.** Each program runs headless for a fixed number of 60 Hz ticks while a scripted player presses keys. Then the checks are evaluated on the running world, e.g. "after holding D for one second, the player is 5 units to the right".

## The tasks (bench v0)

Twenty tasks, every one written from scratch (no starter). Each has a reference solution and passes validation (below).

| Tier | Task | Checks | What it asks for |
|---|---|---:|---|
| 1 | `move_player` | 5 | WASD movement on the ground plane |
| 1 | `countdown` | 6 | a 10 s countdown, clamped at 0, R restarts |
| 1 | `score_combo` | 6 | presses within 1 s build a combo (max 5) that multiplies points |
| 1 | `health_regen` | 8 | damage, death at 0, regeneration after 2 s without a hit |
| 1 | `traffic_light` | 10 | a timed green → yellow → red cycle, a button that cuts green short |
| 1 | `ammo_reload` | 9 | magazine, reserve, a 1.5 s reload that can't be interrupted |
| 1 | `day_night` | 8 | a 24 h clock at 1 h/s, wrapping, counting nights |
| 2 | `coin_collect` | 5 | coins as entities, collected within 0.5 units |
| 2 | `enemy_chase` | 6 | an entity that follows a moving player and stops on contact |
| 2 | `spawner_waves` | 8 | waves of n enemies every 2 s, each living 5 s |
| 2 | `projectile_fire` | 7 | bullets with a range, a target with health |
| 2 | `bomb_radius` | 6 | removing every entity within a radius |
| 2 | `door` | 8 | an open/close state machine with reversible transitions |
| 2 | `pause_menu` | 8 | play / pause / confirm-quit / title modes with a stoppable timer |
| 3 | `dash_cooldown` | 7 | movement plus a dash along the last direction, with a cooldown |
| 3 | `jump_gravity` | 7 | a jump under gravity, no double jumps |
| 3 | `click_move` | 5 | click-to-move with the mouse |
| 3 | `xp_magnet` | 4 | gems pulled to the player, xp and growing level thresholds |
| 3 | `survivor_mini` | 6 | spawning enemies, auto-aimed bolts, health and game over |
| 3 | `snake_grid` | 8 | Snake, with fixed food positions, turning rules and walls |

**Tiers:** 1 is logic and timing, 2 is entities and state, 3 is input-driven play combining both.

**Where the checks come from.** They test behaviour described in `task.md`, at times chosen at least a few ticks away from the events they test. A reasonable implementation that updates a tick earlier or later still passes.

For `enemy_chase`, `xp_magnet` and `survivor_mini`, the check values were derived from a separate Python simulation of the task text, and they agree with the Twe solutions.

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

## Running models

```sh
# a model API (build twec with --features llm-http; ANTHROPIC_API_KEY set)
twec bench run --provider anthropic --model claude-sonnet-5-5 --effort high --samples 5
# a local model through Ollama, or any OpenAI-compatible server (--base-url)
twec bench run --provider openai --model qwen2.5-coder:14b --samples 5
# any program that reads a prompt on stdin and prints a reply
twec bench run --command my_model.sh --samples 5

twec bench regrade bench/runs/<run>        # grade a run's programs again, no model calls
```

**Per sample, the model gets:**
- **The system prompt:** the Twe primer (`docs/llm-primer.md`), every stdlib builtin with its parameters, and how the program is run and tested. It is about 6,000 tokens, cached across rounds and samples.
- **The task:** `task.md` and `twe.md`.

**It answers with a whole program, then has up to `--rounds` rounds (default 4) to fix it.** After each round the program is:
1. checked by `twec verify`, whose diagnostics go back if it finds errors;
2. smoke-run for 2 s of game time with no input, and a crash, hang or load error goes back.

The final program is graded in a child process. The model never sees the checks or the input script.

**The arms:**

| Flag | Effect |
|---|---|
| `--rounds 1` | a single attempt with no feedback |
| `--no-verify` | the smoke run only |
| `--no-smoke` | verify only |
| `--no-primer` | no primer or stdlib listing: only "Twe is a game scripting language" and the run rules |

**A run directory** (`bench/runs/<date>-<provider>/`, or `--out`) holds:
- `run.json`: the settings;
- `samples.jsonl`: per sample, the grade with its stage and failed checks, rounds, round 1's syntax and verify errors, tokens, cost, wall time;
- every final program, and every round's transcript;
- `summary.json` and `summary.md`.

**The summary reports:**
- pass@k for every k up to the number of samples, using the unbiased estimator;
- pass@1 by tier;
- 95% bootstrap confidence intervals over tasks;
- round-1 syntax and verify-clean rates, rounds to pass, replies cut off at the token limit, where failing programs stopped;
- tokens and cost.

**Replies are cached** in `bench/cache/` (git-ignored) by a hash of the provider, the sample number and the whole request. An interrupted run resumes where it stopped (re-running the same command skips finished samples and retries failed calls), and a repeated run costs nothing. A run directory refuses to mix settings.

**Five provider failures in a row stop a run,** as does a configuration error (no key, an unknown model).

## The Python baseline

The same tasks, written in Python 3 with pygame-ce and graded the same way: `--lang python` on `grade`, `check` and `run`.

```sh
python -m venv bench/python/.venv
bench/python/.venv/Scripts/pip install -r bench/python/requirements.txt   # bin/pip elsewhere
twec bench check --lang python --all
twec bench run --lang python --provider anthropic --model claude-sonnet-5-5 --samples 5
```

**What's the same:**
- **The task text** (`task.md`). The interface is `python.md`, which names the same things as `twe.md`:
  - a top-level `var x` becomes `self.x` on the `Game` object;
  - an entity class `Coin` becomes a list `self.coins` of objects with a `pos`;
  - `vec3` becomes `pygame.Vector3`.
- **The input.** twec compiles each task's input script into per-tick commands, and `bench/python/harness.py` replays them through pygame's own APIs:
  - `pygame.key.get_pressed()` and `get_just_pressed()`;
  - KEYDOWN, KEYUP, MOUSEBUTTONDOWN and MOUSEBUTTONUP events, which are also passed to `update`;
  - `pygame.mouse.get_pos()` and `get_pressed()`.

  The key semantics can't drift between the two languages.
- **The checks.** Every check has a `py` twin in `task.toml`, at the same tick, reading the same quantity (`len(game.coins) == 0` for `entities.count(Coin) == 0`).
- **Validation.** The Python twins are validated like the Twe checks: the reference `solution.py` passes, and each twin is failed by a running mutant.
- **The loop.** Up to 4 rounds, with the language's own checker as feedback (`compile()` and pyflakes, in place of `twec verify`), then the same 2-second smoke run.

**What differs, deliberately:**
- **No primer.** The Python system prompt is only how the program is run: a `Game` class with `update(dt, events)` and `draw(screen)`, 60 ticks per second, no main loop of its own. Models know Python and pygame; Twe needs its primer.
- **Python uses Python's tools,** and Twe gets `twec verify` with structured fixes. That is the claim being tested.

**Security.** A Twe program can only do what Twe's stdlib allows. A Python program can do anything your user account can, since the harness `exec`s it. Run Python benchmark sessions on models you trust, or inside a container or VM.

## Validating a task

`twec bench check` accepts a task only if:
1. **The reference solution passes.**
2. **The starter fails**, if there is one.
3. **Every check is caught by some mutant of the solution that runs every tick.**

Mutants are made mechanically: one statement line deleted, one operator flipped (`<` → `>=`, `+` → `-`, `and` → `or`, `true` → `false`, …) or one number changed. Hand-written ones in `mutants/` are added to them.

Rule 3 is what keeps a check from passing vacuously. A mutant that crashes doesn't count, since a crash says nothing about whether the check tests behaviour. When no mechanical mutant can reach a check (string values, say), add a hand-written one.

`tests/bench.rs` runs the validation on every committed task.
