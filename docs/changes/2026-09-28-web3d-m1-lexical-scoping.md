# Design Change Note — lexical scoping (web3d-M1)

**Date:** 2026-09-28
**Milestone:** web3d-M1 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md))
**Status:** shipped (tree-walker). The frozen bytecode VM is unchanged; it is deleted at M1 exit.

## Problem

`docs/06` §4.2 specified block scoping, but the tree-walker resolved names **dynamically**. It used one flat global map; a call saved, overwrote and restored its parameter names; and `let` inside any body wrote a global. The consequences:

- A function's locals **leaked** into globals after it returned.
- A callee **saw its caller's parameters** (`function g(): return y` inside `h(y)` returned `h`'s `y`).
- A **module function** resolved free names in the *importer's* env, so a module calling its own helper failed from another file.
- Assigning an undeclared name **silently created a global**.
- Undefined names surfaced only when the offending branch finally ran.

## Decision

1. **A resolver pass.** New `src/resolve.rs` resolves every identifier under lexical rules and classifies what doesn't resolve:
   - `frame-leak`: another body's local;
   - `block-escape`: used after its block ended;
   - `assign-undeclared`;
   - `undeclared`;
   - `redeclared`: `let` / `var` re-declaring a visible name.

   `eval::run_top_level` runs it before executing anything, and `twec verify` reports the same issues. Undefined names inside functions now get a machine-applicable did-you-mean fix, where inference alone was silent in non-strict mode.
2. **Runtime frames.** Every function, method, dialogue and handler body runs in a `LocalFrame` on `Env::frames`, holding its parameters and locals. Lookup order: locals → `self` fields → home-module globals → env globals. Top-level statements bind globals.
3. **Flat frames, static blocks.** A frame is one flat list. Block visibility is enforced statically, and re-declaring a visible name with `let` / `var` is an error, so a flat runtime frame is indistinguishable from nested block scopes. `for` and comprehension variables may shadow; the runtime saves and restores the shadowed binding. This departs from §4.2's original draft, which allowed `let`-shadowing across nested blocks: shadowing a field with a local is how `hp -= 1` silently misses the entity (Principle 3).
4. **Fibers.** A suspended body's frame is parked on its fiber `Frame` (`Frame::locals`) and pushed back on resume. That replaces the `saved_params` bookkeeping, and locals survive `wait` in state-entry bodies and in suspended function calls. The GC marks parked frames.
5. **Modules.** `FunctionDef` and `MethodDef` carry `home`, the module object they were defined in. The loader now creates that object *before* running the module, so a module function called from the importer resolves names in its own module.
6. **One module path.** `module::prepare_entry` is used by `twec run <file>`, `twec run <dir>` (which now honours `--frames`), `twec play` and `twec play3d`. Before, only `twec run <dir>` loaded imports, and it never ticked frames; `twec run <file>` and `twec play` left imports unbound. That is why both Phase 13 module demos failed.

Slot-indexed frames (locals resolved to indices ahead of time) are the next M1 step. They build on these semantics without changing them.

## Evidence

- **Corpus impact:** the resolver over all 118 example and test programs found **zero** frame leaks and **zero** block escapes, so no existing program depended on dynamic scoping. It did find latent undefined-name bugs in examples, fixed in `a1f3196`.
- `tests/programs/lexical_scope.twe` covers per-call locals, callee isolation, loop-variable shadow and restore, comprehension scope, recursion, and entity methods.
- `tests/programs/fiber_locals.twe`: locals survive `wait` in a state entry and in a suspended function, in normal and GC-stress modes.
- `module::tests::module_functions_resolve_names_in_their_own_module`: a module's `helper` and `scale` are resolved from its own file, and module state is updated in place.
- `tests/eval.rs`: frame-leak, undefined-name-before-run, assign-undeclared and redeclared errors. `tests/verify_v2.rs`: verify reports scope errors and suggests renames.
- `tests/examples_run.rs`: every example runs, including both module demos, and the corpus is lexically clean.
- Full suite passes, including GC stress and fuzz; clippy is clean on native and wasm32.

## Found and fixed along the way

`known_globals()` originally ticked a throwaway env to discover ambient names. That tick hits a GC safepoint, which collects using only the throwaway env's roots and sweeps the *real* env's objects. The stress harness caught it on every program. The tick added no names, so it was removed, and the function now documents that throwaway envs must never reach a safepoint. The heap is per-thread and assumes one live env; that assumption is now written down where it matters.

## Not changed

- The bytecode VM keeps its own semantics; it is deleted at M1 exit.
- Lambdas and closures were never implemented. §4.3 no longer claims them.
