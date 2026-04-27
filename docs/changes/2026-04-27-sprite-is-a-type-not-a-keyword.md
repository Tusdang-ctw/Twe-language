# 2026-04-27 — `sprite` is a type, not a keyword

**Status:** Resolved. **Decision:** B (identifier).

## Background

The lexer's reserved-words set initially included `sprite` per CLAUDE.md's
"First task" directive. After analyzing all eleven examples and the seven
design docs, this conflicts with three locked decisions and required either
mutating Example 10 or violating Principle 4.

## Decision

`sprite` is **not** a reserved keyword. It is a regular identifier — used
as a built-in type name in type-annotation position, and as a stdlib
namespace (`sprite.load`, etc.) per `02-type-system.md` and `05-roadmap.md`.

Example 1's first line is now `let hero = load("hero.png")`. The type of
`hero` is inferred from `load`'s return value (Tier 1 inference, per
`02-type-system.md §5.1`).

## Rationale

1. **Principle 1** (CLAUDE.md, "The Five Principles") names the six
   first-class game concepts: `entity`, `state`, `visual`, `dialogue`,
   `particles`, `scene`. `sprite` is not on that list, and "What is locked"
   in CLAUDE.md re-confirms the six.
2. **Principle 2** ("one obvious way per concept") — three declaration
   introducers (`let`, `var`, `sprite`) violates regularity. Two is one;
   three is "wait, when do I use which?"
3. **Principle 4** ("no context-sensitive parsing") rules out the
   contextual-keyword workaround that would have been needed to keep
   Example 10's `sprite:` field name working.
4. The lowercase token `sprite` already plays three different roles in the
   existing docs: declaration introducer (Example 1), field name in a
   declarative block (Example 10, `sprite: load("slime_king.png")`), and
   stdlib namespace (`02-type-system.md §5.2.1`, `05-roadmap.md §Phase 2`).
   Promoting it to keyword breaks the latter two.
5. Type inference (`02-type-system.md §5.1`) means the keyword form is *not*
   shorter than the inferred form. Char counts for Example 1 line 1:

   | Form | chars |
   |------|-------|
   | `sprite hero = load("hero.png")` (A) | 30 |
   | `let hero: sprite = load("hero.png")` (B with annotation) | 35 |
   | `let hero = load("hero.png")` (B with inference) | **27** |

## Changes in this commit

- `src/lexer.rs`: removed `TokenKind::Sprite`; `sprite` now lexes as
  `Ident("sprite")`. Removed the corresponding match arm in `lex_ident`.
- `docs/01-examples.md`: Example 1 source line 1 changed to
  `let hero = load("hero.png")`. The "Implied decisions" bullet that called
  `sprite` a keyword is rewritten to describe inference and the stdlib
  namespace.
- `tests/lexer.rs`: snapshot input updated to match the new Example 1; new
  test `sprite_is_an_identifier_not_a_keyword` asserts that `sprite` lexes
  as `Ident("sprite")`.
- `tests/snapshots/lexer__lexes_example_1_first_chunk.snap`: regenerated.
- `notes/future-phases.md`: this open question removed; the corresponding
  tooling-debt entry removed.

## Compatibility check (per CLAUDE.md "Examples are the spec")

- **Example 1** changed (intentional, in service of P2 + inference).
- **Examples 2–9, 11**: unchanged.
- **Example 10**: previously *broken* under Option A (its `sprite:` field
  would have become a parse error); now compiles as written.
- `02-type-system.md §5.2.1` (`hero = sprite.load("hero.png")`): unchanged.
- `05-roadmap.md` Phase 2 stdlib (`sprite.load`, …): unchanged.

## Open follow-up (not blocking)

`02-type-system.md §5.2.1` shows `hero = sprite.load("hero.png")` (namespace
form). `01-examples.md §Example 1` after this change shows
`let hero = load("hero.png")` (bare form). Both can coexist (bare `load`
resolves via global stdlib; `sprite.load` is the namespaced form), but a
future doc-cleanup pass should pick one canonical loader API to avoid
confusion.
