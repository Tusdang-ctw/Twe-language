# 2026-05-04 — Phase 9 closeout

## Status: closeout note. Closes Phase 9 (v0.3 — Visuals + assets-for-UI). Eleven sessions shipped between 2026-05-01 and 2026-05-02; the final session (11) was tagged "Phase 9 EXIT GATE" in its commit message (`0de6838`) and demonstrably runs Example 5 end-to-end via `twec play_visual examples/visual_fire.twe`.

## Background

Phase 9 opened 2026-05-01 the same day Phase 8.5 closed. Theme per `docs/05-roadmap.md` §"Phase 9": *ship the headline differentiator (Pillar 3 from `README.md`) and the asset machinery UI in Phase 10 needs.* The big-ticket item — the `visual` block → WGSL fragment-shader compiler — was the explicit reason Pillar 3 was demoted from a v0.1 claim during Phase 7's docs-honesty pass; closing Phase 9 means Pillar 3 is no longer a paper feature.

Eleven sessions, all on the `phase-9:` commit prefix:

| # | Surface | Commit |
|---|---------|--------|
| 1 | `noise()` / `smoothstep()` / `mix()` math stdlib | `4860657` |
| 2 | 2D camera primitive (`camera.follow` / `camera.shake` / `camera.zoom`) | `ea37d9a` |
| 3 | Sprite atlas + frame stepping (`load_atlas`, `sprite_frame`, `sprite_frame_at`) | `d6e38da` |
| 4 | TTF / OTF font loading (replaces macroquad's default font) | `9c10660` |
| 5 | Gamepad input via gilrs (`gamepad.*` / `gamepad_axis.*` / `gamepad_press.*`) | `4a4a32e` |
| 6 | Color pipeline (gamma + lerp + hex + HSV) | `bc8a6d2` |
| 7 | Particles doc-honesty pass (lifecycle + `age_ratio` + emitter despawn on both backends) | `6d530fb` |
| 7b | `on Class.death(e)` global event hook (tree-walker) | `e7e5245` |
| 8 | `visual` block lexer + parser + AST | `38d3ada` |
| 9 | `visual` block subset typechecker (`crate::visual_check`) | `a158700` |
| 10 | `visual` block → WGSL codegen (`crate::visual_wgsl`) | `2b0ecc9` |
| 11 | `visual` block render integration (`crate::play_visual` — wgpu driver, EXIT GATE) | `0de6838` |

No individual session change-notes were authored for Phase 9 sessions (a deviation from the per-session note pattern that Phase 6 / v0.2 used). Commit messages + roadmap §"Phase 9" prose carry the session details; this closeout is the canonical record.

## What shipped — the four highlight surfaces

### `visual` block → WGSL → wgpu (sessions 8 + 9 + 10 + 11)

The Pillar-3 deliverable, built as a four-session vertical slice:

- **Session 8 (`src/lexer.rs` + `src/parser.rs` + `src/ast.rs`).** `visual <Name>:` block parses; body recognises `size:` field, `pixel(uv, time) -> color:` function, and the standard expression grammar.
- **Session 9 (`src/visual_check.rs`, ~520 LOC).** Subset typechecker. Walks the parsed body and rejects: allocating expressions, unbounded loops, mutation, event handlers, and any call outside the GPU-safe whitelist. Errors point at the specific construct so authors don't ship CPU code that "compiles" but explodes at WGSL-validation time.
- **Session 10 (`src/visual_wgsl.rs`, ~490 LOC).** Codegen. Turns a `visual <Name>:` block into a complete WGSL module: vertex shader emitting a fullscreen quad via the `vertex_index` trick, fragment shader calling the compiled `twe_pixel(uv, time)`, custom `noise()` helper that bit-matches the CPU `value_noise_2d` (same Wang hash + golden-ratio offset), and inlined `vec4<f32>` literals for `color.<named-constant>` reads. Integer literals always emit as `f32` so they unify with vector arithmetic without per-call type analysis. WGSL builtins handle `smoothstep` / `mix` / `math.sin` / `math.cos` directly. naga (wgpu's WGSL frontend) validates the emitted module in CI without needing a GPU.
- **Session 11 (`src/play_visual.rs`, ~447 LOC).** wgpu render driver. `twec play_visual <file>` opens a window, builds a render pipeline from the compiled WGSL, drives a `time: f32` uniform from the system clock, draws a fullscreen-quad pass each frame. Hot reload re-builds the pipeline on file change.

**Exit demo:** `twec play_visual examples/visual_fire.twe` renders Example 5's procedural fire shader fullscreen, with the system clock animating it. The shader source in [examples/visual_fire.twe](../../examples/visual_fire.twe) is the verbatim Twe code from `docs/01-examples.md` §"Example 5."

### Particles runtime parity (sessions 7 + 7b)

A doc-honesty pass surfaced that the bytecode-VM mirror of the particles runtime had landed in a prior session and gone undocumented. Session 7 reconciled the doc and added the missing test coverage. Session 7b added the global `on Class.death(e):` event hook on the tree-walker — handlers register at top-level, fire when an instance of the named class transitions despawned → pruned, and bind the dying entity to the param.

The bytecode-VM mirror of the death-event hook is **not** in this phase; the compiler currently errors clearly on the construct rather than silently dropping it. Captured as a deferral below.

### Asset surfaces (sessions 3 + 4)

- **Sprite atlas / frame stepping.** `load_atlas("walk.png", grid: (cols, rows))` + `sprite_frame(handle, pos, frame)` + `sprite_frame_at(handle, pos, size, frame)`. The bundled `examples/atlas_demo.twe` reuses the single-sprite `hero.png` as a degenerate 1×1 atlas to prove the draw-call path; a full spritesheet-driven character animation demo is a slipped exit-criterion (see "What slipped" below).
- **TTF / OTF font loading.** Replaces macroquad's default font path with `load_font(path)` + `text_with_font(...)`. Used by `examples/font_demo.twe`.

### Camera + color + gamepad + math (sessions 1 + 2 + 5 + 6)

- **`math.noise(x, y)` / `math.smoothstep(edge0, edge1, x)` / `math.mix(a, b, t)`.** CPU implementations bit-matched against the WGSL builtins so visual-block authors can prototype on the CPU first.
- **2D camera primitive.** `camera.follow(entity, lerp:)` + `camera.shake(amplitude:, duration:)` + `camera.zoom`. Demoed in `examples/camera_demo.twe` (test-program form: `tests/programs/camera_phase9.twe`).
- **Gamepad via gilrs.** `gamepad.connected` + `gamepad.a / b / x / y / lb / rb / start / back`, analog axes via `gamepad_axis.lx / ly / rx / ry / lt / rt`, latched press events via `gamepad_press.*`. Graceful degradation: if gilrs can't initialise, fields stay at defaults and a one-line message goes to stdout — no crash. Demoed in `examples/gamepad_demo.twe`.
- **Color pipeline.** `color.from_hex("#ff8800")`, `color.from_hsv(h, s, v)`, `color.lerp(a, b, t)`, gamma-correct compositing, HDR-aware blending. The named-constant table (`color.red`, `color.transparent`, etc.) is what the `visual` block codegen inlines as `vec4<f32>` literals.

## Phase 9 exit criteria

Per the roadmap §"Phase 9 — Exit criteria":

- [x] **Example 5 runs end-to-end.** `twec play_visual examples/visual_fire.twe` opens a window with the procedural fire shader, system-clock-animated, hot-reload-aware. Session 11.
- [x] **Example 6 runs end-to-end** (with documented caveat). `examples/particles_demo.twe` runs on the tree-walker; the particles-block runtime ships on **both** backends (per session 7's doc-honesty pass). The integration test (Example 10's boss-fight) requires the bytecode-VM mirror of the `on Class.death(e):` hook, which is the one slipped sub-criterion below.
- [~] **A spritesheet-driven character animation demo ships in `examples/`.** Slipped. `examples/atlas_demo.twe` ships and exercises the draw-call API but uses a degenerate 1×1 atlas because the bundled `hero.png` is a single sprite, not a spritesheet. The comment in the file documents the gap honestly: *"Swap the path and grid for any real spritesheet to see frame stepping."* A real walk-cycle demo needs an asset (8×1 walk-cycle PNG) that the repo doesn't bundle. Authoring + bundling the asset is a follow-on session — call it 9-followup-1.
- [~] **Gamepad + keyboard work interchangeably for `examples/survive.twe`.** Slipped. `examples/survive.twe` still reads `key.right` / `key.left` / `key.space` etc. only; it has not been updated to also poll `gamepad_axis.lx / ly` and `gamepad.a` for fire. The gamepad surface itself ships and is exercised by `examples/gamepad_demo.twe`. A small edit to `survive.twe` would close this; deferred as a follow-on session — call it 9-followup-2.

Two of three exit-criteria sub-bullets cleanly met; the remaining two are asset / wiring tasks rather than surface gaps. Phase 9 closes with these explicit deferrals rather than holding the closeout open over them — both are ≤1-session items that can land any time in v0.3 patch territory.

## Deferred

### To v0.3 follow-on sessions (within this release line)

- **9-followup-1 — spritesheet-driven character animation demo.** Bundle a real walk-cycle PNG and write a 20-line example that cycles frames via `floor(t * 10) % 8`. ≤1 session.
- **9-followup-2 — `survive.twe` gamepad integration.** Read `gamepad_axis.lx / ly` for movement (with a small deadzone) and `gamepad.a` for fire. ≤1 session.
- **Bytecode-VM mirror of the `on Class.death(e):` event hook.** Tree-walker has it (session 7b); the VM compiler currently errors clearly on the construct. Required for Example 10 (boss-fight integration test) to run on the VM. Mid-size session.
- **`save SaveSlot:` block syntax + version migration.** Carried over from Phase 8's deferral list.
- **`tilemap Dungeon:` block syntax.** Carried over from Phase 8's deferral list.

### To Phase 10 (UI + game-shell)

- Layout primitives (`panel` / `flex` / `grid` / `scroll` / `stack`).
- Widgets (`button` / `label` / `slider` / `checkbox` / `dropdown` / `text_input` / `progress_bar`).
- Settings system, localization scaffolding, pause-on-window-blur with per-state opt-out.

Phase 10 has been able to start in parallel with Phase 9 since fonts + atlases landed (sessions 3 + 4); the gating dependency is now closed.

### To the post-Phase-8.5 perf phase (not Phase 10 / 11 territory)

- **3× bytecode-VM speedup vs pre-tag baseline.** Tracked in `docs/changes/2026-05-01-phase-8.5-closeout.md` §"What slipped." Currently homeless on the roadmap (not Phase 10, not Phase 11); the closeout note has the follow-on agenda (criterion harness, profile-guided tuning, dispatch-loop redesign). Phase 11 ("Production hardening") is the most natural absorbing phase but the perf gap should be closed before then if practical.

## Verification

- `cargo build --release` — clean.
- `cargo clippy -- -D warnings` — clean.
- `cargo test` — **544 tests pass** (up from 502 at Phase 8.5 close; +42 across the eleven Phase 9 sessions, distributed across `tests/visual_check.rs` + 9, `tests/visual_wgsl.rs` + 24, and the rest in `tests/eval.rs` / `tests/parse.rs` / `tests/programs/`).
- naga-validation of the WGSL emitted by `crate::visual_wgsl` runs in CI under `tests/visual_wgsl.rs` — proves the codegen produces validate-clean modules without needing a GPU on the CI runner.
- `twec play_visual examples/visual_fire.twe` — manually verified to render Example 5's fire shader fullscreen.

## Doc edits applied as a result

- `CLAUDE.md` Phase discipline updated: Phase 9 closed; the active line moves to "Phase 7 release engineering + Phase 10 UI primitives can both proceed; the perf-gap follow-on phase is the open implicit-priority item."
- `docs/05-roadmap.md` Phase 9 §"Status" rewritten from "substantively complete" to "closed 2026-05-04 per `docs/changes/2026-05-04-phase-9-closeout.md`."
- `README.md` Status section: adds [x] entries for `visual` block runtime, particles runtime parity, asset surfaces (atlas / fonts), camera / color / gamepad / noise math, plus the two slipped sub-criteria as explicit follow-on bullets.
- `docs/01-examples.md` "Runtime delivery status" already reflects Example 5 / 6 status as of 2026-05-02; no further edit needed.