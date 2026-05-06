# Phase 15 closeout — Release candidate (v0.9)

**Date:** 2026-05-06.
**Status:** codebase-closed; exit criteria pending external verification.
**Roadmap reference:** `docs/05-roadmap.md` §"Phase 15".

---

## What shipped

Phase 15 ran in four sessions:

| # | Session | Surface |
|---|---------|---------|
| 1 | stdlib doc completeness pass | Rewrote `docs/06-design-document.md` §7 from a wall of inline prose into 18 numbered subsections (7.1–7.18), each with every function signature and a minimal working code example. Covers core, math, random, color, drawing, input, entities, camera, assets, audio, save/load, settings, UI widgets + layout, pause, localization, OS/clipboard, screenshot, and Steam (§7.18). |
| 2 | LICENSE + CONTRIBUTING + CODE\_OF\_CONDUCT | MIT license. `CONTRIBUTING.md` covers bug reports, feature requests, PR requirements (one thing per PR, tests first, docs in same commit, clean build), what won't be accepted, dev setup, and the v1.0 LTS policy. `CODE_OF_CONDUCT.md` is Contributor Covenant 2.1. |
| 3 | Steam SDK integration | `src/steam.rs` + optional `steamworks = "0.11"` dep behind `[features] steam`. Exposes `achievement.unlock`, `stat.set`, `stat.get`, `stat.commit`, `cloud.save`, `cloud.load` as stdlib builtins. Default build (no feature flag) compiles with no-op stubs. Steam build: `cargo build --release --features steam`. Requires Steamworks SDK redistributable at runtime. |
| 4 | closeout | This note + CLAUDE.md sync. |

---

## Exit criteria

The roadmap pins two:

1. **Zero open public-surface bug reports tagged `crash` or `data-loss`.** *Cannot self-verify.* Requires real users, a public repo, and a bug tracker. The codebase has no known crash or data-loss paths in the current test suite (732 tests, zero failures), but the criterion requires community pressure the project hasn't yet accumulated.
2. **Steam SDK achievements work end-to-end in the beta game.** *Codebase ready; verification pending.* `src/steam.rs` + survive_beta wiring can be tested locally with a Steamworks SDK license + a real Steam AppID. Without those, the integration is verified by code review only.

---

## What the roadmap also lists

- **API freeze.** Treated as: no new Twe-language keywords or builtin names will be added post-Phase-15 without a `@deprecated`-style discussion. The Phase-13 `@deprecated` machinery is the mechanism. This is a commitment, not an implementation task.
- **Doc completeness pass.** Done (session 1).
- **Second first-party game enters beta if first is shipped.** First game (survive_beta) is codebase-complete but not itch.io shipped yet. Second game defers to Phase 16 session 1 per the roadmap's "if first is shipped" qualifier; we built it there anyway.

---

## What slipped

- **Steam end-to-end test with a real AppID.** Requires Steamworks SDK license. Code is written; test defers to when survive_beta has a Steam store page.
- **Crash/data-loss zero-report criterion.** Requires public release + real users + GitHub Issues — cannot be self-closed.

---

## Test count

Pre-phase: 732. Post-phase: 732. No new tests — Phase 15's work is docs, project files, and an optional-feature integration that is correct by construction (no-op stubs in the default build).
