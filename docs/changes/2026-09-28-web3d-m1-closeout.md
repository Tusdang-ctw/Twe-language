# web3d-M1 closeout — lexical scoping and interpreter speed

**Date:** 2026-09-28
**Milestone:** web3d-M1 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md))
**Status:** **closed.** All exit criteria met. Two items are deferred with reasons (see "Deferred").

## Exit criteria

| Criterion | Result | Evidence |
|---|---|---|
| Leak count 0 | **Met** | `tests/examples_run.rs::corpus_has_no_lexical_scope_issues`: every example, test program and eval suite resolves lexically. The corpus had **zero** frame leaks and zero block escapes before the switch. |
| All tests pass, including GC stress | **Met** | 922 pass (the ~145 fewer than M0 are VM-only tests deleted with the VM). The debug `tests/gc_stress.rs` sweep and the 1000-seed `tests/gc_fuzz.rs` are clean. |
| `sum_loop` ≤ 60 ns/iter | **Met — 50–58 ns** (was 213) | `tests/perf_probe.rs` (min-of-N), same machine, against `a1f3196` |
| `entity_update` ≤ 500 ns/update | **Met — 313–373 ns** (was 1205) | same |
| Module-isolation tests | **Met** | `module::tests::module_functions_resolve_names_in_their_own_module`; both Phase 13 module demos run under `tests/examples_run.rs` |

`fib(20)` fell from 689 to about 250 ns per call. Criterion (`benches/interp.rs`) remains the canonical harness, but on the development machine it swung 30–50% between identical builds. The min-of-N probe was the only stable measurement, and every number above comes from it.

## What shipped (commits `a1f3196` … this closeout)

1. **A lexical resolver** (`src/resolve.rs`, [`2026-09-28-web3d-m1-lexical-scoping.md`](2026-09-28-web3d-m1-lexical-scoping.md)). It runs before every program and in `twec verify`, which now gives did-you-mean rename fixes for undefined names inside functions.
2. **Runtime lexical frames.** Functions, methods and handlers get their own locals; a callee can't see its caller's parameters; module functions resolve names in their own module; locals are parked across `wait`. Assigning an undeclared name and re-declaring a visible one are errors.
3. **`import` works everywhere.** It used to work only through `twec run <dir>`, which never ticked frames. Both Phase 13 module demos had never worked interactively.
4. **Interpreter speed, 2.8–4×:**
   - FxHash name maps;
   - in-place writes to existing globals and fields;
   - `Rc<str>` parameters;
   - pooled frame vectors;
   - one cached GC wrapper per instance;
   - borrow-only instance access;
   - a single-flag safepoint check.
5. **Bytecode VM deleted** (`077dd41`, −8.5k lines). `--vm bytecode` now explains the removal.
6. **Every example now runs in CI** (`tests/examples_run.rs`). Writing that test, together with the resolver, found and fixed:
   - latent undefined-name bugs in six examples;
   - `joystick()` rejecting its documented tuple form;
   - working-directory-only asset paths (there is now an asset root).

## Deviations from the plan

- **No slot indices.** The plan called for resolving locals to slot indices. The targets were met without that AST surgery: flat, small frames with linear lookup are already cheap. Slots would change `Expr::Ident`, `Stmt::Let`, `Assign`, `For` and `ListComp` across the parser, printer, JSON and inference. They move to M3, where SoA entity columns rewrite field access anyway and the payoff is larger.
- **Stricter than the spec's shadowing rule.** Re-declaring a visible name with `let` / `var` is an error, whereas `docs/06` §4.2's original draft allowed it across nested blocks. The reasoning is in the scoping note: it keeps flat runtime frames exactly equivalent to block scopes, and it rejects the "local shadows a field" footgun.

## Deferred

- **Safepoints inside function bodies, and the heap cap** (both folded in from M0). Locals now live on the scanned `Env::frames`, but *expression temporaries* (argument vectors, a binary operator's left operand) still live only on the Rust stack, so collecting inside a call is not yet sound. Games are unaffected, since they collect every tick. **Moves to M3**, where the entity-system rewrite restructures evaluation; rooting temporaries there is the natural fix.
- **Slot indices:** M3, as above.

## Found along the way

- A GC bug of my own, caught by the stress harness within the milestone: `known_globals()` ticked a throwaway env, and that tick's safepoint swept the real env's objects. It's fixed, and the rule that throwaway envs must never reach a safepoint is now documented where it matters.
- Three user-facing help strings were mangled by line continuations; one had shipped in M0.
- The tutorial still claimed `wait` didn't work inside `if` / `while` / calls, and `docs/06` §4.3 claimed closures and lambdas that were never implemented. Both are corrected.
