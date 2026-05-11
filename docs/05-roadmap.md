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

**Status:** closed. See `notes/future-phases.md` "Phase 1 retro" — runs Examples 1, 2 (simplified), and the in-tree test programs at ~3000 LOC of Rust.

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

**Status:** closed 2026-04-28. Five of six components shipped; cooperative fibers deferred per `docs/changes/2026-04-28-fibers-deferred-to-phase-3.md`. All four exit-criteria bullets pass — `examples/survive.twe` runs (~120 lines), hot reload works, the 15-item frustration list at `docs/changes/2026-04-28-phase-2-frustration-list.md` is the input for Phase 3.

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

**Status:** closed 2026-04-29. See `docs/changes/2026-04-29-phase-3-and-4-closeout.md` for the full ledger. All four exit-criteria bullets pass; F1 / F4 / F5+F8 / F11 frustrations resolved; bytecode VM, `twec fmt`, tree-sitter grammar, and basic LSP all ship. NaN tagging, incremental tracing GC, computed-goto, and cooperative fibers were re-deferred — fibers move to Phase 5 entry; NaN tagging + GC move to v0.2 / post-v0.1; computed-goto requires nightly Rust and is satisfied vacuously by LLVM jump-table lowering.

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

**Status:** closed 2026-04-29. See `docs/changes/2026-04-29-phase-3-and-4-closeout.md`. All seven components ship: type AST, HM unification, function-body constraint solving, structural class shapes, Optional / Union from multi-return, dimensional unit checking, LSP hover. `twec types` runs cleanly on every on-disk Twe program (5 examples + 27 test programs). Exit criteria: hover passes; type-driven autocomplete shipped on the Phase 5 entry session (same day); "all ten examples type-check unmodified" partially closes (every on-disk program passes) and reopens for the seven Phase-5-blocked programs as those constructs land.

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

**Status:** closed at v0.1-minimum-viable on 2026-04-29. See `docs/changes/2026-04-29-phase-5-closeout.md` for the full ledger. Tasks 1–4 (LSP autocomplete, state-body fibers in both backends, dialogue runtime, predicate hooks) substantively ship. Task 5 (3D backend) ships across sessions (a)–(e) plus a carry-over session for input + hot reload + lighting — `twec play3d examples/hello_3d.twe` runs a Twe-driven scene of cubes the user controls with WASD, lit by a Lambertian directional sun, with mtime-poll hot reload. Tasks 6 (tilemap) and 7 (save schemas) defer to v0.2 along with task-5 follow-ons: `.glb` mesh import, generic primitives, bytecode VM 3D path, `mat4`/`quat`, proper lighting, mouse input.

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

**Status:** closed 2026-04-29 per `docs/changes/2026-04-29-phase-6-closeout.md`. Eight sessions: strict-mode reporting policy (1), annotation enforcement on lets / function params / function returns (2), tutorial draft (3), error-message polish + `did_you_mean` helper (4), strict-mode unknown-identifier diagnostics (5), VS Code packaging readiness (6), `sphere()` primitive + per-primitive instanced render path (7), class field / method annotation enforcement (8). 427 tests pass. Structural-record subtyping + Luau lax-strict widening + tutorial iteration pass-2 deferred to v0.2. Marketplace publish + binaries + website + blog post are Phase 7.

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

**Status:** active as of 2026-04-29 (Phase 6 closed). Mostly non-code work: release engineering, packaging, writing, and the marketplace push. The codebase itself has the v0.1 surface; Phase 7 makes it discoverable.

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

## Post-v0.1 — the v1.0 plan

After v0.1 cuts, Twe is "stable enough to recommend to others" but **not stable enough to ship a paid game on**. Phases 8–16 close that gap. The thesis: **v1.0 means a developer can ship a Vampire-Survivors-class commercial 2D game on Steam using Twe.**

Use case #1 from `README.md` ("2D systematic / RPG hybrid") drives prioritization. The 3D commercial arc — Phases 17–26, documented in `docs/3d-roadmap.md` and the four closeout notes under `docs/changes/2026-05-07-…` — runs as a parallel track and closes alongside v1.0, shipping rapier3d physics, glTF 2.0 multi-node scenes, GPU skinning, shadow maps, and HDR + ACES tone mapping. Roblox-class 3D remains multi-year and out of scope; Tunic-scale open-world is now scheduled as Phase 32 (post-v1.0). See `docs/changes/` for closeout notes that record what slipped from each phase and why.

---

## Phase 8 — v0.2 — Foundations for shipping

**Status:** **closed 2026-05-04** per `docs/changes/2026-05-04-phase-8-closeout.md`. All three roadmap exit criteria met. Seven feature sessions shipped 2026-04-29 / 04-30:

| # | Surface | Session note |
|---|---------|--------------|
| 1 | `.glb` mesh import | `docs/changes/2026-04-29-v0.2-session-1-glb-import.md` |
| 2a | Resumable `if` / `while` blocks | `docs/changes/2026-04-29-v0.2-session-2a-resumable-blocks.md` |
| 2b | Function-body `wait` on tree-walker | `docs/changes/2026-04-29-v0.2-session-2b-function-body-wait.md` |
| 2c | VM nested-block wait parity | `docs/changes/2026-04-30-v0.2-session-2c-vm-wait-parity.md` |
| 3 | Mouse input (both backends) | `docs/changes/2026-04-30-v0.2-session-3-mouse-input.md` |
| 4 | Save / load bottom layer | `docs/changes/2026-04-30-v0.2-session-4-save-load-bottom.md` |
| 5 | Audio v2 (volume + music + stop) | `docs/changes/2026-04-30-v0.2-session-5-audio-v2.md` |
| 6 | Tilemap (stdlib-builtin form) | `docs/changes/2026-04-30-v0.2-session-6-tilemap.md` |
| 7 | VM function-body `wait` (multi-frame fiber) | `docs/changes/2026-04-30-v0.2-session-7-vm-function-body-wait.md` |

**Phase 8.5 — NaN tagging + tracing GC** breaks out as its own sub-phase per `docs/08-nan-tagging.md`. It's the last open Phase 8 line item but XL on its own (4–8 weeks of focused work, 9 sub-sessions 8a–8i). Sequenced separately because attempting it inside a single Phase 8 close-out would either skip GC entirely or break the migration.

**Theme:** close the load-bearing gaps that block any real game from running at all. v0.2 is *not* about polish — it's about the absence of features that make a Survivors-class game impossible.

**Components:**

- Tilemap rendering + collision (Example 9; `tilemap` block runtime).
- `save` block compiler (Example 7), built against `docs/07-save-system.md`'s schema (designed in Phase 7 as a v0.2 prerequisite).
- Mouse input: `mouse.x`, `mouse.y`, `mouse_press.<button>`, `mouse_held.<button>`.
- Function-body `wait` on the bytecode VM — multi-frame `Vec<BcFiberFrame>` save (the deferred half of session 2c; tree-walker already has it via session 2b).
- Audio v2: `sound.play(handle, volume:, pitch:)`, `music.play(handle, loop:)`, mixer channels, fade-in / fade-out.
- **NaN-tagged 64-bit values + incremental tracing GC** — *deferred to Phase 8.5* per `docs/08-nan-tagging.md`. Sub-phase because the migration is genuinely 9 sessions of careful work; rolling it into Phase 8's close-out would either skip GC or break the migration.

**Exit criteria** (Phase 8 proper):

- Example 7 (save/load, layer 1) and Example 9 (tilemap, stdlib form) run on both backends. **Met** as of session 6 (tilemap) / session 4 (save).
- Function-body `wait` works on both backends. **Met** as of session 7.
- Mouse + audio v2 surfaces shipped. **Met** as of sessions 3 + 5.

Phase 8.5 inherits the runtime perf criteria:

- `cargo bench` shows ≥3× tree-walker speedup over the bytecode VM with NaN tagging vs. the pre-tag VM.
- A 1k-entity 60fps stress test produces no visible GC pauses.

---

## Phase 8.5 — NaN tagging + tracing GC

**Status:** in progress. Sessions 8a–8e shipped 2026-04-30 (storage migration done — every value-typed slot in vm/eval/stdlib now lives in `TaggedValue` form). Sessions 8f–8i remain (legacy enum deletion + GC).

**Theme:** rebuild the runtime value representation so the bytecode VM's hot loop isn't dominated by Rc-refcount churn, and so closing cycles (`obj.field = obj`) doesn't leak.

**Components** (each its own session per `docs/08-nan-tagging.md` "Migration sequencing"):

- ✅ 8a — `TaggedValue` module + encode/decode + round-trip tests.
- ✅ 8b — `HeapObject` header + body discriminator.
- ✅ 8c — VM migration: `stack: Vec<Value>` → `Vec<TaggedValue>`; `globals` similarly; full-Value shim.
- ✅ 8d — Tree-walker migration: `Env::bindings` + `Instance::fields` on TaggedValue.
- ✅ 8e — Stdlib + save migration: `Object::fields` + `BcInstance::fields` on TaggedValue; stdlib bootstrap helpers; save shimmed.
- 8f — Delete legacy `Value` enum (~917 `Value::` pattern-match sites to rewrite).
- 8g — Heap allocator + stop-the-world mark + sweep.
- 8h — Roots wiring (VM + eval + fiber frames).
- 8i — Bench against pre-migration baseline; tune.

**Exit criteria:**

- All 475+ existing tests pass against the new value layer.
- A `tests/programs/cycle_collector.twe` program builds an `obj.field = obj` cycle and confirms it's collected after a `gc.full()` call.
- `cargo bench` reports ≥3× speedup on the existing benchmarks vs. pre-tag VM.

**Realistic calendar size:** 4–8 weeks of focused part-time work. Counted as "L" in the size table below and explicitly multi-session in `docs/08-nan-tagging.md`.

---

## Phase 9 — v0.3 — Visuals + assets-for-UI

**Status:** **closed 2026-05-04** per `docs/changes/2026-05-04-phase-9-closeout.md`. Session 11 (2026-05-02) closed the exit gate: `twec play_visual examples/visual_fire.twe` opens a wgpu window and renders Example 5's procedural fire shader fullscreen, driven by a system-clock time uniform. The WGSL emitted by session 10 validates through naga (wgpu's WGSL frontend) in CI without needing a GPU. Phase 9 shipped 11 sessions covering math (1), 2D camera (2), atlases (3), fonts (4), gamepad (5), color pipeline (6), particles doc-honesty (7), death event hook (7b), visual parser (8), visual subset checker (9), visual → WGSL codegen (10), and visual render integration (11). Two sub-criteria slipped to ≤1-session follow-ons: real spritesheet-animation demo (asset bundling) and `examples/survive.twe` gamepad integration. The bytecode-VM mirror of the death-event hook also defers (mid-size session). **544 tests pass.**

**Theme:** ship the headline differentiator (Pillar 3 from `README.md`) and the asset machinery UI in Phase 10 needs.

**Components:**

- `visual` block → WGSL fragment-shader compilation. Example 5 (procedural fire), finally. Subset of math + vector + color stdlib usable inside `visual` bodies. **Status (closed 2026-05-02):** sessions 8 + 9 + 10 + 11 all shipped. **Session 8** wired the parser surface. **Session 9** added `crate::visual_check` — a subset typechecker rejecting allocating expressions, unbounded loops, mutation, event handlers, and any call outside the GPU-safe whitelist. **Session 10** added `crate::visual_wgsl` — the codegen that turns a `visual <Name>:` block into a complete WGSL module: vertex shader emitting a fullscreen quad via the vertex_index trick, fragment shader calling the compiled `twe_pixel(uv, time)`, custom `noise()` helper that bit-matches the CPU `value_noise_2d` (same Wang hash + golden-ratio offset), and inlined `vec4<f32>` literals for `color.<named-constant>` reads. Integer literals always emit as `f32` so they unify with vector arithmetic without per-call type analysis. WGSL builtins handle `smoothstep` / `mix` / `math.sin` / `math.cos` / etc. directly. **Session 11** added `crate::play_visual` — wgpu render driver. `twec play_visual <file>` opens a window, builds a render pipeline from the compiled WGSL, drives a `time: f32` uniform from the system clock, draws a fullscreen-quad pass each frame. Hot reload re-builds the pipeline on file change. naga validation in CI proves the emitted WGSL parses + validates without needing a GPU.
- `particles` runtime against the parsed `particles` block. Example 6 (particle burst), finally. `on_spawn` / `on_update` lifecycle, `p.age_ratio` implicit field, global `on enemy.death(e)` event glue. **Status (2026-05-02 / Phase 9 session 7 doc-honesty pass):** lifecycle + `age_ratio` + emitter despawn ship on **both** the tree-walker and bytecode VM (the VM port landed in a prior session and went undocumented). `spawn ExplosionBurst at e.pos` syntax also ships. The remaining gap is the global `on enemy.death(e)` event hook — that's a parser/AST/dispatch addition scheduled as a Phase 9 follow-on session (call it 7b in the session sequencing).
- Texture atlas + spritesheet loading: `load_atlas("walk.png", grid: (32, 32))`, `sprite(handle, frame: 3, ...)`.
- TTF / OTF font loading. Replaces macroquad's default font.
- 2D camera primitive: `camera.follow(entity, lerp:)`, `camera.shake(amplitude:, duration:)`, `camera.zoom`.
- Color pipeline: HDR-aware blending, gamma-correct compositing.
- Gamepad input: analog axes (left / right stick + triggers), button mapping table.

**Exit criteria:**

- Example 5 and Example 6 run end-to-end.
- A spritesheet-driven character animation demo ships in `examples/`.
- Gamepad + keyboard work interchangeably for `examples/survive.twe`.

---

## Phase 10 — v0.4 — UI + game-shell primitives

**Status:** **closed 2026-05-04** per `docs/changes/2026-05-04-phase-10-closeout.md`. All eleven sessions shipped:

| # | Surface |
|---|---------|
| 1 | `button` |
| 2 | `label`, `progress_bar` |
| 3 | `slider` |
| 4 | `checkbox`, `dropdown` |
| 5 | `text_input` |
| 5b | clipboard (`os.clipboard.read/write`, Ctrl+V paste in `text_input`) |
| 6 | `panel`, `stack`, `flex` |
| 7 | `grid`, `scroll` |
| 8 | `pause(flag)` / `is_paused()` |
| 9 | settings system (`settings.set/get/has/set_default/save/load/try_load`) |
| 10 | localization scaffolding (`lang.set_locale/locale/load/t/tf`) |
| 11 | exit gate — `key_input` widget + `key_held(name)` / `key_pressed(name)` + `examples/pause_menu_demo.twe` (resume / save / quit, multi-locale) + `examples/keybind_demo.twe` (key rebind UI) + `examples/survive.twe` rebound to read keys from `settings` |

**All three exit criteria met.** Complete pause menu in `examples/`, settings round-trip across launches via the v0.2 save layer, and `examples/survive.twe` rebinds keys at runtime through the `key_input`-based UI in `examples/keybind_demo.twe`. **583 tests pass.** The if-expression form `let x = if cond: a else: b` was a Phase 9 follow-on rolled into this track (closes the `examples/gamepad_demo.twe:9` latent bug). Auto-pause-on-window-blur slipped (macroquad 0.4 has no public focus-event API; needs a winit-integration follow-on). Per-state opt-out (`pause: false` / `state foo: persistent`) remains an open syntax question.

Runs in parallel with Phase 9 once fonts + atlases land. **Theme:** everything a Steam pause menu needs.

**Components:**

- Layout primitives: `panel`, `flex`, `grid`, `scroll`, `stack`.
- Widgets: `button`, `label`, `slider`, `checkbox`, `dropdown`, `text_input`, `progress_bar`.
- Text input + clipboard: `os.clipboard.read()` / `os.clipboard.write(s)`.
- Settings system: window size, fullscreen, vsync, monitor selection, master / music / SFX volume sliders, key-binding remap UI. Persists via `save` block (rides v0.2's infra).
- Localization scaffolding: `lang.t("key", args)` + per-locale resource bundles (`lang/en.toml`, `lang/ja.toml`).
- Pause-on-window-blur: fibers suspend on focus loss, resume on focus return. Per-state opt-out for always-running tasks.

**Exit criteria:**

- A complete pause menu ships in `examples/` (resume, settings, quit).
- Settings round-trip across launches.
- `examples/survive.twe` rebinds its keys at runtime via the settings UI.

---

## Phase 11 — v0.5 — Production hardening

**Status:** **closed 2026-05-04** per `docs/changes/2026-05-04-phase-11-closeout.md`. Twelve sessions shipped:

| # | Surface |
|---|---------|
| 1 | `screenshot(path)` builtin + F12 hotkey (PNG via macroquad's `Image::export_png`) |
| 2 | F3 frame-time HUD overlay (current ms / avg / max / fps over 120-frame ring buffer) |
| 3 | Panic-hook crash reporter (readable banner + `twec-crash-<ts>-<pid>.log` bundle) |
| 4 | Debounced hot-reload (`ReloadGate` — 6-frame stable-mtime gate; mid-debounce changes restart the countdown) |
| 5 | `twec profile [--frames N] [-o trace.json] <file>` (Chrome Tracing JSON) |
| 6 | Criterion bench harness `benches/vm.rs` (`sum_loop` / `fib_recursive` / `float_loop` cross-runs) |
| 7 | Bytecode dispatch tuning (in-place stack peek + hoisted int+int / float+float fast paths in `binary_arith` / `compare` / `apply_arith`) |
| 8 | Spritesheet animation demo + procedurally-generated `examples/assets/walk.png` |
| 9 | `examples/survive.twe` gamepad integration (analog stick + d-pad + A/RT fire + Start restart) |
| 10 | VM mirror of `on Class.death(e)` (new `OpCode::RegisterDeathHandler`, `BcDeathHandler`, `BcInstance.death_fired` flag, fire site in `VM::tick`) |
| 11 | `auto_pause_when_idle(seconds)` — opt-in idle-pause primitive |
| 12 | Closeout |
| 11+ (follow-on, same day) | True auto-pause-on-window-blur via `GetForegroundWindow` polling on Windows + `BlurAutoPause` state machine + opt-in `auto_pause_on_blur(true)` Twe builtin. macOS (`NSApplication.isActive`) and Linux (X11 `_NET_ACTIVE_WINDOW` / Wayland `xdg-shell`) focus paths still stubbed. New crate dep: `windows-sys` (cfg(windows) target-only). |

**606 tests pass** (601 in main close + 3 in follow-on; +2 in unrelated drift). Crash reporter and ReloadGate exit criteria met; Luau-parity perf number is bench-measurable but not snapshotted (criterion is the canonical command). The follow-on closes the auto-pause-on-blur slip on Windows; macOS / Linux focus paths remain a sub-day session each whenever a contributor needs them.

**Theme:** the things Valve or a player will hand back to the dev as a build-rejection.

**Components:**

- Crash reporter with user dialog + dump bundle (anonymized, opt-in upload).
- Screenshot + simple video capture builtin (F12 / configurable hotkey).
- Profiler tools: `twec profile <file>` outputs flamegraph-friendly traces; in-game frame-time HUD overlay (toggle with F3 by convention).
- Bytecode dispatch tuning (computed-goto on nightly; otherwise the LLVM-friendly match the existing dispatch already gets — Phase 3 closeout confirmed this).
- Asset hot-reload reliability pass. The current mtime-poll has known races (debounce window, partial-write reads).

**Exit criteria:**

- `cargo bench`'s tightest loops are within 2× of equivalent Lua / Luau on a synthetic benchmark suite.
- A panic from runtime code produces a readable user-facing dialog plus a developer-readable bundle.
- Three weeks of dogfooding produce zero "the file was half-written when reload fired" reports.

---

## Phase 12 — v0.6 — Asset pipeline + cross-platform build

**Status:** **closed 2026-05-05** per `docs/changes/2026-05-05-phase-12-closeout.md`. All twelve sessions shipped; `twec build examples/survive_demo` produces a self-extracting Windows `.exe` that mounts its embedded bundle at startup and launches the game with no Twe install on the target machine. Cross-compile to macOS / Linux ships the `.app` / `.AppDir` directory layouts; the Mach-O / ELF runtime that fills them is Phase-7 release-engineering work. **654 tests pass.**

Session breakdown (twelve, mirroring Phase 10 / 11 cadence):

| # | Session | State |
|---|---------|-------|
| 1 | `twec build` skeleton + project layout convention (`<dir>/main.twe` + `assets/` + optional `twe.toml`); validation + dry-run | shipped |
| 2 | Asset bundling format v1 (`src/bundle.rs`, `twec bundle` CLI, encoder/decoder round-trip) | shipped |
| 3 | `BundleReader` + path-redirected stdlib loaders (sprite / font / audio / glb) | shipped |
| 4 | `twec build --target windows-x86_64` end-to-end (self-extracting `.exe`) | shipped |
| 5 | Build configs (dev / release / profile) via `twe.toml` | shipped |
| 6 | `twec build --target macos-aarch64` (.app skeleton, host-only first) | shipped |
| 7 | `twec build --target linux-x86_64` + AppImage scaffolding | shipped |
| 8 | Bundle compression (zstd) | shipped |
| 9 | Steam SDK redistributable layout (`steam_appid.txt`, Depot manifest stub) | shipped |
| 10 | Build provenance + `twec info <path>` | shipped |
| 11 | EXIT GATE — `examples/survive_demo` ships as a Steam-class `.exe` | shipped |
| 12 | Closeout | shipped |

**Theme:** `twec build my_game/ --target windows-x86_64` produces a single distributable.

**Components:**

- `twec build` subcommand: bundles `.twe` + assets + runtime into a single signed-able binary. Per-platform via `cargo dist`'s machinery (Phase 7 already scaffolds it for the Twe binary itself; this phase generalizes).
- Asset bundling format (versioned, content-hashed, optionally compressed). Replaces "load from a path on disk" with "load from the bundle" at release.
- Build configurations: `dev` (hot reload + debug symbols), `release` (optimized + bundled), `profile` (instrumented).
- Steam-redistributable layout: `steam_appid.txt` location, Depot manifest hooks, redist DLL bundling.

**Exit criteria:**

- A vertical-slice Twe game ships as a 20–60MB single executable that runs on a Windows 10 box without a Twe install.
- A macOS .app and Linux AppImage equivalent ship from the same source tree.

---

## Phase 13 — v0.7 — Modules + type-system stability *(closed 2026-05-06)*

**Status:** closed per `docs/changes/2026-05-06-phase-13-closeout.md`. All twelve sessions shipped. All three exit criteria met.

**Theme:** the public-API freeze that v0.8+ depends on.

**Components:**

- Module / package system: `import` syntax, search paths, version pinning. Single-file → multi-file projects. The directory structure becomes the module graph.
- Strict mode v2: structural-record subtyping under strict (Phase 6 deferral), Luau-style "lax strict" widening rules.
- Verified mode (Tier 3 per `docs/02-type-system.md`): JSON diagnostics for LLM authorship, `twec verify <file>` subcommand, `--! verified` directive.
- API freeze warning system: `@deprecated("since v0.7")` annotations, `--warn-deprecated` flag, deprecation log in CHANGELOG.

**Exit criteria:**

- `twec verify` on a real Twe project returns a JSON document an LLM can self-correct against.
- Deprecation warnings produce ≥ 12 months of carry-over for any v0.7 surface that gets removed in v1.0.
- Two existing examples are split into multi-file modules without rewriting their bodies.

**Dropped from this plan:** user-defined generics. Conflicts with Principle 2 ("one obvious way per concept"). Built-in generic containers (`array of T`, `map of K => V`, `set of T`) stay; user generics are post-v1.0 if at all.

---

## Phase 14 — v0.8 — Beta + dogfood

**Theme:** prove the language by shipping with it, not just by writing tests for it.

**Components:**

- First-party game #1 enters closed beta. A Vampire-Survivors clone — the README's #1 use case. Exercises tilemap, save/load, particles, visuals, audio mixing, settings, gamepad, controller remap, all in one codebase.
- Tutorial v2 in `docs/tutorial.md`: long-form Pong → Survivors → mini-RPG, with screenshots + recorded sessions. The Phase 6 tutorial was first-pass; this is the second pass with a real game's worth of context.
- Examples gallery to ~25 (the Phase 6 deferred target was 20; round up given v0.2–v0.7 added surface).
- Performance fix list driven entirely by what the beta game hits. No speculative perf work.

**Exit criteria:**

- Beta game ships ≥ one paid release on itch.io with positive (≥ 4-star) reviews.
- Tutorial completion tracked: a new contributor builds Pong from the tutorial in ≤ 2 hours.

---

## Phase 15 — v0.9 — Release candidate

**Theme:** stop adding things. Make the existing things solid.

**Components:**

- API freeze. No new public surface. Bug fixes and doc fixes only. `@deprecated` warnings stay; new deprecations don't.
- Doc completeness pass: every keyword, every stdlib function, every block has a documented example.
- Steam SDK integration v1: achievements, statistics. Cloud saves use v0.2's save-format design — ride that, don't rewrite.
- Second first-party game enters beta if first is shipped.

**Exit criteria:**

- Zero open public-surface bug reports tagged `crash` or `data-loss`.
- Steam SDK achievements work end-to-end in the beta game.

---

## Phase 16 — v1.0 — Stable

**Exit gate** (revised from the original "three serious games, six months stable" formulation — softened on the games count because third-party authors are not in the project's control):

- **Two first-party games shipped** on a v0.x release. + N community games (no required count, but the project tracks and links them).
- **Six months of API stability** since the v0.7 freeze.
- LTS commitment: v1.x backports for security + critical fixes for **12 months minimum**.
- Marketing push: Show-HN / blog / demo video pinned to v1.0.

**Components** (mostly non-code):

- v1.0 release blog post.
- v1.x LTS branch policy in `CONTRIBUTING.md`.
- A "shipped on Twe" gallery linking the two first-party games + community submissions.
- Move the v0.x roadmap from "current" to "history" in `docs/05-roadmap.md`; add a v1.x scratch section pointing forward.

---

## Post-v1.0 — Phases 27–41

After v1.0 cuts, the LTS phase begins (`v1.x` branch, 12-month security backports). Fifteen follow-on phases extend the language to genres v1.0 doesn't cover.

**Round 1 (Phases 27–32)** — closed 2026-05-09 / 05-10. The original post-v1.0 plan as scoped in the README/`docs/05` Phase 27–32 section. Drove ten major surfaces from "out of scope" to shipped:

| Phase | Theme | Size |
|-------|-------|------|
| 27 — 2D genre reference examples | Platformer / Tetris / cards prove out the existing stdlib | M (closed 2026-05-09) |
| 28 — 3D commercial polish | Bloom, DoF, cascaded shadows, mipmaps, async preload | L (closed 2026-05-09) |
| 29 — Determinism layer | Fixed timestep, bounded GC, replay/record, sample-accurate audio, close the 3× speedup gap | L (closed 2026-05-09) |
| 30 — WASM / web target | `twec build --target wasm32`; browser-playable 2D | L (closed 2026-05-09) |
| 31 — Multiplayer foundation | Netcode RFC + lockstep over UDP/WebSocket | XL (closed 2026-05-10) |
| 32 — Open-world 3D foundation | Streaming, LOD, terrain, spatial partitioning | XL (closed 2026-05-10) |

**Round 2 (Phases 33–41)** — Phase 33 closed 2026-05-10; Phases 34–41 planned. Phase 33 (LLM differentiator) was added mid-stream from `LLMsPlan.md` and shipped before the gap-audit phases were planned, so it claimed the slot. The remaining round-2 phases (34–41) drive the "What is genuinely *not* ready" gap-audit rows: cross-platform polish, external validation, internet multiplayer, rollback netcode, browser 3D, mobile, console, MMO.

| Phase | Theme | Size |
|-------|-------|------|
| 33 — LLM differentiator | `twec grammar` / `twec verify` v2 / `twec stdlib --json` / `twec llm-loop` / `twec mcp` / `twec corpus` / `twec eval` / `twec mutate` / typed holes | XL (closed 2026-05-10; 11 sessions; 912 tests; LLMsPlan.md acceptance criteria all met) |
| 34 — Cross-platform polish | macOS `NSApplication.isActive` + Linux X11 `_NET_ACTIVE_WINDOW` focus paths + cargo-dist runtime cross-compile (aarch64-linux) + cross-compile CI gate | M |
| 35 — External validation drive | itch.io paid release + Steam AppID + community game pipeline + cross-machine multiplayer playtest + 4km open-world playtest + 6-month API stability close | XL (calendar-bound) |
| 36 — Online multiplayer | Steam P2P / NAT traversal / lobbies / reconnect; extends Phase 31 LAN-only | XL (gated on RFC + Steam AppID) |
| 37 — Rollback netcode | Second netcode model alongside lockstep; pressures Principle 2; for fighting + FPS | XL (gated on Principle 2 carve-out RFC) |
| 38 — Browser 3D | wgpu-on-web; extends Phase 30 WASM 2D to 3D | L (calendar-gated on browser wgpu maturity) |
| 39 — Mobile (iOS / Android) | macroquad supports both platforms; work is `twec build` matrix + signing + store submission | XL |
| 40 — Console targets | Switch / PS5 / Xbox; NDA-gated; cannot be shipped open-source | XL (sketched only) |
| 41 — MMO / Roblox-scale | Sharded servers, area-of-interest networking, persistent world DB, sandboxing | XL² (multi-year; sketched only) |

Phases 27 and 28 were pure code work. Phases 29 + 30 unblocked 31. Phase 32 required extending CLAUDE.md "What is locked" to authorize an engine-internal worker pool (single-threaded VM stays single-threaded for Twe authors; engine internals get parallelism). Round 2 inherits the same disciplines: each phase has explicit exit criteria and a closeout note when it lands. Total scope across both rounds: ~100–150 sessions, multi-year at one session/week.

---

## Phase 27 — Post-v1.0 — 2D genre reference examples

**Status:** **codebase-closed 2026-05-09** per `docs/changes/2026-05-09-phase-27-closeout.md`. All five sessions shipped: platformer (1), tetris (2), cards (3), stdlib gap closure adding `math.mod` / `random.shuffle` / `tilemap_solid_aabb` / `tilemap_aabb_touches` (4), closeout (5). Five new tests; **742 passing.** Four of the eight inline `GAP-N` markers closed; the rest deferred with explicit re-entry phases (sweep / key_repeat / mouse_release / hit_box). Visual playtest is the remaining manual step.

**Theme:** prove the v1.0 stdlib survives genres `survive_beta` didn't pressure. Each example pressures one stdlib axis the existing examples missed.

**Components:**

- `examples/platformer.twe` — coyote time, jump buffer, swept-AABB tile collision, one-way platforms. Pressure-tests whether the v0.2 tilemap stdlib needs a swept-AABB helper.
- `examples/tetris.twe` — 7-bag randomizer, SRS rotation, line clears, DAS/ARR. Pressure-tests key-repeat input handling.
- `examples/cards.twe` — Klondike or a small TCG. Pressure-tests mouse drag-and-drop, layered z-order, modal animation.
- Stdlib gap closure pass — close whatever the three examples reveal.

**Exit criteria:**

- Each example ≤ 500 lines.
- No new stdlib functions added without a Principle 1 / Principle 3 justification.
- README examples gallery updated.

---

## Phase 28 — Post-v1.0 — 3D commercial polish

**Status:** **codebase-closed 2026-05-09** per `docs/changes/2026-05-09-phase-28-closeout.md`. All six sessions shipped: mipmap pyramid + 16× anisotropic filtering (1), cascaded shadow maps with 3 cascades on a 2D-array depth texture + view-z cascade selection (2), inline 12-tap bloom (3), vignette tint color (4), async `.glb` parse on background worker threads (5), closeout (6). New `postfx.*` Twe surface: `bloom(intensity)`, `bloom_threshold(t)`, `vignette_color(c)`. Three new tests in `tests/eval.rs`; **745 passing.** DoF + view-frustum-fitted CSM + multi-tier bloom downsample + linear-space mip resample deferred with explicit re-entry conditions. Visual playtest of `examples/crystal_hunter.twe` is the remaining manual step.

**Theme:** the deferred items from `docs/changes/2026-05-07-phase-24-26-closeout.md`, rolled up. Difference between "playable 3D" and "shippable 3D."

**Components:**

- Mipmap pyramid + anisotropic filtering (closes the Phase 17 deferral).
- Cascaded shadow maps (2 cascades, then 3) extending Phase 19–23's directional shadow path.
- Bloom — HDR threshold + downsample + upsample chain.
- Vignette extension + optional depth-of-field.
- Async asset preload — Rust-side job queue, decouples `.glb` parse from frame loop.

**Exit criteria:**

- `examples/crystal_hunter.twe` runs at 60fps with bloom + cascades on a 4-year-old GPU.
- `examples/survive_beta.exe` shows zero regression on the Phase 11 bench harness (`benches/vm.rs`).
- No new language surface — engine-only phase.

---

## Phase 29 — Post-v1.0 — Determinism layer

**Status:** codebase-closed 2026-05-09 — see `docs/changes/2026-05-09-phase-29-closeout.md`. Six sessions: fixed-timestep accumulator + `time.physics_dt` (1), incremental GC sweep + `gc.budget_ms` / `gc.last_collect_ms` / `gc.bytes_alive` (2), VM immediate-int dispatch tuning (3), `replay.record` / `replay.play` / `replay.stop` + `tests/replay.rs` end-to-end harness (4), tick-accurate audio scheduling — `sound.schedule` / `sound.now` (5), `examples/rhythm_demo.twe` + closeout (6). The Phase 29 plan called for the bytecode-VM 3× speedup gap (`docs/changes/2026-05-01-phase-8.5-closeout.md`) to close on `sum_loop`; that's partial — `fib_recursive` hits 3× (function-call-heavy loops are the bytecode VM's strength), but `sum_loop` and `float_loop` remain bytecode-slower than the tree-walker. Closing the dispatch-loop gap requires computed-goto / direct-threading and is its own follow-on phase.

**Theme:** the only path to fighting / rhythm games AND a hard prerequisite for Phase 31 (lockstep multiplayer). Also closes the unresolved 3× bytecode-VM speedup gap from `docs/changes/2026-05-01-phase-8.5-closeout.md`.

**Components:**

- Fixed-timestep update loop separated from variable-rate render — Glenn Fiedler "Fix Your Timestep!" pattern. Expose `physics_dt` constant.
- Bounded GC pauses — enforce per-frame budget on the Phase 8.5 tracing GC; carry collection across frames if the budget is exceeded.
- Close the 3× speedup gap — replace predicate-dispatch chains in `binary_arith` / `compare` with a tight switch. Bench against `benches/vm.rs`.
- Input frame log + replay primitive — `replay.record(path)` / `replay.play(path)` builtins.
- Sample-accurate audio scheduling — `sound.play_at(beat)` for rhythm games.

**Exit criteria:**

- 60-second replay of `survive_beta` reproduces frame-for-frame across two runs on the same machine.
- `cargo bench` shows ≥ 3× over pre-tag baseline on `sum_loop`.
- `examples/rhythm_demo.twe` measured at < 8ms input-to-pixel latency.

---

## Phase 30 — Post-v1.0 — WASM / web target

**Status:** **codebase-closed 2026-05-09** per `docs/changes/2026-05-09-phase-30-closeout.md`. All six sessions shipped: `BuildTarget::Wasm32` + `build_wasm_target()` + WASM play loop (1), localStorage save/load via `quad-url` (2), click-to-start overlay / AudioContext unlock (3), CSS `aspect-ratio` letterbox (4), `.github/workflows/wasm-demo.yml` CI deploy-to-Pages (5), closeout (6). Browser playtest of the GitHub Pages demo is the remaining manual step. Honest deferrals: end-to-end browser test in CI, `survive_beta` WASM (asset size + wasm-opt), installed-twec WASM build path, 3D WASM, IndexedDB (shipped localStorage instead — synchronous, sufficient for game saves).

**Theme:** browser-playable 2D unlocks distribution (itch.io HTML5 page, embeddable demos, Show-HN-grade reach). macroquad already supports WASM, so most of this is build-pipeline work. Defers the 3D wgpu path on web (browser support uneven).

**Components:**

- `twec build --target wasm32` produces `.html` + `.wasm` + asset bundle. Fourth row in the Phase 12 build matrix.
- File I/O reroute — saves to localStorage (quad-url), assets via `fetch`.
- Audio context unlock on first user gesture (browser autoplay policy).
- Variable canvas sizing — preserve aspect ratio + letterbox.
- CI pipeline — auto-publish `examples/flappy.twe` as web demo on every release.

**Exit criteria:**

- `examples/flappy.twe` runs in Chrome + Firefox at 60fps with sound + keyboard, served from a static host.
- `survive_beta` is deferred to a Phase 31+ follow-on (asset size + perf).

---

## Phase 31 — Post-v1.0 — Multiplayer foundation

**Status note (2026-05-10):** **Codebase-closed** at `docs/changes/2026-05-10-phase-31-closeout.md`. All seven sessions shipped: netcode RFC, UDP transport, 15 `net.*` builtins, lockstep runner with two-thread end-to-end test, canonical-JSON snapshot serialization (`net.hash` + `net.snapshot_json`), `examples/pong_net.twe`, closeout note. **765 tests pass.** Cross-machine LAN playtest is the remaining manual verification step. Honest deferrals: rollback netcode, authoritative C-S, Steam P2P transport (`--features steam-net`), WebSocket browser multiplayer, NAT traversal, disconnect/reconnect handling, >4-peer sessions, VM-mirror of `net.*` builtins.

**Theme:** the biggest scope of the post-v1.0 set. Gated on `docs/changes/<date>-multiplayer-rfc.md` choosing one netcode model. Adding lockstep + rollback + authoritative C-S all at once would violate Principle 2; pick one.

**Recommendation in the RFC:** lockstep over UDP. Smallest surface, leverages the Phase 29 determinism work directly, fits Principle 2's "one obvious way."

**Components** (assuming RFC picks lockstep):

- Netcode RFC committed and merged.
- UDP + WebSocket transport behind one `net.*` API.
- `net.host(port)` / `net.connect(addr)` returning peer handles.
- Lockstep runner — per-tick input exchange, hash-check determinism, configurable input delay.
- Snapshot serialization — reuse Phase 13 verified-mode JSON for debug + a binary tier for production.
- `examples/pong_net.twe`.

**Out of scope explicitly:** matchmaking, central servers, lobby UI. Direct-IP and Steam P2P only.

**Exit criteria:**

- `examples/pong_net.twe` plays peer-to-peer over LAN with 4-frame input delay, deterministic across two machines.
- RFC merged and frozen for the Phase 31 implementation cycle.

---

## Phase 32 — Post-v1.0 — Open-world 3D foundation

**Status note (2026-05-10):** **Codebase-closed** at `docs/changes/2026-05-10-phase-32-closeout.md`. All nine sessions shipped: lock revision (1), `src/spatial.rs` LooseGrid + BVH + WorldSpatial (2), `src/streaming.rs` chunked load/unload state machine (3), `src/lod.rs` per-class LodChain (4), `src/terrain.rs` chunked heightfield (5), `src/cull.rs` Gribb-Hartmann frustum + spatial integration (6), `src/instance.rs` per-asset instance buckets (7), `world.*` ergonomic helpers (8), closeout note (9). 35 new builtins under `world.*` (28) + `terrain.*` (7). **810 tests pass.** Honest deferrals: wgpu render-pipeline integration (data structures ship; render-side consumption is the immediate Phase-32 follow-on dev cycle), occlusion culling beyond frustum, `entity Tree: lod = [...]` parser sugar (v2), SAH-optimal BVH, 3D loose-grid, LOD smooth transitions, integrated 50k-prop bench harness.

**Theme:** Tunic-scale open world. Not Roblox-scale — that's a multi-year follow-on. Open only after Phase 28 + the lock revision land.

**Lock conflict to settle first:** the **single-threaded VM** lock in `CLAUDE.md` "What is locked" must be extended to authorize an engine-internal worker pool for asset I/O / physics step / frustum culling. User-facing Twe code stays single-threaded (Principle 2 intact); engine internals get parallelism (Principle 5 extended). Action: add a one-line addendum to `CLAUDE.md` before Phase 32 session 1.

**Components:**

- Spatial partitioning — loose grid for dynamic objects, BVH for static.
- Chunked streaming with budget-bound async loads (executes the deferral from `docs/3d-roadmap.md` Phase 22).
- Mesh + texture LOD chains.
- Terrain heightfield with chunk tiling.
- Occlusion culling — extension to the Phase 19–23 frustum culling.
- GPU instancing buffer expansion — extension to the Phase 23 dynamic instance buffer.
- Author-facing API stays minimal: `world.stream_radius(m)`, `entity Tree: lod = [near.glb, far.glb]`. No new keywords.

**Exit criteria:**

- 4km × 4km test scene with 50k static props + 500 dynamic NPCs runs at 60fps with < 512MB VRAM.
- Author-facing API stable for 6 months before the v1.x release tag.

---

## Phase 33 — Post-v1.0 — LLM differentiator

**Status:** **codebase-closed 2026-05-10** per `docs/changes/2026-05-10-phase-33-closeout.md`. Eleven sessions (0–10) shipped across four commits. **912 tests pass; clippy + build clean.** The first phase that ships **language-level support for LLM authoring** end-to-end. Every Twe tool that exists is now exposed to an LLM through a structured contract.

**Theme:** drive `LLMsPlan.md` from "design doc" to "shipped." Twe's pitch as an "AI-legible by design" language (Principle 4) becomes concrete: LLMs and tools targeting Twe consume structured contracts, not free-text prose.

**Sessions shipped:**

| # | Surface |
|---|---------|
| 0 | `LLMsPlan.md` strategy doc at repo root |
| 1 | `twec grammar` — GBNF / JSON-Schema / EBNF export of the canonical grammar |
| 2 | `twec verify` v2 — structured `fix: { rationale, edits[{line, col, len, replace}] }` on high-confidence diagnostics |
| 3 | `twec stdlib --json` — manifest of all 235 builtins by category, derived by introspecting `Env` |
| 4 | `twec llm-loop` — provider-trait harness with `FixtureProvider` + `CommandProvider`; per-round JSONL traces seed fine-tune corpus |
| 5 | `twec mcp` — stdio JSON-RPC 2.0 server exposing 7 tools (parse / verify / format / grammar / stdlib_list / stdlib_lookup / apply_patch) |
| 6 | `twec corpus --json` + `@task / @inputs / @expected / @category / @difficulty` headers on all 40 examples |
| 7 | `twec eval` — replay-based benchmark on `eval::run_with_frames` |
| 8 | `twec mutate` — auto-mutates `tests/programs/*.twe` to produce `(broken, verify_json, fix)` triples for fine-tune training |
| 9 | Typed holes (`???`) — lexer + parser + AST + eval + infer + verify + printer integration |
| 10 | Closeout note + README updates |

**Exit criteria:** all Tier 1+2+3 acceptance criteria from `LLMsPlan.md` met. See closeout for the per-tier audit.

---

## Phase 34 — Post-v1.0 — Cross-platform polish

**Status:** open. Inherits the four scratch items from the v1.x roadmap that already had concrete blockers identified:

- macOS auto-pause-on-blur via `NSApplication.isActive`.
- Linux X11 auto-pause-on-blur via `_NET_ACTIVE_WINDOW` property polling + `_NET_WM_PID` lookup.
- Linux Wayland auto-pause-on-blur — documented stub. Wayland focus is per-input-device and only delivered as events to the focused client; no portable way to query "am I focused" from outside the windowing system client (miniquad). The honest stub returns `true` until miniquad surfaces focus events upstream.
- Cargo-dist generalization for cross-compiled per-target `twec` runtimes — fills the Mach-O / ELF binaries that Phase 12's `.app` and `.AppDir` directory layouts need but couldn't ship.

**Why first in round 2 outside Phase 33:** every gap is concrete and small. None require RFC. Cargo-dist work is already-active Phase 7 scratch. Shipping this closes the "macOS / Linux fully polished" row from the gap audit and is the only way Phase 12's cross-platform builds become functionally cross-platform rather than scaffold-only.

**Components:**

| # | Deliverable |
|---|---|
| 1 | macOS focus path — `[[NSApplication sharedApplication] isActive]` via `objc2`; cfg-gated to `target_os = "macos"` |
| 2 | Linux X11 focus path — `_NET_ACTIVE_WINDOW` poll on root window via `x11rb`, then `_NET_WM_PID` lookup, compare to `std::process::id()` |
| 3 | Linux Wayland focus path — documented stub (returns `true`); document that miniquad-upstream focus event surface is the real fix |
| 4 | cargo-dist matrix expansion — `aarch64-unknown-linux-gnu` row in `.github/workflows/release.yml` (cross-compiled on x86_64 Linux runner via `gcc-aarch64-linux-gnu`) |
| 5 | Cross-compile CI gate in `.github/workflows/ci.yml` — `cargo check` against `aarch64-unknown-linux-gnu` and `x86_64-pc-windows-gnu` on every PR; catches breakage before tag time |
| 6 | Closeout |

**Exit criteria:**

- Auto-pause-on-blur works on macOS + X11 (parity with Windows from Phase 11). Wayland stays an honest stub.
- `twec build` produces functional binaries for all five targets (Windows-x86_64, Linux-x86_64, Linux-aarch64, macOS-x86_64, macOS-aarch64) from any host (release.yml matrix).
- A community contributor on macOS or Linux ARM can `twec play examples/survive_beta/main.twe` from the GitHub Release archive without a from-source build.

**Size:** M (one focused month).

---

## Phase 35 — Post-v1.0 — External validation drive

**Status:** open. Non-code phase. Tracks the external-action items still gating Phases 14, 15, and 16 from full closure.

**Why now:** runs in parallel with everything else in this round. The blocker is "no public users have shipped a paid game on Twe yet"; the only way to clear it is to actually ship one or court someone to.

**Components:**

| # | Deliverable |
|---|---|
| 1 | First-party itch.io paid release of `examples/survive_beta` — package, store page, pricing, screenshots, trailer |
| 2 | Steam AppID + end-to-end Steam SDK test (achievements + Cloud saves + Workshop placeholder) on the Phase 15 surface |
| 3 | Cross-machine LAN multiplayer playtest of `examples/pong_net.twe` (Phase 31 verification step) |
| 4 | 4km × 4km open-world playtest extending `examples/crystal_hunter.twe` (Phase 32 verification step + render-pipeline integration follow-on) |
| 5 | Community game pipeline — solicit 2–3 external authors, provide direct support, add their games to the README "Shipped on Twe" gallery |
| 6 | Six-month API stability snapshot review — log every public-surface change since the v0.7 freeze; cut a v1.x branch if drift is real |
| 7 | Closeout note + v1.x LTS branch policy |

**Exit criteria:**

- ≥ 1 first-party game on itch.io with paid sales + ≥ 4-star reviews.
- ≥ 1 community-authored game shipped on a v1.x release.
- Cross-machine LAN multiplayer demonstrated bit-deterministic.
- Six-month API stability window closes cleanly — Phase 16's last open criterion.

**Size:** XL (calendar-bound; can't be sprinted).

---

## Phase 36 — Post-v1.0 — Online multiplayer (matchmaking + NAT + reconnect)

**Status:** codebase-closed 2026-05-11 per `docs/changes/2026-05-11-phase-36-closeout.md`. Extends Phase 31's direct-IP-only lockstep foundation to internet play. Eight sessions shipped (RFC + 7 deliverables). External-action exit criteria (Steam AppID smoke run, two-home-network playtest) are explicit deferrals per the closeout note — same shape as Phase 35.

**Why this slot:** Phase 31 explicitly scoped out matchmaking / lobbies / dedicated servers / NAT / reconnect. The transport (UDP) and the netcode model (lockstep) ship — Phase 36 fills in the discovery + routing layer that turns LAN-only into internet-ready.

**Components:**

| # | Deliverable |
|---|---|
| 1 | Matchmaking RFC — addendum to `docs/changes/2026-05-10-multiplayer-rfc.md`. Pick: Steam P2P (recommended for Steam-first games), custom STUN/TURN, or peer-discovery-via-DHT |
| 2 | `--features steam-net` — Steam P2P transport behind the same `net.*` API surface |
| 3 | NAT traversal — STUN handshake for direct UDP, TURN relay fallback if both peers are double-NATed |
| 4 | Lobby primitives — `net.create_lobby(name, max_peers)`, `net.find_lobbies(query)`, `net.join_lobby(id)` |
| 5 | Reconnect handling — peer-disconnect detection, snapshot-checkpoint resync, host-migration option |
| 6 | Optional: dedicated-server mode — `twec build --target linux-server` produces a headless binary for Steam Game Server |
| 7 | `examples/pong_net_internet.twe` — peer-to-peer pong over the public internet via Steam P2P |
| 8 | Closeout |

**Out of scope:** authoritative client-server (separate from P2P with auth-tier; that's Phase 37 territory if it lands at all). Anti-cheat. Voice chat.

**Exit criteria:**

- Two players on different home networks join via lobby and play `pong_net_internet.twe`.
- Mid-game disconnect and reconnect within 10 seconds doesn't desync.
- Steam P2P path passes the Phase 15 Steam SDK test on a live AppID.

**Size:** XL (gated on RFC + Steam AppID).

---

## Phase 37 — Post-v1.0 — Rollback netcode

**Status:** open. **Pressures Principle 2.** A second netcode model alongside Phase 31's lockstep — for fighting games, fast-paced action, anything where a 4-frame lockstep input delay is a player-feel deal-breaker.

**Why this is risky:** Principle 2 ("one obvious way per concept") forbids shipping two netcode models without justification. The justification is: rollback solves a problem lockstep cannot (sub-frame input feel), and the genre divide is sharp — rhythm + RTS + co-op want lockstep determinism, fighting + first-person shooters want rollback. The RFC must establish that the language exposes one obvious way *per genre*, with a clear `net.mode = "lockstep"` / `net.mode = "rollback"` switch. If the RFC can't justify both shapes cleanly, this phase doesn't ship.

**Components:**

| # | Deliverable |
|---|---|
| 1 | Rollback RFC — addendum to multiplayer RFC. Justify Principle 2 carve-out. Choose GGPO-style or Lockstep-with-Rollback variant |
| 2 | `state` snapshotting — every fixed tick, snapshot rollback-tagged entities to a ring buffer (extends Phase 32's instance buckets pattern) |
| 3 | Rewind + replay — given peer input N frames late, rewind to tick N, re-execute with corrected input |
| 4 | Predicted input — fill peer input gaps with last-input-repeat or velocity-extrapolation |
| 5 | `entity Fighter: rollback = true` — opt-in marker for which entities are snapshot/rewound (others stay lockstep-deterministic) |
| 6 | Visual smoothing — rendered position lerps across the rewind so the local player doesn't see snap-back |
| 7 | `examples/fighter_demo.twe` — 2-player fighting-game proof |
| 8 | Closeout |

**Exit criteria:**

- `examples/fighter_demo.twe` plays at 60fps with sub-2-frame input feel against an opponent on a 60ms-RTT connection.
- Lockstep-mode examples (rhythm, RTS, co-op) continue to work unchanged.
- RFC honestly justifies the dual-mode carve-out from Principle 2.

**Size:** XL (gated on RFC; second netcode model requires careful Principle 2 justification).

---

## Phase 38 — Post-v1.0 — Browser 3D (wgpu-on-web)

**Status:** open. Gated on browser wgpu maturity. As of 2026-05, browser wgpu (WebGPU) support is uneven — Chrome ships, Safari ships in Tech Preview, Firefox lags. Wait for Firefox-stable + Safari-stable before opening this phase.

**Why this slot:** Phase 30 shipped 2D WASM via macroquad's GL backend; 3D needs the wgpu pipeline ported to `target_arch = "wasm32"`, which means swapping out winit + the windows-sys focus path + native font loaders for browser-native equivalents. Most of that is mechanical once browser wgpu is stable.

**Components:**

| # | Deliverable |
|---|---|
| 1 | `BuildTarget::Wasm32_3D` — third row in the Phase 30 WASM matrix, dispatches to wgpu-on-web instead of macroquad-GL |
| 2 | Browser wgpu pipeline — port `src/play3d.rs` so all `#[cfg(not(target_arch = "wasm32"))]` gates that exist purely because of winit / native crates relax to wasm-friendly equivalents |
| 3 | Asset streaming via `fetch` — `.glb` and texture loads work without filesystem access |
| 4 | Audio via WebAudio — Phase 30's AudioContext unlock extends to 3D audio |
| 5 | Browser-friendly `physics.body` — rapier3d already compiles to wasm; verify the determinism story holds |
| 6 | `examples/crystal_hunter_web.twe` — 3D demo published to the same Pages CI as Phase 30 |
| 7 | Closeout |

**Out of scope:** WebXR / VR rendering. Browser-native multiplayer (Phase 36's WebSocket transport covers it).

**Exit criteria:**

- `examples/crystal_hunter.twe` runs in Chrome + Firefox + Safari at 30fps minimum, 60fps target.
- Asset bundle ≤ 20MB for the demo (perf budget for cold-start over residential internet).
- No regression on the 2D WASM Phase 30 demo.

**Size:** L (calendar-gated on browser wgpu maturity; technical work is moderate).

---

## Phase 39 — Post-v1.0 — Mobile (iOS / Android)

**Status:** open. macroquad already supports both platforms; the work is mostly `twec build` target descriptors, signing pipelines, and store-submission tooling.

**Why this slot:** mobile is a distribution unlock for 2D Twe games (Survivors-class touches well to mobile via virtual joysticks). The technical work is bounded. The store-submission gauntlet is the real cost.

**Components:**

| # | Deliverable |
|---|---|
| 1 | `BuildTarget::Aarch64Ios` + `BuildTarget::Aarch64AndroidApk` — fifth and sixth rows in Phase 12's matrix |
| 2 | Touch input — `touch.x` / `touch.y` / `touch.is_active` / `touch.tap_count` builtins; multi-touch via `touch.pointer(i)` |
| 3 | Virtual joystick widget — `joystick(at:, size:, deadzone:)` returning a normalized 2D vector; reuse for `examples/survive_beta` mobile port |
| 4 | iOS code signing — provisioning profile + entitlements; CI integration via `cargo-dist` mobile target descriptors |
| 5 | Android signing + AAB — Play Store submission package |
| 6 | Aspect-ratio handling — extends Phase 30's CSS letterbox to mobile letterbox/pillarbox + safe-area inset awareness |
| 7 | `examples/survive_beta_mobile/` — touch-controlled port of survive_beta with virtual joystick + auto-fire |
| 8 | App Store + Play Store submission tooling docs |
| 9 | Closeout |

**Exit criteria:**

- `examples/survive_beta_mobile` ships on TestFlight (iOS) + internal-track Play Store (Android) at 60fps on 4-year-old phones.
- `twec build --target ios-aarch64 my_game/` produces an `.ipa` from Linux / macOS host.
- Touch input parity with the Phase 30 web demo.

**Size:** XL (technical work is L, store-submission gauntlet adds the rest).

---

## Phase 40 — Post-v1.0 — Console targets (Switch / PS5 / Xbox)

**Status:** **gated on platform-holder partnerships.** Cannot be shipped open-source. Sketched here for completeness; the actual work happens behind NDA with licensed dev kits, in a private fork or partner-maintained branch.

**Why this slot:** consoles are where commercial 2D indie games make most of their money. Twe ignoring them caps the upside of every other phase. But the SDK code, signing keys, and store APIs are NDA-bound by Nintendo / Sony / Microsoft — open-source distribution is incompatible with the platform agreements.

**Components (sketched):**

- Switch — Nintendo SDK port. ARM64. Likely needs custom `wgpu` backend or use of Nintendo's first-party graphics API. Requires Nintendo developer agreement + dev kit.
- PS5 — Sony PlayStation SDK port. AMD64. Custom GNM/GNMX backend. Requires Sony developer agreement + dev kit.
- Xbox Series X|S — Microsoft GDK port. AMD64. DirectX 12 backend (already partial via wgpu's D3D12 backend). Requires Microsoft developer agreement + dev kit.
- Per-platform store integration — achievements, cloud saves, friend lists, controller glyphs.

**How this could realistically ship:** a partner studio licensed for one of the three platforms maintains a private fork during a port, contributes back the platform-agnostic abstractions (e.g., a generalized "console controller" input layer that's useful on PC too), and the platform-specific code stays in the private fork. The open-source `twec` ships only the abstractions.

**Exit criteria:**

- One first-party game on one console store. The path is partner-driven, not implementer-driven.

**Size:** XL — multi-year + NDA-bound. Listed for completeness.

---

## Phase 41 — Post-v1.0 — MMO / Roblox-scale 3D foundation

**Status:** **multi-year, post-v2.0 territory.** The ceiling. Extends Phase 32's Tunic-scale open world to persistent-world / massively-shared-world.

**Why last:** every prior phase is bounded by "ship a single-player or small-multiplayer game." MMO scale needs sharded server architecture, area-of-interest networking, persistent world database, massively-replicated entity systems — none of which are hobby-project deliverables. This phase exists because the gap audit named it; whether Twe ever opens it is a future-implementer decision.

**Components (sketched):**

| # | Deliverable |
|---|---|
| 1 | MMO architecture RFC — sharded servers vs. seamless world vs. instanced zones; pick one |
| 2 | Server-authoritative entity replication — separate from Phase 31 lockstep + Phase 37 rollback (those are peer-to-peer; this is C-S) |
| 3 | Area-of-interest networking — server only sends updates for entities near the player, not all entities |
| 4 | Persistent world DB — `world.persist(entity)` saves to a server-side database (SQLite / PostgreSQL / Redis depending on scale tier) |
| 5 | Massively-replicated entity buckets — extension to Phase 32's instance buckets but with cross-server replication |
| 6 | Sandboxing for user-generated content — Roblox-class servers run player-authored Twe code; needs CPU/memory limits, capability-restricted stdlib, and gas metering |
| 7 | Workshop / mod APIs — first-class user-generated-content publishing |
| 8 | `examples/mmo_demo/` — small persistent-world demo with 100 simultaneous players in a single shard |
| 9 | Closeout |

**Open questions (would need to resolve before the phase even opens):**

- Does Twe extend to a server-hosted multi-tenant runtime, or stay author-side / engine-side only?
- How does the language's no-macros / no-metaprogramming locked decision survive MMO needs (replication attributes, network-tier annotations)?
- Sandboxing — can the existing `unsafe_code = "deny"` discipline scale to running adversarial player code?

**Exit criteria:**

- `examples/mmo_demo` supports 100 concurrent players on a single $20/month VPS.
- Player-authored Twe code can be published, sandboxed, and run on the server without compromising other tenants.

**Size:** XL² (multi-year; gated on architecture RFC; gated on a custom-engine commitment that doesn't yet exist).

---

## What's intentionally *not* in any plan

These are deferred indefinitely (the v3.0+ era or never), with cause:

- **Native code generation** (Luau-style). Post-v1.0; the bytecode VM with NaN tagging + GC is the v1.0 perf story. The 3× speedup gap closure is **Phase 29**, not native codegen.
- **User-defined generics**. Conflicts with Principle 2. Post-v2.x if at all.
- **Macros / metaprogramming**. Off the table per `CLAUDE.md` "What is locked". No-go.
- **Sandboxing for user-generated content**. Tracked as a Phase 41 sub-component; standalone, never.
- **Workshop / mod APIs**. Tracked as a Phase 41 sub-component; standalone, never.
- **Asynchronous gameplay code visible to the user**. Locked: no `async`/`await` per `CLAUDE.md`. Cooperative fibers cover the use cases.

---

## Total scope estimate

The original `Phase 0–7 weeks` table was based on the 2025-design-phase guess of ~57 weeks and proved usefully wrong (Phases 1–6 finished in dramatically less calendar time once development started in earnest). Rather than quote weeks that age poorly, post-v0.1 phases use **size markers**: S (one focused week), M (two-to-four weeks), L (one-to-three months), XL (multi-quarter).

| Phase | Release | Size |
|-------|---------|------|
| 0 — Design lock-in | (pre-v0.1) | M (closed) |
| 1 — Tree-walker | (pre-v0.1) | L (closed) |
| 2 — Vertical-slice game | (pre-v0.1) | M (closed) |
| 3 — Bytecode VM + tooling | (pre-v0.1) | L (closed) |
| 4 — Type system v1 | (pre-v0.1) | L (closed) |
| 5 — 3D + dialogue | (pre-v0.1) | L (closed at v0.1-min-viable) |
| 6 — Tooling + docs | (pre-v0.1) | M (closed) |
| 7 — Release engineering | v0.1 | M (active) |
| 8 — Foundations for shipping | v0.2 | L (closed 2026-05-04; 7 sessions shipped) |
| 8.5 — NaN tagging + tracing GC | v0.2 (perf) | L (closed 2026-05-01; 3× speedup criterion missed, follow-on perf phase pending) |
| 9 — Visuals + assets-for-UI | v0.3 | L (closed 2026-05-04; 11 sessions shipped, 2 sub-criteria slipped to follow-ons) |
| 10 — UI + game-shell | v0.4 | M (closed 2026-05-04; 11 sessions shipped, all 3 exit criteria met) |
| 11 — Production hardening | v0.5 | M (closed 2026-05-04; 12 sessions + same-day follow-on shipped real auto-pause-on-blur via `GetForegroundWindow` polling on Windows; macOS / Linux focus paths still stubbed) |
| 12 — Asset pipeline + build | v0.6 | M |
| 13 — Modules + type-system stability | v0.7 | L (closed 2026-05-06; 12 sessions shipped, all 3 exit criteria met) |
| 14 — Beta + dogfood | v0.8 | XL (codebase-closed 2026-05-06; itch.io ship + reviews pending) |
| 15 — Release candidate | v0.9 | M (codebase-closed 2026-05-06; Steam AppID test + crash-report criterion pending) |
| 16 — Stable | v1.0 | S (codebase-closed 2026-05-06; 6-month API stability window begins; itch.io ships pending) |
| 17–26 — 3D commercial roadmap | v1.0 (3D arc) | XL across 10 phases (codebase-closed 2026-05-07; see `docs/3d-roadmap.md` + `docs/changes/2026-05-07-phase-{17,18,19-23,24-26}-closeout.md`) |
| 27 — 2D genre reference examples | post-v1.0 | M (closed 2026-05-09; 5 sessions; 4 stdlib helpers) |
| 28 — 3D commercial polish | post-v1.0 | L (closed 2026-05-09; 6 sessions; 4 postfx builtins; DoF deferred) |
| 29 — Determinism layer | post-v1.0 | L (planned; closes 3× speedup gap) |
| 30 — WASM / web target | post-v1.0 | L (closed 2026-05-09; 6 sessions; localStorage saves; CSS letterbox; Pages CI) |
| 31 — Multiplayer foundation | post-v1.0 | XL (closed 2026-05-10; lockstep over UDP, 7 sessions, `examples/pong_net.twe`) |
| 32 — Open-world 3D foundation | post-v1.0 | XL (closed 2026-05-10; spatial / streaming / LOD / terrain / cull / instance, 9 sessions, render-pipeline integration deferred) |
| 33 — LLM differentiator | post-v1.0 round 2 | XL (closed 2026-05-10; 11 sessions; 912 tests; LLMsPlan.md acceptance met — grammar export + verify v2 + stdlib manifest + llm-loop + mcp + corpus + eval + mutate + typed holes) |
| 34 — Cross-platform polish | post-v1.0 round 2 | M (planned; macOS / X11 focus paths shipped + cargo-dist runtime cross-compile + cross-check CI) |
| 35 — External validation drive | post-v1.0 round 2 | XL (planned; non-code; itch.io ship + Steam + cross-machine multiplayer + 4km open-world playtest + community pipeline + 6-month API stability close) |
| 36 — Online multiplayer (matchmaking + NAT + reconnect) | post-v1.0 round 2 | XL (planned; gated on RFC; extends Phase 31 LAN-only) |
| 37 — Rollback netcode | post-v1.0 round 2 | XL (planned; gated on Principle 2 carve-out RFC; second netcode model alongside lockstep) |
| 38 — Browser 3D (wgpu-on-web) | post-v1.0 round 2 | L (planned; calendar-gated on browser wgpu maturity) |
| 39 — Mobile (iOS / Android) | post-v1.0 round 2 | XL (planned; technical work is L, store-submission gauntlet adds the rest) |
| 40 — Console targets (Switch / PS5 / Xbox) | post-v2.0 | XL (gated on platform-holder partnerships; cannot be shipped open-source; sketched only) |
| 41 — MMO / Roblox-scale 3D | post-v2.0 / v3.0+ | XL² (multi-year; gated on architecture RFC + custom-engine commitment; sketched only) |

The realistic v1.0 ETA is *whenever the beta and RC games ship*, not a wall-clock date. Don't promise dates. Phases 33–41 are post-v1.0; v1.0 doesn't gate on any of them.

---

## v1.x roadmap scratch

*This section replaces the v0.x roadmap for post-v1.0 planning. v0.x is history above.*

The LTS commitment begins at v1.0: security and critical fixes backport to the `v1.x` branch for 12 months minimum. Breaking changes to the public Twe language surface or `twec` CLI require a `@deprecated` cycle.

**Open post-v1.0 work items.** Bigger items are now phased (see Phases 27–32 above). The following are smaller scratch items not large enough to merit a phase of their own:

| Item | Driver | Status |
|------|--------|--------|
| macOS / Linux auto-pause-on-blur | `NSApplication.isActive` / `_NET_ACTIVE_WINDOW` paths; Windows ships | **Now Phase 34** |
| Bytecode VM kwarg-builtin support | All widget-using examples fall back to tree-walker; closes a known limitation | Tracked, not yet phased |
| 3× bytecode-VM speedup gap from Phase 8.5 | Criterion bench harness ships; perf tuning driven by profiling | **Phase 29 partial; computed-goto / direct-threading is the remaining work** |
| `save SaveSlot:` block syntax | v0.3+ follow-on from Phase 8; key/value layer ships | Tracked, not yet phased |
| `tilemap Dungeon:` block syntax | v0.3+ follow-on from Phase 8 | Tracked, not yet phased |
| Per-state pause opt-out (`pause: false`) | Open syntax question | Open |
| 3D rendering polish (bloom / DoF / cascades / mipmaps / async preload) | Per `docs/changes/2026-05-07-phase-24-26-closeout.md` polish-tier deferrals | **Phase 28 closed; DoF + view-frustum-fitted CSM remain** |
| 2D genre coverage (platformer / Tetris / cards) | Stdlib pressure-test; plug a marketing gap | **Phase 27 — closed 2026-05-09** |
| WASM / web target (2D) | Distribution unlock for 2D | **Phase 30 — closed 2026-05-09** |
| WASM / web target (3D) | wgpu-on-web | **Now Phase 38** |
| LLM authoring tooling | LLMsPlan.md tier 1+2+3 | **Phase 33 — closed 2026-05-10** |
| Multiplayer (LAN, lockstep) | Per-genre game pressure; needs netcode RFC | **Phase 31 — closed 2026-05-10 (lockstep over UDP, 7 sessions, `examples/pong_net.twe`)** |
| Multiplayer (internet, matchmaking, NAT, reconnect) | Steam P2P / NAT traversal | **Now Phase 36** |
| Multiplayer (rollback for fighting games) | Fighting-game player feel | **Now Phase 37** |
| Open-world 3D streaming + LOD | Tunic-scale games; needs lock revision | **Phase 32 — closed 2026-05-10 (9 sessions; spatial + streaming + LOD + terrain + cull + instance + ergonomic API)** |
| Open-world 3D render-pipeline integration | Phase 32 wgpu consumption of buckets + indirect draw | Tracked as Phase 32 follow-on dev cycle |
| Mobile (iOS / Android) | Distribution unlock for 2D + touch input | **Now Phase 39** |
| Console (Switch / PS5 / Xbox) | Indie commercial revenue ceiling | **Now Phase 40 (NDA-gated; cannot ship open-source)** |
| MMO / Roblox-scale 3D | Persistent shared worlds | **Now Phase 41 (multi-year; gated on architecture RFC + custom-engine commitment)** |
| First-party itch.io paid release | Phase 14 / 16 external exit criterion | **Now Phase 35** |
| Steam AppID + end-to-end Steam SDK test | Phase 15 external exit criterion | **Now Phase 35** |
| Cross-machine LAN multiplayer playtest | Phase 31 verification step | **Now Phase 35** |
| 4km × 4km open-world playtest | Phase 32 verification step | **Now Phase 35** |
| Community game pipeline / "Shipped on Twe" gallery | Link + track third-party games | **Now Phase 35** |
| Six-month API stability window | Phase 16 external exit criterion (opens 2026-05-06) | **Now Phase 35** |

---

## Risks and mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Solo-maintainer burnout (Wren scenario) | High | Recruit at least one collaborator before Phase 8. Document everything. |
| Scope creep (every game suggests a feature) | High | The eleven examples are the spec. Reject anything not implied by them or by the v1.0 commercial-2D thesis. |
| Type system proves harder than expected | Medium | Phase 4 staged; non-strict shipped first. Strict mode v2 + verified mode are now Phase 13. |
| Engine integration is harder than language design | Medium | macroquad chosen in Phase 2 specifically to defer this. Custom engine is post-v1.0. |
| LLM tooling support never materializes | Low | Twe's grammar is designed for it from day one. Verified mode in Phase 13 is the explicit LLM-authoring story. |
| Audience indifference | Medium | Differentiators (procedural visuals, AI-friendly grammar, declarative blocks) are real. Marketing rides Phase 16. |
| Pillar 3 (procedural visuals) was claimed to ship in v0.1 but `visual` block isn't implemented | High | Phase 7 docs honesty fix demoted Pillar 3 to v0.3. `visual` block runtime is now a Phase 9 component with explicit exit criteria (Example 5 runs). |
| Beta game #1 (Phase 14) doesn't materialize | High | Start a parallel community-game pipeline by Phase 12. If first-party game stalls, court 2–3 external authors with direct support. |
| NaN tagging slips again | Medium | Pulled into Phase 8 with a hard exit-criterion (3× speedup vs. pre-tag VM); deferred-since-Phase-3 status is unacceptable past v0.2. |

---

## When to stop

This roadmap covers ~14 months. If, at any point during Phases 1–3, the implementer realizes the language fundamentally doesn't work, **stop**. Document why. Take the lessons elsewhere. There is no shame in stopping a hobby language; there is shame in dragging it on past usefulness.

The exit conditions:

- After Phase 1, if the example programs feel forced or awkward in the working interpreter — pause, redesign, or stop.
- After Phase 2, if the vertical-slice game is harder to write in Twe than in Lua/Love2D — the language has not earned its existence. Stop or rethink.
- After Phase 3, if the bytecode VM doesn't deliver the expected performance — investigate, but consider whether Twe needs to exist alongside Lua / Luau or as an alternative.

The healthiest version of this project is one where the implementer is willing, at every phase, to abandon it. That willingness is what produces a good language.
