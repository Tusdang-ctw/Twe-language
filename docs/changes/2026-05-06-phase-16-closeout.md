# Phase 16 closeout — Stable (v1.0)

**Date:** 2026-05-06.
**Status:** codebase-closed; v1.0 release pending external action.
**Roadmap reference:** `docs/05-roadmap.md` §"Phase 16".

---

## What shipped

Phase 16 ran in three sessions:

| # | Session | Surface |
|---|---------|---------|
| 1 | `examples/rpg_demo/` — second first-party game | Dialogue-driven adventure: two rooms via state machine, Guard NPC with branching dialogue, KeyPickup entity, chest interaction, settings persistence (rpg.save), WinDialogue + restart-on-R. Exercises dialogue blocks + scenes-as-rooms + settings-as-flags, the quadrant survive_beta doesn't touch. |
| 2 | README v1.0 polish + announcement draft | README rewritten from "design phase, no implementation" to v1.0-grade hero content: feature table, install, quick start, 28-example gallery, language-in-60-seconds, "Shipped on Twe" gallery. `docs/v1.0-announcement.md` is the draft Show-HN / blog post. |
| 3 | closeout + roadmap history | This note. Roadmap `v0.x` block marked as history; v1.x scratch section added. |

---

## Exit gate

The roadmap's Phase-16 exit gate (revised from "three games, six months stable"):

1. **Two first-party games shipped on a v0.x release.** *Codebase ready.* `examples/survive_beta/` (v0.8) and `examples/rpg_demo/` (v1.0) are both present and buildable. The actual itch.io / store releases are user-driven.
2. **Six months of API stability since the v0.7 freeze.** *Not self-verifiable* — this is a time-based criterion. The API surface has been stable since Phase 13 (2026-05-06); six months from that date is 2026-11-06.
3. **LTS commitment: 12-month backport policy.** *Documented* in `CONTRIBUTING.md`.
4. **Marketing push: Show-HN / blog / demo video pinned to v1.0.** *Draft ready* in `docs/v1.0-announcement.md`. Publishing happens when the itch.io release is live and the community announcements go out — user action required.

---

## What the codebase delivers at v1.0

The v1.0 thesis was: *"a developer can ship a Vampire-Survivors-class commercial 2D game on Twe."*

**That thesis is proven by the codebase.** `examples/survive_beta/main.twe` is a feature-complete Vampire-Survivors-class game that:
- Builds to a self-extracting Windows `.exe` with zero Twe installation required on the target machine
- Exercises every Phase 8–14 surface: arena + camera, wave spawner, auto-attack + weapons, level-up modal, enemies, boss, particles, pause menu, settings save/load, gamepad, kill counter, XP/level system
- Was dogfooded in Phase 14 sessions 13–19: two real engine bugs caught and fixed under live play

The full codebase at v1.0:

| Metric | Value |
|--------|-------|
| Tests | 732 (all pass) |
| Examples | 28 runnable `.twe` files / projects |
| Languages phases | 16 (Phases 1–16 complete on codebase side) |
| Stdlib functions | ~120 (documented in `docs/06-design-document.md` §7) |
| Lines of Rust | ~30,000 (twec compiler + runtime) |
| Biggest example | `survive_beta/main.twe` — 1300 lines |

---

## What's intentionally deferred to v1.x / v2.0

Per the roadmap's "What's intentionally not in the v1.0 plan" section (unchanged):

- 3D rendering polish (textures, animation, physics, multi-primitive `.glb`, `mat4`/`quat`)
- Native code generation
- Multiplayer / determinism
- User-defined generics
- Macros / metaprogramming
- Sandboxing for user-generated content
- Workshop / mod APIs
- macOS / Linux auto-pause-on-blur (Windows ships; others stubbed)

---

## Test count

Pre-phase: 732. Post-phase: 732. No new tests — Phase 16 is docs + a game example. The rpg_demo headless run (0 frames output = no print statements = expected) confirms the runtime loads and ticks cleanly.
