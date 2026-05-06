# Phase 14 closeout — Beta + dogfood (v0.8)

**Date:** 2026-05-06.
**Status:** **codebase-closed; ship + telemetry pending.**
**Roadmap reference:** `docs/05-roadmap.md` §"Phase 14".

---

## What shipped

Phase 14 ran in sixteen sessions — twelve building survive_beta, three tutorial + dogfood-driven fixes, one closeout:

| # | Session | Surface |
|---|---------|---------|
| 1 | scaffold + player + camera + arena | `examples/survive_beta/` 2400×1800 arena, view-clamped camera, basic player rect with WASD + arrow movement. |
| 2 | slime enemy + chase AI + contact damage | First entity class. `entity Slime:` with chase-the-player update + contact damage with i-frames. |
| 3 | wave spawner with time-scaled escalation | `wave_index`, `wave_timer`, escalating slime counts per wave. Off-screen spawn ring keeps fairness. |
| 4 | auto-attack v1 (homing projectile) | `entity Projectile:` with target-on-spawn-frame nearest-enemy aim, 600 u/s speed, 14px hit radius. |
| 5 | XP drops + magnetic collection | `entity XPGem:` spawns on enemy death, drifts toward player within `magnetic_radius`, collected on overlap. |
| 6 | level-up + upgrade picker modal | `state level_up:` with `in_levelup` gate, three random picks from a 5-entry catalogue, `apply_upgrade(id)` mutates globals. |
| 7 | orbiting blade + AoE aura weapons | `entity Blade:` orbiting at `weapon_time * orbit_speed + phase`, `entity Aura:` ticking enemies in `radius`. Multi-blade phase redistribution on upgrade. |
| 8 | bat + skeleton enemy variants | `entity Bat:` faster + lower HP. `entity Skeleton:` ranged with `entity SkeletonBolt:` projectiles. Per-class chase logic. |
| 9 | boss + death/restart polish | `entity Boss:` 50hp, every-5-wave spawn, separate weapon collision rules (damage instead of one-shot). `state game_over:` with restart-on-R + best-wave / best-time tracker. |
| 10 | particles + visual polish | `entity Spark:` death-poof effect via `on Class.death(e)` hooks for every enemy class. `hit_flash` overlay timer + i-frame yellow tint on player. |
| 11 | pause menu + settings save + gamepad | `state paused:` with Resume / Save Bindings / Quit Run buttons. `settings.set_default` keybinds + `settings.try_load` / `settings.save` round-trip. Gamepad Esc / B + d-pad + left-stick read alongside keyboard. |
| 12 | build pipeline integration | `examples/survive_beta/twe.toml` + `twec build examples/survive_beta` produces self-extracting Windows `.exe`. The Phase-12 build pipeline carries an actual game end-to-end. |
| 13 | modal render-transition fix + window sizing | First-playtest engine bugs. `eval::render_frame` (and VM mirror in `vm::render_inner`) silently discarded transitions raised inside `on render():` — every modal-state button was a no-op. Fixed by applying the transition via `enter_state` after the render block runs. `play.rs` window now `high_dpi: false, window_resizable: false` to lock the surface to 640×480 physical pixels (HiDPI + resize was leaving the world drawn in the top-left of an enlarged window with mouse coords past the button rects). Survive_beta side: gated every entity's `render()` on `in_levelup` to match the existing `update()` gate. **732 tests still pass.** |
| 14 | tutorial v2 chapter 1 + pong example | `examples/pong.twe` (200-line player-vs-AI Pong), `docs/tutorial.md` Part II opens with the Pong walkthrough — paddles, input, AI tracker, ball physics + paddle-edge skill mechanic, scoring, `scored` intermission state, restart on R. |
| 15 | tutorial v2 chapters 2 + 3 + dialogue example | Chapter 2 = read-along of `examples/survive_beta/main.twe` (1264 lines mapped to 5 patterns). Chapter 3 = `dialogue:` block primer + `examples/dialogue_demo.twe` reference. Honest about v0.8 dialogue limits (top-level decl only; `wait` inside body is a runtime error). |
| 16 | closeout | This note + CLAUDE.md sync. |

---

## Exit criteria

The roadmap pins two:

1. **Beta game ships ≥ one paid release on itch.io with positive (≥ 4-star) reviews.** *Codebase ready; ship pending.* `examples/survive_beta/` is feature-complete and builds to a redistributable Windows `.exe` via the Phase-12 pipeline. The actual itch.io upload + storefront listing + marketing is outside the codebase — it's the user's release to drive. The phase-as-roadmap-phase doesn't fully close until the upload happens and reviews come in.
2. **Tutorial completion tracked: a new contributor builds Pong from the tutorial in ≤ 2 hours.** *Codebase ready; telemetry pending.* `docs/tutorial.md` Part II ships with the Pong walkthrough as the entry-point game. Whether real contributors hit the 2-hour mark requires external observation that the project doesn't yet have an instrument for.

Neither criterion can be self-verified from inside the repo. Phase 14 is therefore **codebase-closed** rather than fully closed: every line the contract demanded is in main, but the closing telemetry is contingent on user action. Future commits should treat Phase 14 as in-flight on the *external* side and Phase 15 as available for new code work.

---

## Components vs. components

The roadmap lists four components; mapping them to what landed:

| Component | Status |
|-----------|--------|
| First-party game #1 enters closed beta (Vampire-Survivors clone) | **Done.** `examples/survive_beta/main.twe` (1264 lines) + `examples/survive_beta/twe.toml` build config. Exercises tilemap-style arena, save/load (settings-layer), particles (Spark + hit_flash), visuals, settings + key rebind path, gamepad, full Phase-10 pause stack. |
| Tutorial v2 in `docs/tutorial.md` (Pong → Survivors → mini-RPG, screenshots + recorded sessions) | **Codebase part done; media part deferred.** The three chapters exist as Part II of `tutorial.md` and walk through working reference files. Screenshots and recorded sessions defer to a media-pass session whenever someone records them; the doc is structured so they slot in inline. |
| Examples gallery to ~25 | **Done (26).** Pre-phase count was 24; this phase added `pong.twe` and `dialogue_demo.twe` to land at 26. The Phase-6 deferred target was 20; we're well past. |
| Performance fix list driven entirely by what the beta game hits | **One round done.** Session 13 fixed two real engine bugs surfaced in the first playtest (modal render-transition discard; window high_dpi + resize mismatch). No speculative perf work; no further bugs surfaced in the sessions 14–15 dogfood pass. Future rounds re-open as playtest finds them. |

---

## What slipped

- **Itch.io ship + reviews.** Outside the codebase. User's release work.
- **Tutorial-completion telemetry.** Outside the codebase. Needs real contributor observation; project has no Phase-14 surface for tracking it.
- **Tutorial screenshots + recorded sessions.** A media pass, not a code pass. Deferred to whenever someone has the recorder set up; the tutorial copy is structured to receive them.
- **Bytecode VM kwarg-builtin support.** All widget-using examples (every Phase-10 demo + survive_beta + pong) hit the bytecode VM's known kwarg limitation. Tree-walker is the canonical `twec play` path. Closing the gap is its own session under Phase 15 hardening.
- **3× bytecode-VM speedup gap from Phase 8.5.** Untouched this phase. The criterion bench harness ships (Phase 11 session 6) and the dispatch-tuning pass shipped (Phase 11 session 7); the actual 3× number isn't snapshotted yet. Captured for Phase 15 if the gap still pressures release.
- **Auto-pause-on-blur on macOS / Linux.** Windows path stays as the only working implementation; the others remain stubbed `is_focused() = true` until a per-OS contributor lands them.

---

## What's next

Phase 15 — RC — is the canonical next phase per the roadmap. Theme: "stop adding things. Make the existing things solid." API freeze, doc completeness pass, Steam SDK v1, second first-party game (if first ships).

Phase 7 release engineering also stays open — `cargo dist` cross-platform binaries, VS Code marketplace publish, project website, Show-HN blog + demo. Phase 14's dogfood gives that work substantially better headline content (a real Vampire-Survivors-class game, the procedural fire shader, the verify-mode JSON surface).

The v1.0 thesis ("ship a Vampire-Survivors-class commercial 2D game on Twe") is now within shipping distance: the codebase carries an actual such game; the language surface is feature-complete; the build pipeline produces a Steam-class executable. v1.0 ETA remains "whenever the first beta game ships on a store with reviews," which is now bottlenecked on user release work, not on the language.

---

## Test count

Pre-phase: 732 tests (per session 13's "732 tests still pass" check — the headline number was steady through the engine fix; the survive_beta and tutorial sessions don't add unit tests, they ship runnable example files).
Post-phase: 732. Net 0 new tests; phase-14's quality discipline is "ship a real game and play it," not "grow the test count." The two engine bugs caught this phase escaped the existing test harness because they only fire under live render + mouse input — exactly the kind of bug dogfood is meant to find.
