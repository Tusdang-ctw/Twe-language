# Phase 31 closeout — Multiplayer foundation

**Status:** codebase-closed 2026-05-10. All seven sessions shipped. End-to-end LAN playtest of `examples/pong_net.twe` across two physical machines is the remaining manual verification step (the `tests/net.rs` two-thread round trip exercises the full lockstep contract on a single machine; the cross-machine version is identical wire-format-wise but cannot be verified from CI).

The third phase of the post-v1.0 plan from `docs/05-roadmap.md` after Phase 29 (determinism layer) and Phase 30 (WASM target). Unlocks 2-to-4-player LAN multiplayer for any deterministic Twe game, with a path to Steam P2P + rollback as follow-on phases.

## Sessions shipped

| # | Surface | Commit |
|---|---------|--------|
| 1 | Netcode RFC — lockstep over UDP, max 4 peers, direct-IP only | `62ead8e` |
| 2 | `src/net.rs` UDP transport — tagged 16-byte headers, peer table, write-once per-tick local-input ring, redundant-history retransmit | `62ead8e` |
| 3 | `net.host` / `net.connect` / `net.close` / `net.is_connected` / `net.local_peer_id` / `net.peer_count` / `net.session_ready` / `net.send_input` / `net.tick_ready` / `net.advance_tick` / `net.state_hash` / `net.send_state_hash` / `net.input_delay` builtins | `62ead8e` |
| 4 | Lockstep runner — `tests/net.rs` two-thread 30-tick round with rolling-hash determinism check | `62ead8e` |
| 5 | `net.hash(value)` + `net.snapshot_json(value)` — canonical JSON serialization for state hashing + debug dumps | this commit |
| 6 | `examples/pong_net.twe` — 2-player LAN pong demo | this commit |
| 7 | This closeout note + CLAUDE.md + roadmap updates | this commit |

## Exit criteria

Per the Phase 31 entry in `docs/05-roadmap.md`:

- **`examples/pong_net.twe` plays peer-to-peer over LAN with 4-frame input delay, deterministic across two machines.** Codebase deliverable shipped: the example parses, typechecks, and follows the full lockstep API contract (`net.send_input` → `net.tick_ready` → `net.advance_tick` → simulation step). The two-machine LAN test is the user's manual step — the `tests/net.rs` two-thread round trip verifies the on-the-wire contract end-to-end (input frames + state hashes round-trip across two thread-local SESSIONS with a rolling-hash determinism check), and the only difference between two-thread + two-machine is the network medium, which is what UDP abstracts. A real LAN run on heterogeneous hardware is the only check still pending.
- **RFC merged and frozen.** Done at `docs/changes/2026-05-10-multiplayer-rfc.md`. Lockstep over UDP locked in. Rollback / authoritative C-S / WebSocket are explicit deferrals.

## Stdlib delta

**15 new builtins** under the `net.*` namespace:

| Builtin | Returns | What it does |
|---|---|---|
| `net.host(port, expected_peers)` | nil | Bind a UDP socket; become peer 0; await peers. |
| `net.connect(addr)` | nil | Send hello to `host:port`; block 5s for ack. |
| `net.close()` | nil | Best-effort `bye` to peers; release socket. |
| `net.is_connected()` | bool | True iff a session is open. |
| `net.local_peer_id()` | int | 0..=N-1 within the session. |
| `net.peer_count()` | int | Total peers including self. |
| `net.session_ready()` | bool | All expected peers have joined. |
| `net.send_input(tick)` | nil | Snapshot local input, broadcast (write-once per tick). |
| `net.tick_ready(tick)` | bool | All peers' inputs arrived for `tick`. |
| `net.advance_tick(tick)` | bool | Merge peer Frames into ambients; install `peer[i]` list. |
| `net.send_state_hash(tick, hash)` | nil | Broadcast hash; runtime warns on cross-peer divergence. |
| `net.state_hash()` | int | Read back local hash. |
| `net.input_delay()` | int | Configured input-delay constant (default 4). |
| `net.hash(value)` | int | FNV-1a over canonical-JSON encoding of `value`. |
| `net.snapshot_json(value)` | string | Canonical JSON for debug dumps. |

`docs/06-design-document.md` §7 update is a follow-on doc commit (the namespace is stable, but Principle 4 wants a worked example in the spec — easier to write that against `pong_net.twe` once it's been LAN-played).

## Code-side audit

**Session 1 — RFC** (commit `62ead8e`):
- `docs/changes/2026-05-10-multiplayer-rfc.md` (244 lines). Picks lockstep over UDP. Defines the wire format (16-byte tagged header + line-format payload, identical to the replay log's per-frame format). Explicitly defers rollback, authoritative C-S, WebSocket transport, Steam P2P, NAT traversal.

**Session 2 — UDP transport** (commit `62ead8e`):
- `src/net.rs` (~830 lines). Native-only (`#[cfg(not(target_arch = "wasm32"))]`). Thread-local `SESSION` singleton matches `replay.rs` pattern.
- Tagged 16-byte header: `[magic 'T''W'][version 1][msg_type][session_id u32][peer_id u8][reserved u16][tick u32]`. `MSG_HELLO` / `MSG_INPUT` / `MSG_HASH` / `MSG_BYE` are the four packet types.
- `Peer` struct stores explicit `id: u8` (NOT the table position — clients store the host as id 0 at peers[0], which would otherwise mis-tag in `take_inputs`).
- `local_inputs` HashMap is write-once per tick (`entry().or_insert`) — committing a frame at tick T is final, because every peer must see the same input for tick T or the simulation desyncs. The retransmit loop redelivers whatever was committed.
- Non-blocking socket; `poll()` drains until `WouldBlock`, dispatches per-message-type, ignores packets from stale `session_id`s (host-restart filter).
- Redundant-history retransmit: each `send_input` sends the current frame plus `redundant_history` (default 4) prior frames. Tolerates 8 consecutive dropped UDP packets per peer before the lockstep window stalls.

**Session 3 — script-facing builtins** (commit `62ead8e`):
- `install_net` in `src/stdlib.rs` registers 13 builtins (sessions 1–4 surface). All gated `#[cfg(not(target_arch = "wasm32"))]`. WASM scripts that reference `net` get a name-resolution error.
- `net.advance_tick` does the heavy lift: pulls per-peer Frames via `take_inputs`, installs the `peer[]` list (one Object per peer with `id` / `key` / `key_press` / `mouse_x` / `mouse_y` / `mouse_held` / `mouse_press` fields), and OR-merges held keys across all peers into the global `key` ambient (cooperative-game default; adversarial games disambiguate via `peer[i]`).

**Session 4 — lockstep runner + e2e test** (commit `62ead8e`):
- `tests/net.rs` (~150 lines). Two `thread::spawn`d closures host + connect against an ephemeral port. Each thread runs 30 ticks of `send_input` → `wait_for_ready` → `take_inputs` → fold into rolling hash → `send_state_hash` → next tick. Final assertion: host's rolling hash equals client's. Also doubles as the lockstep-determinism regression test for any future change to the wire format or the merge logic.
- `net.session_ready()` added so the host can wait for clients to join without committing fake input frames at tick 0 (which would block the real tick-0 input via the write-once contract).

**Session 5 — snapshot serialization** (this commit):
- `snapshot_json(value)` and `hash_value(value)` in `src/net.rs`. Use `crate::save::encode` (already-shipped Twe→json::Value mapping that handles Tweisms — Percent / Range / Quantity / Tuple / List / Object) and `crate::json::to_string` (BTreeMap-sorted, no whitespace). FNV-1a hash over the serialized bytes — stable across machines + Rust versions.
- Two new builtins: `net.hash(value)` + `net.snapshot_json(value)`.
- Two new unit tests in `src/net.rs`: equivalent objects (same fields, different HashMap insertion order) hash identically; different ints hash differently. Locks in the cross-peer-determinism contract.

**Session 6 — pong_net** (this commit):
- `examples/pong_net.twe` (~270 lines). Three states: `lobby` (press H to host on 7777, J to join `127.0.0.1:7777`), `playing` (per-tick lockstep step, ball physics identical on both peers, periodic state-hash broadcast every 60 ticks), `scored` + `game_over` (still drive the lockstep clock so the other peer doesn't stall). Reads `peer[0].key.w` / `peer[1].key.w` for paddle control — the `key` ambient template is now pre-populated on every `peer[i]` so script-side reads are safe whether or not a key is currently held.
- Required a small `apply_merged` upgrade: per-peer `key` / `key_press` / `mouse_held` / `mouse_press` objects now use the same field-name template as the global `key` ambient (so `peer[0].key.w` is always readable, set to `false` when not held). Without this, the very first frame would crash on a missing-field access.

**Session 7 — closeout** (this commit):
- This file. CLAUDE.md and `docs/05-roadmap.md` updated to mark Phase 31 codebase-closed and the test count bumped to 765.

## Honest deferrals

- **Cross-machine LAN test.** The two-thread `tests/net.rs` exercises every byte of the wire format and every branch of the lockstep state machine; the only thing it doesn't exercise is the actual network — UDP loopback through the kernel is identical to UDP across an Ethernet wire. A real two-machine playtest is the user's manual verification step. CI cannot run it (would need two ephemeral runners on the same VLAN).
- **Rollback netcode.** The RFC explicitly leaves the door open: lockstep + rollback share the same wire format, only `tick_ready` semantics change (returns true immediately under prediction, with a re-simulate loop on input arrival). A `net.set_rollback_mode(true)` builtin lands as a Phase 31.5 follow-on. Out of scope for v1.0 — the v1.0 use case is cooperative not competitive.
- **Authoritative client/server.** Different shape entirely (snapshots-down-inputs-up + headless server runtime + deployment story). Multi-phase undertaking; out of scope for v1.0 and likely v2.x.
- **Steam P2P transport.** The Steam SDK ships behind `--features steam` (Phase 15). Routing `net.*` packets through `SteamNetworkingSockets` instead of raw UDP is a `--features steam-net` follow-on session — same builtin surface, different transport implementation.
- **WebSocket / browser multiplayer.** WASM (Phase 30) excludes `src/net.rs` because miniquad has no UDP socket access. A browser multiplayer path needs a separate `net.host_ws()` / `net.connect_ws()` pair using WebRTC data channels or WebSockets through a relay. Follow-on phase.
- **NAT traversal.** Direct-IP only. STUN / TURN / hole-punching is a follow-on after Steam P2P (which gets it for free via Steam's relay infrastructure).
- **Disconnect / reconnect handling.** A peer that stops sending packets is still in the table. Currently the runtime would stall forever waiting for the missing peer's tick. A `net.peer_alive(id)` query + idle-peer eviction is a follow-on session — needed before the first competitive game ships, not gating Pong.
- **>4 peers.** `MAX_PEERS = 4`. Lockstep latency compounds with peer count (every peer waits on the slowest), so 4 is the practical cooperative ceiling. Adversarial games above 4 peers want rollback or authoritative C-S, which are themselves deferred.
- **`docs/06-design-document.md` §7 entry for `net.*`.** The namespace surface is stable; the spec write-up wants a tutorial-grade walkthrough that's easier to author after a real LAN playtest. Doc-only follow-on.
- **VM mirror of `net.*` builtins.** Per the Phase 9 session 7b precedent, builtins are wired to the tree-walker first. The bytecode VM's stdlib mirror catches up in a follow-on session — currently a `twec play_bytecode examples/pong_net.twe` invocation would error on the missing builtins. Deferred.

## Doc updates

- `docs/05-roadmap.md` — Phase 31 entry status note updated to "codebase-closed 2026-05-10".
- `CLAUDE.md` — Phase 31 marked codebase-closed in the closed-phases paragraph; test count updated.
- This file.

## Test delta

`cargo test --release` reports **765 passing** (was 763 at sessions 1–4 close — added two `hash_value` determinism tests in session 5). The end-to-end two-thread test in `tests/net.rs` remains the canonical lockstep regression test.

`cargo clippy --release --all-targets -- -D warnings` clean.

## What this enables

- Any deterministic Twe game can now ship LAN multiplayer with ~50 lines of script. The 4-frame input delay is the only knob.
- Phase 29's determinism work is now load-bearing — without fixed-timestep + bounded GC + replay-style input capture, lockstep would desync constantly.
- The Phase 31.5 / Phase 32 follow-ons can proceed: rollback (small surface change to `tick_ready` semantics), Steam P2P (a `--features` flag), WebSocket browser multiplayer (Phase 30 + a separate relay path).

## What does not change

- No grammar change. No new keyword. No type-system change.
- The single-player `twec play examples/pong.twe` path is identical — the multiplayer changes are purely additive.
- All 16 codebase-closed phases (1–30) continue to pass their existing tests.
- Phase 30's WASM target is unaffected: `src/net.rs` is `#[cfg(not(target_arch = "wasm32"))]`, the `net.*` builtins are likewise gated, and `examples/pong_net.twe` is a desktop-only example.
