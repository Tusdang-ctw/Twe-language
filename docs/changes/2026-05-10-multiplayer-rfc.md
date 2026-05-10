# Multiplayer RFC — netcode model for Phase 31

**Status:** accepted 2026-05-10. Gate for Phase 31 session 2 onward.

## Question

Phase 31 ("Multiplayer Foundation", `docs/05-roadmap.md`) opens three viable
netcode shapes:

1. **Lockstep** — peers exchange inputs every tick, every peer runs the
   identical deterministic simulation, no game state crosses the wire.
2. **Rollback** — lockstep with prediction + correction, peers run ahead
   on predicted input and re-simulate when remote input arrives late.
3. **Authoritative client/server** — one peer owns the world, others send
   inputs and receive snapshots; can include client-side prediction +
   server reconciliation.

Principle 2 ("one obvious way per concept") forbids shipping all three
behind a uniform `net.*` API — the failure mode of "the language picks
your netcode for you" is shapeless. We pick one. This RFC picks
**lockstep over UDP** for v1.0, with explicit doors left for the others
as a follow-on phase.

## Decision

Phase 31 ships **lockstep over UDP** as the only multiplayer model.

- Peers exchange one input frame per simulation tick over UDP.
- All peers run the identical deterministic simulation.
- No world state crosses the wire — only inputs, plus a periodic state
  hash for desync detection.
- Configurable input delay (default 4 frames) absorbs jitter.
- Direct-IP and Steam P2P only. No matchmaking, no central server, no
  lobby.
- Maximum peers: 4 (the practical lockstep ceiling — every peer waits
  on the slowest, so latency compounds with peer count).

## Why lockstep, not rollback or authoritative

**Rollback** is a strict superset of lockstep: same wire format, same
determinism contract, plus a re-simulation loop. We can ship lockstep
now and add rollback as a Phase 31.5 follow-on without breaking the
`net.*` API. Lockstep alone is sufficient for cooperative games (the
v1.0 use case — `examples/survive_beta`-style co-op) and for the
canonical demo (`examples/pong_net.twe`). Rollback's value is in
adversarial low-latency genres (fighting, FPS); those aren't on the
v1.0 critical path.

**Authoritative C-S** is a different shape entirely — the wire format
is "inputs up, snapshots down", not "inputs both ways". It also
requires a server runtime, headless rendering, and a deployment story.
That's a multi-phase undertaking (server SDK, deploy tooling, hosting
docs) and would be at minimum equal in scope to all of Phases 27–32
combined. Out of scope for v1.0.

**Lockstep wins on three counts:**

1. **Smallest surface.** Two builtins gate the whole feature
   (`net.host`, `net.connect`); the lockstep runner is one
   thread-local state machine. Total Rust LOC budget: ~600 lines.
2. **Leverages Phase 29 determinism.** The fixed-timestep loop, input
   replay log, and tick-accurate audio scheduling shipped in Phase 29
   are the *exact* primitives lockstep needs. Phase 29's `replay::tick`
   is structurally identical to a lockstep input-exchange tick — the
   replay record format is essentially a single-peer lockstep log.
3. **Fits Principle 2.** One netcode shape, one author-facing API.

## Author-facing API (final)

```twe
# Host: open a lockstep session on UDP port 7777, expect 1 remote peer
let session = net.host(7777, 2)   # arg 2 = total_peers including self

# Client: connect to host's address
let session = net.connect("192.168.1.42:7777")

# Per-frame, in any state's on update():
net.send_input()                  # snapshots local input, sends to peers
let ready = net.tick_ready()      # true once all peers' inputs arrived

if ready:
    # The lockstep runner has already overwritten the input ambients
    # (key, mouse, mouse_held) with the synchronized frame. The script
    # runs as if it were single-player, except every peer runs in
    # lockstep on the identical simulated frame.

# Optional: read the per-peer hash for desync logging
let hash = net.state_hash()

# Always close cleanly on quit
net.close()
```

The script never touches sockets, packets, or peer addresses past
`connect()`. Lockstep timing is invisible — the play loop blocks
internally on `tick_ready()` until all peer inputs arrive (or the
input-delay budget elapses, in which case the missing peers are dead).

## Wire format (v1)

UDP packets are length-prefixed, peer-tagged, tick-tagged frames.
Total per-packet overhead: 16 bytes header + per-tick payload.

```text
[u8  magic       = 0x54]   ('T')
[u8  magic       = 0x57]   ('W')
[u8  version     = 1]
[u8  msg_type]             0=hello, 1=input, 2=hash, 3=bye
[u32 session_id]           random per host(); rejects packets from stale runs
[u8  peer_id]              0..=3
[u8  reserved    = 0]
[u32 tick]                 little-endian, simulation tick this packet covers
[..  payload depending on msg_type]
```

- **hello** (4 bytes): expected_peer_count.
- **input** (variable): one frame from `replay.rs`'s `Frame` struct,
  serialized with the same line-format as the replay log (just without
  the trailing newline). Held keys, pressed keys, mouse x/y, mouse
  buttons.
- **hash** (8 bytes): u64 state hash, used for desync detection. Sent
  every 60 ticks (1Hz at 60Hz).
- **bye** (0 bytes): graceful disconnect.

UDP ordering is not assumed. Each peer keeps a per-peer ring buffer of
the last `INPUT_DELAY + 8` ticks of their own outgoing inputs and
retransmits any tick that the host hasn't yet acked (the host acks
implicitly by advancing the simulation past that tick). This is the
standard "redundant input" lockstep technique — packet loss costs
bandwidth but not a desync.

## Determinism contract

The lockstep runner *requires* every peer's simulation to be
bit-exact. Phase 29 closed determinism on:
- fixed-timestep (`time.physics_dt`)
- input ambients (the `Frame` struct used by lockstep is the same as
  replay's)
- audio scheduling (`sound.schedule`, `sound.now`)
- bytecode VM dispatch (immediate-int + float fast paths)

Open determinism risks **caught by `net.state_hash()`**:
- HashMap iteration order on object fields. The hash function MUST
  sort field names. (Done — see `state_hash` impl.)
- Float NaN payload bits. The hash MUST treat all NaNs as equal.
- Allocator-influenced ordering (e.g. handle IDs from
  `physics.body`). The hash function MUST hash handle *positions*
  not handle *ids*.

When the per-peer hash diverges, every peer prints a desync warning
and keeps running (the game is now technically broken, but force-quit
is worse than soft-divergence for debug). A future "halt-on-desync"
toggle is a Principle 3 candidate but not in v1.0.

## What this RFC does *not* settle

- **Rollback follow-on.** When (not if) we add rollback as Phase 31.5,
  the `net.*` API stays — only `net.tick_ready()` semantics change
  (returns true immediately under prediction). New builtin
  `net.set_rollback_mode(true)` toggles it.
- **WebSocket transport.** The roadmap session 2 listed "UDP +
  WebSocket". Lockstep over WebSocket is straightforward (TCP is
  *more* reliable than UDP, just higher-latency); a `net.host_ws()` /
  `net.connect_ws()` pair lands in a follow-on session when WASM
  multiplayer is needed.
- **Steam P2P.** The Steam SDK already ships behind a feature flag
  (Phase 15). Steam's P2P API matches lockstep's transport contract —
  unreliable + reliable channels, peer-tagged. A `--features
  steam-net` follow-on routes packets through SteamNetworkingSockets
  instead of raw UDP.
- **Authoritative server.** Out of scope for v1.0 and probably for
  v2.x as well.
- **NAT traversal.** Direct IP only. STUN / TURN / hole-punching is a
  follow-on after Steam P2P (which gets it for free).

## Exit criteria for Phase 31

Per `docs/05-roadmap.md` Phase 31:

- `examples/pong_net.twe` plays peer-to-peer over LAN with 4-frame
  input delay, deterministic across two machines.
- A replay file recorded on machine A reproduces bit-exact on
  machine B.
- The `net.*` API surface stays under 10 builtins.

## Implementation order (sessions 2–7)

| # | Deliverable | Output |
|---|-------------|--------|
| 1 | This RFC | merged 2026-05-10 |
| 2 | UDP transport (`src/net.rs`) | host + connect + send/recv loop, wire format |
| 3 | `net.host` / `net.connect` builtins | peer handles in stdlib, `net.send_input` / `net.tick_ready` / `net.close` |
| 4 | Lockstep runner | input-delay buffer, per-tick exchange, hash-check, desync log |
| 5 | Snapshot serialization (debug) | reuse Phase 13 verified-mode JSON for state-hash payload + binary tier |
| 6 | `examples/pong_net.twe` | LAN-playable demo |
| 7 | Closeout | `docs/changes/<date>-phase-31-closeout.md` |

This commit covers sessions 1–4. Sessions 5–7 ship in follow-on
commits.
