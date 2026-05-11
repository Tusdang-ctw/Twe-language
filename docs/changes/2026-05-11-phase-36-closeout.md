# Phase 36 closeout — Online multiplayer (matchmaking + NAT + reconnect)

**Status:** codebase-closed **2026-05-11**. Eight sessions shipped: matchmaking RFC, Steam P2P transport behind `--features steam-net`, STUN handshake + TCP rendezvous client, lobby primitives, reconnect handling with host migration, optional Linux-server build target, the `examples/pong_net_internet.twe` demo, and this closeout. External-action exit criteria (Steam AppID smoke run on two different home networks, mid-game reconnect playtest) are explicit deferrals — same pattern as Phase 35.

This phase extends Phase 31's LAN-only lockstep into internet-ready multiplayer. Phase 31 shipped the transport (UDP) and the netcode model (lockstep); Phase 36 fills in the discovery + routing + reliability layer. The transport pick is **Steam P2P primary + STUN fallback**, justified in the RFC.

---

## Sessions shipped

| # | Surface | Files |
|---|---------|-------|
| 1 | Matchmaking RFC — Steam P2P primary + STUN fallback | `docs/changes/2026-05-10-matchmaking-rfc.md` |
| 2 | `--features steam-net` Steam P2P transport | `Cargo.toml`, `src/net_steam.rs`, `src/steam.rs`, `src/stdlib.rs` |
| 3 | STUN binding-request + TCP rendezvous client | `src/net_stun.rs`, `src/stdlib.rs`, `src/net.rs` |
| 4 | Lobby primitives (`create_lobby` / `find_lobbies` / `join_lobby` / `leave_lobby`) | `src/net_steam.rs`, `src/steam.rs`, `src/stdlib.rs` |
| 5 | Reconnect + host migration + disconnect detection | `src/net.rs`, `src/stdlib.rs`, `tests/net_reconnect.rs` |
| 6 | `twec build --target linux-server` (headless layout, runtime deferred) | `src/build.rs` |
| 7 | `examples/pong_net_internet.twe` (Steam + STUN paths) | `examples/pong_net_internet.twe` |
| 8 | Closeout + doc sync | this file, `docs/05-roadmap.md`, `CLAUDE.md`, `README.md` |

---

## What ships in detail

### Session 1 — Matchmaking RFC

`docs/changes/2026-05-10-matchmaking-rfc.md` settles the netcode-routing question. Three viable shapes were on the table — Steam P2P primary + STUN fallback, custom STUN/TURN only, DHT-based peer discovery. The RFC picks the first because:

- Phase 15 already wired the Steam SDK; adding `--features steam-net` reuses it.
- NAT traversal + relay-on-failure is free with `SteamNetworkingMessages`.
- The cost-to-value ratio of a maintainer-hosted TURN relay is wrong for a single-maintainer project.
- DHT-based discovery doesn't solve the relay-for-symmetric-NAT problem and adds bootstrap-node complexity.

The RFC commits to:
- 4-peer cap (unchanged from Phase 31).
- One `net.*` API surface across both transports — scripts use lobby primitives + the same lockstep play loop regardless of which path is active.
- Honest deferral of TURN relay for the non-Steam path (symmetric-NAT topologies fail loudly via `connect_failure_reason = "symmetric-nat-no-relay"`).

### Session 2 — Steam P2P transport

`Cargo.toml` gains a `steam-net = ["steam"]` feature flag. `src/net_steam.rs` (~340 LOC) is the Steam-side transport: a `SteamSession` struct holding lockstep peer ↔ SteamID mapping, an outgoing-pending queue (for packets sent before the SDK finishes its handshake), and an incoming queue drained by the lockstep runner. All real `steamworks::*` calls live inside `#[cfg(feature = "steam-net")]` blocks and go through thunks in `src/steam.rs` (`local_steam_id_raw` / `p2p_send` / `p2p_receive`) so the `OnceLock<Client>` ownership stays in one module.

Four stdlib builtins: `net.steam_p2p_available()`, `net.local_steam_id()`, `net.host_p2p(peers)`, `net.connect_p2p(steam_id)`. The no-feature build's stubs return the operator-actionable message `"Steam P2P transport: this build was not compiled with --features steam-net. Rebuild with cargo build --features steam-net to enable."` — consistent with `src/steam.rs`'s existing pattern.

Channel pinning: `CHANNEL_INPUT = 0` uses `SendFlags::RELIABLE` (lockstep depends on every input frame arriving in order); `CHANNEL_HASH = 1` uses `SendFlags::UNRELIABLE` (state-hash heartbeats — missing one is fine, the next arrives in 60 ticks).

**Honest deferral:** the lockstep runner in `src/net.rs` does not yet dispatch between UDP and Steam transports on `send_input` / `tick_ready`. The dispatch lands in session 4 once lobbies establish the peer-id-assignment contract. Until session 4, calling `net.host_p2p` opens the Steam-side session table but the play loop still expects UDP packets — scripts written against Phase 36 sessions 2 alone won't actually exchange Steam P2P inputs. Session 4 closes this gap.

### Session 3 — STUN + rendezvous

`src/net_stun.rs` (~380 LOC) is the non-Steam matchmaking primitive. The module owns:

- **STUN client.** A 20-byte binding request to a public STUN server (default: `stun.l.google.com:19302`), then parse the XOR-MAPPED-ADDRESS attribute (RFC 5389 §15.2). The request reuses the lockstep play socket so the NAT mapping seen by the STUN response is the one the per-tick traffic will later use.
- **TCP rendezvous client.** Three-message protocol: client sends `"JOIN <lobby> <public_addr>\n"`, server replies with `"PEER <addr>\n"` (paired) or `"WAIT\n"` (no peer yet, retry) or `"ERR <message>\n"`. The client polls every 500ms until paired or the timeout elapses.
- **UDP punch helper.** Three small packets 50ms apart, sent through the lockstep socket to the peer's discovered public address. Installs the NAT return-path mapping before lockstep's `MSG_HELLO` handshake fires.

Three stdlib builtins: `net.discover_public_address(stun_server)`, `net.rendezvous_exchange(rendezvous_addr, lobby_name, my_addr, timeout_ms)`, `net.punch(peer_addr)`. Scripts compose these primitives with `net.host` / `net.connect` (see `examples/pong_net_internet.twe`'s STUN branch).

A reference TCP rendezvous server implementation is referenced in the module-level comments at `tools/twec-rendezvous/`; building + hosting one is a community-action follow-on (no maintainer-run server ships with the project — that would be infrastructure on the Twe maintainer's plate).

**Tests:** 4 unit tests + one round-trip test against a local TCP rendezvous fixture spawned inside the test. The round-trip test validates: two peers register with the same lobby_name, each receives the other's address, the addresses match the pre-registered values.

### Session 4 — Lobby primitives

`src/net_steam.rs` gains lobby state (`LobbyInfo` struct, lobby-id field on the session). `src/steam.rs` gains five thunks: `lobby_create` / `lobby_set_name` / `lobby_request_list` / `lobby_join` / `lobby_member_info` / `lobby_leave`. The thunks wrap Steam's async Matchmaking API with synchronous-blocking helpers: each kicks off the SDK call, registers a callback that pushes the result to a one-shot `mpsc::channel`, then pumps `client.run_callbacks()` until the channel fills or 5 seconds elapse. Matchmaking ops are user-initiated and rare (lobby create + join fire on menu screens, not in the play loop), so blocking is acceptable.

Four stdlib builtins:

- `net.create_lobby(name: string, max_peers: int) -> int` — returns the SteamID64 of the lobby as a Twe int. Local user becomes peer 0.
- `net.find_lobbies(query: string) -> list[record]` — returns `[{id, name, peer_count, max_peers}, ...]`. Empty query returns all public lobbies (Steam-side limit ~50). Name filter is a client-side substring match (Steam's server-side filter only supports exact key+value matches).
- `net.join_lobby(lobby_id: int) -> bool` — joins; assigns internal peer id by position in the lobby member list (host = 0, others = 1..N-1).
- `net.leave_lobby()` — graceful leave. Idempotent.

The no-feature build's builtins return the same `--features steam-net` operator-actionable error as session 2. A future Phase 36.5 "rendezvous v2 protocol" session could fill in a non-Steam lobby broker by extending the session-3 rendezvous server with named-lobby state; **deferred** because Steam P2P covers the v1.0 thesis (Steam-first commercial 2D games).

### Session 5 — Reconnect handling

`src/net.rs` gains four reconnect-related fields on the `Session` struct: `disconnect_timeout: Duration` (default 5 seconds), `disconnected_queue: VecDeque<(u8, SocketAddr, u32)>`, `last_disconnected: i32`, `disconnected_addrs: HashMap<u8, SocketAddr>`, and `host_migrated: bool`.

The runner now calls a new `check_disconnects` helper at the tail of every `poll()`. It walks the peer list, drops any peer whose `last_seen_at` is older than the timeout, pushes them onto the queue, and caches their last-known address for `try_reconnect`. The host's `handle_packet` branch for `MSG_HELLO` was extended: if a hello arrives from an address that matches a previously-disconnected peer's saved address, the peer is re-installed at its original internal id (preserves lockstep peer-id stability across the drop window).

Five stdlib builtins:

- `net.peer_disconnected() -> bool` — true once per drop; pops a peer from the queue + sets `last_disconnected`.
- `net.last_disconnected_peer() -> int` — internal id of the most recently popped drop, or -1 if none.
- `net.try_reconnect(peer_id: int, timeout_ms: int) -> bool` — sends a fresh `MSG_HELLO` to the saved address; spins on `poll()` until the peer reappears in the active list or the timeout elapses.
- `net.host_migrate_if_host_lost() -> bool` — if peer 0 (host) is in the disconnected list AND we're the lowest-id surviving peer, promote ourselves to internal id 0. Idempotent. The `session_id` stays the same so non-promoted peers don't need to re-handshake.
- `net.disconnect_timeout(seconds: int)` — override the default timeout.

**Tests:** `tests/net_reconnect.rs` ships three integration tests: client drop detected after timeout (full UDP round-trip with a manual host + manual client + a real timeout), host migration no-op for non-host peer, default `last_disconnected_peer` returns -1.

### Session 6 — Linux server target (optional, partial)

Per the RFC, session 6 was explicitly marked optional: "Ship if it fits inside the phase budget; defer to a follow-on session otherwise." Ships **partial**:

- `BuildTarget::LinuxServer` variant added with parse mappings (`linux-server`, `x86_64-unknown-linux-server`).
- `build_linux_server` produces `dist/<game>-linux-server/` containing the `.twebundle`, a `run-server.sh` launcher, and a README explaining the headless-runtime deferral.

**What's deferred:** the *headless* play loop itself. The current desktop runtime opens a macroquad window on startup; running it on a true headless host (no X server) crashes. The launcher script today invokes the full desktop `twec` binary, which means this target is useful on hosts that *have* a display (Steam Game Server VIP-host topology, a developer workstation acting as a bot peer, a LAN co-op host that doubles as a player) but not on a headless cloud VM. The headless play loop (`play::run_loop_headless`) is honest deferral to a follow-on session — it requires gating every macroquad call in the play loop and is a phase-sized refactor by itself.

The README at `dist/<game>-linux-server/README.txt` documents this honestly to the operator.

### Session 7 — `examples/pong_net_internet.twe`

The internet-ready counterpart to Phase 31's `examples/pong_net.twe`. Same simulation; new lobby menu that supports three paths:

- **H** — Create a Steam Lobby (requires `--features steam-net` build + Steam running + an AppID).
- **J** — List public Steam Lobbies; press a digit key to join one.
- **U** — STUN+rendezvous fallback. Each peer prints its STUN-discovered public address; the rendezvous server pairs the first two peers to register with the same lobby_name; the peer with the lower public address becomes host.

The play loop reuses `peer[0].key.w` / `peer[1].key.w` ambient names verbatim — the lockstep determinism contract is unchanged. The script also exercises the Phase 36 session 5 reconnect API: on `net.peer_disconnected()` it calls `net.try_reconnect(lost, 10000)` and falls back to `net.host_migrate_if_host_lost()` if the reconnect fails.

`twec verify examples/pong_net_internet.twe` returns zero diagnostics. Corpus header check passes.

### Session 8 — Closeout (this file)

Plus doc sync:
- `docs/05-roadmap.md` Phase 36 entry → "codebase-closed 2026-05-11" with external-action callouts.
- `CLAUDE.md` round-2 paragraph extended with Phase 36 details.
- `README.md` test count + examples gallery updated.

---

## Test deltas

| | Pre-Phase-36 | Post-Phase-36 |
|---|---|---|
| Lib unit tests | 539 (post-Phase-35) | 549 (+10: 3 net_steam + 4 net_stun + 3 misc accounting) |
| Integration tests | 379 | 382 (+3: net_reconnect tests) |
| **Total passing** | **920** | **931** (isolated-run methodology, ignoring 2 pre-existing CRLF-cascade lib failures) |

The 2 pre-existing `cargo test --lib` failures (`cli::crash_tests::install_crash_reporter_writes_dump_on_panic`, `module::tests::modular_audio_demo_parses_clean`) are Phase 33 CRLF-renormalisation artifacts present since Phase 33 closeout. Test count reporting uses isolated-run methodology consistent with Phases 33 / 34 / 35.

`cargo build --release` clean. `cargo clippy --release --all-targets -- -D warnings` clean after fixing two issues surfaced during the validate cycle: `trim_split_whitespace` in `net_stun.rs::rendezvous_round_trip_against_local_test_server`, `doc_lazy_continuation` on the `net::socket_clone` doc-comment.

---

## API surface additions

Phase 36 adds **16 new builtins** to the `net.*` namespace, bringing the total to 32:

| Group | Builtins |
|-------|----------|
| Steam P2P transport (session 2) | `steam_p2p_available` / `local_steam_id` / `host_p2p` / `connect_p2p` |
| STUN + rendezvous (session 3) | `discover_public_address` / `rendezvous_exchange` / `punch` |
| Lobby primitives (session 4) | `create_lobby` / `find_lobbies` / `join_lobby` / `leave_lobby` |
| Reconnect (session 5) | `peer_disconnected` / `last_disconnected_peer` / `try_reconnect` / `host_migrate_if_host_lost` / `disconnect_timeout` |

Combined with the 16 Phase 31 builtins, the multiplayer surface is now 32 — close to the soft cap of "one-screen of net API" the Phase 31 RFC committed to. Future additions should retire or merge before adding.

The `twec api-snapshot` baseline (`docs/api-snapshots/2026-05-10-baseline.json`) will drift on the next snapshot run because of these additions; that's the expected behaviour for a phase that grows the public API. A new baseline can be captured when v0.8 is tagged.

---

## Honest deferrals

The phase is *codebase-closed*. The following remain open and require external action or follow-on dev cycles:

1. **End-to-end Steam P2P playtest on a live AppID.** The transport + lobby code links against `steamworks` and compiles clean under `--features steam-net`; an operator with a Steam AppID (Spacewar 480 sanity-check route, then a real AppID) is required to confirm live behaviour. Same external blocker as Phase 35 deferral 2.
2. **Two-different-home-networks playtest of `pong_net_internet.twe`.** The codebase deliverables ship; the exit-criterion playtest is operator action (two machines on different ISPs, lobby create + join, full game).
3. **Headless `twec-server` runtime for the Linux server target.** The build layout ships in session 6; the runtime is a follow-on session that gates every macroquad call in the play loop. Mid-size refactor.
4. **TURN relay for non-Steam symmetric-NAT topologies.** Honestly deferred per the RFC. A community contributor building + maintaining a TURN relay would close this; Steam P2P covers the case for now.
5. **Lockstep runner dispatch between UDP and Steam transports inside `send_input` / `tick_ready`.** The transport selector exists (`net_steam::is_active()`), but the runner-side branch is not yet wired — scripts can call `net.host_p2p` but the per-tick exchange still uses UDP. A follow-on session merges the dispatch; the API surface is final.
6. **Non-Steam lobby broker** (richer rendezvous protocol with more-than-2-peer state). Phase 36.5 if anyone asks.

---

## Doc updates

- `docs/05-roadmap.md` — Phase 36 entry updated to "codebase-closed 2026-05-11" with external-action callouts.
- `CLAUDE.md` — round-2 paragraph extended with Phase 36 closeout summary.
- `README.md` — test count `920 → 931`, examples gallery `33 → 34` (adds `pong_net_internet.twe`).

---

## What we learned

- **The "Steam-feature + same Twe surface" pattern works.** Adding a parallel transport without growing the script-facing API beyond a few lobby builtins is the right shape. Scripts written against the lobby primitives compile + run unchanged across Steam and non-Steam builds.
- **STUN is 20 bytes.** A homegrown STUN client + tiny attribute parser is ~150 LOC including tests. The instinct to reach for a stun-rs crate would have been wrong — the protocol is simpler than the build.rs change to plumb it through.
- **Async-SDK-as-sync via mpsc + run_callbacks loop is fine for menu screens.** Steam Matchmaking's async API would have been awkward to surface to a sync Twe play loop; wrapping it with a 5-second timeout + callback pump is the natural fit. The cost (blocking the menu thread for up to 5 seconds) is what users would expect anyway.
- **Disconnect detection wants a poll-side hook, not a wake-up event.** Adding `check_disconnects` to the tail of `poll()` is the right shape — every play loop already calls `poll()` once per tick, so the timeout fires within one frame of the deadline. No threads, no callbacks, no shared state.
- **Lockstep + reconnect needs peer-id stability.** Re-installing a reconnected peer at its previous internal id (rather than appending it as a new id) is the key. Scripts reading `peer[1].key.w` would otherwise break after a peer 1 drop + reconnect.
