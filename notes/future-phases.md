# notes/future-phases.md

> Items deferred from the active phase. Capture here so we don't lose them;
> do not act on them until the active task warrants.

## Phase 1 retro (closed)

Phase 1 closed at commit `7c4c06c`. Tree-walker runs Examples 1, 2 (simplified),
and the eleven test programs in `tests/programs/`. Code totals ~3000 LOC of
Rust against the roadmap's 6000-LOC budget — leaner because the runtime-heavy
items the roadmap parked in Phase 1 (full unit-aware arithmetic, `wait`/`every`
fiber semantics, scene/dialogue runtime) cleanly migrated to later phases.

What ships:

- Lexer: triple strings, escapes, hex/binary/separator literals, ranges,
  percents, units, all keywords, INDENT/DEDENT, line and block comments.
- Parser: statements, all expressions, type annotations, declarative
  blocks with inheritance and methods, functions, control flow.
- Evaluator: tree-walks the AST, headless frame loop, full method dispatch
  with `self`, return / break / continue control flow.
- Values: nil, bool, int, float, string, percent, quantity, range, tuple,
  object, class, instance, function, builtin.
- Stdlib: print, load (sprite stub), key (input stub), math.
- CLI: `run [--frames N] <file>`, `parse <file>` (JSON dump), `version`.

## Phase 2 retro (closed 2026-04-28)

Five of six components shipped; cooperative fibers deferred per
`docs/changes/2026-04-28-fibers-deferred-to-phase-3.md`. All four
exit-criteria bullets pass: `examples/survive.twe` runs as a real
bullet-vs-monster game (~120 lines, comfortably under the 500-line
budget), hot reload works (`795fab3`), the 15-item frustration list
(`docs/changes/2026-04-28-phase-2-frustration-list.md`) drove Phase 3.

Six-component checklist:

- [x] macroquad bindings — sprite drawing, input, audio.
- [~] Coroutines / fibers — deferred (see Phase 3 retro and Phase 5
      entry plan).
- [x] Basic ECS world — `entities.of(Class)` / `entities.count(Class)`.
- [x] `particles` block — `9f78083`.
- [x] Hot reload via mtime polling.
- [x] Game stdlib — `key.*`, `screen.*`, `time.dt`, `load`, `sprite()`,
      `sound.*`.

## Phase 3 retro (closed 2026-04-29)

See `docs/changes/2026-04-29-phase-3-and-4-closeout.md` for the full
ledger. All four roadmap exit-criteria bullets pass:

- [x] Bytecode VM ships behind `--vm bytecode`; `twec play` wires through.
      Benchmarks live in `tests/bench.rs`.
- [x] `twec fmt` round-trips every test file.
- [x] Tree-sitter grammar (`tree-sitter-twe/`) parses all eleven examples.
- [x] `twec lsp` + VS Code extension ship; hover, go-to-def, inline
      diagnostics work.

Frustration-list resolutions:

- [x] F1 keyword arguments — `a7d52cc`.
- [x] F4 bounded every-clock catch-up — `14ae250`.
- [x] F5 / F8 state-scoped `on update(dt):` — `bf4bb37`.
- [x] F11 `or` / `and` keep value-returning semantics —
      `docs/changes/2026-04-28-or-and-keep-value-returning.md`.

Re-deferred (see closeout note for full reasoning):

- NaN-tagged 64-bit values → v0.2 / post-v0.1.
- Incremental tracing GC → v0.2 / post-v0.1 (rides the value-layer change).
- Cooperative fibers + `wait` → **Phase 5 entry**, before dialogue runtime.
- Computed-goto → nightly-only on Rust; vacuously satisfied by LLVM's
  match-to-jump-table lowering.

## Phase 4 retro (closed 2026-04-29)

Same closeout note. All seven roadmap components ship; `twec types`
runs cleanly on all on-disk example programs. Exit criteria:

- [~] All ten examples type-check unmodified — bottlenecked on Phase 5
      constructs (`dialogue`, `visual`, `save`, 3D, tilemap). Reopens as
      a Phase 5 dependency.
- [x] Hover shows correct types.
- [ ] Type-driven autocomplete — deferred to Phase 5 entry.
      `src/lsp.rs:7` records the deferral.

## Phase 6 retro (closed 2026-04-29)

See `docs/changes/2026-04-29-phase-6-closeout.md`. Eight sessions
shipped strict mode (full enforcement on lets / function params /
function returns / class fields / method params / method returns),
`did_you_mean` for unknown idents / fields / states, tutorial draft,
error-message polish, VS Code packaging readiness, and the
`sphere()` primitive (first v0.2 carry-over delivered into v0.1).
**427 tests pass.**

Sub-task ledger preserved here for future reference:

## Phase 6 plan (closed)

Phase 6 is "tooling, polish, documentation" per `docs/05-roadmap.md`.
Sub-tasks ordered by leverage:

1. [x] **Strict-mode type checker — session 1** (2026-04-29).
   `# strict` directive opt-in; comparison, arithmetic, return,
   call-arg unify failures surface as diagnostics in `twec types`
   + LSP. See `docs/changes/2026-04-29-phase-6-session-1-strict-mode.md`.
2. [x] **Strict-mode session 2 — annotation enforcement** (2026-04-29).
   AST keeps `Param { name, ty }` and `Stmt::Let.ty` and
   `Stmt::FunctionDecl.ret`. Strict unifies each annotation against
   the inferred type at decl time. Method-param annotations stay
   parsed-and-discarded (session 5+). See
   `docs/changes/2026-04-29-phase-6-sessions-2-3-4.md`.
3. [x] **Tutorial draft** (2026-04-29). `docs/tutorial.md` —
   ~1500-word first-hour walkthrough ending in a 3D demo. Honest
   about what v0.1 ships vs what v0.2 absorbs.
4. [x] **Error-message polish** (2026-04-29). `did_you_mean`
   helper wired into unknown-field and unknown-state-in-transition
   errors. Help text filled in on six common bare errors
   (`return`/`break`/`continue` outside their context, `self`
   outside method, tuple OOB, `every` unit missing).
5. [x] **Strict-mode session 3** (2026-04-29). `did_you_mean` for
   unknown identifiers in strict mode, plus stdlib-name seeding so
   strict programs don't error on every `print` / `vec3` /
   `math.*` call. See `docs/changes/2026-04-29-phase-6-sessions-5-6-7.md`.
6. [~] **VS Code extension packaging** (2026-04-29). README +
   `package.json` polish + `.vscodeignore` for `vsce package`.
   Marketplace publish itself rides Phase 7 (release).
7. [x] **Strict-mode session 4 — class member annotations**
   (2026-04-29). `DeclMember::Field` keeps `ty: Option<Type>`;
   `DeclMember::Method` keeps `params: Vec<Param>` and `ret`.
   Strict unifies field annotations against the value's inferred
   type, method params against fresh vars at class-shape
   registration, method returns against the body's collected
   union. See `docs/changes/2026-04-29-phase-6-closeout.md`.
8. [ ] **Tutorial iteration pass-2** — screenshots, a longer
   walkthrough (Pong or a tiny RPG end-to-end). Driven by user
   feedback once v0.1 is in real hands. → v0.2.
9. [ ] **Structural-record subtyping under strict** — when class A
   has fields {a, b, c} and a function takes a `{a, b}` record,
   strict should accept A. Real type-system work; needs design
   conversation. → v0.2.
10. [ ] **Luau "lax strict" widening rules** — call-site context
    can widen a strict function's param types when both sides are
    `?`. Luau-specific design choice; defer until users push back
    on the current strictness. → v0.2.

## Phase 5 retro (closed 2026-04-29)

See `docs/changes/2026-04-29-phase-5-closeout.md` for the full ledger.
Tasks 1–4 substantively ship; task 5 (3D backend) ships the
`twec play3d` round-trip — Twe-driven scene of cubes with input,
hot reload, and Lambertian lighting. Tasks 6 (tilemap) and 7 (save)
defer to v0.2 along with the task-5 follow-ons (`.glb` mesh import,
generic primitives, bytecode VM 3D path, `mat4`/`quat`).

Sub-task ledger preserved here for future reference:

1. [x] **LSP autocomplete** + on-disk type-check sweep — shipped 2026-04-29.
   `textDocument/completion` returns user symbols (with inferred types as
   detail), Twe keywords, and stdlib namespaces (both bare `math` and
   dotted `math.abs`). All 32 on-disk Twe programs type-check cleanly.
   Closes the Phase 4 autocomplete backlog item.
2. [~] **Cooperative fibers + `wait <duration>`** — *partially shipped 2026-04-29*.
   Both backends now support `wait` as a direct statement of a state's
   on-entry body. Tree-walker uses an instance-side resume index;
   bytecode VM (`OpCode::Wait`) saves the chunk + IP on the BcInstance
   and re-pushes the frame on resume. While suspended, the state's
   clocks and on-update pause. Outstanding work: function-body `wait`
   (needs CPS or heap-allocated call frames), fiber-backed `every`
   rewrite (current accumulator stays — no functional gap), `wait`
   inside `dialogue` (per-dialogue scheduler).
3. [~] **Dialogue runtime + UI primitives** — *partially shipped 2026-04-29*.
   Tree-walker ships `dialogue Foo:` as a parameterless callable, `say`
   (with and without actor) prints to the out buffer, `choice:` prints
   numbered labels and runs the first branch (deterministic for v0.1).
   `actor` keyword shipped as alias for `let`. Outstanding: per-dialogue
   scheduler for `wait` inside dialogue, interactive choice selection
   (UI design conversation), bytecode VM support.
4. [x] **State-machine AI predicate hooks** — *shipped 2026-04-29*.
   `on <predicate>:` handlers inside states. Edge-triggered: each
   predicate is evaluated every frame; body fires on the false → true
   transition, doesn't re-fire while stable true. Both backends
   (tree-walker uses an instance-side `Vec<bool>`; bytecode VM
   compiles each predicate to a no-arg method-shape `BcFunction`
   that returns a value). Closes Example 4's surface modulo the
   user-defined methods.
5. **3D rendering backend** — *in progress; multi-session project*.
   - [x] **Session (a) — wgpu scaffold + clear-color** (2026-04-29).
     `twec play3d <file>` opens a winit window, configures a wgpu
     surface, clears to dark teal each frame. Three new deps:
     `wgpu = "22.0"`, `winit = "0.30"`, `pollster = "0.3"`.
     See `docs/changes/2026-04-29-phase-5-task-5-session-1-wgpu-scaffold.md`.
   - [x] **Session (b) — flat-shading pipeline** (2026-04-29).
     Vertex / index / camera buffers, WGSL shader with `vs_main` /
     `fs_main`, depth attachment, back-face culling. Hardcoded cube
     stands in for `.glb` import (file loading rides session d so
     the loader API matches the Twe-side `mesh()` surface). New dep
     `bytemuck = "1"`.
     See `docs/changes/2026-04-29-phase-5-task-5-sessions-bc-cube-and-camera.md`.
   - [x] **Session (c) — camera system** (2026-04-29). Uniform-buffer
     view-projection, hand-rolled column-major matrix math
     (`perspective`, `look_at`, `rotate_y`, `mul`). Right-handed,
     +y up, depth ∈ [0, 1]. Five matrix-math unit tests. No glam
     dependency — Twe's own `vec3` / `mat4` ship in session (e).
   - [x] **Session (d) — Twe-driven 3D scene** (2026-04-29). Top-level
     `on render():` handler, `cube(at:, color:, size:)` builtin,
     `camera.eye/.target/.up` ambient fields. Instanced cube
     rendering: one draw call per frame, up to 4096 cubes.
     `examples/hello_3d.twe` orbits the camera around a ring of
     cubes. Bytecode VM 3D path, `.glb` import, winit input → `key.*`,
     and hot reload all remain as session (d) follow-ons.
     See `docs/changes/2026-04-29-phase-5-task-5-sessions-de-twe-driven-3d.md`.
   - [x] **Session (e) — vec3 + math primitives** (2026-04-29). `vec3(x, y, z)`
     constructor (Twe tuples already do component-wise +/-/* and
     `.x`/`.y`/`.z`, so vec3 is just a 3-tuple). `math.sin`, `math.cos`,
     `math.pi` added. `mat4` / `quat` deferred — no consumer in v0.1.
   - [x] **Carry-over session — input + hot reload + lighting** (2026-04-29).
     winit `KeyboardInput` → Twe `key.*` and `key_press.*` Objects;
     mtime-poll hot reload mirroring `play.rs`; per-vertex normals +
     Lambertian directional-light shading replacing the per-face
     brightness hack. `examples/hello_3d.twe` updated to drive the
     camera with WASD / arrows.
6. **Tilemap rendering + collision** — *deferred*. Should ride task
   5's renderer; doing it before 3D risks throwaway 2D-only code.
   Example 9.
7. **`save` block compiler** — *deferred — needs design conversation*.
   Format / versioning / migration story belongs in a new
   `docs/07-save-system.md` before implementation. Example 7.

Exit criterion (roadmap, original): a small 3D action-RPG (town +
3 NPCs + tilemap dungeon + boss fight) ships in under 1500 lines of
Twe. **Reachable only after tasks 5 + 6 ship.** See
`docs/changes/2026-04-29-phase-5-status.md` for the three reasonable
paths from here (schedule the rest, declare v0.1-minimum-viable,
or pivot to Phase 6 with v0.2 absorbing 3D / tilemap / save).

## Open language-design questions

### Unicode in identifiers

`06-design-document.md §2.1` permits any Unicode scalar value in identifiers;
`§2.4`'s EBNF restricts identifiers to ASCII letters, digits, and underscore.
Current lexer follows §2.4 (ASCII-only). Reconcile §2.1 or §2.4 before any
non-ASCII identifier enters the test suite.

### Loader API doc-cleanup — RESOLVED 2026-04-28

Bare `load(path)` is canonical (matches Example 1, the spec). `02-type-system.md`'s
`sprite.load(...)` form should be edited to match. Drawing is explicit:
`sprite(handle, at, [size])` inside `on render():`. Implicit auto-rendering
(the original Example 1 ideal where setting `hero.pos` is enough) remains
deferred to Phase 3+.

## v0.2 — in progress

Items are shipping incrementally on the v0.2 work track in
parallel with Phase 7 (release prep). Each session lands a
runnable artifact and gets a `docs/changes/<date>-v0.2-session-N-*.md`
note.

- [x] **Session 1 — `.glb` mesh import** (2026-04-29). `gltf = "1"`
      (default-features off) for pure-Rust glTF 2.0 binary
      decode. `Primitive::Mesh(u32)` carries an `Env`-interned
      path id; `play3d::RenderState` lazy-loads + caches each
      `GpuMesh` on first sight, fails gracefully on bad paths.
      `mesh(path: string, at: vec3, color: rgba, size: float)`
      builtin queues a draw the same way `cube` / `sphere` do.
      First-primitive-of-first-mesh is taken; multi-primitive
      scenes + node transforms + materials are follow-ons.
      `examples/hello_glb.twe` + a 536-byte fixture
      `examples/assets/triangle.glb` round-trip the loader. See
      `docs/changes/2026-04-29-v0.2-session-1-glb-import.md`.
- [x] **Session 2a — resumable `if` / `while`** (2026-04-29). First
      of three sessions on the function-body `wait` track.
      Replaces `Instance::entry_resume_index: Option<usize>` with
      `entry_resume_path: Vec<PathEntry>` so `wait` works inside
      `if` / `elif` / `else` / `while` blocks at any nesting
      depth within a state's `on_entry`. `Branch::IfElif(idx)`
      preserves the chosen elif arm across suspend / resume.
      `for` bodies + function calls remain v0.2 session 2b
      territory. See
      `docs/changes/2026-04-29-v0.2-session-2a-resumable-blocks.md`.
- [x] **Session 2b — function-body `wait`** (2026-04-29). Reifies
      the call stack as `Instance::fiber_frames: Vec<Frame>` so
      `wait` works inside a function called from a state's
      `on_entry` (and inside a function called from another
      function called from there, etc.). Frame ordering is
      bottom-to-top (state-entry at index 0; innermost call at
      `len-1`); push-before-run keeps the order correct
      regardless of when the wait fires. Restricted to
      bare-name `Stmt::Expr` calls of `Value::Function`
      callees; method-body wait, `for`-body wait, and
      call-as-expression (`let x = f()`) remain follow-ons.
      See `docs/changes/2026-04-29-v0.2-session-2b-function-body-wait.md`.
- [x] **Session 2c — VM nested-block wait parity** (2026-04-30).
      Brings the bytecode VM to 2a-equivalence: `wait` works
      inside `if` / `elif` / `else` / `while` blocks at the top
      of state on_entry. `Frame::allows_wait` flag in the
      compiler gates `OP_WAIT` emission; `OpCode::Wait` saves
      the value-stack slice (`entry_resume_locals`) so locals
      declared inside the body survive the suspension.
      Function-body wait on the VM remains deferred — it needs
      a multi-frame fiber save (`Vec<BcFiberFrame>` on
      `BcInstance`) capturing the entire call stack + each
      frame's stack slice. See
      `docs/changes/2026-04-30-v0.2-session-2c-vm-wait-parity.md`.
- [x] **Session 3 — mouse input** (2026-04-30). Three new
      ambient Objects: `mouse` (`.x`, `.y`, `.pos`, `.wheel`),
      `mouse_held.{left,middle,right}` (continuous), and
      `mouse_press.{left,middle,right}` (edge-triggered).
      Both backends (`twec play` macroquad, `twec play3d`
      winit) wire identically. Closes the Phase 8 mouse line
      item; unblocks Phase 10's UI primitives.
      `examples/mouse_demo.twe` exercises all three Objects.
      See `docs/changes/2026-04-30-v0.2-session-3-mouse-input.md`.
- [x] **Session 4 — save / load bottom layer** (2026-04-30).
      `src/save.rs` module + `save_to(path, value)` and
      `load_from(path) -> value` stdlib builtins. Operates on
      Twe's serializable subset (primitives + Percent + Range
      + Quantity + Tuple + List + Object); refuses Function /
      Class / Instance / Builtin / stdlib ambients with
      messages pointing at the offending type. Atomic write
      via write-to-`<path>.tmp` + rename. Tagged JSON shape
      so Tuple-vs-List, Percent, Range, Quantity round-trip
      losslessly. Schema-block syntax, version migration, and
      versioned binary + CRC format ride session 5+ per
      `docs/07-save-system.md`. See
      `docs/changes/2026-04-30-v0.2-session-4-save-load-bottom.md`.
- [x] **Session 5 — audio v2** (2026-04-30). `sound.*` gains
      `play_at(handle, volume)`, `stop(handle)`,
      `set_volume(handle, volume)`. New `music.*` namespace
      with `play` / `play_at` / `stop` (same underlying handles,
      `looped: true`). `SOUND_CACHE` shared across builtins.
      Pitch + mixer channels + streaming + crossfade deferred —
      quad-snd backend doesn't support them. Semantic surface
      is forward-compatible if a richer backend lands later.
      `examples/audio_demo.twe` exercises the new surface.
      See `docs/changes/2026-04-30-v0.2-session-5-audio-v2.md`.
- [x] **Session 6 — tilemap (stdlib-builtin form)** (2026-04-30).
      `tilemap(layout, tile_size, tiles)`, `tilemap_render(map, at)`,
      `tilemap_at(map, x, y)`, `tilemap_solid_at(map, x, y)`.
      Returns an Object with `width` / `height` / `tile_size` /
      `cells` / `tiles` introspection fields. Per-trait colored
      rect renderer (sprite-atlas form rides Phase 9). Closes the
      Phase 8 tilemap line item; `tilemap Name:` block syntax
      from Example 9 still pending — same runtime, follow-on
      parser pass. `examples/tilemap_demo.twe` demonstrates the
      slide-along-walls collision pattern via `tilemap_solid_at`.
      See `docs/changes/2026-04-30-v0.2-session-6-tilemap.md`.
- [x] **Session 7 — VM function-body `wait`** (2026-04-30).
      Reifies the bytecode VM's call stack on `OP_WAIT`:
      `BcInstance` gains `fiber_frames: Vec<BcFiberFrame>` +
      `fiber_stack: Vec<Value>` (replaces single-frame
      `entry_resume_*`). VM tracks `state_entry_frame_depth`
      so OP_WAIT knows how many frames to capture; resume
      replays them in order. Compiler drops the
      `Frame::allows_wait` flag — runtime gate at
      `state_entry_frame_depth` replaces compile-time
      rejection. VM now matches the tree-walker's
      function-body wait surface (session 2b). Method-body
      wait, call-as-expression wait, and `for`-body wait
      remain follow-ons on both backends.
      See `docs/changes/2026-04-30-v0.2-session-7-vm-function-body-wait.md`.

## Triage backlog

> **Canonical post-v0.1 plan: `docs/05-roadmap.md` Phases 8–16 (v0.2 → v1.0).** This file is the *triage backlog* — items that don't fit cleanly into a phase yet (sub-feature deferrals, design questions awaiting resolution, debt items, indefinite-defer items). New ad-hoc items go here; phase-defining items go to the roadmap.

### Sub-feature deferrals (ride existing roadmap phases)

These are scoped narrower than a roadmap entry. Capture is for memory.

- **Method-body `wait`** — same shape as session 2b's function-body wait but `FrameKind::Method { recv, def }` and self-binding on resume. Lands alongside Phase 8's "function-body `wait` on bytecode VM" item.
- **Call-as-expression with `wait`** (`let x = f()` where `f` waits) — needs a parent-side "expecting value at path P" annotation so resume can pipe the return value back into the parent expression evaluator. Phase 8.
- **`for`-body `wait`** — needs iterator-state preservation (`Branch::For { iter_index: usize }` on `PathEntry`). Phase 8.
- **Per-dialogue scheduler for `wait` inside `dialogue`** — Phase 9 (visuals + dialogue interactive choice).
- **Bytecode VM dialogue support** — tree-walker has dialogue today; VM doesn't. Lands with Phase 9's particle / visual runtime work, since both touch the same compile-stmt path.
- **`.obj` mesh loading** — same `Primitive::Mesh` plumbing as `.glb`; different crate (`tobj`). Off the v1.0 critical path.
- **Multi-primitive `.glb` scenes / node transforms / materials** — 3D maintenance track; not v1.0 critical.
- **Computed flat normals for `.glb` files without normals** — currently fall back to up-vector; small follow-on.
- **`plane()` 3D primitive** — two triangles, trivially the same pattern as `sphere`. Ship when wanted.
- **`mat4` / `quat` stdlib types** — 3D maintenance; needed when `entity.transform` / `camera.rotate(quat)` consume them.
- **3D lighting (point / area / shadow)** — 3D maintenance.
- **Bytecode VM 3D `on render():` codegen** — 3D maintenance.

### Open design questions

- **Save schema syntax + versioning** — `docs/07-save-system.md` is the Phase 7 prerequisite for Phase 8's `save` compiler. Migration semantics, Steam Cloud quotas, atomicity story all open until that doc lands.
- **Pause-on-focus-loss opt-out** — fibers suspend by default on window blur (Phase 10). Per-state opt-out for always-running tasks needs a syntax. `state foo: persistent`? `pause: false` field? Open in `CLAUDE.md` §"What is open".
- **Input remapping UX** — keyboard / gamepad rebind UI (Phase 10). Live-rebind vs. menu-only? Conflict resolution? Open in `CLAUDE.md` §"What is open".
- **Unicode in identifiers** — `06-design-document.md §2.1` permits any Unicode scalar; `§2.4`'s EBNF restricts to ASCII. Lexer follows §2.4. Reconcile before any non-ASCII identifier enters the test suite.
- **Phase 4 ten-example type-check sweep** — partially closed: every on-disk Twe program type-checks. The `docs/01-examples.md` programs using `dialogue` / `visual` / `save` / 3D / `tilemap` / predicate hooks reopen as those constructs land in their respective phases.

### Resolved (kept for history)

- **Loader API** — RESOLVED 2026-04-28. Bare `load(path)` is canonical.
- **`sprite.load(...)` form in `02-type-system.md`** — RESOLVED with the loader-API decision.
- **Tutorial iteration pass-2** — superseded by Phase 14's tutorial v2.
- **Strict-mode type checker** — shipped Phase 6.
- **`.glb` mesh import + `mesh()` builtin** — shipped v0.2 session 1.
- **`sphere()` 3D primitive** — shipped Phase 6 session 7.
- **Function-body `wait` (tree-walker)** — shipped v0.2 session 2b.
- **VM nested-block `wait`** — shipped v0.2 session 2c.

### Indefinite deferrals (out of v1.0 scope)

Per `docs/05-roadmap.md` "What's intentionally not in the v1.0 plan":

- **Native code generation** (Luau-style).
- **Multiplayer / determinism**.
- **User-defined generics** (Principle 2 conflict).
- **Macros / metaprogramming** (locked in `CLAUDE.md`).
- **Sandboxing for UGC**.
- **Workshop / mod APIs**.
- **Roblox-class 3D** (textures, animation, physics, complex scene graphs).

### Language-shape items (out-of-band, no phase yet)

These would each be their own design pass. Listed for completeness:

- **List comprehensions** (Snake NP3 — defer until 5+ users want them).
- **`on enter:` / `on exit:` state hooks** (Snake NP9 — defer until 3+ examples want them).
- **String interpolation `\u{...}` Unicode escapes**.
- **Compound unit literals** (`5 m/s`).
- **`set of T` type literal** (`{1, 2, 3}` and `set()` for empty).
- **`match` expressions** (`docs/06-design-document.md §3.6`).
- **`map` literals** (`{ "k": v }`, `docs/06-design-document.md §3.5`).
- **List slicing** (`[i:j]`).
- **Tuple-typed list elements explicitly annotated** (Snake NP10).
- **`function` return-type runtime checking** (currently parsed-and-strict-checked but not runtime-checked).

## Tooling debt

- **Pick a license.** `README.md` says "TBD: MIT or Apache-2.0". Phase 7 release-prep item.
- **`docs/02-type-system.md` and `docs/05-roadmap.md`** mention
  `sprite.load` while `docs/01-examples.md` Example 1 uses bare `load`.
  Reconcile during Phase 7 docs honesty pass.
- **README.md "Status" section is stale.** Claims tree-walker / vertical-slice / bytecode VM / tooling / v0.1 are unchecked when they're shipped. Phase 7 README polish.
- **`docs/01-examples.md` claims `visual` and `particles` ship in v0.1.** They don't (`particles` parses, runtime no-ops; `visual` has no parser support). Phase 7 docs honesty fix demotes both to v0.3 (per `docs/05-roadmap.md` Phase 9).
