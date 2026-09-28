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
