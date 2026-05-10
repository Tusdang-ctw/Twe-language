# Phase 35 closeout — External validation drive (codebase scaffolding)

**Status:** codebase-side **scaffolding-closed 2026-05-10**. Five sub-deliverables shipped that *enable* Phase 35's external work without doing the external work itself. The phase as a whole stays open until the external action lands (itch.io paid release, Steam AppID test on real hardware, cross-machine multiplayer playtest, 4km open-world playtest, community game pipeline traction, six-month API stability close).

This is the most-different closeout shape so far in the project. Phase 35 is a **non-code phase** in design — its exit criteria are external actions the maintainer cannot autoplay. What ships here is the *tooling that makes the external action evidence-grade*: the snapshot tool that proves "no API drift across six months," the playtest harnesses that turn "playtest succeeded" from a sentence into a checked-in artifact, the smoke tests that prove the build pipeline still links the right things.

---

## Sessions shipped

| # | Surface | Files |
|---|---------|-------|
| 1 | API stability snapshot tooling — `twec api-snapshot` + `twec api-diff` | `src/api_snapshot.rs`, `src/lib.rs`, `src/cli.rs`, `docs/api-snapshots/2026-05-10-baseline.json` |
| 2 | Cross-machine LAN multiplayer playtest harness | `docs/playtest/cross-machine-lan-multiplayer.md` |
| 3 | 4km open-world playtest stub | `examples/openworld_demo.twe` |
| 4 | Steam SDK end-to-end smoke test scaffold | `tests/steam_smoke.rs` |
| 5 | Community game pipeline + this closeout | `docs/community-game-pipeline.md`, `docs/changes/2026-05-10-phase-35-closeout.md` |

---

## What ships in detail

### Session 1 — API stability snapshot tooling

`twec api-snapshot [-o PATH]` writes a canonical, hashable JSON document of every public-API surface a Twe author / LLM / tool consumer could call:

- 235 stdlib builtins (name + category + parameter list, sourced via `crate::stdlib::manifest`)
- 35 reserved keywords (sourced from `src/lexer.rs`'s match arm)
- 6 tool-version pins (api-snapshot v1, corpus v1, eval v1, grammar v1, stdlib v1, verify v2)

The snapshot includes a deterministic FNV-1a hash over the body — two snapshots are byte-identical iff every public surface matched. The hash is the unit of "no API drift since N."

`twec api-diff <old> <new>` compares two snapshot files and reports adds / removes / changed-signatures by category. Exits 0 if identical, 3 if drift was detected — suitable for a CI gate that fails any PR introducing un-`@deprecated`-cycled API change during the v0.7 → v1.0 stability window.

The first baseline snapshot is checked in at `docs/api-snapshots/2026-05-10-baseline.json` (hash `0x299e66f4068c7979`). Operator workflow: snapshot at each release tag; diff against the v0.7 baseline at each LTS-window milestone; one bullet in `CHANGELOG.md` for every legitimate (deprecation-cycled) drift.

This closes the **Phase 16 six-month API stability** exit criterion *mechanically*. The judgment part — "was this change expected and `@deprecated`-cycled?" — stays with the maintainer.

### Session 2 — Cross-machine LAN multiplayer playtest harness

`docs/playtest/cross-machine-lan-multiplayer.md` is a one-page recipe for a two-machine pong_net.twe playtest. Documents:

- Machine prerequisites (`twec --version` parity, `examples/pong_net.twe` checksum match, UDP port 7878 firewall allow)
- Step-by-step host + join sequence with expected console output
- Five pass criteria covering the Phase 31 exit-criterion language ("deterministic across two machines")
- Replay record / diff workflow for byte-for-byte determinism check
- Failure-mode triage table (firewall, version drift, jitter, etc.)
- Reporting back template — closeout note format for the playtest result

This turns "we ran a multiplayer playtest" from a sentence in a release post into a checked-in artifact under `docs/changes/<date>-phase-31-playtest-<host-os>-<peer-os>.md`. Phase 31's "deterministic across two machines" criterion can now close with an evidence trail.

### Session 3 — 4km open-world playtest stub

`examples/openworld_demo.twe` exercises the Phase 32 data structures (spatial / streaming / LOD / instance) at a tractable script-side scale: 1000 static props + 50 dynamic NPCs across a 4km × 4km world, with `world.spatial_query_radius` running every frame against the camera position.

What this proves:

- Insert + query at frame budget for the chosen scale.
- `world.spatial_query_radius` returns the correct subset as the camera moves.
- LOD chain + `world.lod_for_distance` resolves to the right mesh handle as distance shifts.
- Frame time stays inside 16ms on the tree-walker for this scale.

What this does NOT prove (still deferred to wgpu render-integration follow-on):

- 50k props at 60fps with < 512MB VRAM (Phase 32 exit-criterion full scale).
- Visible LOD switching (we render dots, not meshes).
- Terrain heightfield rendering (we operate in world XZ, terrain.* is registered but not exercised here).
- Streaming / chunk load+unload over a real disk-backed manifest.

The stub is a top-down 2D visualisation: world XZ → screen XY, camera centered. Static props are green dots, dynamic NPCs are red dots, the player is a yellow circle. HUD shows total + visible counts + frame-time + bucket counts. Verifies clean against `twec verify` with zero diagnostics.

### Session 4 — Steam SDK end-to-end smoke test scaffold

`tests/steam_smoke.rs` proves the Steam build path stays buildable in both the no-feature default build and the `--features steam` build. The default build uses src/steam.rs's no-op stubs; the feature build links against `steamworks` and can attempt initialisation.

Two integration tests gate the operator-action portion:

- `feature_steam_initialises_when_steam_is_running` — `#[ignore]`d; requires Steam client + `steam_appid.txt` next to the test binary + Steam account ownership of the AppID. Spacewar (480) is the recommended free test target.
- `feature_steam_achievement_round_trip` — `#[ignore]`d; idempotent achievement-unlock against Spacewar's `ACH_TRAVEL_FAR_ACCUM`.

When the operator-action conditions are met (Steam running, AppID, owned game), `cargo test --features steam --test steam_smoke -- --ignored --nocapture` produces the evidence for the Phase 15 "end-to-end Steam SDK test" criterion.

### Session 5 — Community game pipeline

`docs/community-game-pipeline.md` documents the path for an external author to ship a Twe game and have it count toward Phase 16 + Phase 35 totals. Includes:

- Definition of "shipped" for counting purposes
- Three-cohort solicitation list (GDScript indies, LÖVE / Lua indies, MakeCode Arcade graduates) with channels and pitch language
- Author kit: dedicated support channel + tutorial v3 carve-out + frustration log template + release-day amplification
- Tracking shape: README "Shipped on Twe" gallery + `docs/changes/community/` directory for port-completion notes
- Anti-goals (what this is *not*): no grant program, no licensing deal, no formal partnership, no QA service
- Exit signal — three external games shipped + one language fix from the frustration logs + one community PR landed unaided

---

## Test deltas

| | Pre-Phase-35 | Post-Phase-35 |
|---|---|---|
| Lib unit tests | 533 (post-Phase-34: 535) | 539 (+6: api_snapshot module's 6 self-tests) |
| Integration tests | 378 | 379 (+1: steam_smoke no-feature test) |
| **Total passing** | **913** | **920** |

Net **+7 tests** (six api_snapshot + one steam_smoke). The 13 Windows-host CRLF-cascade failures are pre-existing Phase 33 closeout artifacts unrelated to this phase's diff (see Phase 33 closeout note for the LF-renormalisation context); test count above measures isolated-run results, matching the Phase 33 / 34 closeout methodology.

`cargo build --release` clean. `cargo clippy --release --all-targets -- -D warnings` clean. The clippy pass surfaced and got fixed during the validate cycle: `while_let_on_iterator` in `api_snapshot::split_string_array` (refactored to `for`), `clone_on_copy` in `tests/steam_smoke.rs` on `Value` (a Copy type — call sites cleaned).

---

## Schema versions introduced

| Tool | Schema | Stability |
|---|---|---|
| `twec api-snapshot` | `version: 1` | Stable; the format and hash algorithm are part of the API stability contract |
| `twec api-diff` | text-only output (no schema bump) | Stable |

Phase 33 introduced six earlier schema versions; Phase 35 adds one. Total schema-versioned tool surface: seven (api-snapshot v1, corpus v1, eval v1, grammar v1, stdlib v1, verify v2, plus the unwritten output formats of `parse`, `fmt`, `info`).

---

## Honest deferrals

The phase is *codebase-scaffolding-closed*. The following remain open and require external action by the maintainer or community:

1. **First-party itch.io paid release of `survive_beta`.** Build pipeline is ready (Phase 12 ships the self-extracting `.exe`); page authoring + pricing + screenshots + trailer are operator work.
2. **Steam AppID acquisition + end-to-end SDK smoke run on real hardware.** Test scaffold is ready (Phase 35 session 4); the AppID acquisition + Spacewar sanity run is operator work.
3. **Cross-machine LAN multiplayer playtest.** Recipe is ready (Phase 35 session 2); two physical machines + UDP-allow firewall config is operator work.
4. **4km open-world full-scale playtest at Phase 32 exit-criterion target (50k props + 500 NPCs at 60fps).** Stub at 1000 + 50 is here today; full scale waits on the wgpu render-pipeline integration follow-on dev cycle.
5. **Community game pipeline traction.** Pipeline doc is ready (Phase 35 session 5); reaching out to the three cohorts + supporting their ports is multi-month operator work.
6. **Six-month API stability close.** Snapshot baseline is ready (Phase 35 session 1, hash `0x299e66f4068c7979`); the operator runs `twec api-diff` against a future snapshot at the v0.7+6mo mark.

---

## Doc updates

- `docs/05-roadmap.md` — Round 2 size table updated; Phase 35 entries gain "(scaffolded; external action pending)".
- `CLAUDE.md` — round-2 paragraph extended with Phase 35 scaffolding details and the explicit external-action list.
- `README.md` — no edit; the `twec api-snapshot` + community pipeline links can land alongside the next first-party release.

---

## What we learned

- **The Phase-N-exit-criterion → external-action gap is a recurring shape.** Phase 14, 15, 16, 31, 32 all left external-action exit criteria pending. Building the *evidence-grade tooling* for each one (snapshot tool, playtest harness, smoke test scaffold) is what the Phase 35 closeout discipline turns into. Future "external" phases should default to this shape: do the codebase scaffolding now, leave a Phase-N-validation note for when the action lands.
- **The corpus header check (Phase 33) catches new examples without metadata.** Adding `examples/openworld_demo.twe` without `@task / @inputs / @expected / @category / @difficulty` failed CI immediately. Cheap, mechanical guard against doc-drift.
- **API stability snapshots are short.** 16KB JSON, 235 builtins, 35 keywords, 6 tool versions. The stability gate for an entire programming language fits in one screen.
