# web3d-M5 session 5: the Python baseline

**Date:** 2026-10-01
**Milestone:** web3d-M5 ([plan](2026-10-01-web3d-m5-plan.md); previous: [bench run](2026-10-01-web3d-m5-bench-run.md), [pre-measurement fixes](2026-10-01-web3d-m5-pre-measurement-fixes.md))
**Code:**
- `bench/python/harness.py` and `requirements.txt` (new);
- `src/bench.rs`: `Lang`, `Grader`, the Python job, `run_child`, Python-aware mutants;
- `src/bench_run.rs`: the language as a run setting;
- `src/llm_loop.rs`: `run_loop_with`, `StaticVerdict`, the fence language;
- `src/cli.rs`: `--lang` / `--python`;
- 20 × `python.md` + `solution.py`, and 137 `py` checks;
- `tests/bench_python.rs` (new).

## The design: same tasks, same input, same checks

The kill line compares Twe with Python, so everything that isn't the language has to match.

- **One source of truth for input.** twec compiles a task's input script into per-tick commands, and hands the Python harness the program, those commands and the checks as JSON. The harness replays the commands through pygame's own input APIs:
  - `key.get_pressed()` and `get_just_pressed()`;
  - KEYDOWN, KEYUP and mouse-button events, also passed to `update`;
  - `mouse.get_pos()` and `get_pressed()`.

  Key-press semantics are defined once, in Rust.
- **Twin checks.** Every check gets a `py` expression at the same tick, translated mechanically and reviewed:
  - `entities.count(C)` → `len(game.cs)`;
  - `math.length(a - b)` → `(a - b).length()`;
  - globals → `game.<name>`.
- **The same validation.** All 20 Python reference solutions pass. All 137 Python checks are each failed by at least one running mutant of `solution.py` (`twec bench check --lang python --all`: 20 of 20, 90 s).
- **The same grade format.** The harness prints the JSON `twec bench grade` prints: stages `parse`, `load`, `run` (tick and line), `checks`, and `timeout` (killed by the parent).
- **The same loop.** Up to 4 rounds. The language's own checker is the feedback (`compile()` and pyflakes for Python, the counterpart of `twec verify`); pyflakes' undefined names count as errors, and its style warnings don't. Then the same 2-second smoke run.

**What differs, deliberately:**
- **The system prompt.** Python's is only the run contract (a `Game` class with `update(dt, events)` and `draw(screen)`, 60 ticks per second, no main loop of its own), since models know Python and pygame. Twe's carries its primer and stdlib listing.
- **The tools.** Python gets Python's tools. Twe's claimed advantage is the grammar, the primer and `verify`'s structured fixes, so these differences are exactly what the benchmark tests.

**Interface conventions** (`python.md` against `twe.md`):
- top-level `var x` → `self.x` on `Game`;
- entity class `Coin` → a list `self.coins` of objects with a `pos`;
- `vec3` → `pygame.Vector3`;
- the snake → a list of `(x, y)` tuples of ints.

## Engineering

- **A shared child-process runner (`run_child`)** for both graders. It feeds stdin and drains stdout and stderr on threads; before, stderr was read only after exit and could fill and stall.
- **Python-aware mutants:** `'` strings are left alone; `True` / `False` flip where Twe's `true` / `false` do. Mutants that don't compile fail at `parse` and don't count as runnable.
- **The loop is language-neutral.** `run_loop_with` takes the static check, and `LoopOptions.lang` sets the fence tag for replies, starters and edit errors. `run_loop` / `run_loop_checked` keep Twe's verify.
- **Run directories record `lang`.** Programs are saved as `.py`, and `regrade` reads the language from `run.json`.
- **The interpreter.** `--python PATH`; by default, `bench/python/.venv` when it exists (made from the pinned `requirements.txt`: pygame-ce 2.5.8, pyflakes 3.2.0).

## Security

The harness `exec`s model-written Python, which can do anything the user can, unlike a Twe program. The harness docstring and `bench/README.md` say so, and recommend a container or VM for Python sessions on models you don't trust.

## Verification

- **`tests/bench_python.rs`:**
  - every task's Python side exists and every `solution.py` passes;
  - the harness's grades match the Twe grader's (a flipped W fails the same check);
  - its parse, load, run (with tick and line) and timeout stages;
  - the static check (pyflakes errors against warnings, syntax errors);
  - a Python run where round 1 has an undefined name: pyflakes sends it back, round 2 passes, and the run is recorded as `python`.
- **Skipping.** These tests need the venv. Without it they print why and pass, so the suite still runs without Python.
- **`twec bench check --lang python --all`:** 20 of 20 valid.
- **Updated tests:** `tests/bench.rs` and `tests/bench_run.rs` use the `Grader` API.
