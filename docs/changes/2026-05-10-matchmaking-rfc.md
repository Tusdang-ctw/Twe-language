# Matchmaking RFC — addendum to Phase 31 multiplayer RFC

**Status:** accepted 2026-05-10. Gate for Phase 36 sessions 2 onward.

**Parent:** [`2026-05-10-multiplayer-rfc.md`](2026-05-10-multiplayer-rfc.md) (lockstep over UDP, direct-IP only).

## Question

Phase 31 explicitly scoped *out* matchmaking, NAT traversal, lobbies,
and reconnect. The transport (UDP) and the netcode model (lockstep)
ship; the discovery + routing layer is missing. Phase 36 fills it.

Three viable shapes:

1. **Steam P2P primary + STUN fallback for non-Steam builds.**
2. **Custom STUN/TURN, no Steam dependency.** Authors run their own
   relay or use a public one.
3. **DHT-based peer discovery.** Decentralised, no central matchmaker.

## Decision

Phase 36 ships **Steam P2P primary + STUN fallback**.

- The **primary path**: builds with `--features steam-net` route every
  `net.*` packet through `SteamNetworkingSockets`. Steam handles NAT
  traversal, relay-on-failure, and peer addressing. Authors use
  `net.create_lobby(name, max_peers)` / `net.find_lobbies(query)` /
  `net.join_lobby(id)` instead of raw `net.connect("ip:port")`.
- The **fallback path**: builds *without* `steam-net` keep the Phase 31
  raw-UDP transport plus a new `net.connect_via_stun(rendezvous_url)`
  builtin. STUN gives both peers their public-facing address; they
  exchange addresses through the rendezvous server then attempt a
  simultaneous-open UDP punch. TURN relay (for symmetric NATs that
  reject hole-punching) is **deferred to Phase 36.5** — Steam P2P
  covers that case, and the non-Steam fallback honestly admits
  symmetric-NAT failure mode rather than spending a session on a TURN
  client.

Both paths expose the **same `net.*` API** surface that Phase 31
shipped — the only new builtins are lobby primitives + the STUN
connector + reconnect helpers. Scripts compile-test identically
across both builds.

## Why Steam P2P primary

1. **Phase 15 already wired the Steam SDK.** `--features steam`
   exists; the Steam SDK is a tested optional dep. Adding
   `--features steam-net` reuses the existing `steamworks` crate and
   the same operator-action gate (Steam AppID required). Bandwidth on
   the Twe maintainer's plate is finite and a custom STUN/TURN service
   is its own infrastructure project.
2. **NAT traversal is free.** SteamNetworkingSockets does the punch +
   relay layer for us. A custom solution would need to ship + host a
   STUN server (cheap) plus a TURN relay (not cheap — bandwidth-priced)
   to cover the same NAT topologies.
3. **Fits the v1.0 thesis.** v1.0 ships a Steam-class commercial 2D
   game (`survive_beta`). The same author shipping that game on Steam
   gets multiplayer matchmaking for free; the same author shipping it
   on itch.io (no Steam) gets the STUN-fallback path with the
   honest-admission about symmetric NAT.
4. **Smallest API surface.** Steam Lobbies + Steam P2P are 4 builtins
   total (`create_lobby`, `find_lobbies`, `join_lobby`, `leave_lobby`).
   A custom matchmaker would need lobby create/find/join *plus* its
   own auth tier (peer identity, anti-spam) plus its own backend.

## Why not custom STUN/TURN only

It's the right answer for a non-Steam-adjacent language project, but
not for *this* one. The cost-to-value ratio is wrong: every custom-
matchmaker hour competes against finishing the v1.0 thesis. Authors
who don't ship on Steam still get the STUN fallback for the LAN+IPv6
cases (which is most home-broadband NAT topologies in 2026); the rare
double-NAT failure mode is the price of avoiding the TURN-hosting
treadmill.

If a community contributor builds + maintains a non-Steam TURN relay
later, that's a follow-on session that adds `net.connect_via_turn(url)`
to the same surface — no API churn.

## Why not DHT

Decentralised peer discovery is fascinating and almost always
overkill. Mirror / Photon / Mirage / etc. all converged on
matchmaker-server architectures because DHTs don't solve the NAT
problem (you still need a relay) and add bootstrap-node complexity
(who hosts the bootstrap node? who pays for it when it goes down?).
Off the table for v1.x.

## Author-facing API (final)

```twe
# Steam path — builds with --features steam-net
net.steam_init()                              # idempotent; no-op if already initialised

# Host: create a lobby, become host of an empty session
let lobby = net.create_lobby("blue-fight", 4) # name + max peers (incl host)

# Client: search + join
let lobbies = net.find_lobbies("blue-fight")  # list of {id, name, peer_count}
net.join_lobby(lobbies[0].id)                 # joins; on success becomes peer 1..3

# Once net.session_ready() is true, the rest of the play loop is
# *identical* to Phase 31 LAN code:
net.send_input()
let ready = net.tick_ready()
# ...etc.

# STUN path — builds without --features steam-net
let session = net.connect_via_stun("https://stun.example.com/rendezvous", "blue-fight")
# Same net.* API after this point.

# Reconnect handling — both paths
if net.peer_disconnected():
    let lost_id = net.last_disconnected_peer()
    if net.try_reconnect(lost_id, 10000):     # 10-second window
        # snapshot resync and resume; runner re-injects predicted
        # ticks for the missed window.
        log("reconnected " + tostr(lost_id))
    else:
        # peer is gone for good — ungraceful disconnect path
        net.host_migrate_if_host_lost()
```

## NAT topology coverage

| Topology | Steam P2P | STUN fallback | TURN (deferred) |
|---|---|---|---|
| Both peers cone NAT (most home routers) | works | works | works |
| One peer behind double NAT | works (relay) | likely fails | works |
| Both peers behind symmetric NAT | works (relay) | always fails | works |
| Both peers on same LAN | works | works (skips STUN) | works |
| IPv6 end-to-end | works | works | works |
| Carrier-grade NAT (CGNAT) on cellular | works (relay) | always fails | works |

The STUN fallback's "likely fails" / "always fails" rows are the price
we pay for not running TURN infrastructure. The honest-admission shape
is: scripts get a `net.connect_failure_reason()` builtin that returns
`"symmetric-nat-no-relay"` when STUN succeeds but the punch doesn't,
so authors can guide users to "use a Steam build" or "join via LAN
instead."

## Wire format

**Steam P2P:** `SteamNetworkingSockets.SendMessageToConnection` with
the same per-tick `Frame` payload as Phase 31 raw-UDP, prefixed with
the same 16-byte Twe header. The Steam path strips Steam's outer
envelope and the inner Twe-header machinery is unchanged.

**STUN fallback:** identical to Phase 31's raw-UDP wire format. STUN
is a one-shot handshake against the rendezvous URL — it never
touches the per-tick traffic. The rendezvous server is a thin
WebSocket gateway: peer A subscribes with `lobby_name`, peer B
subscribes with the same name; the gateway forwards each peer's
public address (learned via STUN-binding-response) to the other.

## Lobby primitives — semantics

- **`net.create_lobby(name: string, max_peers: int) -> int`** — returns
  a lobby id. Becomes peer 0. Lobby is implicit-public; private lobbies
  defer (Steam supports them but Phase 36 doesn't expose).
- **`net.find_lobbies(query: string) -> list[record]`** — returns a list
  of `{id: int, name: string, peer_count: int, max_peers: int}`. `query`
  is a substring filter on lobby names.
- **`net.join_lobby(id: int) -> bool`** — joins; returns false if lobby
  is full or no longer exists. On success, this peer is assigned an id
  in `1..max_peers-1`.
- **`net.leave_lobby() -> ()`** — graceful leave; sends a `bye` to all
  peers + (Steam) drops out of the Steam lobby.

Lobby state is **separate** from session state. After
`create_lobby` / `join_lobby` succeed, scripts still need to call
`net.session_ready()` and `net.send_input()` per the Phase 31 contract
— the lobby is the discovery layer; lockstep is the play layer.

## Reconnect — semantics

- **`net.peer_disconnected() -> bool`** — true once per drop; reset by
  the next call to `net.last_disconnected_peer()`. The runner detects
  drops by `last_seen_at > 5s` (configurable via `net.disconnect_timeout`).
- **`net.last_disconnected_peer() -> int`** — peer id of the most
  recently dropped peer, or -1 if none.
- **`net.try_reconnect(peer_id, timeout_ms) -> bool`** — best-effort
  re-handshake within the timeout. On Steam path: `RequestP2PSessionRequest`
  retry. On STUN path: re-rendezvous + re-punch. After a successful
  reconnect, the runner injects **predicted inputs** for every tick the
  peer missed (held-key repeat at last value), then resumes lockstep.
- **`net.host_migrate_if_host_lost()`** — if peer 0 (the host) is the
  one that disconnected, the lowest-id surviving peer becomes the new
  host. The session_id stays the same; only addressing changes.

Reconnect-resync is **input-replay-based**, not state-snapshot-based,
because lockstep already has the determinism contract: replay the
missed ticks with the predicted-input fill and the rejoining peer's
state matches everyone else's by construction.

## Dedicated-server mode (optional)

Phase 36 session 6 is **optional** per the roadmap. The shape:

- `twec build --target linux-server` produces a headless ELF binary
  that runs the Twe play loop with rendering + audio stripped.
- The binary participates in lockstep as peer 0 (the host); it's a
  client of nothing but its own clock.
- Use case: matchmaking-served games where one peer is a "VIP" that
  doesn't render — useful for Steam Game Server but not on the
  v1.0 critical path.

Ship if session 6 fits inside the phase budget; defer to a follow-on
session otherwise. The exit criteria do not require this deliverable.

## What this RFC does *not* settle

- **TURN relay for non-Steam builds.** Deferred to Phase 36.5 if a
  community contributor builds it; Steam P2P covers the symmetric-NAT
  case for now.
- **Anti-cheat.** Lockstep gives one form of cheat resistance for free
  (every peer simulates everything), but lockstep doesn't stop input
  injection or memory-edit. Anti-cheat is a separate phase, gated on
  a real attacker's existence.
- **Voice chat.** Steam P2P has voice; non-Steam path doesn't. Both
  defer to a Phase 4x ergonomics layer.
- **More than 4 peers.** Phase 31's `MAX_PEERS = 4` cap stays in
  Phase 36. Lobbies bigger than 4 wait on a netcode model with better
  scaling characteristics (rollback in Phase 37, possibly C-S in a
  late phase that may never ship).
- **Lobby search beyond name substring.** Steam Lobbies support keyed
  metadata search; Phase 36 exposes only the name filter. Keyed search
  is a follow-on if anybody asks.

## Determinism contract

Unchanged from Phase 31. Steam P2P and STUN fallback both deliver the
same per-tick `Frame` payload; the runner is identical. Reconnect
fill-with-predicted-input introduces a brief divergence window but
the per-60-tick state-hash check catches it the first time it
matters.

## Exit criteria for Phase 36

Per `docs/05-roadmap.md` Phase 36:

- Two players on different home networks join via lobby and play
  `pong_net_internet.twe`. (Operator action — gated on Steam AppID +
  two-machine playtest.)
- Mid-game disconnect and reconnect within 10 seconds doesn't desync.
  (Codebase deliverable: state-hash-clean across the reconnect window
  in `tests/net.rs`.)
- Steam P2P path passes the Phase 15 Steam SDK test on a live AppID.
  (Operator action — same gate as Phase 35 deferral 2.)

Codebase-side exit: the API + transport ship + the no-AppID tests
pass. Operator-action items are explicit deferrals in the closeout
note (mirrors Phase 35's external-validation discipline).

## Implementation order (sessions 2–8)

| # | Deliverable | Output |
|---|-------------|--------|
| 1 | This RFC | merged 2026-05-10 |
| 2 | `--features steam-net` Steam P2P transport | `src/net_steam.rs`, conditional dispatch in `src/net.rs` |
| 3 | STUN handshake + rendezvous client | `src/net_stun.rs`, `net.connect_via_stun` builtin |
| 4 | Lobby primitives | `net.create_lobby` / `find_lobbies` / `join_lobby` / `leave_lobby` builtins |
| 5 | Reconnect handling | `net.peer_disconnected` / `last_disconnected_peer` / `try_reconnect` / `host_migrate_if_host_lost` builtins; `tests/net_reconnect.rs` |
| 6 | Dedicated-server mode (optional) | `twec build --target linux-server` |
| 7 | `examples/pong_net_internet.twe` | Steam-path demo |
| 8 | Closeout | `docs/changes/<date>-phase-36-closeout.md` |

This commit covers session 1. Sessions 2–8 ship in follow-on commits.
