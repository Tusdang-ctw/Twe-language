# Doc 04 — Reading List

> A curated list of things to read before, during, and after building Twe. Annotated. Prioritized. The reading is not optional — these resources contain decades of hard-won lessons that will save months of mistakes.

---

## Tier 1 — Read before writing the first line of code

### *Crafting Interpreters* by Bob Nystrom

The single most important resource on this list. Free online at <https://craftinginterpreters.com>.

A book that walks you through building a complete language (Lox) in two ways: first as a tree-walking interpreter in Java, then as a bytecode VM in C. Both implementations are production-quality and exquisitely explained.

What you get from it:

- The exact pattern Twe v0.1 should follow (recursive descent parser, tree-walking interpreter, then upgrade to bytecode).
- NaN-tagging for value representation (Chapter 30).
- Closures, garbage collection, and method resolution explained from first principles.
- An author whose Wren is the reference for what we're building.

How to read it: cover to cover, ideally typing in the code. Plan for ~40 hours. Do this before you write any Twe code.

### "Position Paper: Goals of the Luau Type System" (Brown, Friesen, Jeffrey, HATRA 2021)

Available at <https://arxiv.org/abs/2109.11397>.

A short academic paper from the Roblox team explaining their type system philosophy. Defines "goal-driven developers" and the "no false positives" non-strict mode. Foundational for `02-type-system.md`.

### "Goals of the Luau Type System, Two Years On" (HATRA 2023)

The follow-up. Discusses semantic subtyping and how the original goals held up under real-world use.

### Bevy ECS Quick Start

Available at <https://bevy.org/learn/quick-start/>.

The official tutorial. Read the "ECS" and "Resources" sections. You only need an hour or two; you're not learning Bevy to use it, you're learning the API design pattern Twe will adopt.

### The Wren source code

GitHub: <https://github.com/wren-lang/wren>. Specifically `src/vm/`.

Roughly 4,000 semicolons of C99. Heavily commented. The most important pages of this codebase, in priority order:

1. `wren_compiler.c` — the single-pass compiler. This is the structural template for Twe's compiler.
2. `wren_vm.c` — the bytecode interpreter loop.
3. `wren_value.h` and `wren_value.c` — the NaN-tagged value representation.
4. `wren.h` — the embedding C API.

Time investment: ~10–20 hours of careful reading.

---

## Tier 2 — Read during implementation

### *Game Engine Architecture* by Jason Gregory

The canonical reference for AAA engine architecture. You don't need every chapter, but the chapters on:

- Game loops and timing
- Memory management
- Resource and asset management
- Game systems (animation, physics, audio)

...will repeatedly clarify decisions. Borrow it from a library; you don't need to own it.

### *Game Programming Patterns* by Bob Nystrom

Free online at <https://gameprogrammingpatterns.com>. Same author as *Crafting Interpreters*.

Especially relevant chapters:

- **Game Loop** — informs `on update(dt)` design.
- **Component** — the ECS pattern, conversationally explained.
- **State** — directly relevant to Twe's `state` blocks in `ai` declarations.
- **Event Queue** — informs `on <event>:` design.
- **Service Locator** — informs how `scene`, `camera`, `time` are exposed as ambient resources.

This is shorter than *Crafting Interpreters* and easier to dip into. ~10 hours total.

### "Privacy-Respecting Type Error Telemetry at Scale" (Greenman, Jeffrey, et al., 2024)

Available at <https://users.cs.utah.edu/~blg/publications/hf/gjks-pj-2024.pdf>.

The empirical companion to the Luau papers. Describes how Roblox collects type-error telemetry from real users without violating privacy, and what they learned. Extremely useful when designing Twe's own dev environment.

### Luau RFCs

GitHub: <https://github.com/luau-lang/rfcs>.

The Luau team designs in public via RFCs. Every accepted (and rejected) RFC is a case study in language evolution. Skim the directory; read three or four that look relevant when you hit a design question.

### Bevy ECS Cheat Book (Unofficial)

<https://bevy-cheatbook.github.io>.

Practical, code-heavy reference for Bevy's ECS API. Useful when designing Twe's translation from `on update(dt, hero: Hero)` signatures to ECS queries.

### "AI Coders Are Among Us: Rethinking Programming Language Grammar Towards Efficient Code Generation" (arXiv:2404.16333, 2024)

The research foundation for the LLM-friendly grammar argument. Proposes "AI-oriented grammar" and benchmarks token reduction. Useful before making syntactic decisions that look "human-friendly" but cost LLM tokens.

### PICO-8 manual

<https://www.lexaloffle.com/pico-8.php>.

Read it for the philosophy more than the technical details. PICO-8 is the most successful indie language environment of the last decade not because it's powerful, but because it removed every barrier between idea and pixels. Twe should aspire to that elimination of pointless work.

---

## Tier 3 — Reference during deep dives

### *Programming Language Pragmatics* by Michael Scott

The textbook. Slow, thorough, encyclopedic. When you hit a question like "what should my scope rules look like?", this has the answer. Borrow it; don't read it linearly.

### *Types and Programming Languages* by Benjamin Pierce ("TAPL")

The type-theory reference. You will need it when implementing the strict mode of Twe's type system. Chapters on type inference (22), subtyping (15), and recursive types (20) are most relevant.

### "Gradual Typing for Functional Languages" by Siek and Taha

The original gradual typing paper. Cited by every gradual-typed language since.

### *Lua Reference Manual*

<https://www.lua.org/manual/5.4/>.

Even though Twe deviates from Lua, the reference manual is short, clear, and shows what a small language with a complete spec looks like. Twe's eventual reference manual should aspire to this style.

### Roblox Luau documentation

<https://luau.org>.

Practical reference for what gradual typing looks like in a shipping product. Especially the "Type Checking" pages.

### Defold engine documentation

<https://defold.com/manuals/script/>.

Defold ships Lua, but their docs do an unusually good job of explaining "scripting in a game engine" generally. Useful reference for what Twe's engine integration should look like to a user.

---

## Tier 4 — Adjacent and inspirational

These aren't directly applicable to Twe but will deepen the implementer's intuition.

### *Structure and Interpretation of Computer Programs* (SICP)

The foundational CS text. The chapters on metacircular evaluators, register machines, and compilation are directly relevant to interpreter implementation. Free online: <https://mitp-content-server.mit.edu/books/content/sectbyfn/books_pres_0/6515/sicp.zip/index.html>.

### *Compilers: Principles, Techniques, and Tools* (the "Dragon Book") by Aho, Lam, Sethi, Ullman

Reference, not reading. Look up topics; don't read cover-to-cover.

### Roc programming language documentation

<https://roc-lang.org>.

A modern functional language with refreshing design choices around effects and platforms. Not a direct influence, but reading their design rationales sharpens taste.

### "Out of the Tar Pit" by Moseley and Marks

A 2006 paper on managing accidental complexity in software. Read it once, every five years, forever.

---

## What to read in what order

A suggested 4-week reading plan before starting implementation:

| Week | Reading | Hours |
|------|---------|-------|
| 1 | *Crafting Interpreters* Parts I & II (tree-walker) | 20 |
| 2 | *Crafting Interpreters* Part III (bytecode VM) | 20 |
| 3 | Luau papers + Bevy ECS Quick Start + Game Programming Patterns (selected chapters) | 15 |
| 4 | Wren source code (slow, careful read) | 15 |

Total: ~70 hours. This is roughly what an experienced developer needs to absorb the prerequisites for Twe v0.1.

If you're newer to language implementation, double the *Crafting Interpreters* time. If you're a language nerd already, halve the Luau / Bevy time but spend the saved hours playing with Wren.

---

## Reading hygiene

A few suggestions to make the reading actually stick:

1. **Type the code as you read.** *Crafting Interpreters* is meant to be implemented, not skimmed. By the end of Part II you should have a working Lox interpreter on disk.
2. **Take notes in this repository.** Add a `notes/` folder. Each major reading gets a note with: what I learned, what surprised me, what I disagree with, what to apply to Twe.
3. **Read the rejected ideas, not just the accepted ones.** Luau RFCs that were rejected often teach more than accepted ones. Same for Wren issues that were closed without merging.
4. **Don't read everything before starting.** After the Tier 1 list, start writing Twe. Read Tier 2 as you hit problems.

---

## A final note on AI assistance

You will use AI tools heavily during implementation. That is fine and expected. But there is no substitute for actually reading the books on this list. AI assistants can produce a tree-walking interpreter; they cannot give you the *judgment* required to make design decisions over months of work. The reading is what builds judgment. Skipping it produces fragile work.
