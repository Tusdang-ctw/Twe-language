# web3d-M5 session 8: the error-fix corpus

**Date:** 2026-10-01
**Milestone:** web3d-M5 ([plan](2026-10-01-web3d-m5-plan.md); previous: [ablations and Studio](2026-10-01-web3d-m5-ablations-studio.md))
**Code:**
- `src/mutator.rs`: 7 new rules, `run_roots`, a clean-original filter, a canary guard;
- `src/cli.rs`: `--root` repeatable; the default roots;
- `tests/mutator.rs`.

## What shipped

**`twec mutate` produces 549 triples.** Each is a broken program, its `twec verify` diagnostics, and the fix. The target, from the Phase 33 closeout, was at least 500. The sources are the default roots, `tests/programs` and `examples`: 123 files, 86 of which yield triples.

**The new rules model errors seen in this milestone:**

| Rule | The error | Triples | What verify says |
|---|---|---:|---|
| `identifier_typo` (Phase 33, capped at 5 per file) | a misspelt name | 271 | `name-error.unknown`, with a rename fix |
| `missing_var` | a top-level `var` written without `var` | 98 | `scope-error.assign-undeclared` |
| `python_literal` | `True` / `False` / `None` | 92 | `name-error.unknown` |
| `duplicate_handler` | a second top-level `on update` | 23 | `duplicate-handler` (new in session 6) |
| `python_modulo` | `a % b` for `math.mod(a, b)` | 22 | lex or parse error |
| `entity_on_update` | `on update(dt):` in an entity, as the old primer taught | 19 | parse error |
| `list_len_call` | `.len()` | 14 | `list has no field 'len'` (new in the pre-measurement fixes) |
| `return_in_handler` | an early `return` in `on update` | 10 | `scope-error.return` (new in the pre-measurement fixes) |

269 of the triples carry a structured fix (`fix.edits`).

**Safeguards:**
- **Only originals that verify clean are used,** so every triple's target is a correct program.
- **Any file containing the benchmark canary is refused,** so benchmark solutions can't reach a training corpus even if `bench/` is passed as a root. The test passes it on purpose.
- **Sites are capped** (5 per file for typos, 3 per rule otherwise), so long examples don't swamp the error kinds. Uncapped, typos were 780 of 1,058.

**Not committed.** The corpus is about 8.6 MB of JSONL and regenerates deterministically with `twec mutate`, so `corpus/` is git-ignored.

## Verification

`tests/mutator.rs` adds a test that the default roots, plus `bench/tasks`, give at least 500 triples across at least 8 rules. It also checks:
- every original verifies;
- every mutated program fails verify;
- nothing carries the canary.
