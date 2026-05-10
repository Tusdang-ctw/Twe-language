# Community game pipeline

> Phase 35 sub-deliverable: a documented path for an external author to ship a Twe game and have it counted toward Phase 16 ("two first-party games + N community games") and Phase 35 ("≥ 1 community-authored game shipped on a v1.x release").

## Goals

1. **Lower the floor.** The first three external authors should be able to ship without source-diving the Twe codebase.
2. **Capture frustration.** Each external port produces a frustration list à la Phase 2 — the input to the next round of language-design corrections.
3. **Visible track record.** Shipped community games get linked from the README "Shipped on Twe" gallery so prospective authors see traction.

## What "shipped" means

For the purposes of Phase 16 + Phase 35 counting:

- A standalone `.exe` / `.app` / `.AppImage` produced by `twec build`, OR a browser playable from the Phase 30 WASM target.
- Distribution: itch.io public release page (paid or free), Steam release, or web release on a project-owned domain. Private builds, school assignments, and game-jam Discord posts don't count.
- Authored primarily in Twe. A 90/10 Twe / Rust split is fine; a 10/90 split with Twe as a glue language is not.
- Not built on a fork that adds new language features. Stdlib helpers and patches are fine — language extensions require RFC.

## Solicitation list (initial 2-3 candidates)

Target community: small commercial-2D devs who already ship on itch.io, can read Rust enough to build from source if the binary release lags, and have an existing audience that makes a port a marketing event.

| Cohort | Where to find them | Why they'd pick Twe |
|--------|-------------------|---------------------|
| GDScript indies looking to escape Godot perf ceilings | r/godot, GDScript-focused itch.io creators | NaN-tagged VM + tracing GC + `twec build` matrix vs. GDScript |
| Lua / LÖVE indies | LÖVE Discord, lovers.io | First-class `entity` / `state` blocks vs. table-based OOP |
| MakeCode Arcade graduates | MakeCode forums | Step up to a real language without losing the "game concepts are first-class" feel |

The pitch:

> Twe is a game-first language with `entity`, `state`, `visual`, `dialogue`, and `particles` as language constructs, not library calls. v0.1 ships a Vampire-Survivors-class build (`survive_beta`) that produces a self-extracting `.exe` from one source tree. Want to be the first community shipper? We'll provide direct support, your name + game in the README, and a bug-fix hotline for any rough edges you hit.

## Author kit

When a candidate accepts:

1. **Direct support channel.** Discord DM or GitHub Discussions thread, dedicated. Maintainer-side response SLA: same-day for blockers, three-day for everything else.
2. **Tutorial v3 carve-out.** `docs/tutorial.md` is a generalist guide; for the first community port we'll add a "your-genre quickstart" sub-doc tailored to whatever they're building (platformer? puzzle? roguelike?). Becomes a permanent doc after.
3. **Frustration log template.** `docs/changes/<date>-community-port-<name>-frustration-log.md` — same shape as `docs/changes/2026-04-28-phase-2-frustration-list.md`. Filed at the end of the port, drives follow-on language work.
4. **Release-day amplification.** Maintainer-authored social post + README update + GitHub Release announcement when the game ships.

## Tracking

- README "Shipped on Twe" gallery has one row per shipped community game. Format:
  | Game | Author | Released | Twe version | Link |
  |------|--------|----------|-------------|------|
- `docs/changes/community/` directory holds port-completion notes from each shipper. Pattern matches the closeout-note discipline: what shipped, what slipped, frustration list, reusable patterns extracted.

## What this is *not*

- A grant program. No money changes hands.
- A licensing deal. Authors keep 100% of their game, IP, and revenue.
- A formal partnership. There's no "Twe Foundation" or contract.
- A QA service. Authors test their own games; the Twe maintainer provides language/runtime support, not gameplay testing.

## Exit signal

The community pipeline is "working" when:

- 3+ external games have shipped on a v1.x release.
- The frustration logs have produced ≥ 1 language-level fix that wouldn't have happened from internal dogfood alone.
- A community contributor has authored and merged a non-trivial PR (stdlib helper, doc improvement, build pipeline fix) without maintainer hand-holding.

Phase 35 closes when the first community game ships. The other signals are the Phase 16 LTS-window stability check.
