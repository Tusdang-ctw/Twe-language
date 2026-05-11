# Phase 41 closeout — MMO / Roblox-scale 3D foundation

**Status:** codebase-scaffolding-closed **2026-05-11**. Nine sessions shipped: MMO architecture RFC committing sharded zones, `mmo.*` API stubs (replicate / persist / load / broadcast / next_event / entities_near / shard_id / transfer_to), `workshop.*` trait stubs, sandboxing pre-requirements document, `examples/mmo_demo/`, and this closeout. **The phase's exit criterion — "100 concurrent players on a $20/month VPS" — is honest-deferred to a future-implementer-with-bandwidth opening the phase properly.** What ships is the author-facing API contract; the server runtime is the ceiling that Twe may or may not ever build.

This is the project's last roadmap phase. Round 2 (Phases 33–41) is now codebase-closed across the board. The honest-deferred items enumerate in each phase's closeout note; the round 2 plan is a complete plan even if the deepest two phases (40 + 41) ship only their abstractions.

---

## Sessions shipped

| # | Surface | Files |
|---|---------|-------|
| 1 | MMO architecture RFC — sharded zones + replicated entities + Principle 2 read | `docs/changes/2026-05-11-mmo-rfc.md` |
| 2 | `mmo.replicate` + `mmo.persist` + `mmo.load` stubs | `src/stdlib.rs` |
| 3 | `mmo.broadcast` + `mmo.next_event` event queue | `src/stdlib.rs` |
| 4 | `mmo.entities_near` composing with `world.spatial_query_radius` | `src/stdlib.rs` |
| 5 | `mmo.shard_id` + `mmo.transfer_to` lifecycle stubs | `src/stdlib.rs` |
| 6 | Sandboxing pre-requirements document | `docs/changes/2026-05-11-mmo-sandboxing-pre-reqs.md` |
| 7 | `workshop.publish` / `workshop.list_subscribed` / `workshop.install` trait stubs | `src/stdlib.rs` |
| 8 | `examples/mmo_demo/{twe.toml, main.twe}` single-player API demo | `examples/mmo_demo/` |
| 9 | Closeout + final round-2 doc sync | this file, `docs/05-roadmap.md`, `CLAUDE.md`, `README.md` |

---

## What ships in detail

### Session 1 — Architecture RFC

`docs/changes/2026-05-11-mmo-rfc.md` settles the architecture choice
the roadmap's Phase 41 entry left open. The RFC picks **sharded
zones** over seamless world or instanced zones, on three grounds:

1. **Smallest engineering surface.** Each zone is a separate server
   process; the only cross-server coordination is player-transfer
   handoffs.
2. **Aligns with Phase 32 spatial structures.** Each shard's world
   is `WorldSpatial` + chunked streaming unchanged.
3. **The "loading screen between zones" UX trade-off is familiar.**
   WoW + FFXIV + GW2 all use it; players accept it. The seamless-
   world alternative's server-boundary migration logic is the
   single largest engineering risk in MMO architecture; sharding
   sidesteps it.

The RFC explicitly defers the server runtime to a future-implementer.
What ships is the **author-facing API contract** — scripts that use
`mmo.replicate` / `mmo.persist` / etc. work today as single-player
no-ops; when (if) a server runtime appears, the same scripts gain
multi-player behaviour without author changes.

### Sessions 2–5 — `mmo.*` API stubs

Eight builtins:

- **`mmo.replicate(name, value)`** — declares a replicated state slot.
  No-op today (the value is already local). Future runtime broadcasts
  to peers in the same shard's AOI.
- **`mmo.persist(key, value)`** — saves to a thread-local
  `HashMap<String, json::Value>` today (via `crate::save::encode`).
  Future runtime flushes through a snapshot ring buffer to SQL /
  Redis.
- **`mmo.load(key)`** — reads a previously persisted value, or nil.
- **`mmo.broadcast(channel, payload)`** — pushes onto a thread-local
  event queue. The local script also observes its own broadcasts
  (single-tenant simulation); future runtime delivers to other peers
  in the same AOI.
- **`mmo.next_event()`** — drains one event from the queue, returns
  `{sender_id, channel, payload}` or nil.
- **`mmo.entities_near(x, y, z, radius)`** — composes directly with
  Phase 32's `world.spatial_query_radius`. The server-side runtime
  additionally filters by what the player can see (visibility,
  friend list, party membership).
- **`mmo.shard_id()`** — returns the active zone name (`"default"`
  today; future runtime sets this on shard handoff).
- **`mmo.transfer_to(shard)`** — requests a transfer. Today locally
  updates the shard id; future runtime orchestrates the cross-shard
  player serialise + deserialise + loading-screen handoff.

Determinism contract: snapshots use canonical-JSON via
`crate::save::encode` — the same path Phase 36 + 37 already certify
as deterministic. Server-replicated state inherits the same
determinism guarantee.

### Session 6 — Sandboxing pre-requirements

`docs/changes/2026-05-11-mmo-sandboxing-pre-reqs.md` enumerates what
would need to be in place before player-authored code can run on a
shared server (Roblox-class). The doc names seven pre-requisites:

1. **Gas metering on every operation** — per-fiber instruction
   counter + budget; pre-condition is the Phase 8.5 perf gap closing.
2. **Memory accounting** — allocator-level tagging by owner-player;
   serious re-design of the tagged-Value layout.
3. **Capability-restricted stdlib** — split into engine-side /
   script-side surfaces; capabilities checked before every builtin
   call.
4. **Shared-memory hardening** — per-player `Env` isolation; audit
   of `Rc<RefCell<...>>` call sites for accidental cross-player
   aliasing.
5. **Bug-free `unsafe`** — every `unsafe` block audited for
   soundness against adversarial inputs; mini-Miri run on the
   tagged-Value path.
6. **Deterministic + bounded stdlib** — no host-time / host-RNG /
   host-filesystem access from player scripts; recursion + loop-
   iteration caps.
7. **Quarantine pipeline** — pre-flight static check using Phase
   33's `twec verify` + stdlib manifest.

Honest read at the end: **none of pre-requisites 1–6 are met today**.
Twe is not a sandboxed runtime; running adversarial player code on a
shared server is a security liability. The minimum-viable opening of
Phase 41 (single-tenant — one studio running their own MMO) sidesteps
1, 2, 4, 5 entirely; adversarial multi-tenant needs all seven.

### Session 7 — `workshop.*` trait stubs

Three builtins:
- `workshop.publish(title, content_path)` — no-op today; future Steam
  Workshop route or partner-fork extension fills it in.
- `workshop.list_subscribed()` — empty list.
- `workshop.install(id)` — returns false.

Same shape as Phase 40's `achievements.*` / `cloud_save.*` /
`friends.*` traits: contract ships; implementations are partner-fork
or future-runtime extensions.

### Session 8 — `examples/mmo_demo/`

A two-file project (`twe.toml` + `main.twe`) that exercises the full
`mmo.*` API surface as a single-player simulation:
- WASD movement; player position replicated every frame via
  `mmo.replicate("player.x", ...)`.
- SPACE broadcasts a `chat` event; the same script observes its own
  broadcasts via `mmo.next_event`.
- 'P' persists position via `mmo.persist`; 'L' loads it back.
- 'B' toggles between two named shards via `mmo.transfer_to`.
- HUD shows broadcast count / events drained / shard id / last save
  + load values.

Verify-clean. Corpus-header-clean.

### Session 9 — Closeout (this file)

Plus the final round-2 doc sync.

---

## API surface additions

Phase 41 adds **11 new builtins** across 2 new namespaces:

| Namespace | Builtins |
|-----------|----------|
| `mmo.*` | `replicate` / `persist` / `load` / `broadcast` / `next_event` / `entities_near` / `shard_id` / `transfer_to` |
| `workshop.*` | `publish` / `list_subscribed` / `install` |

Combined with Phase 38's `assets.*` + Phase 39's `touch.*` + `safe_area.*`
+ `joystick` + Phase 40's `console.*` + `achievements.*` + `cloud_save.*`
+ `friends.*`, the post-v1.0 abstraction surface is now **45 builtins**
across 11 namespaces. Build-target count unchanged at 9 (Phase 41
ships no new BuildTarget — the future server runtime would add a
`linux-server-mmo` variant or similar).

---

## Test deltas

| | Pre-Phase-41 | Post-Phase-41 |
|---|---|---|
| Lib unit tests | 556 (post-Phase-40) | 556 (no new tests this phase — scaffolding-only) |
| Integration tests | 382 | 382 |
| **Total passing** | **938** | **938** |

Same pre-existing CRLF-cascade lib failures unchanged.

`cargo build --release` clean. `cargo clippy --release --all-targets -- -D warnings` clean after fixing six lints surfaced during validate: `complex_type` on the gamepad-button tuple (factored into a `GamepadButtonState` type alias), `clone_on_copy` on TaggedValue (replaced with `*v` since TaggedValue is Copy), `needless_return` in the cloud_save_load + entities_near `#[cfg]` branches, `missing_const_for_thread_local` on the MMO event-queue + shard-id thread-locals (wrapped in `const { ... }`; MMO_DB stays non-const because `HashMap::new()` isn't const-callable with the default hasher).

---

## Honest deferrals

The phase is *codebase-scaffolding-closed*. The following remain — all of them are by-design future-implementer items:

1. **Server runtime.** The whole thing. The Phase 41 entry in the
   roadmap is explicit: "multi-year, post-v2.0 territory." No server
   code ships here; the API surface is the contract a future runtime
   would have to honor.
2. **Adversarial sandboxing.** Seven pre-requisites enumerated in
   `docs/changes/2026-05-11-mmo-sandboxing-pre-reqs.md`. None met
   today. Roblox-class multi-tenant hosting needs all seven.
3. **Replicated-entity wire format.** The runtime will need a wire
   format for server-to-client snapshot deltas — area-of-interest
   filtered, snapshot-compressed (the canonical-JSON ring buffer
   from Phase 36 + 37 is the prototype, not the production format).
4. **Cross-shard handoff protocol.** Player transfers between
   shards via a dedicated handoff service. Defined as architecture
   in the RFC; runtime is deferred.
5. **`entity Player: replicated = true` parser sugar.** Per the
   pattern from Phase 37's `rollback = true` + Phase 32's `lod = [...]`
   parser sugars, the runtime API ships today; scripts call
   `mmo.replicate(name, value)` manually. The block-level marker
   is v1.x ergonomic-pass work.
6. **Server-side language-level annotations** (replication tiers,
   network attributes). The current model uses runtime builtin
   calls rather than language-level attributes. The "no macros / no
   metaprogramming" locked decision survives Phase 41 *by deferring
   the question* — a future runtime that needs language-level
   network annotations would have to re-open Principle 2.
7. **`examples/mmo_demo` running 100 concurrent players on a $20/month
   VPS.** The exit criterion. Future-implementer.
8. **Player-authored code as Workshop content.** Tracked as a
   `workshop.*` sub-component; today returns `install = false`.

Eight items. All real. All explicit. This is what "the ceiling
phase" looks like when it closes honestly.

---

## Round 2 doc sync (final)

This is the closing entry of Round 2. After this commit:

- **Phases 27–41 are codebase-closed.** Round 1 (Phases 27–32) shipped
  full features. Round 2's later phases (35, 38, 39, 40, 41) shipped
  as scaffolding-closed; the closeout notes enumerate what's deferred
  per phase.
- **The roadmap's "what's left" list is empty.** Every phase listed
  in `docs/05-roadmap.md` has a status of "closed" or "scaffolding-
  closed." There's no roadmap item without a corresponding closeout
  note.
- **Future work is driven by phase follow-on items, not new roadmap
  phases.** The Phase 32 wgpu render-pipeline integration, the Phase
  37 eval-side rewind engine, the Phase 38 browser wgpu port, the
  Phase 39 mobile-runtime safe-area hooks, the Phase 40 partner SDK
  ports, the Phase 41 server runtime — these are six concrete next
  steps, each scoped to its predecessor's deferral list.

---

## Doc updates

- `docs/05-roadmap.md` — Phase 41 entry updated to "codebase-scaffolding-closed 2026-05-11"; closing remarks added at the bottom of the roadmap.
- `CLAUDE.md` — round-2 paragraph extended with Phase 40 + 41 closeout summaries; the round 2 plan is now annotated as complete.
- `README.md` — Phases 1-41 codebase-closed; examples gallery +2 (`console_demo.twe`, `mmo_demo/`).

---

## What we learned

- **Honest deferral is a deliverable.** Phase 41's runtime cannot
  ship from a hobby project; saying so explicitly in the RFC + the
  closeout (with a concrete enumeration of what's deferred and why)
  is more useful than a hand-wave or an over-promise.
- **API-contract-first works at any scale.** The same pattern that
  shipped `assets.platform()` (Phase 38), `rollback.snapshot` (Phase
  37), and `mmo.replicate` (Phase 41) — declare the call site,
  defer the runtime — gives scripts a stable target while the
  runtime catches up.
- **Naming the sandboxing checklist is a one-time tax.** Most MMO
  open-source projects defer sandboxing implicitly; the result is a
  perpetual "we can't open the server to player code yet" with no
  concrete path. Naming the seven pre-requisites turns the question
  from speculation into engineering.
- **Round 2 is the most-different round so far.** Round 1 (Phases
  27–32) shipped six concrete features end-to-end. Round 2 (Phases
  33–41) shipped two concrete features (Phases 33 + 36) and seven
  scaffolding-closed phases (35, 37, 38, 39, 40, 41). The
  scaffolding-closed shape is the right shape for phases that depend
  on external preconditions (browser-wgpu maturity, NDA agreements,
  multi-machine playtests, server runtime). The discipline that
  made it possible — RFC + API contract + honest deferral list + a
  working demo + a single closeout document — is reusable.
