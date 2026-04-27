# notes/future-phases.md

> Items intentionally deferred from the current Phase 1 work. Capture here so we
> don't lose them; do not act on them until the active task warrants.

## Open language-design questions

### `sprite` as keyword vs built-in type

Status: **lexed as a keyword** in `src/lexer.rs`, per CLAUDE.md's First Task
directive. Conflicts with `06-design-document.md §10.1`'s reserved-words list,
which omits `sprite`. The parenthetical "(or a built-in type)" in
`01-examples.md` Example 1 leaves this open. Resolution required before the
parser handles the `sprite hero = ...` declaration form.

Two viable shapes:

1. Add `sprite` to §10.1 as a reserved word; declarations look like
   `sprite ident = expr` in addition to `let`/`var`. Ergonomic for Example 1
   but adds a one-off declaration form.
2. Treat `sprite` as a built-in type identifier; require `let hero: sprite = load(...)`.
   Uniform with `let`/`var` but more boilerplate at the entry point.

### Unicode in identifiers

`06-design-document.md §2.1` permits any Unicode scalar value in identifiers;
`§2.4`'s EBNF restricts identifiers to ASCII letters, digits, and underscore.
Current lexer follows §2.4 (ASCII-only). Reconcile §2.1 or §2.4 before any
non-ASCII identifier enters the test suite.

## Lexer features deferred

Required by the design doc but not by Example 1's first chunk:

- `INDENT` / `DEDENT` tokens (§2.2). Required for declarative blocks.
  Includes the "mixing tabs and spaces in one file is a parse error" rule.
- Comments: `#` line, `#:` doc, `#- ... -#` block (§2.3).
- Escape sequences in strings (§2.5.2). Currently rejected with a help message.
- String interpolation `"hi {name}"` (§2.5.2).
- Triple-quoted strings `""" ... """` (§2.5.2; needed by Example 9).
- Float literals, hex `0xFF`, binary `0b1010`, digit separators `1_000_000`
  (§2.5.1).
- Range literals `..` and `..<` (§2.5.4; needed by Example 2).
- Percent literals `5%` (§2.5.5; needed by Example 2).
- Unit literals `3kg`, `5 m/s`, `90deg` (§2.5.6; needed by Example 1 onward).
- Comparison and logical operators (`==`, `!=`, `<`, `>`, `<=`, `>=`,
  `and`, `or`, `not`).
- Brackets and braces (`[ ] { }`).
- Recoverable error reporting — current lexer fails fast on the first error.

## Parser, AST, evaluator

Module stubs exist (`src/parser.rs`, `src/ast.rs`, `src/eval.rs`,
`src/value.rs`, `src/stdlib.rs`) but are intentionally empty until the lexer
covers enough of Example 1 to feed a parser.

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

## Tooling debt before the next session

- **Reconcile the `sprite` keyword** with `docs/06-design-document.md §10.1` —
  either add it with a note in `docs/changes/`, or remove it from the lexer.
- **Create `docs/changes/`** the first time a design pivot lands; CLAUDE.md
  doc-discipline expects it.
- **Pick a license.** `README.md` says "TBD: MIT or Apache-2.0".
