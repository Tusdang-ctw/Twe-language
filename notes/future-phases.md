# notes/future-phases.md

> Items deferred from the active phase. Capture here so we don't lose them;
> do not act on them until the active task warrants.

## Phase 1 retro (complete)

Phase 1 closed at commit `7c4c06c`. Tree-walker runs Examples 1, 2 (simplified),
and the eleven test programs in `tests/programs/`. Code totals ~3000 LOC of
Rust against the roadmap's 6000-LOC budget — leaner because the runtime-heavy
items the roadmap parked in Phase 1 (full unit-aware arithmetic, `wait`/`every`
fiber semantics, scene/dialogue runtime) cleanly migrate to Phase 2.

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

## Phase 2 plan (active)

Goal: ship Snake first as the warm-up game (per `docs/example-11-snake.md`),
then iterate to a Vampire Survivors clone for the formal Phase 2 exit.

Milestone order, by dependency (graphics-free first):

1. Lists (NP2) — `[1, 2, 3]`, `.append`, `.prepend`, `.pop_back`, `.length`,
   `[i]` indexing, `in` operator.
2. Random — deterministic xorshift PRNG, `random.int(range)`,
   `random.choice(list)`, `range.roll()`.
3. String interpolation `"Score: {score}"` (per §2.5.2).
4. `scene` blocks with var-typed fields and an `initial:` state.
5. `state` blocks inside `scene`, `every <duration>:`, `->` transitions.
6. `key_press.<name>` press events (NP1) — distinct from held `key.<name>`.
7. macroquad dependency + window + main loop.
8. Drawing primitives `rect`, `text`, `circle`, `line` (NP5).
9. `screen` ambient resource (NP6).
10. `on render():` distinct from `on update(dt):`.
11. Real keyboard input wired into `key.*` and `key_press.*`.
12. `tests/programs/snake.twe` ships and runs at 60 fps.

After Snake: pivot to the Vampire Survivors slice for the actual Phase 2
exit criteria (playable game, hot reload, < 500 lines of Twe, frustration
list for Phase 3).

## Open language-design questions

### Unicode in identifiers

`06-design-document.md §2.1` permits any Unicode scalar value in identifiers;
`§2.4`'s EBNF restricts identifiers to ASCII letters, digits, and underscore.
Current lexer follows §2.4 (ASCII-only). Reconcile §2.1 or §2.4 before any
non-ASCII identifier enters the test suite.

### Loader API doc-cleanup

`02-type-system.md` shows `sprite.load(...)` (namespaced); `01-examples.md`
Example 1 (post 2026-04-27) shows `load(...)` (bare). Both work; pick a
canonical form during Phase 2 stdlib build-out.

## Phase 2 deferred (caught in scope)

These ship after Snake, before the VS slice:

- `set of T` type literal `{1, 2, 3}` and `set()` for empty (NP4).
- `match` expressions (§3.6).
- `map` literals `{ "k": v }` (§3.5).
- List slicing `[i:j]`.
- Tuple-typed list elements explicitly annotated (NP10) — the type
  annotation parser already accepts this in non-strict mode.
- `function` return-type checking (currently parsed-and-ignored).

## Phase 3+ items captured to keep current scope honest

Per CLAUDE.md ("If you find yourself thinking about NaN tagging or shader
compilation, stop. Note it here."):

- NaN-tagged 64-bit value representation (Phase 3).
- Single-pass bytecode compiler from AST (Phase 3).
- Incremental tracing GC (Phase 3).
- Computed-goto interpreter loop (Phase 3).
- Strict-mode type checker (Phase 4).
- Verified-mode JSON diagnostics (Phase 4+).
- `visual` block → fragment shader compilation (Phase 5).
- LSP server, formatter, tree-sitter grammar (Phase 3 tooling).
- List comprehensions (Snake NP3 — defer until 5+ wants in Phase 2).
- `on enter:` / `on exit:` state hooks (Snake NP9 — defer until 3+ wants).
- String interpolation `\u{...}` Unicode escapes.
- Compound unit literals `5 m/s` (Phase 2 if dimensional checking ships).

## Tooling debt

- **Pick a license.** `README.md` says "TBD: MIT or Apache-2.0".
- **`docs/02-type-system.md` and `docs/05-roadmap.md`** mention `sprite.load`
  while `docs/01-examples.md` Example 1 uses bare `load`. Pick one
  during Phase 2 stdlib build-out.
