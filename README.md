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

1. **2D systematic / RPG hybrid** (Vampire Survivors meets Diablo): item systems, modifiers, inventories, progression trees.
2. **3D RPG** (small-scale Tunic / BotW): scene management, NPC AI, dialogue, quests, save/load.
3. **Physics + visual showcase** (Noita / shader-driven games): pure-code visuals, particle systems, procedural graphics with no texture assets required.

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
- [x] Ten example programs written
- [ ] Formal grammar in EBNF
- [ ] Tree-walking interpreter (vertical slice)
- [ ] Vertical-slice game built in Twe
- [ ] Bytecode VM
- [ ] Tooling (LSP, formatter, tree-sitter grammar)
- [ ] v0.1 release

See `docs/05-roadmap.md` for the detailed plan.

## License

TBD. The intent is permissive (MIT or Apache-2.0).
