# 2026-04-29 — Phase 5 closeout

## Status: closeout note. Closes Phase 5 at the v0.1-minimum-viable surface and enumerates the residual work for v0.2.

## Background

Phase 5 (3D + scenes + dialogue) opened on the same day that Phases 3 and 4 closed (`docs/changes/2026-04-29-phase-3-and-4-closeout.md`) and ran across roughly a dozen sessions. The roadmap budgeted 8–12 weeks for the phase; the substantive surface shipped in a much shorter timespan because the implementer scoped each session aggressively and kept follow-on items in `notes/future-phases.md` rather than letting any single session balloon.

This note draws the line between "shipped in v0.1" and "deferred to v0.2." Path 2 from the original Phase 5 status note (`docs/changes/2026-04-29-phase-5-status.md`) — declare v0.1-minimum-viable, push 3D / tilemap / save remainders to v0.2 — becomes the official framing.

## Closed in v0.1

### Task 1 — LSP autocomplete

`textDocument/completion` returns user symbols (with inferred types as detail), Twe keywords, and stdlib namespaces (both bare `math` and dotted `math.abs`). Closes the last Phase 4 backlog item.

### Task 2 — cooperative fibers + `wait`

State on-entry `wait <duration>` ships in **both backends**. Tree-walker uses an instance-side resume index; bytecode VM (`OpCode::Wait`) saves the chunk + resume IP on `BcInstance` and re-pushes the frame on resume. While suspended, the state's clocks + on_update pause.

**Deferred to v0.2:** function-body `wait` (needs heap-allocated call frames or compile-time CPS rewrite), fiber-backed `every`-clock rewrite (no functional gap — current accumulator passes all tests).

### Task 3 — dialogue runtime

Tree-walker ships `dialogue Foo:` as a parameterless callable, `say [<actor>:] <text>` printing to the out buffer, `choice:` with deterministic first-branch selection, and `actor` keyword as alias for `let`.

**Deferred to v0.2:** per-dialogue scheduler so `wait` inside dialogue suspends correctly, interactive choice selection (needs UI design conversation), bytecode VM dialogue support.

### Task 4 — state-machine AI predicate hooks

`on <predicate>:` handlers inside state bodies. Edge-triggered: false→true transition fires the body, stable-true doesn't re-fire. Both backends. Closes Example 4's surface.

### Task 5 — 3D rendering backend (sessions a–e + carry-over)

Five-session arc plus a follow-on session for input + hot reload + lighting:

- **(a)** wgpu scaffold + clear-color window via `twec play3d`.
- **(b)** Vertex / index / camera buffers, WGSL flat-shading pipeline, depth attachment.
- **(c)** Camera system — uniform-buffer view-projection, hand-rolled column-major matrix math (no glam dep).
- **(d)** Twe-driven scene — top-level `on render():`, `cube(at:, color:, size:)` builtin, mutable `camera.eye/.target/.up` ambient fields, instanced rendering up to 4096 cubes/frame.
- **(e)** `vec3(x, y, z)` constructor, `math.sin` / `math.cos` / `math.pi`. (Twe tuples already do component-wise +/-/* and `.x`/`.y`/`.z`, so vec3 is just a constructor.)
- **Carry-over (this session):** winit input → Twe `key.*` (WASD / arrows / space / escape / etc. reach the script in `play3d`); mtime-poll hot reload (edit the file, save, the window picks up changes without restart); Lambertian directional-light shading via per-vertex normals (replaces the per-face brightness hack).

`examples/hello_3d.twe` is the canonical demo: a central white cube ringed by six colored cubes, lit by a sun, controllable via WASD with hot reload.

**Deferred to v0.2:** `.glb` / `.obj` mesh import, generic `mesh()` / `sphere()` / `plane()` primitives, bytecode VM 3D path, `mat4` / `quat` types in the stdlib, proper light primitives (point / area / shadows), the third-person follow-camera Example 8 specifies.

### Phase boundaries clarified

Each Phase 5 session ended with a closeout note in `docs/changes/`. The pattern that emerged:

1. What shipped — bulleted against the roadmap surface.
2. What slipped — with reasons and target re-entry phase.
3. Doc edits applied as a result.

That pattern is now standard practice. It's the only mechanism that kept this phase honest across so many sessions.

## Deferred to v0.2

### Task 5 follow-ons (3D system fleshing out)

- **`.glb` / `.obj` mesh import.** Needs a crate-choice conversation. `gltf` is the standard but heavy; `easy-gltf` is lighter; OBJ-first via a 50-line hand-rolled parser is the lightest path. Should ride the user's first real asset rather than picking blind.
- **Generic primitives**: `sphere()`, `plane()`, eventually `mesh(geom: ...)`. Sphere needs UV-sphere generation; plane is two triangles; mesh waits on the file-format decision.
- **Bytecode VM 3D path**. Mirrors how the dialogue / state-machine work was done — a session that wires per-frame interpreter calls into `play3d`'s loop. Likely 1–2 sessions.
- **`mat4` / `quat` types in stdlib**. Land when something consumes them — `entity.transform`, `camera.rotate(quat)`, etc.
- **Proper lighting**. Multiple lights (point + directional + ambient), maybe a `light()` builtin and a `lights[]` registry alongside the cube queue. Shadow mapping is a separate session beyond that.
- **Third-person / first-person / free camera modes**. The `camera` Object today only carries eye/target/up; mode-switching deferred to when consumers care.

### Task 6 — tilemap rendering + collision

Should ride task 5's `.glb` / mesh decisions — Example 9 is described as a 3D dungeon, so a 3D tilemap (extruded grid of meshes) makes more sense than 2D-only tilemap code that gets thrown away. Schedule after task 5's mesh import.

### Task 7 — `save` block compiler

Explicitly needs a design conversation before code. Open questions: serialization format (JSON? Postcard? Custom binary?), versioning model (per-block tags? Per-file?), migration mechanics (declarative? Code-driven?), entity-reference round-trips, fiber-state preservation. Belongs in a new `docs/07-save-system.md` first.

## What this means for v0.1 release readiness

Twe v0.1 is a working language for:

- **2D state-driven games** (Phase 2 vertical slice — Survive, Snake — still runs). State machines, predicate hooks, `every` clocks, `on update(dt):`, hot reload, macroquad-backed rendering and audio.
- **Dialogue + branching** (Phase 5 task 3). Tree-walker only; the surface is enough to author Example-3-style dialogue routines, deterministic choice for testing.
- **Cooperative fibers in state bodies** (Phase 5 task 2). Both backends.
- **3D scenes with cubes, camera, input, hot reload, lighting** (Phase 5 task 5). One primitive (`cube`), but a complete round-trip from Twe to wgpu.
- **Type system v1** (Phase 4) — non-strict gradual inference, dimensional units, optional / union types, LSP hover + autocomplete.
- **Tooling** (Phase 3) — `twec fmt`, tree-sitter grammar, basic LSP, tree-walker + bytecode VM.

What v0.1 doesn't ship:

- **3D mesh import** — programs are limited to compositions of cubes.
- **Tilemap** — Example 9 not runnable until v0.2.
- **Save / load** — Example 7 not runnable until v0.2.
- **NaN-tagged values + tracing GC** (Phase 3 deferral) — bytecode VM is correct but not the 5–20× target speedup. v0.2 work.
- **Strict mode + verified mode** — Phase 6 / future-phases work.

## Phase 6 entry

Phase 6 is "tooling, polish, documentation" per the roadmap. The natural openers:

1. **Strict mode** — turning the type system from "don't surface false positives" into "annotate `strict mode` and surface every unification failure." Phase 4 shipped the inference engine; strict mode is the same engine with a different reporting policy.
2. **Tutorial draft** — the Rust Book equivalent for Twe. Long-form, walks through building a small game from scratch. Surfaces every paper cut in the language design.
3. **Error message polish** — every error gets a help link / a "did you mean" / a fix suggestion. Per the "no silent footguns" principle.
4. **VS Code extension on the marketplace** — the LSP exists; packaging + release is a separate task.

Or — alternatively — schedule the v0.2 work (mesh import, tilemap, save) as a parallel track and defer Phase 6 until those land.

## Doc edits applied as a result

- `CLAUDE.md` Phase 5 status updated to "closed at v0.1-minimum-viable; v0.2 absorbs 3D mesh import, tilemap, save."
- `docs/05-roadmap.md` Phase 5 §"Status" reflects closeout, lists the v0.2 carry.
- `docs/01-examples.md` gets a banner: Examples 7, 8 (full), 9, 10 are v0.2 targets and don't run on v0.1. (Examples 8 partial — basic 3D camera works; full third-person follow + mesh loading don't.)
- `notes/future-phases.md` consolidates the v0.2 carry list and removes the "Phase 5 task" granularity (the phase is closed).
