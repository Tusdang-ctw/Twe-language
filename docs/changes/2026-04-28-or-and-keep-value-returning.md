# 2026-04-28 — `or` / `and` keep value-returning semantics

## Status: design lock-in (no code change)

## Background

Phase 2 frustration list F11:

> `if key.right or key.d: ...` works because of truthiness, but
> `a or b` returning `a` (when `a` is truthy) means downstream code
> can't rely on the result being a Bool.

The frustration entry left the decision punted: "Locked decision for
v0.2; recorded in `docs/changes/`." This note is that record, made one
phase earlier than promised because Phase 3 session 4 asked for it.

## Decision

`or` and `and` stay **value-returning, short-circuit, Python-like**:

| Expression | Result (current, unchanged) |
|---|---|
| `a and b` | `a` if `a` is falsy, else `b` (without evaluating `b` if `a` is falsy) |
| `a or b`  | `a` if `a` is truthy, else `b` (without evaluating `b` if `a` is truthy) |
| `not a`   | strict `Bool(!is_truthy(a))` |

`is_truthy` follows Twe's locked rule (Principle 3, `docs/03-runtime.md`):
**only `false` is falsy.** `nil`, `0`, empty strings, empty lists, and
zero-length tuples are all truthy. This is what makes value-returning
`or` / `and` safe — the C/JavaScript footguns (`if (count) { ... }`
silently skipping when `count == 0`) cannot apply.

## Why not strict-bool

- **Survive, Snake, Hero, sprite_demo, particles_demo all use `or` /
  `and` only inside `if` conditions.** No shipped program reads the
  *value* produced by `or` / `and`, so strict-bool would impose
  ergonomic cost (lost `x or default` idiom) for zero practical
  benefit in the v0.1 corpus.

- **The "looks-like-Bool but isn't" complaint dies under Twe's
  truthy rule.** In Lua / Python / JS, `0 or "default"` returns
  `"default"` because `0` is falsy — a real footgun. In Twe, `0` is
  truthy, so `0 or "default"` returns `0`. Same rule for `nil`. The
  one place this *can* still surprise — `false or "default"` returns
  `"default"` even though the user might have wanted `Bool(true)` —
  is exactly the case where the user almost certainly *did* want
  the default value, since `false` is the only falsy thing.

- **Strict mode (Phase 4) can opt in via the type system.**
  `let result: Bool = a or b` will type-error in strict mode unless
  both operands are Bool. That gives the no-surprise guarantee to
  users who want it, without forcing the cost on everyone.

- **Consistency with Wren and Lua.** Both languages took the same
  call. Twe's design lineage (per CLAUDE.md "Always-available
  references") favours alignment here.

## What this changes in the docs

- `docs/06-design-document.md` §3.5 gets a one-line note clarifying
  that `or` / `and` are value-returning, `not` is strict.
- The frustration list F11 entry marks closed, points here.
- A regression test `or_and_value_returning_semantics` locks in the
  behaviour so a future change can't silently flip it.

## Reopening

If a Phase 4+ user complaint surfaces — e.g. an LLM-generated program
treats `or`'s result as Bool and an unrelated bug stems from that —
revisit by editing this note. Strict mode (Phase 4) gives an escape
hatch first; only revisit value-returning if strict mode also turns
out to be insufficient.
