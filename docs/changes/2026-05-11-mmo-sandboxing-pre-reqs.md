# MMO sandboxing pre-requirements

> Phase 41 session 6. Companion to
> [`docs/changes/2026-05-11-mmo-rfc.md`](2026-05-11-mmo-rfc.md).

If Twe ever runs adversarial player-authored code on shared servers
(Roblox-class), the language + runtime need hardening before the
first untrusted byte executes. This document catalogues what would
need to be in place — *not* a design for the sandbox itself, but the
checklist a future-implementer would work through when (if) Phase 41
opens properly.

The document exists so the question "is Twe ready to host
player-authored code today?" has a concrete answer (**no**), and the
follow-on work needed is enumerated rather than vibes-based.

---

## Threat model

Adversarial player Twe code might:

- **Steal CPU.** Infinite loops, exponential-explosion algorithms,
  malicious sleep-busy-poll patterns that wedge a server thread.
- **Steal memory.** Allocating gigabytes of arrays / strings to OOM
  the server.
- **Snoop on other tenants.** Reading other players' state from
  shared memory, the database, the filesystem.
- **Tamper with the engine.** Exploiting `unsafe` code paths or
  language bugs to escape the script sandbox and run native code.
- **Network abuse.** Sending crafted packets to other peers,
  exfiltrating data to external services, DDoS amplification.

The sandbox has to defend against each of these. The hardening list
below is *the* prerequisites — any one of them missing means user-
generated code on the server is a security liability.

---

## Pre-requisite 1: gas metering on every operation

The bytecode VM today (Phase 8.5) executes instructions until the
program terminates or yields voluntarily via a fiber. There's no
"this player has used 100ms of CPU; pause them" mechanism.

**What's required:**

- A per-fiber instruction counter that increments on every bytecode
  dispatch.
- A configurable budget — `mmo.gas_limit_per_tick(player, n)` sets
  the per-player budget.
- When the counter exceeds the budget, the fiber suspends; control
  returns to the runtime. Scripts that consume too much gas get a
  warning event; repeat offenders get throttled or kicked.

**Why it's hard:** every instruction is a hot path. Adding a counter
increment to bytecode dispatch costs ~10% raw throughput, per
benchmarking notes from similar runtimes (Lua's debug.sethook, Luau's
interrupt vector). The cost has to be eatable.

**Pre-condition:** the Phase 8.5 "3× speedup vs pre-tag-VM" perf gap
should close *first*. Gas metering on top of a slow VM compounds.

---

## Pre-requisite 2: memory accounting

Every allocation in script context needs to be charged to a player
account. The current GC (Phase 8.5 tracing) doesn't track allocation
provenance.

**What's required:**

- Allocator-level tagging — each `Value::from_list` / `Value::from_object`
  / `Value::from_string` call records which player it's for.
- Per-player memory budget. `mmo.memory_limit_per_player(player, mb)`
  sets the cap.
- Allocation failure when the budget is exceeded. Returns a Twe-side
  error the script can catch (or the player is kicked if errors are
  unhandled).

**Why it's hard:** the tagged-Value representation doesn't reserve
bits for an owner-id. We'd either need a side table (extra cache-miss
per allocation) or to grow the heap-body header. Either path
invalidates parts of the Phase 8.5 NaN-tagging contract; serious
re-design.

---

## Pre-requisite 3: capability-restricted stdlib

The default stdlib exposes filesystem I/O (`save_to_path`),
network I/O (`net.host`, `net.connect`), process control (`quit`),
and platform integrations (`achievements.unlock`, Steam routes).
Player code on the server must not call these.

**What's required:**

- A "capability set" struct that the runtime checks before every
  stdlib call. Default-deny on the server.
- The full `mmo.*` namespace would be available to player code; the
  rest is gated.
- Stdlib functions document their required capability — e.g.
  `save_to_path` is `capability: filesystem`.
- A capability-aware loader: when the server starts a player's
  fiber, it installs the player's capability set; calls outside the
  set raise a runtime error.

**Why it's hard:** the current stdlib has no notion of capability —
every builtin is unconditionally exposed. Splitting the stdlib into
"engine-side" and "script-side" surfaces is essentially a Phase 6
ergonomic re-do, which v0.1 explicitly punted on.

---

## Pre-requisite 4: shared-memory hardening

Twe Values are reference-counted; today, two scripts in the same
runtime can in principle share a `Value::from_list` if the runtime
hands them the same `Rc<RefCell<...>>`. Player A could see Player
B's state.

**What's required:**

- Per-player Env isolation: every player's globals + stdlib state
  live in a distinct `Env`. The server's "world" state is exposed
  via the `mmo.*` namespace, which always copies (not aliases).
- Audit pass over the current `Rc<RefCell<...>>` call sites to
  confirm no script-accessible `Value` aliases cross player
  boundaries.

**Why it's medium-hard:** the runtime already has multiple `Env`
instances (modules, fibers). The hardening is mostly verification
+ test discipline + per-player Env at startup.

---

## Pre-requisite 5: bug-free `unsafe`

The codebase narrowly uses `unsafe` in `src/tagged_value.rs` (NaN
tagging requires it) and `src/window_focus.rs` (Win32 + macOS
Objective-C bindings). The rest of the repo is `unsafe_code = "deny"`.

**What's required:**

- Every `unsafe` block in the codebase audited for soundness against
  the adversarial-script threat model. Today: cleared for trusted
  scripts; not cleared for adversarial ones (no specific bug, but
  no certified review either).
- A "mini-Miri" run on the tagged-Value path with adversarial inputs.
- A bug-bounty program once the sandbox is real.

**Why it's hard:** verifying unsafe-soundness against adversarial
inputs is a multi-month engineering task. Tools like Miri help but
don't catch every class of issue.

---

## Pre-requisite 6: deterministic + bounded stdlib

Player scripts running on the server must not depend on host
characteristics:

- **No host-time access.** Player code reads `time.now()` from a
  server-controlled clock, not the OS clock. This is also a
  determinism requirement for the eventual server-authoritative
  replication.
- **No host-RNG access.** `random.*` reads from a server-seeded RNG
  per player, not `/dev/urandom`. Players don't get a different
  RNG stream by reconnecting.
- **No host-filesystem access.** Per pre-requisite 3.
- **Bounded recursion depth.** `recursion_limit_per_fiber(n)`.
  Today fibers can recurse until the stack overflows.
- **Bounded loop iteration.** Per pre-requisite 1 (gas).

**Why it's medium-hard:** most of these are clamp-in-stdlib changes
with a per-runtime config struct. The recursion-depth one needs
runtime support.

---

## Pre-requisite 7: a quarantine pipeline for player code

Before player Twe code runs on the server, the runtime should:

- Parse + verify the script (uses Phase 33's `twec verify`).
- Reject scripts that import disallowed modules or call disallowed
  stdlib functions (pre-requisite 3).
- Optionally: re-run the script in a "ghost" instance for a few
  ticks before promoting it to a live server, to catch obvious
  performance pathologies.

**Why it's easy** (relatively): Phase 33's verify v2 + stdlib
manifest already give the static-analysis tool needed.

---

## Out-of-scope for sandboxing pre-reqs

These are real concerns but not blockers to opening Phase 41:

- DDoS protection at the network layer. That's an infra problem, not
  a language problem.
- Encrypted player data at rest. Standard database-encryption story.
- Cross-site scripting in the workshop publishing UI. Web-side.
- IP-level abuse. Cloud-provider firewalling story.

---

## Honest read

Today (2026-05-11), **none of pre-requisites 1–6 are met**. Twe is
not a sandboxed runtime. Player-authored code running on a shared
server would be a security liability.

Whether Twe ever closes this gap is the future-implementer decision
the Phase 41 RFC honestly defers. The gap is enumerated here so that
decision is informed rather than speculative.

The minimum-viable opening of Phase 41 (single-tenant server, no
adversarial player code — e.g. a single studio running their own
MMO with their own scripts) sidesteps pre-requisites 1, 2, 4, 5
entirely. Pre-requisites 3, 6, 7 are still relevant; the rest become
relevant only when adversarial multi-tenant joins the picture.
