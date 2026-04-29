# 2026-04-29 — Phase 5 status

## Status: progress / scope note. Records what shipped across four Phase 5 sessions and what remains as discrete future sessions.

## Background

Phase 5 (3D + scenes + dialogue) opened on 2026-04-29 immediately after the Phase 3+4 closeout (`docs/changes/2026-04-29-phase-3-and-4-closeout.md`). The roadmap allocates 8–12 weeks for the phase. Across this date's sessions we shipped a substantive subset of tasks 1–4. Tasks 5–7 are explicitly out of scope for in-session work — each is a multi-week project on its own and must be scheduled separately.

This note is the scope ledger for "where Phase 5 actually is."

## Tasks 1–4 — what shipped

### Task 1: LSP autocomplete (closed)

`textDocument/completion` returns user symbols (with inferred type as detail), Twe keywords, and stdlib namespaces (both bare `math` and dotted `math.abs`). All on-disk Twe programs type-check cleanly. Closed the last Phase 4 backlog item the same day Phase 5 opened.

### Task 2: cooperative fibers + `wait` (substantively shipped, scoped)

State-body `wait` ships in **both backends**:

- **Tree-walker:** new `run_state_entry` ([eval.rs:657](src/eval.rs#L657)) walks `state.on_entry` resumably; on `Stmt::Wait` it stores a resume index + remaining seconds on the instance and returns. `tick_scene` decrements the timer and resumes when zero.
- **Bytecode VM:** new `OpCode::Wait` ([bytecode.rs:218](src/bytecode.rs#L218)). Compiler `compile_on_entry` ([compiler.rs:1469](src/compiler.rs#L1469)) intercepts top-level `Stmt::Wait` and emits the duration as a `Float` constant + `OP_WAIT`. The dispatch loop saves the chunk + resume IP + remaining seconds on `BcInstance`, then collapses the frame the same way `Return` does. `tick_scene`'s prelude re-pushes the frame and continues dispatch from the saved IP.

While suspended the state's clocks + on_update pause — the state is "asleep." Outside a state's on-entry, `wait` is a clear runtime error (tree-walker) or compile error (bytecode VM).

**Deferred (not in scope for v0.1):**

- Function-body `wait` — needs heap-allocated call frames or compile-time CPS rewrite. Each is a separate session. v0.2 candidate.
- Fiber-backed `every` rewrite — current per-state accumulator passes all every-clock tests including the catch-up cap. Pure refactor with no functional gap; deferred.

### Task 3: dialogue runtime (substantively shipped, scoped)

Tree-walker ships:

- `dialogue Foo:` — top-level declaration; registers the body as a parameterless `Value::Function` so calling `Foo()` runs the body via the existing function-call path.
- `say <text>` and `say <actor>: <text>` — print to the out buffer. Actor expression's display: `Instance` shows class name, string shows itself, anything else falls back to `display`.
- `choice:` with indented `<label>:` branches. Prints each label numbered `[1]`, `[2]`, …, then runs the **first** branch's body. Deterministic for v0.1 testing.
- `actor x = expr` — alias for `let x = expr` (token-level, dispatched to `parse_let`).
- Field access fix: `random.choice(list)` continues to work post-`choice`-keyword via a `keyword_spelling` helper that accepts any keyword's source spelling after `.`.

**Deferred (not in scope for v0.1):**

- Bytecode VM dialogue support — compile-time error points at `--vm tree`. The per-dialogue scheduler (below) ships first; bytecode follows.
- Per-dialogue scheduler — `wait` inside a dialogue body errors with the same "wait outside state on_entry" message every other non-state-entry context produces. A scheduler that suspends a dialogue at a `wait` (separate from the per-instance state-entry suspension) is needed to make Example 3's `wait 0.5s` between dialogue lines work. Deferred to a Phase 5 follow-on or v0.2.
- Interactive choice selection — needs UI design conversation (input modality, prompt rendering). Deterministic first-branch is enough to ship the engine.

### Task 4: state-machine AI predicate hooks (closed)

`on <predicate>:` handlers inside state bodies. Edge-triggered: the runtime evaluates each predicate every frame, compares against the per-instance last value, fires the body on a false → true transition. Both backends:

- **Tree-walker:** `StateDef.on_predicates: Vec<PredicateHandlerDef>`, `Instance.predicate_last_values: Vec<bool>`. `tick_scene` evaluates predicates after on_update, before clocks.
- **Bytecode VM:** Each predicate compiles to a no-arg method-shape `BcFunction` whose chunk evaluates the expression and `OP_RETURN`s the value. The body is its own `BcFunction`. `BcStateDef.on_predicates: Vec<(Rc<BcFunction>, Rc<BcFunction>)>`. Tick loop invokes the predicate, checks `is_truthy`, fires body on edge.

Initial last-value is `false` so a predicate that's already true on the first tick after entry fires immediately — matches game-state-machine intuition while staying technically edge-triggered.

Closes Example 4's `on player.within(awareness):` / `on hp < 20%:` surface modulo the user-defined methods (`player.within(...)` etc.), which remain user code.

## Tasks 5–7 — explicitly NOT in scope for in-session work

These each require multi-session commitment and design conversations. Listed here so they don't disappear, with reasons for the deferral:

### Task 5: 3D rendering backend (deferred — multi-week project)

The roadmap's largest single task. Requires:

- wgpu (or alternative) integration into the macroquad-driven runtime, possibly as a parallel rendering path or as a replacement.
- Shader pipeline with vertex + fragment + (later) compute stages.
- Mesh loading from `.glb` / `.gltf` (need an asset format decision).
- Camera system: third-person, first-person, free, with smoothing — Example 8's surface.
- 3D math primitives in stdlib (`vec3`, `quat`, transformation matrices).
- Coordinate-system convention (left-handed vs right-handed, y-up vs z-up).

Each bullet is its own session. The roadmap's 8–12 week budget for Phase 5 is mostly this task. Recommend scheduling as a series of sessions: (a) wgpu scaffold + clear-color, (b) mesh import + flat shading, (c) camera system, (d) integration with `entity` for spawning meshes, (e) 3D math stdlib.

### Task 6: tilemap rendering + collision (deferred)

Example 9's surface. Requires either:

- 2D rendering on top of the macroquad backend (smaller scope, but a separate concept from Twe's existing entity / particle systems).
- Or 3D tilemap that rides task 5's renderer — preferred since Example 9 is described as a 3D dungeon in the integration test (Example 10).

The right order is task 5 first, then task 6 reuses the renderer. Doing tilemap before 3D risks throwaway 2D-only code.

### Task 7: `save` block compiler (deferred — needs design conversation)

Example 7's surface. The locked decisions don't pin down:

- Serialisation format (JSON? Custom binary? Postcard?).
- Versioning model — schema-per-file vs version-tag-per-block.
- Migration mechanics — declarative migrations vs code-driven.
- Round-trip guarantees for entity references and active fibers.

This wants a design-doc round before implementation, not a coding session. Recommend producing a `docs/07-save-system.md` first, then implementing.

### Other follow-ons captured in `notes/future-phases.md`

- **Function-body `wait`** (task 2 follow-up): heap-allocated call frames or CPS rewrite. Multi-session.
- **Per-dialogue scheduler** (task 3 follow-up): suspend a dialogue at `wait` separately from per-instance state-entry suspension. ~1 session.
- **Bytecode VM dialogue support** (task 3 follow-up): mirror the tree-walker's dialogue/say/choice via the existing function-call path. ~1 session.
- **Interactive choice selection** (task 3 follow-up): needs UI design conversation.
- **NaN-tagged values + tracing GC** (Phase 3 leftovers): v0.2 work.

## Where Phase 5 actually stands

Phase 5 exit criterion in `docs/05-roadmap.md`: *"a small 3D action-RPG (town + 3 NPCs + tilemap dungeon + boss fight) ships in under 1500 lines of Twe."*

That criterion **cannot** be met without tasks 5 and 6 (3D + tilemap). What we have is:

- The non-rendering side of Phase 5 substantively shipped: state machines with predicate hooks, fibers in state bodies, dialogue routines, type system + autocomplete.
- A working language for *2D* games with state-machine AI, dialogue, and pause-on-wait.
- A clear runway to ship the renderer when those sessions are scheduled.

Three reasonable paths from here:

1. **Schedule tasks 5–7 as discrete future sessions** and finish Phase 5 as planned (roadmap budget).
2. **Declare Phase 5 v0.1-minimum-viable** at this surface — Phase 5 ships without 3D / tilemap / save; those move to v0.2. Update `01-examples.md` to note that Examples 7, 8, 9, 10 land in v0.2.
3. **Pivot to Phase 6** (tooling polish, strict mode, docs) and let v0.1 ship as a 2D-only language. Phase 5's 3D work happens in parallel on a separate branch or post-v0.1.

Path 1 is the most ambitious, path 3 is the lowest-risk for shipping v0.1. Path 2 is the honest middle.

## Doc edits applied as a result

- `CLAUDE.md` Phase discipline updated to reflect tasks 1–4 partial-ship and tasks 5–7 deferral.
- `docs/05-roadmap.md` Phase 5 §"Status" line points here.
- `docs/06-design-document.md` §4.6 (fiber implementation status), §4.10a (dialogue runtime), §4.8 (state machines / predicates) updated.
- `notes/future-phases.md` task list reflects substantively-shipped vs deferred.
