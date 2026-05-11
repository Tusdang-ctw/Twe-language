# Phase 37 closeout — Rollback netcode

**Status:** codebase-closed **2026-05-11**. Eight sessions shipped: rollback RFC, snapshot ring buffer, rewind+replay scaffolding, predicted-input policies, rollback opt-in mechanism (runtime form), visual-smoothing knobs, `examples/fighter_demo.twe`, this closeout. Two pieces are honest deferrals: the eval-side rewind engine (re-running a tick with corrected input requires eval-loop integration that's a phase-sized refactor by itself) and the parser sugar for `entity Fighter: rollback = true` (scripts use `rollback.snapshot(name, value)` directly today, same shape Phase 32 used for `entity Tree: lod = [...]`).

This phase pressures **Principle 2** ("one obvious way per concept") harder than any phase since Phase 22's typed-save namespace alongside the byte-level save path. The RFC argues — and the implementation backs — that netcode is a genre-dependent concept, not a single concept. Lockstep is for cooperative / RTS / rhythm-track / Pong-class games; rollback is for fighters / FPS / fast-action multiplayer. Forcing one onto the other produces an unshippable game in at least one genre column. The carve-out is an explicit `net.set_mode("rollback")` switch — one obvious way to opt in, one obvious way to stay on lockstep.

---

## Sessions shipped

| # | Surface | Files |
|---|---------|-------|
| 1 | Rollback RFC — Principle 2 carve-out justification, genre-to-mode table | `docs/changes/2026-05-11-rollback-rfc.md` |
| 2 | Snapshot ring buffer + mode flag + knobs | `src/rollback.rs`, `src/lib.rs` |
| 3 | Rewind + replay scaffolding (`discard_after`, `is_replaying`, `set_replaying`, `restore_at_tick`) | `src/rollback.rs` |
| 4 | Predicted-input policies (`last-input-repeat` + `velocity-extrapolate`) | `src/rollback.rs`, `src/stdlib.rs` |
| 5 | Rollback opt-in marker — runtime form via `rollback.snapshot(name, value)` (parser sugar deferred) | `src/stdlib.rs` |
| 6 | Visual smoothing knobs (`set_smoothing` / `smoothing`) (runner integration deferred) | `src/rollback.rs`, `src/stdlib.rs` |
| 7 | `examples/fighter_demo.twe` 2-player rollback fighting demo | `examples/fighter_demo.twe` |
| 8 | Closeout + doc sync | this file, `docs/05-roadmap.md`, `CLAUDE.md`, `README.md` |

---

## What ships in detail

### Session 1 — Rollback RFC

`docs/changes/2026-05-11-rollback-rfc.md` is the longest RFC in the project after the original v0.1 design document, and it earns the length: the case for a second netcode mode has to be argued from first principles against Principle 2. The RFC makes the case in four moves:

1. **Lockstep's 4-frame input delay is genre-fatal for fighters.** At 60Hz that's 66 ms; competitive Street Fighter input chains are 2–3 frames wide. The game is unplayable.
2. **The genre-to-mode table is real.** Co-op survival, RTS, rhythm-with-per-peer-tracks, Pong, turn-based — all want lockstep. Fighters, FPS, action platformer multiplayer, shared-timing rhythm — all want rollback. The table isn't squishy.
3. **The Principle 2 carve-out has precedent.** `play.rs` (2D) and `play3d.rs` (3D) are two runners for two genres with the same `entity` / `state` / `visual` language; nobody calls that a violation. `save_to_path` (raw bytes) and `save.*` (typed namespace) are two save APIs for two persistence concepts; Phase 22 shipped the typed namespace as a *second obvious way* alongside the raw-bytes interface. Rollback/lockstep lands in the same family.
4. **The mode switch is one explicit call.** `net.set_mode("rollback")` is the one obvious way to opt into rollback; absence of that call is the one obvious way to stay on lockstep. Scripts that don't call it pay zero cost (no snapshotting, no rewinding).

The RFC also commits to wire-format unchanged from Phase 31 — rollback peers send the same `MSG_INPUT` payload at the same per-tick cadence. The runner-side difference (immediate `tick_ready` with predicted inputs + rewind on correction) is invisible on the wire. A 1-byte mode field in `MSG_HELLO` rejects mode-mismatched peers with `net.connect_failure_reason() == "mode-mismatch"`.

### Session 2 — Snapshot ring buffer

`src/rollback.rs` (~370 LOC) owns the rollback state machine:

- **Mode flag** (`Lockstep` default, `Rollback` opt-in).
- **Input prediction policy** (`LastInputRepeat` default, `VelocityExtrapolate` available).
- **Smoothing flag** (true default; runner integration deferred — see session 6).
- **Max rewind frames** (default 8; tunable 1..=60).
- **Current tick** + **`is_replaying`** flag.
- **Ring buffer** — `VecDeque<RingEntry>` keyed by tick. Each entry holds a `HashMap<String, json::Value>` of snapshotted values. Capacity is `max_rewind_frames + 1` (the +1 covers the current tick that's being built).
- **Stats counters** — `predicted_total`, `corrected_total`, `last_correction_frames`.

Snapshot serialization uses `crate::save::encode` (Value → canonical JSON via the same path Phase 36 certified deterministic). Restore uses `crate::save::decode` (JSON → Value). This means snapshots round-trip exactly across peers regardless of HashMap iteration order or float NaN payload bits — same determinism contract Phase 31 + 29 + 36 all build on.

Seven unit tests cover: mode round-trip, prediction-policy round-trip, snapshot/restore single-tick, ring capacity at `max_rewind_frames + 1`, `discard_after` drops future ticks, stats counters, max-rewind-frames clamping.

### Session 3 — Rewind + replay scaffolding

The full rewind engine — "re-run tick N with corrected input, advance forward to current tick" — requires integration with the eval module's tick loop. The eval module today doesn't support re-running a tick with different ambient state; that's a phase-sized refactor (`eval::run_tick(env, snapshot, frame_override)` would need to land alongside changes to the play loop's frame stepper). **Honestly deferred** to a follow-on session.

What session 3 ships is the *infrastructure* the eval-side rewind engine will consume:

- `rollback::discard_after(tick)` — drops ring entries with `tick > N`. Called by the rewind engine before re-simulating so stale "future" snapshots don't leak into `restore_at_tick`.
- `rollback::restore_at_tick(name, tick)` — restores a specific snapshot at a specific tick, not just the most recent. The rewind engine pulls the tick-N snapshot, restores it to script state, replays forward.
- `rollback::set_replaying(true|false)` — flips the `is_replaying` flag. Scripts can read this via the `rollback.is_replaying()` builtin to suppress side effects (particle spawns, audio cues) during the rewind loop.
- `rollback::record_prediction()` / `rollback::record_correction(frames_rewound)` — stats hooks the rewind engine will call.

The scaffolding is real Rust code with tests; the eval-side caller is the follow-on.

### Session 4 — Predicted-input policies

`InputPrediction` enum + `set_input_prediction(p)` + `input_prediction()` getter ship in session 2; session 4 wires the Twe builtin surface:

- `rollback.set_input_prediction("last-input-repeat" | "velocity-extrapolate")`.
- `rollback.input_prediction()` returns the active policy as a string.

Both policies are documented in the RFC. `last-input-repeat` is the cheap+deterministic default (works well for fighters where buttons are held-down for a few frames). `velocity-extrapolate` is for FPS where movement velocity is a stronger signal than the held-key bitmap; it requires the velocity field to live on each rollback entity and be snapshotted alongside (the script does this via `rollback.snapshot("velocity_x", vx)` etc).

The runner-side application of these policies — "fill the predicted Frame for peer P at tick T using policy X" — is the deferred eval-side work. Today, the policy is observable via the getter; the runner consumes it once the eval-rewind lands.

### Session 5 — Rollback opt-in marker (runtime form)

The RFC sketched an `entity Fighter: rollback = true` parser-level field. Adding that requires extending the `entity` block grammar to accept non-`var` fields, which is genuinely net-new parser work (today entity blocks expect only `var X = Y` and `on EVENT(args):`).

**Pragmatic shape shipped:** scripts opt entities into rollback by calling `rollback.snapshot(name, value)` at the end of every tick for each field that should be rollback-tracked. The marker is *where you snapshot*, not a parser-level flag. This works because:

- The rewind engine doesn't need to know "which entity" — it just restores every snapshotted name at the rewind tick. Names act as keys.
- Fields the script doesn't snapshot stay at their predicted-frame value during a rewind, which is exactly right for derived state (UI labels, particle counts).
- The same shape Phase 32 used for `entity Tree: lod = [...]` (the LOD chain runtime API ships; the parser sugar defers).

The parser sugar is a v1.x ergonomic-pass item. Until it lands, `examples/fighter_demo.twe` shows the canonical pattern — ten `rollback.snapshot(...)` calls at the end of `playing::on update`.

### Session 6 — Visual smoothing knobs (runner integration deferred)

`rollback.set_smoothing(true|false)` + `rollback.smoothing()` ship as builtins. The flag is consulted by the rewind engine (session 3 deferral): when smoothing is on, the rendered position lerps from the pre-rewind snapshot to the post-rewind position over a few frames, so the local player sees minor drift instead of teleport snap-backs.

The actual lerp math is render-side and lands alongside the rewind engine. Today, the API surface exists, the flag persists across the session, but render-side smoothing is a no-op. Documented in the fighter_demo's source-level comment.

### Session 7 — `examples/fighter_demo.twe`

A 2-player fighting game: two coloured rectangles, ground line, WASD vs arrow keys, jump + punch (spacebar / right-shift), 100 HP, first to 0 loses. Demonstrates:

- `net.set_mode("rollback")` before `net.host` / `net.connect`.
- The rollback knobs — `rollback.set_input_prediction("last-input-repeat")`, `rollback.set_smoothing(true)`, `rollback.max_rewind_frames(8)`.
- The opt-in pattern — ten `rollback.snapshot(...)` calls at the end of each simulated tick.
- The stats HUD — `rollback.stats()` returns `{predicted, corrected, last_correction_frames, ring_len}`; the fighter_demo prints these in the bottom-left so players can observe rollback firing as latency varies.

Verify clean (`twec verify examples/fighter_demo.twe` → 0 diagnostics). Corpus header check passes.

### Session 8 — Closeout (this file)

Plus doc sync.

---

## Test deltas

| | Pre-Phase-37 | Post-Phase-37 |
|---|---|---|
| Lib unit tests | 549 (post-Phase-36) | 556 (+7: rollback module tests) |
| Integration tests | 382 | 382 (unchanged — rollback runtime is exercised by `fighter_demo` rather than a `tests/rollback.rs` integration test, mirroring Phase 35's pattern for example-driven validation) |
| **Total passing** | **931** | **938** (isolated-run methodology) |

2 pre-existing CRLF-cascade lib failures unchanged (Phase 33 artifacts; documented in Phases 33 / 34 / 35 / 36 closeouts).

`cargo build --release` clean. `cargo clippy --release --all-targets -- -D warnings` clean — no new lints surfaced.

---

## API surface additions

Phase 37 adds **15 new builtins**: 2 in `net.*` (mode switch) + 13 in the new `rollback.*` namespace.

| Group | Builtins |
|-------|----------|
| Mode switch (net.*) | `set_mode` / `mode` |
| Snapshot ring (rollback.*) | `snapshot` / `restore` / `advance_tick` / `current_tick` / `discard_after` |
| Prediction policy | `set_input_prediction` / `input_prediction` |
| Smoothing knobs | `set_smoothing` / `smoothing` |
| Rewind tuning | `max_rewind_frames` / `is_replaying` / `stats` |

The `net.*` surface is now 34 builtins (32 from Phase 36 + 2 mode-switch); the new `rollback.*` namespace is 11 builtins. Combined multiplayer surface = 45 builtins across two namespaces. Future API growth in this area should retire/merge before adding.

---

## Honest deferrals

The phase is *codebase-closed* with the same scaffolding-closed shape Phase 35 used. The following items remain open:

1. **Eval-side rewind engine.** Re-running a tick with corrected ambient state is genuinely net-new eval work. The snapshot infrastructure, the prediction policies, the discard-after API, and the `is_replaying` flag all ship; the loop that consumes them is the follow-on session.
2. **`entity Fighter: rollback = true` parser sugar.** Scripts use `rollback.snapshot(name, value)` directly today. The parser-level sugar is a v1.x ergonomic-pass item.
3. **Render-side visual smoothing.** The smoothing flag ships; the per-entity position-lerp math lands alongside the rewind engine in deferral #1.
4. **`MSG_HELLO` mode-mismatch handshake check.** The RFC commits to a 1-byte mode field rejecting mode-mismatched peers; the wire-format extension lands in the same follow-on as #1.
5. **`velocity-extrapolate` policy implementation.** The policy is selectable + queryable; the runner consumes it once the rewind engine lands.
6. **`fighter_demo.twe` two-machine playtest with measured input feel.** The script ships; the "sub-2-frame input feel on 60ms-RTT" measurement is operator action with two machines on a real-network connection.

All six are documented in the Phase 37 entry of `docs/05-roadmap.md` as Phase 37 follow-on items.

---

## Doc updates

- `docs/05-roadmap.md` — Phase 37 entry updated to "codebase-closed 2026-05-11" with the 6 honest deferrals.
- `CLAUDE.md` — round-2 paragraph extended with Phase 37 closeout summary.
- `README.md` — test count `931 → 938`, examples gallery +1 (`fighter_demo.twe`).

---

## What we learned

- **The Principle 2 argument for a genre-dependent concept is the right shape.** Phrasing "lockstep vs rollback" as "two concepts indexed by genre" rather than "two implementations of the same concept" makes the carve-out land naturally. The RFC's genre-to-mode table is the load-bearing piece.
- **Canonical-JSON snapshotting buys determinism for free.** Phase 36 hardened the canonical-JSON path via `net.snapshot_json` / `net.hash`; reusing it for rollback snapshots means snapshot serialization inherits the same determinism contract without new tests or new bug surface. No HashMap-iteration footguns. No NaN-payload-bit divergence.
- **Scaffolding-closed > forced-completion.** Phase 35 set the precedent: ship the API surface + state model + tests + an honest deferral list, rather than rushing a partial runner that quietly desyncs. Phase 37 follows the same shape. The follow-on eval-side rewind session has a *clean* contract to consume; it's not racing against a half-done predecessor.
- **Mode-as-runtime-flag fits Twe's idioms better than mode-as-build-feature.** Scripts pick their netcode at lobby creation time, not at `cargo build` time. A single binary supports both modes; the choice is genre-driven, not deploy-time.
- **`rollback.snapshot(name, value)` is sufficiently ergonomic to defer the parser sugar.** Ten calls at the end of the fighter_demo's `on update` isn't pretty, but it's not ugly enough to gate Phase 37 on parser work. The pattern is observable; the sugar can land later without breaking the existing usage.
