# web3d-M5: fixes before the first measurement

**Date:** 2026-10-01
**Milestone:** web3d-M5 ([plan](2026-10-01-web3d-m5-plan.md)); between sessions 4 and 5
**Code:**
- `src/verify.rs`: `collect_misplaced_returns`;
- `src/infer.rs`: list members;
- `docs/llm-primer.md` and `src/primer.rs`;
- `examples/tetris.twe`, `grid_duel.twe`, `pong_net.twe`, `pong_net_internet.twe`;
- `tests/primer.rs` (new).

## Why now

Session 3's pilot found three problems in the material every model writing Twe is given, before any benchmark run:
- **The primer's entity example didn't parse.** It wrote `on update(dt):` inside an entity, which is not the syntax.
- **Verify passed two programs that fail when run:** a `return` inside an `on update` handler, and `.len()` on a list.
- **The primer showed key and predicate events at the top level,** where they are errors; and two of its examples used undefined names.

Measuring Twe with a primer that teaches broken syntax would measure the primer's bugs. These are bugs in documentation and tooling, not language changes; the M5 rule against changing the language mid-measurement isn't touched. No run had happened yet, so no result is affected.

## What changed

**`twec verify` reports `return` outside a function or method body** (`scope-error.return`):
- in top-level code;
- in an `on update`, `on render` or class-event handler;
- in a state's body or handlers;
- in a dialogue.

These are exactly the places the runtime rejects it (`call_depth == 0`). Before, it was an error only when the `return` actually ran.

**It found four shipped examples that crash:**
- **`tetris`** crashed on every hard drop (Space) and whenever a piece locked.
- **`grid_duel`, `pong_net` and `pong_net_internet`** crashed whenever a peer's input hadn't arrived yet.

All four are rewritten: the rest of the handler sits under `if net.tick_ready(tick):`, and in tetris under an `else`, with the `return` in the gravity loop now a `break`.

**`twec verify` reports a member a list doesn't have,** where inference knows the value is a list (`list has no field 'len'`). The members are fixed: `.length`, `.append`, `.prepend`, `.pop_back`, `.pop_front`, `.contains`, `.set`.

**The primer** (`docs/llm-primer.md`) changes:
- **Every `twe` block now verifies.** The entity example uses `function update(dt):`. The events example puts key, predicate and `every` handlers inside a state, and says what's legal at top level. The state-machine and look examples are self-contained. The keyword overview is marked as text, not code.
- **Three facts are added to the golden rules:** where `return` is legal, the list members, and that `spawn` returns nothing.
- **The concise primer** (`src/primer.rs`, served by MCP and Studio) gets the same corrections.

**`tests/primer.rs` keeps it that way:** every `twe` block in the primer, and every curated example, must pass `twec verify`.

## Added during session 6: the `nil` literal

`docs/06` §2.5.3 specifies `true`, `false` and `nil` as literals, and the primer names `nil` as a value. Functions already return nil, but the name was never bound, so `x == nil` failed to load with "name 'nil' is not defined". The new `inventory_stacks` task hit it, written the way a model would write it.

`nil` is now bound by the stdlib as a constant (`stdlib::install`). This is the specified design, implemented before any measurement, not a new language feature. `tests/programs/nil_literal.twe` pins it.

## Verification

- **Verify unit tests:**
  - `return` in a handler and in a state is reported;
  - nested in functions and methods it isn't;
  - list members: `.len()` is reported, and the real members aren't.
- **No false positives.** Running the new checks over every `.twe` file in the repository (167, plus the pilot's 20) finds only the four real crashes above.
- **The crash is real.** A state `on update` with a `return` in a branch fails when run, as reported.
- **Tests:** 1074 pass.
