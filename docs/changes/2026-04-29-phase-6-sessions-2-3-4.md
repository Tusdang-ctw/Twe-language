# 2026-04-29 — Phase 6 sessions 2–4: annotations, tutorial, error polish

## Status: implementation note. Three sessions in one commit.

## Background

Phase 6 session 1 (`docs/changes/2026-04-29-phase-6-session-1-strict-mode.md`) shipped the strict-mode reporting policy. Sessions 2 (annotation enforcement), 3 (tutorial draft), and 4 (error-message polish) close the rest of the Phase 6 v0.1-relevant surface in one shot.

## Session 2 — annotation enforcement

### AST changes

- New `pub struct ast::Param { name: String, ty: Option<Type> }`.
- `Stmt::FunctionDecl.params: Vec<String>` → `Vec<Param>`. Added `Stmt::FunctionDecl.ret: Option<Type>`.
- `Stmt::Let` gained `ty: Option<Type>`.

`DeclMember::Method.params` stays `Vec<String>` for v0.1 — annotation enforcement on methods rides session 5+ when class-shape unification is generalised. The parser still parses method-param annotations and discards them so existing programs keep parsing.

### Parser

`parse_type` returns `Result<Option<Type>, ParseError>`. Recognised primitives map to `Type` variants (`int`, `float`, `bool`, `string` / `str`, `nil`, `range`); class names and qualified types (`vector.x`) return `None` — strict mode then degrades gracefully and doesn't enforce them.

`parse_let`, `parse_function`, `parse_param` populate the new fields. `parse_function` consumes `-> <type>` for the return annotation.

### Inferer

In strict mode, three new constraints fire at decl time:

- `let x: int = "hi"` — unify the value's inferred type against the annotation. **kind:** `"let annotation"`.
- `function f(n: int)` — unify each param's fresh var against its annotation before walking the body. Subsequent call sites then trigger "call argument: type mismatch" via the existing arg-unify path. **kind:** `"param annotation"`.
- `function f() -> int: return "hi"` — unify the function's `ret_var` against the annotated return type. The body's collected returns then either fit or produce a return-type union that fails the unify. **kind:** `"return annotation"`.

In non-strict mode (the default), all three unifies still happen but their failures stay silent — same Luau-style "no false positives" contract as the rest of the engine.

### Tests

6 new tests in `infer::tests`:

- `strict_let_annotation_violation_surfaces`
- `strict_let_annotation_clean_passes`
- `strict_param_annotation_pins_arg_check`
- `strict_return_annotation_violation_surfaces`
- `non_strict_drops_annotation_violations`
- `unrecognised_type_name_silently_skips_enforcement`

### Snapshot updates

The three `parse__json_dump_for_*.snap` snapshots were regenerated to include the new `annotation` / `returnAnnotation` fields on the JSON output. No behavioural change — just reflecting the AST's new shape.

## Session 3 — tutorial draft

`docs/tutorial.md` — a first-hour walkthrough that ends with a small playable game. ~1500 words covering:

- "What Twe is" — the design audience (humans + LLMs as first-class authors), the phrase "game concepts are first-class."
- A `scene` with state machines and `every`-clocks; running it headless via `twec run --frames N`.
- `let` vs `var`, optional annotations.
- `on update(dt):` and `on render():` — the simulation/render split and why each lives where it does.
- Input via the `key.*` ambient and `key_press.*` for edge-triggered.
- Entities + `spawn` / `despawn` / `entities.of(Class)`.
- Predicate hooks (`on hp <= 30:`) and how their edge-triggered semantics avoid the "did I check this last frame?" boilerplate.
- Cooperative `wait <duration>` and the v0.1 restriction (state on-entry only).
- Dialogue: `dialogue Name:` with `say`, `choice`.
- A full 3D demo program the reader can paste into `twec play3d` and edit live (hot reload).
- Strict mode: the `# strict` directive, what it flags today, what's still session-5+ work (class/method annotations).
- "Where to go next" pointers into the design docs and `notes/future-phases.md`.

The tutorial is honest about what v0.1 ships vs what v0.2 absorbs — the explicit "v0.1 doesn't yet ship: `.glb` mesh import, tilemap rendering, save/load schemas, function-body `wait`, mouse input" pre-empts users hitting walls without being told why.

This is a first-draft floor; the next pass would walk through building Pong or a tiny RPG end-to-end with screenshots, but the words are enough to ship v0.1.

## Session 4 — error-message polish

### `did_you_mean` helper in `src/value.rs`

A bounded Levenshtein-distance suggestion engine. Distance ≤ 1 for short names (≤ 4 chars), ≤ 2 for longer names. Returns `None` on exact match or tie — printing "did you mean: foo or bar?" is worse than printing nothing, since the user pursues a wrong fix when the suggestion is mis-targeted.

Implementation: standard 2-row dynamic-programming Levenshtein, with an early-exit when a row's minimum exceeds the cap. Empty target returns `None`.

6 unit tests (`one_char_typo`, `unrelated`, `exact_match`, `tie`, `short_names_distance_1`, `longer_names_distance_2`) pin the contract.

### Wired into runtime errors

- **Unknown field on instance** (`eval::eval_assign` for `Field` target) — when `obj.field` references a field that doesn't exist on the instance, the help line now offers `did you mean \`<close-match>\`?` if there's a single clear suggestion. Falls back to the previous explanation if no match.
- **Unknown state in transition** (`eval::enter_state` and `vm::enter_state`) — `-> fini` when the actual state is `finish` produces `help: did you mean \`-> finish\`?`. Both backends.

### Help text filled in on six bare errors

Every `help: None` site that surfaces a common runtime error now carries actionable text:

- `\`return\` is only valid inside a function or method body` — explains state-body alternatives.
- `\`break\` is only valid inside a loop` — names the two loop forms.
- `\`continue\` is only valid inside a loop` — same.
- `\`self\` is only valid inside a method body` — explains where `self` is bound.
- `tuple index N out of bounds (length M)` — names the valid index range.
- `every <duration> needs a time unit` — lists the four supported units with examples.

The remaining ~85 `help: None` sites are mostly internal compile-error / VM-bug paths — they wouldn't help a user even if filled in. Polishing those has diminishing returns and was deliberately scoped out.

### Verification

End-to-end smoke test on a typo:

```
scene Demo:
    initial: alert
    state alert:
        every 50ms: -> finis
    state finish:
```

```
$ twec run --frames 5 typo.twe
runtime error: 0:0: no state named 'finis'
  help: did you mean `-> finish`?
```

The suggestion appears at the right edit distance (1 — `finis` to `finish` is one insertion). Distance-2 typos like `fini` produce no suggestion (correctly — `fini` could plausibly mean `finish`, `final`, `find`, or be a totally separate name; printing one when ambiguous would mislead).

## Verification

- `cargo build --release` — clean.
- `cargo clippy -- -D warnings` — clean.
- `cargo test` — **418 tests pass** (12 new tests across the three sessions: 6 annotation + 6 `did_you_mean`).
- Type-check sweep across all 33 on-disk programs — all pass. None opt into strict mode, so annotation enforcement stays inert for them.

## Doc edits applied as a result

- `docs/02-type-system.md` — Tier 2 strict-mode section gains the annotation-enforcement status note.
- `docs/05-roadmap.md` — Phase 6 §"Status" reflects sessions 1–4.
- `notes/future-phases.md` — Phase 6 plan: sessions 1–4 marked done; method-annotation enforcement and tutorial-iteration noted as session-5+ work.
- `CLAUDE.md` — Phase 6 plan updated.
