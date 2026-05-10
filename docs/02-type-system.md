# Doc 02 — Type System Position

> Type systems are how a language tells you what is possible. They are also how a language tells you to go away. The challenge for Twe is to do the first without doing the second.

---

## The thesis

Twe's type system will be **gradual, inferred, and three-tiered**: non-strict for beginners, strict for shipping code, and a third "verified" mode designed specifically for LLM authorship.

This position is drawn almost entirely from Roblox's Luau, which has solved a near-identical problem at a scale Twe will not see for years. Where Twe deviates from Luau, this document is explicit about why.

---

## Why this question is contested

There are three live positions in scripting language design:

1. **Dynamic-only** (Lua, vanilla JavaScript, classic Python). Easy to start, painful at scale, near-useless for LLMs because errors only surface at runtime.

2. **Static-mandatory** (Rust, Haskell, TypeScript-with-`noImplicitAny`). Catches bugs early, supports tooling beautifully, but creates a steep learning curve and frequent friction with goal-driven beginners.

3. **Gradual** (TypeScript-default, Python with hints, Luau, Elixir). Types are optional and additive. The language is dynamic by default; you can pay for static checking where it earns its keep.

For a game-dev language whose audience spans children making first scripts and studios shipping commercial games, **gradual is the only defensible answer**. The question is *how* gradual.

---

## What Luau got right

The Luau team's stated philosophy is:

> "For most goal-driven developers, the type system should help or get out of the way."

This is a radical departure from Rust-style "fight the compiler until it lets you in." It encodes a respect for the user's intent: if they want to run a script, the type system should not block them; if they want feedback, it should provide accurate feedback.

Luau implements this with two modes:

- **Strict mode** aims for soundness — every flagged error is a real bug.
- **Non-strict mode** aims for "no false positives" — flagged code is guaranteed to be either dead code or to actually produce a runtime error.

The non-strict mode is the breakthrough. It uses *success typing* and *semantic subtyping* to never complain about something that might work. Beginners get autocomplete and rename refactoring; they don't get scolded for code that runs.

Real-world track record: Luau is used by Roblox (millions of creators), Remedy Entertainment (Alan Wake 2 — they removed roughly 80,000 lines of legacy scripting code by migrating), Digital Extremes (Warframe), and Giants Software (Farming Simulator 2025). Native code generation gives 1.5x–2.5x speedups on compute-heavy code; the compiler processes about 950k lines per second on a Ryzen 5900X. These are shipping numbers, not research numbers.

---

## Twe's three-tier type system

Twe takes Luau's two modes and adds a third, designed for the specific case of LLM-authored code.

### Tier 1: Non-strict (default)

Behavior:

- Types are inferred wherever possible.
- Annotations are optional.
- The checker only flags code it can prove will fail at runtime.
- Used by: beginners, prototypes, game jams, anyone iterating fast.

Example:

```twe
hero = load("hero.png")               # type inferred: Sprite (canonical v0.1 form)
hero.pos = (100, 100)                  # ok — Sprite has .pos
hero.glubjorm = "what"                 # ok at parse time, errors at runtime
                                        # (non-strict won't flag because it
                                        #  can't prove .glubjorm doesn't exist)
```

The default tier is permissive. It is the tier that should "feel like" Lua to beginners. Errors are still caught — but at runtime, with helpful messages.

### Tier 2: Strict

Behavior:

- Annotations on function signatures are required.
- The checker is sound: if it doesn't flag something, you have a guarantee.
- Used by: shipping code, library authors, anyone who wants compile-time safety.

Activation: a `# strict` (or `#! strict`) magic-comment line on one of the first ten lines of the source. Twe uses `#` for comments, so the directive is a regular comment that the inferer pre-scans for. Luau-style `--!` is also fine but `#` is the established Twe spelling.

**v0.1 implementation status (Phase 6, closed).** The directive + reporting policy + `Vec<TypeError>` accumulation ship in both `twec types` (exits non-zero on errors) and the LSP (`severity: Error` diagnostics inline). Strict mode catches: comparison mismatches, return-type conflicts, function-call argument mismatches, concrete-type arithmetic mismatches, `let` / function-param / function-return annotation violations, class field annotation violations, method param + return annotation violations, and unknown-identifier references (with `did_you_mean` suggestions). Stdlib globals are pre-seeded so `print` / `vec3` / `math.*` don't trip. See the Phase 6 closeout note `docs/changes/2026-04-29-phase-6-closeout.md`. **Deferred to v0.2:** structural-record subtyping under strict (e.g., a `{a, b}`-shaped function accepting an instance with extra fields), Luau-style "lax strict" widening rules.

Example:

```twe
# strict

function damage(target: Entity, amount: int) -> bool:
    if target.invulnerable: return false
    target.hp -= amount
    return true

# this errors at compile time (after session 2 lands annotation
# enforcement; today, the comparison/arithmetic strict checks
# fire on most real type mismatches but not yet on annotation
# violations):
damage(player, "lots")
#               ^^^^^ expected int, got string
```

### Tier 3: Verified (the LLM tier)

This is Twe's contribution.

Behavior:

- Strict mode + structured JSON diagnostics.
- All public functions must have full type signatures.
- Errors emit as machine-parseable JSON containing: file, span, expected, actual, suggested fix, related context.
- The compiler exposes a `verify` command that returns a single JSON document an LLM can consume to self-correct.

Activation: `--! verified` at file top, or `twec verify <file>` from the CLI.

Why a separate tier? Because LLM authoring has different needs than human authoring:

- LLMs don't read terminal output well; they read structured data well.
- LLMs benefit from signature constraints (they're less likely to hallucinate parameters when signatures are explicit).
- LLMs benefit from having a single feedback loop instead of three (linter, type checker, test runner).

Verified mode is a contract: *"if this file passes `twec verify`, an LLM can be confident the code is internally consistent."*

**v0.x implementation status (Phase 33 session 2, schema v2).** The
`twec verify` command emits the canonical document below. Activated by
`# verified` in the first ten lines of the source, or by passing the
file directly to the CLI. Versioned via `tool` + `version` so external
tools can negotiate cleanly.

```json
{
  "tool": "twec-verify",
  "version": 2,
  "file": "boss.twe",
  "strict": true,
  "verified": true,
  "summary": { "errors": 1, "warnings": 0 },
  "diagnostics": [
    {
      "kind": "name-error.unknown",
      "severity": "error",
      "line": 42,
      "col": 18,
      "message": "unknown name `aple`",
      "help": "did you mean `apple`?",
      "fix": {
        "rationale": "rename `aple` to `apple` (suggested by did_you_mean)",
        "edits": [
          { "line": 42, "col": 18, "len": 4, "replace": "apple" }
        ]
      }
    }
  ]
}
```

The `fix` field carries a structured patch the LLM can apply without
re-parsing free-text help. Each edit is anchored by 1-based line+col,
a byte length to replace, and the replacement text (may be empty for
deletions or contain newlines). Multiple edits inside one fix apply
non-overlappingly to the same source.

**v2 vs v1 compatibility.** All v1 fields are preserved; v2 only
*adds* the `fix` field on each diagnostic. v1 consumers reading v2
output continue to work — they just don't see the structured fix.
Schema-aware consumers should read `version` and pick up the v2
field when present.

**Coverage.** v2 ships fixes for the high-confidence
`name-error.unknown` (did_you_mean rename) kind. Other kinds —
literal-replaceable type mismatches, missing `return`, missing
annotation insertion — ride follow-on sessions because each requires
a dedicated synthesizer that can recover the original source span.
Diagnostics without a structured fix continue to carry `help` text.

This is how an LLM sits in a self-correction loop with the Twe compiler without any human in the middle.

---

## Inference strategy

Twe will use **Hindley-Milner-style inference** with extensions for:

- **Records / table types** (Luau's "structural table types"): the type of `{ x: int, y: int }` is structurally compared, not by name.
- **Tagged unions** (sum types): `Color = Red | Green | Blue | RGB(r: int, g: int, b: int)`.
- **Optional types**: `T?` is shorthand for `T | nil`.
- **Type packs** for variadic functions (Luau's contribution; needed because Twe inherits Lua's "0 or more return values" model... actually, see "Deviations from Luau" below).

Inference fires bottom-up from literals, propagates through operators, and unifies at function boundaries. The user should rarely need to write a type annotation outside of:

- Schemas (Example 7).
- Public API of stdlib and engine bindings.
- `--! strict` or `--! verified` files.

---

## Deviations from Luau

Twe is not Lua-compatible, which gives us freedom to fix some long-standing pain points:

| Luau (inherited from Lua) | Twe |
|---|---|
| 1-indexed arrays | 0-indexed arrays |
| `nil` and `false` both falsy; everything else truthy | Only `false` is falsy. `nil` requires explicit comparison |
| Functions return 0 or more values; no tuple | Functions return exactly one value (which may be a tuple) |
| `:` for method call vs `.` for field access | One operator: `.`. Method dispatch is unambiguous because methods are declared inside blocks |
| `metatables` for OOP | Declarative blocks (`item`, `entity`, `state`); single inheritance via `extends` |
| Operator overloading via metatables | Operator overloading via explicit `op +`, `op *` declarations on types |
| String concat with `..` | String concat with `+` (overloaded) and string interpolation `"hello {name}"` |

The biggest of these is probably the function-return rule. Luau inherits Lua's "0 or more values" because of compatibility; Twe has no such constraint. Functions return one value. Multiple returns are tuples, which can be destructured: `x, y = get_pos()`.

---

## Built-in types and their semantics

| Type | Notes |
|---|---|
| `int`, `float` | Distinct. `int` is i64; `float` is f64. Arithmetic between them auto-promotes to `float`. |
| `bool` | Only `true` / `false`. |
| `string` | UTF-8. Indexing returns a grapheme cluster, not a byte. |
| `nil` | Explicit absence. Cannot be coerced to `false`. |
| `vector` | `Vec2` or `Vec3`. Compiler picks based on context. |
| `color` | RGBA, components are `0..1` floats. Named constants in `color.*`. |
| `range` | Numeric range, e.g., `10..15`. Has `.roll()`, `.lerp(t)`, `.contains(x)`. |
| `duration` | Time-typed. Constructed via unit literals: `0.5s`, `200ms`, `2min`. |
| `length` | Distance-typed. `5m`, `30cm`, `100px`. |
| `mass` | `3kg`, `500g`. |
| `angle` | `90deg`, `pi rad`. Interconvertible. |
| `array of T` / `T[]` | Dynamic array. 0-indexed. |
| `map of K => V` | Hash map. |
| `set of T` | Hash set. |
| `T?` | Optional `T`. |
| `T \| U` | Tagged union. |

**Dimensional types are checked.** `5m + 3s` is a type error. `5m / 1s` produces `length / duration`, which the stdlib defines as `velocity` (`m/s`). This catches a real class of game bugs.

---

## What we are *not* doing in v0.1

- **Generics / parametric polymorphism.** Twe v0.1 will have built-in generic containers (`array of T`, etc.) but no user-defined generics. Adding them later is straightforward; getting them wrong now is hard to undo.
- **Refinement types / dependent types.** Tempting for game state ("hp must be in 0..max_hp"), but a research-grade feature. Defer to v1.0+ if at all.
- **Effect types / async coloring.** Coroutines are transparent. No `async fn` vs `fn` split.
- **Trait / interface polymorphism.** A single inheritance hierarchy plus tagged unions is enough for v0.1. If we discover we need traits, we'll know from real games.

These are deferred, not rejected. If real Twe code makes them necessary, they get added.

---

## Risks and open questions

**Risk: Three modes is one too many.** Verified mode might be better expressed as a flag on strict mode rather than a separate tier. We'll know after the first vertical-slice game; if no one ever uses non-strict + verified, the tiers might collapse.

**Open question: Should units be in the core type system, or in the stdlib?** Putting them in the core means the parser handles `5m` natively and the type checker enforces compatibility. Putting them in the stdlib means a more uniform language at the cost of unit-checking quality. **Tentative answer: core.** Game bugs from confused units are too common to ignore.

**Open question: Type variance.** Should `array of Cat` be a subtype of `array of Animal`? (Covariance.) This is a notorious source of bugs in Java. Tentative answer: invariant by default, with an explicit `array of out T` for covariant cases. Decided when we hit the first real case.

**Open question: How aggressive should non-strict's "no false positives" guarantee be?** Luau's definition is "flagged code is dead or will fail at runtime." There are weaker / stronger versions. We follow Luau exactly until we have a reason not to.

---

## Concrete next step

The type system is not on the critical path for v0.1. The plan:

1. **v0.1 ships with non-strict mode only.** Inference exists; annotations are accepted; types power autocomplete; nothing is rejected.
2. **v0.2 adds strict mode.** Soundness checking. Library authors and shipping projects opt in.
3. **v0.3 adds verified mode.** JSON diagnostics. LLM tooling integration.

This staging matches Luau's actual history — they shipped with inference for tooling first, then added soundness later. We can copy their playbook.

---

## References

See `04-reading-list.md` for the canonical Luau papers, especially:

- "Position Paper: Goals of the Luau Type System" (HATRA 2021).
- "Goals of the Luau Type System, Two Years On" (HATRA 2023).
- "Privacy-Respecting Type Error Telemetry at Scale" (2024) — empirical data on real-world type error patterns.
