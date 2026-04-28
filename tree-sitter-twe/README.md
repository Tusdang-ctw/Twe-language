# tree-sitter-twe

Tree-sitter grammar for the [Twe](../) game scripting language.

The grammar accepts every program in [`tests/programs/`](../tests/programs/)
and the eleven example programs (per Phase-3 exit criterion 3 in
[`docs/05-roadmap.md`](../docs/05-roadmap.md)). The output CST is
suitable for editor syntax highlighting, structural navigation
(go-to-definition over locals, fold ranges), and incremental
re-parsing on edits.

Twe is indentation-based (no `{`/`}`), so this grammar pairs the
JavaScript [`grammar.js`](grammar.js) with a custom external
scanner in [`src/scanner.c`](src/scanner.c) that emits `_newline`,
`_indent`, and `_dedent` tokens — same approach as
`tree-sitter-python`. Newlines inside `(...)`, `[...]`, and
interpolated strings are suppressed so multi-line expressions
work naturally.

## Build & test

```bash
cd tree-sitter-twe
npm install                    # pulls tree-sitter-cli
npx tree-sitter generate       # emits src/parser.c from grammar.js
npx tree-sitter test           # runs every test/corpus/*.txt
```

`tree-sitter generate` runs the scanner through node-gyp, so a C
compiler must be on PATH (msvc on Windows, gcc/clang on Unix).

## Try it on a real Twe file

```bash
npx tree-sitter parse ../tests/programs/methods.twe
```

## Layout

- [`grammar.js`](grammar.js) — the grammar in tree-sitter's JS dialect.
- [`src/scanner.c`](src/scanner.c) — INDENT / DEDENT / NEWLINE
  tokenizer that complements the generated grammar.
- [`test/corpus/`](test/corpus/) — table-driven syntax tests in
  the tree-sitter [test format](https://tree-sitter.github.io/tree-sitter/creating-parsers#command-test).
  Each `.txt` file is a list of `(name, source, expected CST)`
  triples; `tree-sitter test` parses each source and asserts the
  CST matches.

## Status

Sessions 18 + 19 of Phase 3. Covers the full Twe surface used by
the eleven examples. Future work:

- `queries/highlights.scm` for editor syntax highlighting.
- `queries/locals.scm` for scope-aware go-to-definition.
- `queries/folds.scm` for editor fold ranges.
- VS Code extension that wraps this grammar (session 20).
