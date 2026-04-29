# 2026-04-29 — Phase 6 session 1: strict-mode opt-in

## Status: implementation note. Opens Phase 6 (tooling, polish, documentation). Lands the strict-mode reporting policy that Phase 4 deferred.

## Background

Phase 4 shipped Hindley-Milner inference with a "no false positives" reporting policy: when unification fails, the offending types stay `Unknown` and no error reaches the user. That's Luau's contract — non-strict mode prioritises silence over completeness, so legacy code keeps running.

`docs/02-type-system.md` describes a three-tier model: non-strict (default) → strict (opt-in) → verified (LLM-assisted, post-v0.1). This session lands tier 2.

The deferral was explicit in `docs/changes/2026-04-29-phase-3-and-4-closeout.md` — the inference engine itself is enough; strict mode is the same engine surfacing failures instead of dropping them.

## What ships

### Opt-in syntax: `# strict` directive

A magic-comment line on one of the first ten lines of the source. Equivalent forms accepted: `# strict`, `#! strict`, `#strict`, `#!strict`. After trimming whitespace, the line must equal one of those exactly — `# strict mode` and `# strict-ish` don't trigger.

Why magic comment over a top-level keyword:

- A `strict` keyword would steal the identifier from existing programs. Any of the 32 on-disk programs could legally have `let strict = true`; promoting `strict` to a keyword would break them.
- Magic-comment is the established convention — Perl `use strict`, Luau `--!strict`, Python `# coding: utf-8`. Familiar surface for users coming from those languages.
- Restricted to the first ten lines so a `# strict` deep in the file (in dead code, a string-formatted help text, a multi-line comment) doesn't flip the mode by accident.

### Strict reporting policy

Same inference engine, same unification, same constraint solving. The only difference: `Inferer.strict: bool` flag + `errors: Vec<TypeError>` accumulator. Every `let _ = unify(...)` call site replaced with `self.try_unify(a, b, line, col, kind)`:

- Non-strict (default): `try_unify` calls unify and drops the result. Identical to v0.1 behaviour.
- Strict: on `Err`, push a `TypeError` carrying source line/col, the conflicting types as printed strings, the constraint kind (`"comparison"`, `"and/or operands"`, `"string \`+\`"`, `"arithmetic"`, `"return"`, `"call argument"`), and a help suggestion.

Source positions thread through `Expr::Binary { line, col }`, `Expr::Call { line, col }`, and `Stmt::FunctionDecl { line, col }`. `binop_type`, `infer_add`, `infer_arith`, `walk_function_body`, and `finalise_return_type` all gained `(line: u32, col: u32)` parameters so the diagnostics point at the user's source.

### Concrete-type arithmetic mismatch

The original `infer_arith` for two concrete-but-non-numeric types (e.g. `Str + Int`) silently returned `Type::Unknown` — no `unify` call, so nothing to drop. Strict mode now also pushes a diagnostic for that case, since `5m + 3s` (the dimensional-units example in `docs/02 §"Dimensional units"`) is the canonical strict-mode error.

### CLI: `twec types <file>`

- Reads the source.
- Detects `# strict` via `infer::detect_strict`.
- Calls `infer_program_strict(program, strict)` which returns `(Bindings, Vec<TypeError>)`.
- Prints bindings (sorted by name, as before).
- Prints any errors to stderr in `path:line:col: type error: <message>` form with `help:` lines.
- Exits **non-zero** when strict errors exist, so CI / pre-commit hooks gate strict files on success. Non-strict files never reach this branch (errors vec is always empty when `strict = false`).

### LSP: type-error diagnostics

`collect_diagnostics` now runs strict inference on opted-in files and surfaces type errors as `severity: Error` diagnostics alongside parse errors. VS Code shows them inline immediately on edit.

## Tests

13 new tests in `src/infer.rs::tests`:

- 4 directive-detection tests: canonical forms accepted, first-ten-line restriction, partial-match rejection.
- 6 strict-mode behaviour tests: comparison mismatch surfaces an error, return type conflict surfaces a call-argument error (the natural firing path given current widening rules), line/col carry from source, help text present, non-strict drops everything.
- All 397 prior tests remain green. Total: **406**.

Plus a new `tests/programs/strict_clean.twe` — a strict program that type-checks cleanly, sanity-check that strict mode is additive (clean programs stay clean).

## What this does NOT yet ship

Captured for sessions 2-3 of strict mode:

- **Annotation-driven errors.** Function-param and variable type annotations (`function add(a: int, b: int) -> int:`) are still parsed and discarded. To surface "you said `a: int` but called `add("hi")`," the AST has to keep `Vec<Param { name, ty: Option<Type> }>` and the inferer has to unify the annotation against the fresh var at function-decl time. Session 2.
- **"Did you mean" hints on identifier-not-found.** Currently a typo'd ident silently resolves to `Type::Unknown`. Strict mode could fuzzy-match against the current scope and suggest. Session 3.
- **Structural-record subtyping under strict.** When a class shape has fields A, B, C and a method expects {A, B}, strict should accept (subtype) but currently doesn't have the structural check. Session 3.
- **`# strict` test coverage on the eleven examples.** None of `docs/01-examples.md` programs opt in today; an audit would show how clean each is under strict. Worth doing once annotations are in (session 2).
- **Function-argument widening rules (Luau "lax strict").** Luau allows `(int) -> int` to accept a string at the call site if the call-site context is also `?`. Replicating that needs careful design and isn't a v0.1 commitment.

These follow-ons are normal Phase 6 sessions, not v0.2 deferrals — strict mode is on the v0.1 release path per `docs/02-type-system.md` § "Strict opt-in (v0.1+)".

## Verification

- `cargo build --release` — clean.
- `cargo clippy -- -D warnings` — clean.
- `cargo test` — 406 tests pass.
- `twec types tests/programs/strict_clean.twe` — exits 0, prints bindings.
- `twec types <strict file with mismatch>` — exits 1, prints bindings + `path:line:col: type error: ...` + help.
- Type-check sweep across all 33 on-disk programs — all pass (none opt into strict).

## Doc edits applied as a result

- `notes/future-phases.md` Phase 6 section gains an entry for strict-mode session 1; sessions 2-3 listed as planned follow-ons.
- `CLAUDE.md` Phase 6 plan adjusts: strict-mode session 1 is shipped; tutorial / error polish / VS Code packaging remain as openers.
- `docs/02-type-system.md` Strict-mode section gets a status note (deferred → shipped session 1).
- `docs/05-roadmap.md` Phase 6 entry mentions strict-mode session 1 landing.
