# Twe

> A game-first programming language designed for the AI-collaboration era.

Twe is a scripting language being designed from scratch for 2D and 3D game development, with a runtime that will eventually be co-designed with a custom game engine. It is currently in the **design phase** — no implementation exists yet. The goal of this repository is to lock in the design decisions before any code is written.

## Why another language?

The honest answer to "why not just use Lua?" is documented in `docs/03-runtime.md`. The short version: the Godot team tried embedding Lua, Python, and Squirrel in their engine for over a decade and abandoned all of them in favor of a custom language (GDScript) because none of them could be cleanly integrated with native vector types, garbage collection budgets, class extension, and editor tooling. Twe takes that lesson seriously.

The design principles, in order of priority:

1. **Game concepts are first-class.** `entity`, `state`, `visual`, `dialogue` are language constructs, not library calls.
2. **One obvious way per concept.** Regularity is what makes a language easy for both humans and LLMs.
3. **No silent footguns.** 0-indexed, explicit nil, block-scoped, dimensional units, errors that suggest fixes.
4. **AI-legible by design.** Predictable grammar, structured diagnostics, round-trippable AST.
5. **Engine-native.** Twe's runtime *is* the engine's runtime. No FFI dance.

## Target use cases

Three games Twe must be excellent at:

1. **2D systematic / RPG hybrid** (Vampire Survivors meets Diablo): item systems, modifiers, inventories, progression trees. **This is the v1.0 success criterion** — see `docs/05-roadmap.md` for the v1.0 thesis.
2. **3D RPG** (small-scale Tunic / BotW): scene management, NPC AI, dialogue, quests, save/load. v0.1 ships cubes/spheres/`.glb` import; full polish (animation, physics, materials) is post-v1.0.
3. **Physics + visual showcase** (Noita / shader-driven games): pure-code visuals, particle systems, procedural graphics with no texture assets required. **Coming in v0.3** — the `visual` block → WGSL shader compilation runtime ships in Phase 9 of the roadmap. (v0.1's `visual` keyword is not yet wired to a real compiler; the Phase 7 docs honesty pass demoted this from a v0.1 claim.)

## Documents

| # | Doc | Purpose |
|---|-----|---------|
| 1 | [`docs/01-examples.md`](docs/01-examples.md) | Ten example programs that imply ~80% of the language design |
| 2 | [`docs/02-type-system.md`](docs/02-type-system.md) | Type system position, drawn from Roblox's Luau |
| 3 | [`docs/03-runtime.md`](docs/03-runtime.md) | Runtime architecture (Wren + Bevy ECS) and pitfalls to avoid |
| 4 | [`docs/04-reading-list.md`](docs/04-reading-list.md) | Curated reading list for the implementer |
| 5 | [`docs/05-roadmap.md`](docs/05-roadmap.md) | Phased roadmap from spec to v1.0 |
| 6 | [`docs/06-design-document.md`](docs/06-design-document.md) | Formal language specification (principles, lexical, grammar, semantics) |

## Status

- [x] Research phase complete (Lua, Luau, Wren, GDScript, Bevy, fantasy consoles, AI-friendly grammar)
- [x] Design principles drafted
- [x] Eleven example programs written (`docs/01-examples.md` + `docs/example-11-snake.md`)
- [x] Formal grammar in EBNF (`docs/06-design-document.md` §3)
- [x] Tree-walking interpreter (Phase 1, closed)
- [x] Vertical-slice game built in Twe (`examples/survive.twe`, Phase 2)
- [x] Bytecode VM (Phase 3)
- [x] Tooling: LSP, formatter, tree-sitter grammar, VS Code extension (Phases 3 + 6)
- [x] Type system v1, non-strict + strict modes (Phases 4 + 6)
- [x] 3D rendering surface — cubes, spheres, `.glb` meshes, hot reload (Phase 5 + Phase 8 session 1)
- [x] Cooperative fibers (`wait` works in nested blocks + functions, both backends — Phases 5 + 8)
- [x] **Phase 8 closed** (v0.2 — Foundations for shipping): mouse input, save/load bottom layer, audio v2, tilemap (stdlib form), `.glb` mesh import, function-body `wait` on the VM. See `docs/changes/2026-05-04-phase-8-closeout.md`.
- [x] **Phase 8.5 closed** (NaN tagging + tracing GC): `TaggedValue` + thread-local mark+sweep tracing GC across both backends, with auto-collect safepoints. **502 tests pass.** The 3× speedup-vs-pre-tag-VM exit criterion is not met — see `docs/changes/2026-05-01-phase-8.5-closeout.md` for the perf gap and the follow-on tuning agenda.
- [x] **Phase 9 closed** (v0.3 — Visuals + assets-for-UI): `visual` block → WGSL codegen + wgpu render driver (Pillar 3 is no longer a paper feature — `twec play_visual examples/visual_fire.twe` runs Example 5's procedural fire shader end-to-end), particles runtime on both backends, `on Class.death(e)` event hook (tree-walker), sprite atlases, TTF/OTF fonts, 2D camera, color pipeline, gamepad input, `noise()` / `smoothstep()` / `mix()` math stdlib. **544 tests pass.** See `docs/changes/2026-05-04-phase-9-closeout.md`.
- [ ] **Phase 7: v0.1 release** — `cargo dist` binaries, VS Code marketplace, website, CONTRIBUTING.md, license decision. Active. (Codebase is now substantially beyond the original v0.1 surface; could retag as v0.2 / v0.3 at release time.)
- [~] **Phase 10: v0.4 — UI + game-shell** — sessions 1–8 shipped 2026-05-04. Widgets: `button`, `label`, `progress_bar`, `slider`, `checkbox`, `dropdown`, `text_input`. Clipboard (`os.clipboard.read/write`) via arboard with Ctrl+V paste in text_input. Layout: `panel`, `stack`, `flex`, `grid`, `scroll`. Pause: `pause(flag)` / `is_paused()`. Demos: `examples/button_demo.twe`, `examples/widgets_demo.twe`, `examples/layout_demo.twe`, `examples/pause_demo.twe`. Closed the `gamepad_demo.twe:9` latent parser bug along the way (`if cond: a else: b` is now an expression). Remaining: settings system, localization scaffolding, auto-pause-on-window-blur (deferred — needs winit focus events), exit-gate pause-menu integration.
- [ ] **Phase 9 follow-ons** (≤1-session each): real spritesheet-animation demo (asset bundling); `examples/survive.twe` gamepad integration; bytecode-VM mirror of the `on Class.death(e):` event hook.
- [ ] **Bytecode-VM perf gap from Phase 8.5** — currently 1.1×–1.8× slower than pre-tag baseline on tight integer loops; closing the gap is captured as a follow-on perf phase (criterion harness, profile-guided tuning, dispatch-loop redesign).
- [ ] **Phase 11–16 → v1.0** — production hardening, asset pipeline, modules + type-system stability, beta + dogfood, RC, stable. v1.0 = "ship a Vampire-Survivors-class commercial 2D game on Twe."

See `docs/05-roadmap.md` for the detailed plan.

## License

TBD. The intent is permissive (MIT or Apache-2.0).
