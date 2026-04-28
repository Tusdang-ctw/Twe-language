# Doc 05 — Roadmap

> A phased plan from "design document on a hard drive" to "v1.0 language used by real game developers."
>
> All time estimates assume a single primary developer working ~15 hours per week with AI assistance and the reading list completed. Adjust upward for less time, broader scope, or unfinished prerequisites.

---

## Guiding principle

**Every phase ships a runnable artifact.** No phase ends with "the design is better now"; every phase ends with code or a game that works.

The reason: language design is infinitely deep, but games are concrete. A working artifact at the end of each phase forces the design to confront reality.

---

## Phase 0 — Design lock-in (current phase)

**Duration:** 2–4 weeks.

**Goal:** finish the design documents in this repository, then *stop*.

**Deliverables:**

- [x] `01-examples.md` — ten example programs.
- [x] `02-type-system.md` — type system position.
- [x] `03-runtime.md` — runtime architecture and pitfalls.
- [x] `04-reading-list.md` — reading list.
- [x] `05-roadmap.md` — this document.
- [x] `06-design-document.md` — formal specification.
- [ ] EBNF grammar (a section of `06-design-document.md`).
- [ ] Project naming, license decision, repository setup.

**Exit criteria:**

- The ten example programs parse correctly against the EBNF (by hand — no implementation yet).
- The design has been shared with at least three people (online forum, Discord, or trusted developers) and survived their critiques.
- The implementer has read all of Tier 1 in `04-reading-list.md`.

**Anti-goals:**

- Don't write a parser yet.
- Don't pick an implementation language for the engine yet.
- Don't get distracted by "what about feature X?" — capture in an issue tracker, defer.

---

## Phase 1 — Tree-walking interpreter

**Duration:** 6–10 weeks.

**Goal:** the simplest possible Twe interpreter that runs the ten example programs (or simplified versions of them).

**Stack:** Rust. Single binary, no GUI.

**Components:**

1. **Lexer** — hand-written, ~500 LOC.
2. **Parser** — recursive descent, ~1500 LOC.
3. **AST** — tagged enum, ~300 LOC.
4. **Tree-walking evaluator** — ~2000 LOC.
5. **Built-in types** — int, float, bool, string, vector, color, range, duration. ~1000 LOC.
6. **Stdlib stubs** — `print`, `random`, `math`, basic IO. ~500 LOC.
7. **CLI** — `twec run <file>`, `twec parse <file>` (dumps AST as JSON for tooling). ~200 LOC.

Total target: ~6000 LOC of Rust.

**What works at the end:**

- Top-level statements run.
- Functions, blocks, control flow.
- Range, percent, duration, and length literals.
- The `entity` and `state` declarative blocks (no ECS yet — just regular method dispatch).
- A simplified `on update(dt):` that prints once per second.

**What does not work yet:**

- No graphics.
- No type checking (parser accepts annotations but ignores them).
- No coroutines.
- No `visual` blocks.

**Exit criteria:**

- `twec run examples/hello.twe` runs Example 1 (sans graphics — prints positions instead).
- `twec run examples/inventory.twe` runs a non-graphical version of Example 2.
- All passing tests in `tests/` are real Twe programs, not unit tests of the parser.

---

## Phase 2 — Vertical-slice game

**Duration:** 4–8 weeks.

**Goal:** a small but complete 2D game written in Twe, using a real graphics backend.

**Why this phase exists:** the language design is theoretical until it meets a game. This phase is where awkwardness in the language reveals itself.

**Stack:** Rust + the Twe interpreter from Phase 1, with Twe bound to **macroquad** (smaller and simpler than full Bevy for this purpose). macroquad gives 2D drawing, input, audio, and a window in one crate.

**Game choice:** a Vampire Survivors clone.

Reasons:

- 2D, fits on a single screen.
- Heavy on items, modifiers, upgrades — exercises Pillar 1.
- Lots of enemies — exercises ECS-style queries.
- Particle effects on enemy death — exercises Pillar 3.
- Achievable in ~50 hours of building once the engine works.

**Components added in this phase:**

1. **macroquad bindings** for sprite drawing, input, audio.
2. ~~**Coroutines / fibers** in the interpreter~~ — **deferred to Phase 3** per `docs/changes/2026-04-28-fibers-deferred-to-phase-3.md`. No shipped Phase 2 example pressured the feature; the implementation work doesn't transfer to the bytecode VM.
3. **Basic ECS world** under the interpreter — entities, components, simple queries.
4. **`particles` block** with CPU-side particle updating.
5. **Hot reload** of the running script.
6. **Game-specific stdlib:** `sprite.load`, `key.is_pressed`, `screen.size`, `time.dt`, etc.

**Exit criteria:**

- A playable Vampire Survivors clone runs from a Twe source file.
- Saving the source file hot-reloads the running game.
- Total Twe code for the game is under 500 lines (Lua / GDScript reference: 800–1000).
- The implementer has a list of language frustrations encountered during the build. **This list is the input for Phase 3.**

---

## Phase 3 — Design correction + bytecode VM

**Duration:** 8–12 weeks.

**Goal:** apply the lessons from Phase 2's frustration list, then upgrade from tree-walking to bytecode for performance.

This is the phase where the design becomes mature.

**Design corrections (driven by Phase 2 frustration list):**

- Likely candidates for change: the way modifiers compose (Example 2), the `on hp < 20%:` predicate syntax, the boundary between `entity` and `item`, error message quality.
- Each correction is documented as a "design change note" in this repository.

**Bytecode VM:**

- NaN-tagged value representation.
- Single-pass compiler from AST to bytecode.
- Bytecode interpreter loop with computed-goto where possible.
- Incremental tracing GC.
- **Cooperative fibers** (deferred from Phase 2 per `docs/changes/2026-04-28-fibers-deferred-to-phase-3.md`). Bytecode IPs naturally support suspension/resumption; the VM gets a per-fiber call stack and the runtime advances all live fibers each frame with budget protection. Surface: `wait <duration>`, fiber-backed `every <duration>:` (rewrite of the current per-state accumulator), and a `dialogue`-ready primitive for Phase 5.
- Target: 5x–20x faster than tree-walker on hot code paths.

**Other deliverables:**

- A formatter (`twec fmt`).
- Tree-sitter grammar (enables syntax highlighting in every editor).
- Basic LSP server (`twec lsp`) with go-to-definition and rename.

**Exit criteria:**

- Phase 2 game runs at 60fps with 500+ enemies on screen.
- Format and parse round-trips on every file in the test suite (idempotency).
- Tree-sitter grammar parses the ten examples.
- LSP works in VS Code with at least syntax highlighting, go-to-def, and inline errors.

---

## Phase 4 — Type system v1 (non-strict mode)

**Duration:** 6–10 weeks.

**Goal:** ship the non-strict mode of the type system from `02-type-system.md`.

**Components:**

- Type representation (algebraic types in the implementer's data model).
- Hindley-Milner-style inference with extensions.
- Structural table types.
- Tagged unions.
- Optional types.
- Dimensional unit checking (`5m + 3s` errors).
- Editor integration: types power autocomplete and on-hover docs.

**Exit criteria:**

- All ten example programs type-check in non-strict mode without modification.
- LSP shows correct types on hover for ~95% of expressions in the example programs.
- Twe code with no annotations gets useful type-driven autocomplete.

---

## Phase 5 — 3D + scenes + dialogue

**Duration:** 8–12 weeks.

**Goal:** ship the rest of the example programs. Specifically, get Examples 3 (dialogue), 4 (state-machine AI), 7 (save/load), 8 (3D camera), 9 (tilemap), and 10 (boss fight) running.

**Components:**

- 3D rendering backend (likely **wgpu** directly, possibly behind a small abstraction).
- Mesh loading (`load_mesh` for `.glb`).
- Camera system (third-person, first-person, free).
- Tilemap rendering and collision.
- Dialogue UI primitives (text rendering, choice prompts).
- State-machine block compiler.
- `save` block compiler with versioning and migration.

**Exit criteria:**

- A second vertical-slice game ships: a small 3D action-RPG with a town, three NPCs with dialogue, a tilemap dungeon, and a boss fight using all the patterns from Example 10.
- Total game code: under 1500 lines of Twe.

---

## Phase 6 — Tooling, polish, documentation

**Duration:** 6–10 weeks.

**Goal:** Twe is usable by people who aren't the implementer.

**Components:**

- Comprehensive tutorial (think the Rust Book, but for Twe).
- Reference manual (every keyword, every stdlib function).
- Examples gallery (~20 small programs covering common patterns).
- Strict mode (Tier 2 of the type system).
- Better error messages (every error has a help link).
- VS Code extension on the marketplace.
- Web playground at `twe-lang.org/play`.

**Exit criteria:**

- A new developer with no prior Twe experience can build a working game from the tutorial in under a weekend.
- The error-message quality survives a dogfooding session with three new users without major complaint.

---

## Phase 7 — v0.1 public release

**Duration:** 2–4 weeks of release prep.

**Goal:** Twe v0.1 is announced and available.

**Components:**

- GitHub Release with binaries for Windows, macOS, Linux.
- Installer / package manager listings.
- Project website with docs, playground, examples gallery.
- Show-HN-quality blog post and demo video.
- A clear contribution guide and governance model.

**Important:** Twe v0.1 is *not* "feature complete." It is "stable enough to recommend to others." The expectation is that v0.1 will reveal new design problems through community use.

---

## Phase 8 onward — v0.2 through v1.0

After v0.1 the work is responsive rather than scheduled. Driven by:

- Real-world bug reports.
- Performance profiling on actual games.
- The verified mode (Tier 3 of the type system).
- Native code generation (Luau-style) for additional speed.
- Module / package system for sharing libraries.
- Sandboxing for user-generated content.
- Multiplayer / determinism story.

The pacing target: minor releases every 2–3 months, with a v1.0 commitment when the language has been stable for at least six months and three serious games have shipped using it.

---

## Total estimated time to v0.1

If the project moves at the pace described:

| Phase | Weeks |
|-------|-------|
| 0 — Design | 4 |
| 1 — Tree-walker | 8 |
| 2 — Vertical slice | 6 |
| 3 — Bytecode VM + tooling | 10 |
| 4 — Type system v1 | 8 |
| 5 — 3D + dialogue | 10 |
| 6 — Tooling + docs | 8 |
| 7 — Release | 3 |
| **Total** | **~57 weeks** |

That's about 14 months of part-time work, or 6–7 months of full-time work. If history is any guide, the actual number will be 1.5x–2x this estimate. Plan accordingly.

---

## Risks and mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Solo-maintainer burnout (Wren scenario) | High | Recruit at least one collaborator before Phase 3. Document everything. |
| Scope creep (every game suggests a feature) | High | The ten examples are the spec. Reject anything not implied by them through Phase 4. |
| Type system proves harder than expected | Medium | Phase 4 is staged; non-strict ships first. Strict can slip to v0.2. |
| Engine integration is harder than language design | Medium | Use macroquad in Phase 2 specifically to defer this. Custom engine is post-v0.1. |
| LLM tooling support never materializes | Low | Twe's grammar is designed for it from day one. JSON diagnostics in Phase 3. |
| Audience indifference | Medium | The differentiators (procedural visuals, AI-friendly grammar, declarative blocks) are real. Marketing matters; budget Phase 6 properly. |

---

## When to stop

This roadmap covers ~14 months. If, at any point during Phases 1–3, the implementer realizes the language fundamentally doesn't work, **stop**. Document why. Take the lessons elsewhere. There is no shame in stopping a hobby language; there is shame in dragging it on past usefulness.

The exit conditions:

- After Phase 1, if the example programs feel forced or awkward in the working interpreter — pause, redesign, or stop.
- After Phase 2, if the vertical-slice game is harder to write in Twe than in Lua/Love2D — the language has not earned its existence. Stop or rethink.
- After Phase 3, if the bytecode VM doesn't deliver the expected performance — investigate, but consider whether Twe needs to exist alongside Lua / Luau or as an alternative.

The healthiest version of this project is one where the implementer is willing, at every phase, to abandon it. That willingness is what produces a good language.
