# Doc 03 — Runtime Architecture

> The runtime is what the language *actually does*. The surface syntax is a contract with the user; the runtime is a contract with reality.
>
> This document combines three sources: Wren's VM design, Bevy's ECS API design, and a list of pitfalls observed in Lua, GDScript, and other game-scripting languages.

---

## High-level architecture

```
┌─────────────────────────────────────────────────────┐
│                    Twe source code                  │
└─────────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────┐
│  Lexer ──► Parser ──► AST ──► Type checker (opt.)  │
└─────────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────┐
│            Bytecode compiler (single-pass)          │
└─────────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────┐
│   Twe VM   ◄────►   Engine runtime  ◄────►   GPU    │
│   • fibers          • ECS world          • shaders  │
│   • bytecode        • physics            • render   │
│   • GC              • input / audio                 │
└─────────────────────────────────────────────────────┘
```

The Twe VM and the engine runtime share an address space and a memory model. They are not "embedded language + host" in the Lua sense — they are co-designed components. Engine objects (entities, transforms, sprites) are first-class Twe values, not opaque userdata.

---

## VM design — drawn from Wren

Wren is the closest reference for what Twe's VM should be. Wren is a small, fast, class-based scripting language by Bob Nystrom (author of *Crafting Interpreters*); its VM is roughly 4,000 semicolons of C99, readable in an afternoon. It has been adopted by the Luxe game engine and TIC-80.

What we steal from Wren:

### Fibers as first-class concurrency

Wren's killer feature: lightweight cooperative fibers as a *core language primitive*, not a library. A fiber suspends itself with `yield`, resumes with `call`. No callback hell, no async coloring.

In Twe, fibers power:

- `wait <duration>` (suspends the current fiber for a duration)
- `every <duration>:` (a recurring fiber)
- `dialogue` blocks (a fiber per dialogue)
- `state` blocks within `ai` declarations
- `on <event>:` handlers when they need to await

The user never types the word "fiber." The runtime spawns one per `dialogue`, one per `state`, one per active coroutine, transparently.

### Single-pass compiler to tight bytecode

Wren compiles directly from source to bytecode in a single pass with one token of lookahead. This is fast, simple, and good enough for a scripting language.

Twe v0.1 will follow this model. Hand-written recursive descent parser, single-pass codegen, no separate optimization phase. Fast iteration for the user, simple maintenance for us.

When we eventually need performance, the Luau model is the upgrade path: bytecode + optional native code generation (Luau gets 1.5x–2.5x via this). That's a v0.3+ concern.

### Tight bytecode with NaN-tagged values

Wren uses NaN-tagging to pack all values into 64 bits (booleans, small integers, doubles, pointers to heap objects). This is a standard trick that pays off enormously in cache behavior.

Twe v0.1 will use NaN-tagging for the value representation. *Crafting Interpreters* covers exactly how to do this in Chapter 30 ("Optimization").

### Embedding-first C API

Wren's C API is designed for embedding in applications. It provides:

- A handle type for holding references to Twe values from native code.
- A slot-based stack for passing values between Twe and native.
- Reentrant calls (native code can call back into Twe and vice versa).

We follow this pattern. The engine is "native code"; everything the engine wants to expose to Twe goes through the slot API. Engine-side objects are first-class to Twe — but the boundary is a clean C ABI.

---

## ECS as the runtime model — drawn from Bevy

Bevy is the most successful code-first ECS engine in modern gamedev. Reading its API design teaches us how an ECS world should be exposed to user code.

### What Bevy got right

In Bevy, components are plain data types with a marker trait, entities are integer IDs, and systems are regular functions whose parameter types declare what they need. The world infers parallelism from system signatures automatically.

```rust
// Bevy
fn move_player(
    keyboard: Res<Input<KeyCode>>,
    mut player: Query<&mut Transform, With<Player>>,
) { ... }
```

The genius of this API is that **the function signature *is* the query**. There is no `world.query("...")` ceremony. The parameter `Query<&mut Transform, With<Player>>` *means* "give me mutable access to Transform components on entities that also have a Player marker." The runtime builds the query plan from the type.

### Translating to Twe

Twe's `on update`, `on <event>`, and `state` blocks compile to Bevy-style systems. The function-signature-as-query pattern translates directly:

```twe
on update(dt, hero: Hero, enemies: list of Enemy):
    for enemy in enemies:
        if hero.distance_to(enemy) < 50:
            enemy.alert(hero)
```

This signature compiles to a system that queries:

- The `Hero` archetype, expecting exactly one match (singleton).
- The `Enemy` archetype, returning all matches as a list.

The user doesn't think "ECS." They think "I want the hero and the enemies." The runtime does the rest.

### Resources vs components

Bevy distinguishes *components* (per-entity data) from *resources* (singleton global data — time, input, asset registry). Twe makes the same distinction but hides it:

- Things declared with `entity`, `item`, `enemy`, etc. become components.
- Things accessed as ambient context (`scene`, `camera`, `time`, `key`) are resources.

The user sees neither word. They see the API.

### Marker components for query filtering

Bevy uses empty struct types as "marker components" for filtering: `Query<&Transform, With<Player>>` means "transforms on entities that have the Player marker." Twe uses the same idea but exposes it through inheritance:

```twe
entity Hero extends Player:
    ...

# Then:
on update(dt, heroes: list of Player):    # matches Hero, and any other Player subtype
    ...
```

Inheritance acts as the marker hierarchy. Cleaner than tagging, more familiar to users from other languages.

### Required components

Bevy's `#[require(...)]` lets a component declare that any entity it's added to must also have certain other components, recursively. Twe expresses this as `extends`:

```twe
entity Sprite:
    pos: vector
    image: texture

entity Hero extends Sprite:    # Heroes always have pos and image
    hp: int
    speed: float
```

Adding a `Hero` automatically materializes the `Sprite` component fields (`pos`, `image`) — same machinery as Bevy's required components, accessed through familiar OOP-like syntax.

### Deferred mutations via Commands

Bevy systems can't mutate the world directly while iterating; instead they queue changes via a `Commands` buffer applied at sync points. This is what makes parallel system execution sound.

Twe inherits this discipline but again, hides it. When user code says `spawn ExplosionBurst at e.pos`, the runtime queues a spawn command and applies it at the end of the current frame. No race conditions; no user-visible complexity.

---

## Memory management

Twe v0.1 uses **incremental tracing GC** with a write barrier, modeled on Luau's collector (which is itself based on Go's incremental GC with a PID controller for heap pacing). This was specifically chosen by the Luau team to reduce pause times for game workloads.

Key design constraints:

- **No stop-the-world for more than 1ms in v0.1.** Incremental marking, with budgets per frame.
- **Generational** in v0.2 if profiling shows young-generation pressure.
- **No GC for fixed-size value types.** Vectors, colors, durations, ranges, percents — these are stack-allocated, NaN-tagged, or interned.
- **Manual resource handles for engine objects.** Sprites, meshes, sounds — managed by the engine, with finalizers for cleanup. Not tracked by the GC.

This is a deliberate move *away* from Lua's classic GC behavior, which has bitten enough game studios (Roblox, Warframe, Alan Wake 2) to justify the extra implementation work.

---

## Concurrency model

Cooperative fibers + a single-threaded VM. No threads in v0.1.

**v0.1 status (Phase 5 tasks 2 & 3):** `wait <duration>` ships in **both** backends (tree-walker and bytecode VM) for state on-entry bodies. `dialogue` / `say` / `choice` ship in the tree-walker as a sequential runtime — say prints, choice picks the first branch, wait inside dialogue is deferred to a per-dialogue scheduler in a follow-on session. Other fiber-using surfaces (function-body wait, fiber-backed `every` rewrite, bytecode dialogue) are listed in `notes/future-phases.md`.

The reasoning:

- The vast majority of game scripting is not CPU-bound.
- Multi-threaded GC is hard. Multi-threaded scripting + multi-threaded engine is a debugging nightmare.
- Bevy's parallelism is engine-side (rendering, physics, asset loading). The script layer can stay single-threaded and still feel fast.

Fibers are scheduled cooperatively. The runtime advances all active fibers each frame, in declaration order, with budget protection (a runaway fiber can't stall the frame).

If profiling shows CPU bottlenecks in v0.2+, we add a worker pool for explicitly-marked compute tasks (`task compute_pathfinding(...) -> path`) using message passing — never shared mutable state.

---

## Hot reload

Hot reload of Twe code is a v0.1 requirement, not a v0.2 nice-to-have. Reason: the "first five seconds" experience extends to "the first time you change a value and see it update without restarting." Without hot reload, Twe is just another language; with it, Twe feels alive.

Implementation strategy:

- File watcher in the dev runtime.
- On change: re-parse and re-compile the file.
- Replace functions and event handlers atomically.
- Preserve entity state and currently-running fibers where possible.
- For declarative blocks (`item`, `entity`), recompile the schema and migrate existing instances.

This is hard to get right. Luau and PICO-8 both do it well; we should study both.

---

## Pitfalls list — things to *avoid*

This is the negative space. Each item below is a documented mistake from a real language that Twe will not repeat.

### From Lua

1. **1-indexed arrays.** Out. Twe is 0-indexed. Every other modern language is 0-indexed; matching the world is more valuable than matching Lua.
2. **`nil`/`false` both falsy.** Out. Only `false` is falsy. Comparing to `nil` is explicit.
3. **`:` vs `.` method dispatch.** Out. One operator (`.`). Methods are declared inside blocks, so dispatch is unambiguous.
4. **Metatables for everything.** Out. Twe has declarative blocks; OOP is single-inheritance via `extends`; operator overloading is explicit (`op +`).
5. **`..` for string concat.** Out. `+` and string interpolation.
6. **0 or more return values without tuples.** Out. Functions return exactly one value (which may be a tuple).
7. **Global-by-default variables.** Out. Variables are block-scoped. `global x = 5` exists for explicit globals.
8. **No standard library to speak of.** Out. Twe ships with batteries — math, vectors, color, ranges, lists, maps, sets, strings, files, JSON, all the game-dev primitives.

### From Wren

1. **No operator overloading on built-in math types.** Wren's class-based dispatch made `vec * 2` awkward. Twe supports `op *` on user types and ships built-in operators on `vector`, `color`, `range`, etc., natively.
2. **Class-based-only object model.** Forcing every entity to be a class with getters/setters is heavy. Twe's `item Sword:` is *not* a class — it's a declaration the runtime interprets.
3. **No type system.** For LLM authoring, no types is a serious limitation. Twe has gradual typing from v0.1 (see `02-type-system.md`).
4. **Solo maintainer drift.** Bob Nystrom stepped away from Wren in 2018 partly because the project's success became socially stressful for one person. Twe needs shared ownership and clear governance from the start. **Solo language projects collapse under success, not failure.**

### From GDScript

1. **Tightly coupled to one engine.** GDScript is excellent inside Godot and useless outside. Twe is engine-agnostic in v0.1 — the runtime can bind to raylib, macroquad, Bevy, or a custom engine. We co-design Twe with our engine in v0.2+, but we don't marry them.
2. **Slow execution before native code generation.** GDScript was historically interpreted-only and slow. Twe plans for native code generation as a v0.3 milestone (Luau's path).
3. **Sometimes-statically-typed, sometimes-not.** GDScript has gradual typing but the rules around when it engages are confusing to newcomers. Twe's three-tier system (non-strict / strict / verified) is explicit about which mode is active.

### From the broader scripting-language graveyard

1. **Embedding via FFI dance.** Most embeddable languages (Lua, Python, JS, Squirrel) require manual binding code that becomes huge and bug-prone. Twe's runtime *is* the engine's runtime; binding code is generated, not hand-written.
2. **Shipping without tooling.** Languages without an LSP, formatter, and tree-sitter grammar are dead on arrival in 2026+. Twe ships these in v0.1, not later.
3. **Shipping without telemetry.** Luau's 2024 telemetry paper showed that real-world type-error patterns differ wildly from what designers predict. Twe's dev environment will collect privacy-respecting telemetry from day one to inform language evolution.
4. **Featuritis.** Adding a feature is easy; removing one is impossible. Twe v0.1 has six core declarative blocks (`entity`, `state`, `visual`, `particles`, `scene`, `dialogue`). Everything else lives in stdlib until proven necessary.
5. **Macros / metaprogramming.** Out for v0.1. Macros make code unreadable to LLMs. Add them only if a real need is documented.

---

## Open architectural questions

These are unresolved and will need answering before serious implementation:

1. **Implementation language: Rust or C++?** Rust gives us memory safety, modern tooling, and matches Bevy's ecosystem; C++ gives us mature middleware and faster game-dev hiring. **Tentative: Rust** for the VM and runtime, with a clean C ABI for embedding in any host. Reconsider if Rust compile times become a development drag.

2. **Bytecode format stability.** Stable across versions (mod-friendly) or free to break (faster iteration)? **Tentative: unstable in v0.x, stabilize at v1.0.** Mods ship source, not bytecode, until then.

3. **Module / package system.** A real one is needed (modders, library authors). We defer to v0.2 but design v0.1 with module boundaries in mind.

4. **Sandboxing.** User-generated content (Roblox-style) requires sandboxed execution. Defer to v1.0; design with isolation hooks in mind.

5. **Determinism.** Multiplayer / netcode demands deterministic execution. Float math is the usual culprit. Defer hard guarantees to v1.0+; ship "best effort" determinism in v0.x.

---

## Implementation language decision

Final commitment for v0.1 implementation: **Rust**.

Rationale:

- Bevy proves Rust is viable and ergonomic for game runtimes.
- Cargo is the best build system in mainstream use.
- The borrow checker eliminates entire classes of GC implementation bugs.
- `wgpu` gives cross-platform GPU access.
- `Rapier` gives mature physics.
- The runtime can expose a clean C ABI for non-Rust embedders.

Cost: longer compile times than C, smaller talent pool than C++, occasional borrow-checker fights. Acceptable.

---

## What this enables

If we pull this architecture off, Twe will have:

- A VM under 10,000 LOC that fits in one head.
- Coroutines / fibers transparent to the user.
- ECS-style systems generated from function signatures.
- Hot reload from day one.
- Memory safety in the VM implementation.
- A clean C ABI for embedding in any engine.
- A tooling story (LSP, formatter, tree-sitter) on par with TypeScript and Rust.

That's a defensible, modern, game-first scripting language. The next document — `04-reading-list.md` — tells the implementer where to learn how.
