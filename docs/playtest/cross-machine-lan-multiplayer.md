# Cross-machine LAN multiplayer playtest

> Phase 31 codebase-closed 2026-05-10 with `examples/pong_net.twe` and a two-thread end-to-end test (`tests/net.rs`). This harness moves the test off-machine — Phase 31's `**examples/pong_net.twe** plays peer-to-peer over LAN with 4-frame input delay, deterministic across two machines` exit criterion can only close after a real two-machine playtest. This doc is the recipe.

## What you need

- **Two machines on the same LAN.** Wired or Wi-Fi both fine. Each must be reachable by the other on UDP port 7878 (default for `net.host`). If a personal firewall is on, allow `twec.exe` (Windows) / `twec` (macOS / Linux) inbound + outbound on 7878.
- **`twec` v0.1+ on each machine.** From the GitHub release archive or `cargo install --git ...`. Version reported by `twec --version` must match between the two machines — lockstep determinism breaks across version drift.
- **One Twe source file:** [`examples/pong_net.twe`](../../examples/pong_net.twe). Same byte-for-byte file on both machines. (`shasum -a 256 examples/pong_net.twe` should match.)

## Step-by-step

### On the host machine (call it Alice, IP `192.168.x.A`)

```sh
# 1. Confirm version + checksum so the two boxes are running the same code.
twec --version
shasum -a 256 examples/pong_net.twe   # macOS / Linux
certutil -hashfile examples\pong_net.twe SHA256   # Windows

# 2. Discover Alice's LAN IP. Write it down — Bob types it into 'connect'.
ipconfig | grep IPv4         # Windows
ifconfig | grep 'inet '      # macOS / Linux

# 3. Launch the host. The script calls net.host(7878) and then waits
#    in the lobby state for one peer to connect.
twec play examples/pong_net.twe --host
```

Expected console output on Alice:

```
[twec] hosting on 0.0.0.0:7878
[twec] waiting for peer...
```

### On the joining machine (call it Bob, IP `192.168.x.B`)

```sh
# 1. Verify version + checksum match Alice.
twec --version
shasum -a 256 examples/pong_net.twe

# 2. Connect to Alice. Replace 192.168.x.A with the IP you wrote down.
twec play examples/pong_net.twe --connect 192.168.x.A:7878
```

Expected console output on Bob:

```
[twec] connecting to 192.168.x.A:7878...
[twec] handshake complete; local peer id = 1
```

Expected console output on Alice the moment Bob connects:

```
[twec] peer connected; remote peer id = 1
[twec] session ready (2 peers)
```

Both windows should now show the pong arena with two paddles. Alice
controls the left paddle (peer 0); Bob controls the right paddle
(peer 1). The ball spawns on a determinism-checked tick.

## Pass criteria

The Phase 31 exit criterion is *deterministic across two machines*. Pass conditions:

1. **Both windows show the same ball position.** If Alice's ball is at (320, 240) and Bob's is at (321, 239), the lockstep hash check has caught a desync — abort and file a bug.
2. **Both windows show the same score after 30 seconds of play.** Score is updated only on goal events, which are derived from ball position + paddle position. If scores diverge, the lockstep determinism is broken.
3. **Input delay feels like ~4 frames (~67ms at 60fps).** Visible but not painful. If it feels like 100ms+, network jitter is overwhelming the ring buffer — investigate ping (`ping 192.168.x.B`) before reporting.
4. **No `[twec] desync detected at tick N: Alice=0xAAAA Bob=0xBBBB` log line.** This is the hash-mismatch diagnostic and means the determinism has broken between the two peers. Save the replay file (next section) and file a bug.
5. **The session survives one minute of continuous play.** No spurious disconnect, no frozen frames.

## Recording a session for replay

Phase 29 shipped `replay.record` / `replay.play`. To record a playtest for later analysis:

```sh
# On Alice:
twec play examples/pong_net.twe --host --record alice.replay

# On Bob:
twec play examples/pong_net.twe --connect 192.168.x.A:7878 --record bob.replay
```

Save both `.replay` files. They should be byte-for-byte equal — the lockstep input log is identical on both sides. Run `diff alice.replay bob.replay` after the session; the only legitimate diff is the per-machine timestamp header (the 8-byte preamble, not the input log).

To replay locally (single machine):

```sh
twec play examples/pong_net.twe --replay alice.replay
```

## Failure mode triage

| Symptom | Likely cause | Try |
|---------|--------------|-----|
| Bob's `connect` hangs | Firewall blocks UDP 7878 | Allow `twec` through both firewalls |
| `connecting...` then timeout | Wrong IP or wrong port | Re-check Alice's IP; confirm port 7878 isn't already used |
| Different ball positions | Version drift between machines | `twec --version` must match exactly; re-install if not |
| Score diverges | One machine ran a different `pong_net.twe` | Verify `shasum` matches |
| Both crash with "fiber stack overflow" | Same script bug on both peers (deterministic) | This is a Twe bug, not a netcode bug |
| Frame freeze, then catch-up | Network jitter; ring buffer drained | Increase `net.input_delay(8)` in script for higher-jitter networks |

## What this validates

- **Phase 31 exit criterion #1**: `examples/pong_net.twe` plays peer-to-peer over LAN with 4-frame input delay, deterministic across two machines.
- **Phase 35 sub-deliverable**: cross-machine LAN multiplayer playtest. Closeout note records the result; a bit-deterministic 60-second session counts as the criterion met.

## What this does *not* validate

- Internet play. Phase 36 covers NAT traversal + matchmaking + reconnect.
- Steam P2P. Phase 36 covers `--features steam-net`.
- More than 2 peers. The Phase 31 RFC capped at 4 max; the harness is 2-peer-only.
- WebSocket / browser multiplayer. Phase 31 RFC explicitly scoped this out.

## Reporting back

After running the playtest, file the result in the closeout-note format:

```
docs/changes/<YYYY-MM-DD>-phase-31-playtest-<host-os>-<peer-os>.md
```

Pattern:

- Two-line summary: pass / fail + reproduction step or bug link.
- Hardware: which two machines, what kind of LAN.
- Reproduction transcript: paste the console output from both sides.
- Replay files: store under `docs/playtest/replays/` for later regression checks.

A clean playtest closes Phase 31's manual exit criterion and adds a bullet to the v1.x stability log.
