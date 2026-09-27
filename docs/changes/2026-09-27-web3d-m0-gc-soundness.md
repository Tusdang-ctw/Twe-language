# Design Change Note — GC soundness (web3d-M0)

**Date:** 2026-09-27
**Milestone:** web3d-M0 (see [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md))
**Status:** shipped (tree-walker). Bytecode VM: known hazard, not fixed (frozen, see below).

## Problem

A plain, correct Twe program crashed the process:

```twe
for it in [[k, [k, k * 2]] for k in 0..<200]:
    let junk = [it[0], [it[0]]]     # any allocation in the body
    total += it[1][1]
```

Under normal memory pressure `twec run` died with exit code
`0xC0000374` (`STATUS_HEAP_CORRUPTION`); an earlier probe produced
wrong values instead (`list index 1 out of bounds (length 1)`). Memory
unsafety reachable from safe script code.

Five root causes:

1. **Safepoints inside expressions.** `run_block` collected between
   statements, and `run_block` also executes function bodies reached
   mid-expression (`f(a, g())`, `x + h()`, a comprehension element).
   The caller's half-evaluated temporaries live only on the Rust stack
   there. The original comment ("no Rust-stack-only intermediate
   values between statements") was false.
2. **`for` snapshots unrooted.** The iterable's element snapshot and
   the shadowed loop variable lived only in Rust locals while the body
   ran statements.
3. **Swapped-out `self` unrooted.** Scene runners (`tick_scene`,
   `enter_state`, key dispatch, render) held the previous `self`
   wrapper on the Rust stack while running bodies; restoring a swept
   wrapper left `self` dangling.
4. **Incremental sweep corrupted marks.** Objects allocated mid-sweep
   were pre-marked and prepended to `all_objects`, where the cursor
   never visited them — their stale mark survived into the next cycle,
   so `mark_value` short-circuited and never traced their children.
   Freeing the original head with a null `sweep_prev` also overwrote
   `all_objects`, unlinking every object prepended during the sweep.
5. **Missing roots.** `Env.module_cache`, the stdlib thread-locals
   `SAVE_STORE` / `LANG_PLURAL_CLOSURES`, and class parent chains (and
   an instance's class defaults) were never marked.

The existing "aggressive GC" tests could not catch any of this:
`gc_set_threshold(0)` is reset by the adaptive threshold after the
first sweep, so they forced exactly one collection.

## Decision

Per the M0 design (collect only where the Rust stack is provably
clean; pin the few exceptions explicitly):

- **Tree-walker safepoints only at call depth 0**, plus a guaranteed
  safepoint at the start of `eval::tick_frame` (driver level — all
  callers are play loops / `run_with_frames`). Function and method
  bodies never collect. Game code still collects once per tick.
- **`heap::RootScope`** — an RAII guard over a thread-local explicit
  root stack, scanned by every collection. Used for `for` iterables,
  their snapshots, the shadowed loop variable, and swapped-out `self`
  (`root_saved_self`).
- **Young list** — mid-sweep allocations go on a separate unmarked
  list, spliced onto `all_objects` when the sweep completes.
- **Roots added** — `module_cache` and `stdlib::scan_stdlib_roots()`
  in `Env::scan_roots`; `mark_class` walks the parent chain and is
  applied to every instance's class.
- **Stress mode** — `TWE_GC_STRESS=1` (or `heap::gc_set_stress`)
  collects at every safepoint with an unlimited sweep budget and
  *poisons* swept objects (`HeapObject::freed`) instead of freeing
  them. Any later deref panics with "GC use-after-free", so a missing
  root is a deterministic failure instead of silent memory reuse.

Rejected: RAII root guards at every temporary (error-prone across the
14k-line stdlib); making `TaggedValue` non-`Copy` / returning to `Rc`
(discards NaN tagging, very large refactor). See the M0 section of the
pivot plan.

## Trade-off

A long-running function that allocates heavily (a top-level call that
loops for a long time building garbage) no longer collects until it
returns, so its peak memory is higher. Game code runs in ticks and is
unaffected. M1's slot-based frames root locals on a scanned stack,
which lets safepoints move back inside functions; a hard heap cap for
this case is tracked for M0 follow-up.

## Also in this change

- **macroquad backend guard.** Headless runs and `twec play3d` called
  macroquad draw/input APIs outside a macroquad window, aborting the
  process on its `THREAD_ID` assertion (`text()` in a 3D render hook;
  `touch.is_active()` headless in `examples/pixel_pop.twe`).
  `stdlib::set_macroquad_live` is set by the four macroquad loops in
  `play.rs`; the 21 2D drawing / UI builtins now go through
  `require_render_2d` (a clear `RuntimeError` in 3D), and touch
  queries report "no touches" headless.
- `.gitignore` covers crash-reporter `.replay` files.

## Evidence

- `tests/programs/gc_comprehension_uaf.twe` — before: native heap
  corruption normally, `GC use-after-free` panic under stress. After:
  identical correct output in both modes.
- `tests/gc_stress.rs` — every `tests/programs/*.twe` and
  `examples/*.twe`, at 0 and 10 frames, normal vs `TWE_GC_STRESS=1`:
  all agree.
- `tests/eval.rs` — `two_d_draw_in_3d_render_errors_instead_of_panicking`,
  `touch_queries_are_inert_headless`.
- Full suite: 1,043 pass, 0 fail; clippy clean. Tree-walker benches
  A/B'd against the previous HEAD on the same machine: parity within
  run-to-run noise.

## Known and not fixed

- **Bytecode VM.** Its dispatch-loop safepoint has the same class of
  hazard (builtin argument vectors are not rooted, `vm.rs` ~1052). The
  VM is frozen and opt-in (`--vm bytecode`); per the pivot plan it is
  gated behind `experimental` and removed at M1 exit, so it is not
  being fixed. Don't ship games on `--vm bytecode`.
- The nightly mutator fuzz and the heap cap from the M0 plan are still
  to do.
