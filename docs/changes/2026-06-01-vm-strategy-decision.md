# Design Decision: Resolving the tree-walker / bytecode-VM split

**Date:** 2026-06-01
**Status:** ACCEPTED — Option C ratified by maintainer 2026-06-01. Spike
executed; the data-gate resolves to **Option B (freeze the VM as
experimental; tree-walker is the canonical v1.0 runtime)**. See §7 Resolution.
**Author:** craft-hardening line (post-v1.0.2).

> ⚠️ **This reopens a locked decision.** CLAUDE.md "What is locked" states:
> *"VM strategy: tree-walker for v0.1, bytecode VM for v0.3+."* That entry
> encodes a bet — that a single-pass bytecode VM would be the faster
> production runtime. This document does not unilaterally overturn that
> bet; it lays out the evidence and asks the maintainer to ratify one of
> three paths. Until ratified, the status quo (tree-walker default, VM
> opt-in) stands.

---

## 1. Why this is on the table

Twe ships **two interpreters** with overlapping responsibilities:

- **Tree-walker** (`src/eval.rs`, ~3,521 LOC) — the default. Runs every
  one of the 53 examples and all 56 test programs. `examples/survive_beta`
  (the v1.0-thesis Vampire-Survivors demo) runs on it.
- **Bytecode VM** (`src/compiler.rs` ~2,245 + `src/vm.rs` ~4,185 +
  `src/bytecode.rs` ~755 = **~7,185 LOC**) — opt-in via `--vm bytecode`
  (`cli.rs` default is `tree`, see `parse_common_flags`).

Two interpreters means **every new language feature must be implemented
twice** and kept in lockstep. The differential parity harness
(`tests/parity.rs`) exists *only* to police that divergence — it is pure
overhead the design imposes on itself. The split is the single largest
source of duplicated-semantics maintenance drag in the codebase.

The bet that justified that cost was performance. The data below says the
bet has not paid off **on the workloads that matter for the v1.0 thesis.**

## 2. Evidence

### 2.1 Performance (fresh `cargo bench --bench vm`, 2026-06-01, this machine)

| Benchmark | Tree-walker | Bytecode VM | Ratio |
|---|---|---|---|
| `sum_loop` (tight integer loop) | 22.2 ms | 32.1 ms | **VM 1.45× slower** |
| `float_loop` (tight float loop) | 16.7 ms | 30.6 ms | **VM 1.83× slower** |
| `fib_recursive` (function-call heavy) | 13.1 ms | 4.26 ms | **VM 3.07× faster** |

This reproduces the Phase 29 closeout numbers
(`docs/changes/2026-05-09-phase-29-closeout.md`). The Phase 8.5 exit
criterion ("≥3× speedup over the pre-tag VM / vs tree-walker") is **met on
`fib_recursive` and missed on the tight loops.**

**Why this matters for the thesis:** a Vampire-Survivors-class game's hot
path is *not* deep recursion. It is thousands of per-entity arithmetic
updates per frame — exactly the `sum_loop` / `float_loop` shape. On that
shape the VM is **~1.5–1.8× slower than the tree-walker today.** Shipping a
VS-class game on the VM as-is would make it *slower*, not faster. The one
workload the VM wins (recursion) barely appears in game loops.

### 2.2 The gap is partly fixable without exotic techniques

The Phase 29 closeout attributes the remaining gap to "computed-goto /
direct-threading, which Rust doesn't expose without nightly + unsafe." That
is true for the *last* increment, but the dispatch loop
(`src/vm.rs:353-372`) carries removable per-instruction overhead that does
**not** need computed-goto:

1. `if ip >= chunk.code.len()` — a bounds check every instruction
   (the compiler always emits `OP_RETURN`; this is defensive only).
2. `if crate::heap::gc_should_collect()` — a **thread-local lookup every
   instruction**. Could be checked only on allocating opcodes or batched
   every N instructions / at back-edges.
3. `let line = chunk.lines[ip]` — a line-table read **every instruction**,
   needed only when an error is actually raised. Could be looked up lazily
   from `ip` on the error path.

These three are cheap, safe-Rust wins that plausibly close a meaningful
fraction of the 1.45–1.83× gap. We have not measured how much — that is the
crux of the spike proposed below.

### 2.3 Feature-parity debt (constructs the VM rejects but the tree-walker runs)

From `src/compiler.rs` (`unsupported(...)` + "not yet" errors):

| Construct | compiler.rs | Note |
|---|---|---|
| `dialogue` / `say` / `choice` | ~536 | tree-walker only (Phase 5 task 3) |
| quantity literals (`3kg`, `100ms`) | ~1204 | Phase 4 gap |
| keyword arguments in calls | ~1240 | needs VM builtin-dispatch |
| non-literal field defaults | ~1528 | v0.1 limit (both backends) |
| class inheritance (`extends`) | ~1499 | unimplemented |
| top-level `on render()` | ~461 | tree-walker only (Phase 5 task 5) |
| non-death class events | ~428 | only `on X.death(e)` ships |
| typed holes `???` | ~1217 | tree-walker only (Phase 33) |

(Bare sibling-method calls were just closed in the craft-hardening pass —
`compiler.rs` now lowers them to `self.m()`.) Reaching full parity is
several sessions of work, each of which is *only* needed because a second
backend exists.

### 2.4 Which backend is real

`twec run` / `twec play` default to the tree-walker
(`cli.rs` `parse_common_flags`, documented `cli.rs:28-33`). Shipped games
embed both but the tree-walker is the canonical path. **The VM is, today, a
parallel experiment that has not become the production runtime it was
slated to be in "v0.3+".**

## 3. The decision

> **Is the bytecode VM still on the path to becoming Twe's primary runtime,
> or is it an experiment we should freeze and stop paying parity tax on?**

### Option A — Commit to the VM as primary
Close the perf gap (cheap wins → then computed-goto/threading on nightly)
and pay down the full parity backlog (§2.3) until the VM runs everything the
tree-walker does. Eventually retire the tree-walker or relegate it to a
reference oracle.

- **Pro:** one runtime long-term; bytecode is the right substrate for a
  high-entity-count game engine; the `fib` win shows the value rep is sound.
- **Con:** the parity backlog is multi-session; the tight-loop gap may need
  nightly+unsafe; and we'd be optimizing the *slower* backend toward the
  faster one. Highest cost, deferred payoff.

### Option B — Freeze the VM as experimental; tree-walker is canonical
Declare the tree-walker the committed v1.0 runtime. Drop the *obligation*
to dual-implement every new feature; new language features land in the
tree-walker first and reach the VM only opportunistically. Keep the VM and
the parity harness as a "no-regression on what the VM already supports"
gate. Focus runtime-perf effort on the tree-walker (profile it for
entity-heavy scenes; it's already the faster backend on game loops).

- **Pro:** stops the dual-implementation bleed immediately; aligns effort
  with the thesis (we ship on the tree-walker); fully reversible.
- **Con:** concedes the "bytecode VM for v0.3+" ambition; a tree-walker has
  a lower performance ceiling than a tuned bytecode VM for very large entity
  counts.

### Option C — Phased, data-gated (RECOMMENDED)
Do Option B's bleed-stop **now**, run a small time-boxed perf spike, and let
*data* decide whether to escalate to Option A:

1. **Now — stop the bleed.** Make the tree-walker the canonical v1.0
   runtime in CLAUDE.md. Downgrade parity from "every feature must be dual"
   to "the parity harness guards what the VM *already* supports; new
   features are tree-walker-first." (No code change beyond a CLAUDE.md /
   roadmap edit.)
2. **Add a game-representative benchmark.** The current benches are
   microbenchmarks. Add a `entity_update` bench to `benches/vm.rs`: N
   entities each running a small arithmetic `update(dt)` for K frames — the
   actual VS-class hot path. Decisions should be made against *this*, not
   `sum_loop`.
3. **Time-boxed VM perf spike (1–2 sessions).** Land the three safe-Rust
   dispatch wins from §2.2 and re-measure against `sum_loop`, `float_loop`,
   and the new `entity_update` bench.
4. **Decision gate.** If the spike brings the VM to **≥ tree-walker parity**
   on the tight-loop + `entity_update` benches → escalate to Option A
   (commit to VM, pay down parity backlog). If it does **not** → commit to
   Option B (freeze VM, tree-walker is the v1.0 runtime) and redirect
   perf effort to the tree-walker.

- **Pro:** no irreversible call made on a hunch; cheap experiment with a
  clear success metric; introduces the benchmark that should have been
  driving this all along.
- **Con:** defers the final answer by 1–2 sessions.

## 4. Recommendation

**Option C.** The honest reading of the data is that the VM is not currently
earning its parity tax *for the v1.0 thesis* — but the gap has identified,
cheap, un-tried fixes, so freezing it outright (Option B) would be premature
and committing to it outright (Option A) would be optimizing on faith. Stop
the bleed today, spend 1–2 sessions on the cheap wins plus a
game-representative benchmark, and ratify A-vs-B on the resulting numbers.

The two principles in tension are **Principle 2 ("one obvious way per
concept")** — which argues *against* maintaining two interpreters
indefinitely — and the locked **"bytecode VM for v0.3+"** ambition. Option C
honors Principle 2's spirit immediately (one canonical runtime now) without
discarding the VM investment before testing whether it can win.

## 5. If ratified, concrete next steps (Option C)

1. Edit CLAUDE.md "What is locked" + `docs/05-roadmap.md`: tree-walker is
   the canonical v1.0 runtime; VM parity obligation downgraded; record this
   doc as the rationale.
2. Add `entity_update` benchmark to `benches/vm.rs`.
3. Perf spike in `src/vm.rs`: lazy line decode, batched/opcode-gated GC
   safepoint, drop the per-instruction bounds check. Re-run `cargo bench`.
4. Write the A-vs-B verdict as a follow-up note with the spike numbers.

## 6. Open question for the maintainer

Which path do we take — A (commit to VM), B (freeze VM), or C (phased,
data-gated)? This document recommends **C**.

---

## 7. Resolution (2026-06-01)

Maintainer ratified **Option C**. The phased steps were executed:

### 7.1 Game-representative benchmark added
`benches/vm.rs` gained `entity_update`: 2,000 entities each running an
arithmetic `update(dt)` for 30 frames (~60k update calls) — the real
VS-class per-frame shape.

### 7.2 Perf spike landed
`src/vm.rs` dispatch loop now **strides the GC safepoint**: the
per-instruction `gc_should_collect()` (a thread-local + `RefCell` borrow)
is gated behind a local countdown (`GC_CHECK_STRIDE = 64`). Clearly correct
— a collect still fires within 64 instructions of the heap crossing its
threshold. Both aggressive-GC stress tests (`vm_entity_tick_runs_under_
aggressive_gc`, `snake_runs_under_aggressive_gc`) stay green; full suite
green; clippy clean. (The other two candidate wins — lazy line decode and
dropping the bounds check — were *not* taken: `line` is threaded into every
handler signature, so lazy decode is a wide, risky refactor for marginal
gain; the bounds check is a well-predicted branch and a safety net.)

### 7.3 Measured effect (`cargo bench --bench vm -- --quick`, this machine)

VM absolute times, before vs after the spike (tree-walker times are noisy
run-to-run; the VM column is the stable signal):

| Benchmark | VM before | VM after | VM vs tree (after) |
|---|---|---|---|
| `sum_loop` | ~35.1 ms | **~24–26 ms** | ~1.2–1.65× slower |
| `float_loop` | ~24.8 ms | ~27 ms | ~1.3–1.65× slower |
| `entity_update` (game hot path) | ~82.6 ms | ~80–81 ms | **~1.3–1.5× slower** |
| `fib_recursive` | ~4.1 ms | ~3.8–4.1 ms | ~2.8× *faster* |

The spike is a real win on the tightest loop (`sum_loop` VM dropped ~30%),
confirming the per-instruction GC check was a genuine cost. But it did
**not** bring the VM to parity: the VM is still ~1.3–1.5× slower than the
tree-walker on the game-representative `entity_update`, and only wins on
recursion (`fib`), which is not the game hot path. Most of `entity_update`'s
gap is the VM's method-invoke / frame push-pop machinery and the `match op`
dispatch — closing those needs computed-goto / direct-threading (nightly +
unsafe), a multi-session VM-internals investment.

### 7.4 Verdict: Option B

The data-gate condition for escalating to Option A ("spike brings the VM to
≥ tree-walker parity on the tight-loop + `entity_update` benches") is **not
met**. Therefore:

- **The tree-walker is the canonical v1.0 runtime.** It is faster on every
  loop-shaped workload including the game hot path, and it already runs
  every example and ships every game.
- **The bytecode VM is frozen as experimental.** It stays in-tree, opt-in
  via `--vm bytecode`, and the parity harness keeps guarding what it already
  supports. New language features are **tree-walker-first**; the dual-
  implementation *obligation* is dropped.
- **Re-entry criterion.** The VM returns to the critical path only if it
  demonstrably **beats the tree-walker on `entity_update`** (not a
  microbenchmark, not recursion). That almost certainly requires the
  computed-goto / direct-threading refactor; it is justified only if a real
  game profiles as interpreter-dispatch-bound — which `survive_beta`
  (~100s of entities) currently does not.
- The GC-stride win **stays** regardless: it improves the VM unconditionally
  and costs nothing.

This honors Principle 2 ("one obvious way per concept") — one canonical
runtime — while preserving the VM investment behind a concrete, measurable
re-entry bar.
