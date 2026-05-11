# Rollback RFC — second netcode mode alongside lockstep

**Status:** accepted 2026-05-11. Gate for Phase 37 sessions 2 onward.

**Parent:** [`2026-05-10-multiplayer-rfc.md`](2026-05-10-multiplayer-rfc.md) (Phase 31 lockstep over UDP) + [`2026-05-10-matchmaking-rfc.md`](2026-05-10-matchmaking-rfc.md) (Phase 36 Steam P2P + STUN).

## Question

Phase 31 explicitly named rollback as a "Phase 31.5 follow-on" but
deferred. Phase 37 reopens it. The hard question is **Principle 2**:

> One obvious way per concept.

Shipping a second netcode model alongside lockstep risks "the language
picks your netcode for you" — exactly the failure mode the principle
exists to prevent. The Phase 37 entry in `docs/05-roadmap.md` warns:
"If the RFC can't justify both shapes cleanly, this phase doesn't
ship."

This RFC is the justification. The TL;DR: **netcode is two concepts
along the genre axis, not one**. Lockstep and rollback are not two
competing implementations of the same player experience — they are
different player experiences serving different game shapes. Forcing
one onto the other produces an unshippable game in at least one
genre column.

## Decision

Phase 37 ships **rollback as a second netcode mode**, opt-in via a
single explicit switch:

```twe
net.set_mode("rollback")   # default at session-start: "lockstep"
```

The mode is **per-session**, not per-build. The Twe runtime ships
both runners; scripts pick one at lobby creation time based on the
genre they're building. Switching mid-session is rejected with a
Principle-3-style error.

Within each mode, the `net.*` API is **identical**:
`net.send_input(tick)`, `net.tick_ready(tick)`, `net.advance_tick(tick)`,
`peer[i].key.w`, the state-hash desync detector — all unchanged.
Rollback adds two surface elements:

1. An entity-level opt-in: `entity Fighter: rollback = true`. Only
   rollback-tagged entities are snapshotted + rewound; everything
   else is lockstep-deterministic (UI, music, particles, scenery).
2. A small `rollback.*` namespace for predicted-input policy +
   visual-smoothing knobs (`rollback.set_input_prediction(...)`,
   `rollback.set_smoothing(...)`, `rollback.max_rewind_frames(...)`).

That's it. Two builtins in a sub-namespace + one entity field. No
runner-level fork in the script-facing surface.

## Why rollback is genuinely necessary

The textbook case is fighting games: in Street Fighter, a 4-frame
input delay at 60Hz (the Phase 31 lockstep default) means **66 ms**
between pressing the punch button and the punch coming out on screen.
At competitive levels of play, fighting-game input chains are 2-3
frames wide. The 4-frame lockstep delay turns frame-perfect combos
into impossible combos. The genre is unplayable.

Rollback solves this: each peer runs the simulation **on its own
inputs immediately**, predicts the peers' inputs (last-input-repeat
or velocity-extrapolation), and re-simulates if the predicted inputs
turn out wrong when the real ones arrive. Sub-frame input feel
without sacrificing eventual state determinism across peers.

The same constraint applies in:

- **FPS / fast-action 3D.** The Phase 35 community-pipeline cohorts
  include LÖVE / Lua indies — many of whom ship fast-action games
  where 66 ms input delay would be a deal-breaker.
- **Rhythm games on multiplayer modes.** A 4-frame delay turns
  beatmap timing windows into impossible-to-hit ones.
- **Action platformers with multiplayer races.** Speedrun-style play
  needs sub-frame input feel.

Lockstep is fine — and *better* than rollback — in:

- **Cooperative survival** (the `survive_beta` line — Vampire
  Survivors-class). Input is held-key WASD; the 66 ms delay is
  imperceptible at the design's pacing.
- **Real-time strategy.** Click-to-move with multi-frame animations;
  no player notices input latency at this scale.
- **Turn-based or near-turn-based games.** Lockstep is essentially
  free.
- **Rhythm games where each peer plays their own track.** No
  cross-peer timing.

The two columns are real. Forcing rollback's complexity onto
lockstep-suitable games taxes them with snapshot/rewind machinery
for no win; forcing lockstep onto rollback-required games kills the
genre. The carve-out is justified.

## Principle 2 justification

Principle 2 says "one obvious way per concept." The argument for
rollback as a second mode rests on the claim that **netcode is a
genre-dependent concept**, not a single concept. Analogies inside
Twe:

- **2D vs 3D play loops.** `play.rs` and `play3d.rs` are different
  runners for different genres. Same `entity` / `state` / `visual`
  language; different play loop. Authors pick at scene-definition
  time. No one calls this a Principle 2 violation.
- **`save_to_path` vs `save.*` typed namespace.** Two ways to save
  data, one explicit (raw bytes), one structured (typed values).
  Phase 22 added the typed namespace as a *second obvious way*
  alongside the byte-level interface, with the rationale: "two
  concepts (raw persistence + structured persistence), not one."

The rollback / lockstep split lands in the same family. The mode
switch is **explicit** — `net.set_mode("rollback")` is the one
obvious way to opt into rollback, the absence of that call is the
one obvious way to stay on lockstep.

If a future game ships both modes in the same binary (offering
"competitive mode" + "casual co-op"), the script makes the choice
at lobby creation time. The script-side API to read what mode is
active is `net.mode()` returning `"lockstep"` or `"rollback"`.

## Genre-to-mode table

| Genre | Mode | Rationale |
|---|---|---|
| Cooperative survival (`survive_beta`-class) | lockstep | Held-key input, design tolerates 66ms delay |
| Vampire Survivors / Diablo PvE | lockstep | World state must be deterministic across peers |
| RTS / 4X | lockstep | Click-to-move at frame budget |
| Rhythm (single-player tracks) | lockstep | Per-peer timing windows |
| Pong | lockstep | The Phase 31 demo; 66ms is fine |
| Fighting (Street Fighter, Smash, Guilty Gear) | **rollback** | Frame-perfect input chains |
| FPS (Quake, CS, Valorant) | **rollback** | Aim feel + recoil reset windows |
| Action platformer (multiplayer races) | **rollback** | Speedrun input precision |
| Rhythm (head-to-head with shared timing) | **rollback** | Shared beatmap; per-peer delays would desync |

This table is documented in `docs/05-roadmap.md` Phase 37 entry +
copied into the rollback chapter of the tutorial when it lands.

## API surface

### Mode switch

```twe
net.set_mode("rollback")  # before net.host/connect/create_lobby
net.mode()                # returns "lockstep" | "rollback"
```

### Entity opt-in

```twe
entity Fighter:
    rollback = true   # snapshot/rewound by the rollback runner
    var x = 0.0
    var y = 0.0
    var health = 100
```

Non-marked entities are lockstep-deterministic — they don't get
snapshotted, they don't get rewound. UI, scenery, particles, music,
background dialogue stay out of the rollback path. **Default is
`rollback = false`** (every existing example continues to work
without modification).

### Rollback knobs

```twe
rollback.set_input_prediction("last-input-repeat")   # default
rollback.set_input_prediction("velocity-extrapolate")
rollback.set_smoothing(true)   # default; rendered position lerps
                               # across rewinds so the local player
                               # doesn't see snap-back
rollback.set_smoothing(false)
rollback.max_rewind_frames(8)  # default 8; tunable per game
rollback.stats()               # debug: returns
                               # {predicted, corrected, last_correction_frames}
```

### What stays the same

- `net.send_input(tick)` / `net.tick_ready(tick)` / `net.advance_tick(tick)`.
- `peer[0].key.w` / `peer[1].key.w` ambients.
- `net.state_hash()` / `net.send_state_hash()` for desync detection.
- Lobby primitives (`net.create_lobby` / `net.find_lobbies` / `net.join_lobby`).
- All of Phase 36 reconnect handling.

## Wire format

**Unchanged from Phase 31.** Rollback peers exchange the same
`MSG_INPUT` payload at the same per-tick cadence over the same UDP
transport (or Steam P2P, or STUN-fallback). The difference is
**runner-side**:

- Lockstep runner: blocks `tick_ready(tick)` until every peer's
  input for `tick` has arrived, then advances.
- Rollback runner: returns `tick_ready(tick) == true` **immediately**
  using predicted inputs for peers that haven't arrived yet, then
  rewinds + re-simulates when the real inputs catch up.

This means a rollback peer can connect to a lockstep peer in the
same session — they exchange the same packets. But the runners
must agree at session-start which mode is active, because the
*replay* semantics differ. Phase 37 enforces this with a 1-byte
mode field in `MSG_HELLO`; mode-mismatched peers fail handshake with
`net.connect_failure_reason() == "mode-mismatch"`.

## Determinism contract

Rollback **requires** every peer's simulation to be bit-exact, same
as lockstep. Phase 29 closed determinism on `time.physics_dt`, input
ambients, audio scheduling, and bytecode VM dispatch — those all
carry over. Rollback adds:

- **Snapshot serialisation must be deterministic.** Phase 36's
  `net.snapshot_json` canonical-JSON+FNV1a path is reused.
- **Predicted-input policy must be deterministic across peers.** Both
  peers run the same prediction rule on the same missing-input gap;
  no peer-local random fill. `last-input-repeat` is trivially
  deterministic; `velocity-extrapolate` uses the last-frame velocity
  field on each rollback entity (which is itself part of the
  snapshot).

A new field `last_correction_frames` is added to the desync log —
rollback's recovery from a wrong prediction can be observed by the
maintainer when debugging.

## Implementation order (sessions 2–8)

| # | Deliverable | Output |
|---|-------------|--------|
| 1 | This RFC | merged 2026-05-11 |
| 2 | State snapshotting | `src/rollback.rs`: ring buffer, `snapshot_tick` + `restore_tick`; deterministic serialisation of `rollback = true` entities |
| 3 | Rewind + replay engine | rewind to tick N, re-execute with corrected input, advance back to current tick |
| 4 | Predicted input | `last-input-repeat` (default) + `velocity-extrapolate` policies; `rollback.*` setter builtins |
| 5 | `entity Fighter: rollback = true` parser + AST + runtime marker; `entities.is_rollback(e)` query |
| 6 | Visual smoothing | render-side position lerp across rewinds; per-entity `_smoothed_x` / `_smoothed_y` cached fields |
| 7 | `examples/fighter_demo.twe` | 2-player rollback fighting-game proof |
| 8 | Closeout | `docs/changes/<date>-phase-37-closeout.md` |

## What this RFC does *not* settle

- **A third mode.** Authoritative client/server stays out of scope
  (the Phase 31 RFC's reasoning still holds — that's a multi-phase
  undertaking).
- **Lag compensation beyond rollback.** Server-side rewind, hit-box
  rollback for FPS — out of scope; the rollback runner only rewinds
  *predicted-input gaps*, not "what the shooter saw."
- **Rollback for entities with side effects.** Spawning a particle
  inside a rolled-back tick would replay the particle on every
  rewind. The closeout note documents this as a Principle 3 footgun
  to call out explicitly in the rollback chapter of the tutorial:
  scripts running side effects inside rolled-back entities should
  guard them with `if not rollback.is_replaying(): ...`.
- **Snapshot compression.** Snapshots are tagged JSON via Phase 36
  `net.snapshot_json`. A binary tier is a v1.x performance lever if
  the JSON snapshots get too big at scale.

## Exit criteria for Phase 37

Per `docs/05-roadmap.md` Phase 37:

- `examples/fighter_demo.twe` plays at 60fps with sub-2-frame input
  feel against an opponent on a 60ms-RTT connection. **(Codebase
  + benched run; operator-action for the 60ms-RTT real-network
  measurement.)**
- Lockstep-mode examples (`pong_net.twe`, `survive_beta` if it ever
  goes online) continue to work unchanged. **(Codebase deliverable.)**
- This RFC honestly justifies the dual-mode carve-out from
  Principle 2. **(This document.)**

Codebase-side exit: the runner + opt-in marker + fighter_demo ship,
lockstep regressions are zero, and the RFC's Principle 2
justification stands up to a Round 2 re-reading at v1.x.
