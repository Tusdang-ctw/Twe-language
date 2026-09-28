# Design change: M3 speeds up the interpreter, not the storage layout

**Date:** 2026-09-28
**Milestone:** web3d-M3 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md))
**Decision by:** the user, from the measurements below.

## What the plan said

M3 step 1 was **archetype (SoA) storage**: one set of columns per entity class, generational `EntityRef` handles in place of instance objects, and the interpreter reading and writing columns directly. Its aim was the update-speed exit criterion: script tick ≤ 4 ms in wasm and ≤ 300 ns per update natively, on 5,000 entities.

## What the measurements say

Four variants of the `swarm_3d` enemy update, run headless (`twec run --frames`, release, min of 5 runs, 5,000 entities, 300 ticks), before any M3 interpreter work:

| Variant | ns / update |
|---|---|
| As written (`vec3` position, `math.sqrt`) | 2,459 |
| Same logic on two float fields, no `vec3` | 1,418 |
| … and no `math.sqrt` call | 1,144 |
| Empty `update` (dispatch only) | 104 |

- **Per-entity dispatch is ~100 ns.** That is the part columnar storage speeds up.
- **The costs that matter are elsewhere:**
  - `vec3` allocation plus tuple field reads: ~1,040 ns.
  - One builtin call: ~275 ns. Arguments were copied twice, plus allocations.
  - String-keyed name lookups across about 12 statements: ~1,040 ns. Every identifier scans the frame, then hashes into `self`'s fields, then into the globals.

Columnar storage would also move states, `every` clocks, fibers, save and net snapshots onto a new representation. That is a large rewrite for about 4% of the cost.

## Decision

**Interpreter first.** Entities stay as they are: one object per instance. M3 attacks the measured costs:

1. **Builtin calls.** No redundant argument copies, and pooled argument buffers.
2. **Names by slot.** Static frame layouts for locals, a per-class field layout for instance fields (prefix-compatible under single inheritance), and indexed globals. This is the slot-index work M1 deferred into M3.
3. **Allocation-free `vec3`** where the measurements still point at it.

Each step is re-measured against the targets. Columnar storage returns only if the kernel needs bulk native access to entity data (physics, culling or LOD at 50k+ entities), which is not before M4.

## Progress

| Step | `swarm_3d` update (ns, native) |
|---|---|
| Baseline | 2,459 |
| Builtin calls pass positional arguments without copying | 2,105 |
| Pooled argument vectors; methods and functions don't copy positional arguments | 1,969 |
| Locals by slot: the resolver annotates names (`ast::Res`), the runtime indexes frames | 1,370 |
| Tuples store elements inline (`Rc<[T]>`); `.x`/`.y`/`.z` and vec3 decoding borrow instead of cloning | 1,234 |
| A callee the resolver placed among the globals skips the `self`-method lookup | 1,194 |
| Globals by index (`Env` value vector + name index), read through a per-name hint | 1,135 |
| Instance fields as a short ordered vector (`value::Fields`) in a per-class layout, read through the hint | 1,108 |
| Float-with-float arithmetic first; `is_tuple`/`is_instance`/… read the cached body kind instead of borrowing the body | 1,070 |
| `tick_entities` reuses the last class's `update` lookup | 1,017 |

**The wasm measurement.** `web/bench.mjs` runs the web runtime's `bench_ticks` export under Node. It is the same interpreter build as the browser, headless, so the number doesn't depend on a visible page. CI runs it on every PR (report only). At this step, `examples/swarm_3d.twe` measured a **5.45 ms** median tick (best 4.87 ms) against the 4 ms target. After the global and field hints it measured **~5.0 ms** (best 4.3 ms). Then **4.67–4.81 ms** (best 3.96 ms).

**Chrome profile (CDP sampling, wasm name section).** The first browser profile found a cost the native numbers couldn't show: about 18% of the frame was spent reading the clock. The incremental GC sweep checked its time budget after every freed object, and on wasm each clock read is a JS round trip (`window()` → `performance()` → `now()`).

The sweep now checks every 256 objects, and the web shell caches `Performance`. In Chrome the tick went from 9.4–10.5 to 8.0–8.8 ms, and the frame rate from 114–127 to 134–141 fps, near the 144 Hz display cap.

The remaining profile:
- name resolution (`lookup_name`, `get_field`, `Env::get`, instance access, `find_method`, hashing): ~28% of busy time;
- the tree-walk itself (`eval_expr` / `eval_stmt` / arithmetic): ~26%;
- allocation (malloc, free, sweep): ~7%.

That confirms slot-indexed names as the next step.

## How slots work

`resolve` gives each frame's locals a slot, in order of first declaration, parameters first. That is the order the runtime binds them. It records a `Res` on every name:
- `Local { slot, name }`
- `Field`
- `Global`

It records nothing when it can't resolve the name.

The runtime then reads and writes a local as `frame.locals[slot]`. It checks that the slot holds that name (by pointer, since the resolver shares one `Rc<str>` per local), and skips the frame scan for fields and globals.

On any mismatch it falls back to the by-name lookup, so a wrong annotation is slow, never wrong. `eval::slot_misses()` counts those fallbacks. `tests/slots.rs` runs every test program and example and requires zero, which pins the resolver's frame model to the runtime's.
