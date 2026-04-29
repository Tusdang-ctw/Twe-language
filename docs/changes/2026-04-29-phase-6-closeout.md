# 2026-04-29 — Phase 6 closeout

## Status: closeout note. Closes Phase 6 (tooling, polish, documentation) and enumerates the residual work that lives in Phase 7 (release) or v0.2.

## Background

Phase 6 opened on the same day Phase 5 closed (`docs/changes/2026-04-29-phase-5-closeout.md`) and ran across eight sessions:

- **Session 1** — `# strict` directive + reporting policy (`2026-04-29-phase-6-session-1-strict-mode.md`).
- **Sessions 2–4** — annotation enforcement on `let` / function params / return types, tutorial draft, error-message polish (`2026-04-29-phase-6-sessions-2-3-4.md`).
- **Sessions 5–7** — strict-mode unknown-identifier diagnostics, VS Code packaging readiness, `sphere()` 3D primitive (`2026-04-29-phase-6-sessions-5-6-7.md`).
- **Session 8** (this commit) — class member annotation enforcement.

This note draws the v0.1-release-ready line.

## Session 8 — class member annotation enforcement

### What ships

- **`DeclMember::Field`** gains `ty: Option<Type>` — the parser keeps the parsed annotation instead of discarding it.
- **`DeclMember::Method`** changes `params: Vec<String>` → `Vec<Param>` and adds `ret: Option<Type>` — symmetric with `Stmt::FunctionDecl` from session 2.

### Inferer enforcement (strict mode)

- **`field annotation`** — `entity Hero: hp: int = "hi"` unifies the value's inferred type (Str) against the annotation (Int). Strict surfaces the mismatch; non-strict drops it. The annotation, when present, becomes the field's canonical type in the class shape (overriding the inferred type) so subsequent `instance.hp` accesses see Int.
- **`method param annotation`** — same shape as the function-param case (session 2). Each method param's fresh var is unified against its annotation at class-shape registration time.
- **`method return annotation`** — same shape as the function-return case. The method's `ret_var` is unified against the annotation, and the body's collected returns then either fit or surface a "return: type mismatch."

5 new tests:
- `strict_field_annotation_violation_surfaces`
- `strict_field_annotation_clean_passes`
- `strict_method_param_annotation_pins_call_site`
- `strict_method_return_annotation_violation_surfaces`
- `non_strict_drops_class_annotation_violations`

### Printer

The formatter now round-trips field annotations (`name: int = 0` instead of stripping the type) and method annotations (`name(p: int) -> int:`). The `formats_scene_with_state_and_every` test was updated — its previous expectation was based on the stripped form, which was a pre-session-4 limitation.

### Snapshot updates

`parse__json_dump_for_example_2_simplified.snap` regenerated to include the new `annotation` / `returnAnnotation` fields on the JSON output. No behavior change, just AST shape reflection.

### Not in scope (deferred to v0.2)

- **Structural-record subtyping under strict.** When class A has fields {a, b, c} and a function takes a `{a, b}` record, strict should accept A. Currently non-strict accepts because Unknown widens; strict would need a real subtype-check rule. Real type-system work — needs design before code.
- **Luau "lax strict" widening rules.** Luau allows `(int) -> int` to accept a string at the call site if the call-site context is also `?`. Replicating that needs the inference engine to track "context strictness" alongside structural types — substantial. Defer until users actually push back on the current strict strictness.

## Phase 6 — what shipped

| # | Surface | Status |
|---|---------|--------|
| 1 | Strict mode reporting policy | ✅ Session 1 |
| 2 | Annotation enforcement on `let` / function params / return | ✅ Session 2 |
| 3 | Tutorial draft (`docs/tutorial.md`) | ✅ Session 3 |
| 4 | Error-message polish + `did_you_mean` helper | ✅ Session 4 |
| 5 | Strict-mode unknown-identifier `did_you_mean` | ✅ Session 5 |
| 6 | VS Code extension packaging readiness | ✅ Session 6 |
| 7 | `sphere()` 3D primitive (first v0.2 carry-over delivered) | ✅ Session 7 |
| 8 | Class field + method annotation enforcement | ✅ Session 8 |

## Deferred

### To v0.2

- **Structural-record subtyping under strict** (needs design conversation).
- **Luau lax-strict widening rules** (Luau-specific design decision).
- **Tutorial iteration pass-2** — screenshots, longer end-to-end walkthrough. Driven by user feedback once v0.1 is in real hands. Captured in `notes/future-phases.md`.
- All v0.2 carry-over items already enumerated in `docs/changes/2026-04-29-phase-5-closeout.md` (`.glb` mesh import, tilemap, save schemas, NaN tagging, function-body `wait`, mouse input, etc.).

### To Phase 7 (release)

- **VS Code marketplace publish** — packaging is ready (`vsce package` works); the publish itself is account-bound and rides the v0.1 release cut.
- **GitHub Release with binaries** for Windows / macOS / Linux.
- **Project website** — playground, examples gallery, docs index.
- **Show-HN-quality blog post + demo video.**
- **Contribution guide and governance model.**

These are Phase 7 deliverables per `docs/05-roadmap.md` — not Phase 6 follow-ons.

## Phase 6 exit criteria

The roadmap's Phase 6 goal is *"Twe is usable by people who aren't the implementer."* Status against the implied checklist:

- [x] **Comprehensive tutorial** — `docs/tutorial.md`. First-pass complete; iteration is post-v0.1.
- [x] **Reference manual** — `docs/06-design-document.md` is the formal spec; every keyword + every stdlib function is documented through Phase 6's surface additions.
- [x] **Strict mode (Tier 2 of the type system)** — opt-in directive + reporting policy + annotation enforcement on lets, function params, function returns, class fields, method params, method returns. Outstanding strict-mode work (structural subtyping, lax-strict widening) deferred to v0.2 — they're refinements, not blockers.
- [x] **Better error messages** — every common error now carries help text; `did_you_mean` suggestions on unknown fields, unknown states, and unknown identifiers in strict mode.
- [~] **VS Code extension** — packaging-ready, but not yet published. Publishing rides Phase 7 (release).
- [ ] **Web playground** at `twe-lang.org/play` — explicitly Phase 6 polish per the roadmap; deferred. Real consumers don't need a hosted playground for v0.1; a published release with the LSP and tutorial is enough.
- [ ] **Examples gallery (~20 small programs)** — current `examples/` directory has 5 programs; expanding to 20 is a Phase 7 polish task driven by what users build.

The strict reading of the roadmap leaves two checkbox items partially unchecked (playground hosting, examples gallery). Both are reasonably scoped to Phase 7 release-prep rather than blocking Phase 6 closure — they're "make Twe more discoverable post-release," not "make Twe usable."

## Verification

- `cargo build --release` — clean.
- `cargo clippy -- -D warnings` — clean.
- `cargo test` — **427 tests pass** (5 new in session 8; no regressions).
- Type-check sweep across all 33 on-disk programs — all pass.

## Doc edits applied as a result

- `CLAUDE.md` Phase discipline updated: Phase 6 closed; Phase 7 (release) becomes the active phase.
- `docs/05-roadmap.md` Phase 6 §"Status" reflects closeout; Phase 7 §"Status" lists the v0.1 release prep items.
- `notes/future-phases.md` consolidates Phase 6 retro and v0.2 carry list.
- `docs/02-type-system.md` Tier 2 strict-mode section: removes "session 5+" caveats now that class/method annotations enforce.
