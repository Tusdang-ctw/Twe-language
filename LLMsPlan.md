# LLMs Plan — Making Twe the Most LLM-Native Language

> Phase 33 strategy doc. Read this if you want to understand why Twe makes
> different design choices than every other game scripting language —
> and how those choices add up to a defensible moat for LLM-driven
> game development.

---

## Thesis

Most languages were designed for humans, then retrofitted for LLMs. Twe is
designed for both audiences from day one — and Phase 33 closes the loop
end-to-end so that an LLM authoring Twe code is **mechanically prevented**
from making the three mistakes that dominate LLM coding error budgets:

| Failure mode | Mechanism that prevents it | Status |
|---|---|---|
| Hallucinating syntax that doesn't parse | **Constrained generation against an exported grammar** (`twec grammar`) | Phase 33 §1 |
| Hallucinating stdlib functions / signatures | **Stdlib JSON manifest** (`twec stdlib --json`) the model is grounded on | Phase 33 §3 |
| Shipping a "fix" that doesn't compile | **Machine-applicable fix patches** in `twec verify` JSON v2 + verifier in the loop | Phase 33 §2 |

These aren't band-aids over a human-first language. They're concrete
artifacts derived from design choices Twe made in Phase 0 — a small
LL(1)-ish grammar, a single registration site for builtins, structured
JSON diagnostics. The Phase 33 work is *exposing* those choices as
machine-consumable contracts.

After the contracts ship, we add three more layers on top:

- **A reference loop** (`twec llm-loop`) — a CLI that proves the moat works end-to-end against any provider.
- **An MCP server** (`twec mcp`) — every Twe tool becomes available to any MCP client (Claude Desktop, Cursor, the future Twe Studio).
- **An automated benchmark** (`twec eval`) — replay-based scoring so "did the LLM produce a working game?" is a number, not an opinion.

---

## Why this is defensible

Three structural advantages other languages can't easily copy:

1. **Twe's grammar is small enough to constrain.** The full grammar fits
   in one file. Languages with context-sensitive parsing (Python, Ruby) or
   massive surface area (TypeScript, C++) cannot expose a usable grammar
   constraint to the model.

2. **Game concepts are first-class blocks** (`entity`, `state`, `scene`,
   `dialogue`, `visual`, `particles`). The model doesn't have to assemble
   a game from a hundred library calls — it picks one of six obvious
   shapes. Per-token error rate compounds far less per-program when the
   structure is uniform.

3. **The contract surface is tight.** One method-call syntax, one OOP
   idiom, single inheritance, only `false` is falsy, 0-indexed arrays.
   Each of these collapses a decision the model would otherwise have to
   make — and a place where it could make it wrong.

Combine the three and a small (cheap) LLM hits the same correctness as a
frontier LLM on Python — because the *language and tooling* are doing
work the model would otherwise have to.

---

## What ships in Phase 33

Ten sessions, listed in dependency order. Each ships a runnable artifact,
follows the project's `phase-33:` commit prefix, and ends with `cargo
test` + `cargo clippy --release --all-targets -- -D warnings` clean.

### Tier 1 — The contract surface (sessions 1–3)

**Session 1 — `twec grammar`.** Export the language grammar as
GBNF (llama.cpp / local models), JSON Schema (OpenAI structured outputs),
and EBNF (docs / tree-sitter). One source of truth in `src/grammar.rs`,
three rendered formats. Round-trip tested against every example.

**Session 2 — Verify JSON v2 with structured fixes.** Promote
`help: Option<String>` to `fix: Option<Fix>` where `Fix { edits: Vec<Edit>,
rationale: String }`. An LLM consuming verify output can apply patches
mechanically, not by parsing free-text help. Versioned (`version: 2`);
v1 consumers keep working.

**Session 3 — Stdlib JSON manifest.** `twec stdlib --json` enumerates
all 325 builtins with `{name, category, params, return_type, doc, since,
deprecated}`. Pinned into the LLM's system prompt or RAG index. API
hallucination becomes mechanically impossible — the grammar forbids
unknown tokens, and the manifest enumerates the legal callable surface.

### Tier 2 — Closing the loop (sessions 4–6)

**Session 4 — `twec llm-loop`.** End-to-end harness: prompt → generate
(constrained) → write file → `twec verify --json` → if errors, feed back
as an assistant turn → repeat N times. Logs every round-trip to
`traces/<timestamp>.jsonl` for later fine-tune corpus harvesting. Gated
behind `--features llm-loop` — default builds have zero LLM overhead.

**Session 5 — `twec mcp` (MCP server).** Stdio-transport Model Context
Protocol server exposing `parse`, `verify`, `format`, `grammar`,
`stdlib_lookup`, `find_symbol`, `apply_patch`, `run_program`. Claude
Desktop / Cursor / the future Twe Studio gain first-class Twe support
with one config paste — no bespoke integration per client.

**Session 6 — Examples-as-corpus.** Every example in `examples/` gains
a structured header (`@task`, `@inputs`, `@expected`, `@category`,
`@difficulty`). `twec corpus --json` emits the labeled set. These become
the few-shot pool, the eval seed, and the fine-tune training data.

### Tier 3 — Measurable & self-improving (sessions 7–10)

**Session 7 — `twec eval` (replay-based benchmark).** Standardized,
automated benchmark on top of the existing `replay.rs` (Phase 29).
Each suite is `{prompt.md, expected.replay, fixture_inputs.json,
scoring.toml}`. Drives generation via `llm-loop`, runs the result
under `replay::play`, hashes state at N ticks, compares to expected.
This is the public leaderboard *and* the future fine-tune's reward signal.

**Session 8 — Error→fix corpus generator.** Auto-mutate every program
in `tests/programs/` (wrong type literal, missing import, off-by-one
index, missing lifecycle method, deprecated builtin, swapped args).
Capture `(broken, verify_json, fix_json)` triples. Zero human labeling
because the original is known-good and the verifier output is the label.

**Session 9 — Typed holes (`???`).** Let LLMs emit a structural skeleton
with `???` placeholders, then fill them iteratively. Verify reports each
hole's expected type and in-scope bindings. Matches how humans draft and
dramatically helps small models that can't hold a whole file in working
planning context. Purely additive syntax; runtime errors if executed
(an authoring affordance, not a runtime feature).

**Session 10 — Closeout.** Standard closeout note, README + CLAUDE.md +
roadmap updates, full test/clippy pass, bench drift check.

---

## Success criteria

A clean checkout passes all of these at end-of-phase:

**Tier 1**
- `twec grammar --format gbnf` → non-empty grammar; `cargo test --test grammar` round-trips every example.
- `twec verify <file>` JSON contains `version: 2` and a `fix` field on high-confidence diagnostics; `cargo test --test verify_v2` round-trips broken→fixed.
- `twec stdlib --json` enumerates ≥325 builtins; `cargo test --test stdlib_manifest` asserts no install-vs-manifest drift.

**Tier 2**
- `twec llm-loop --provider claude --prompt examples/llm_prompts/snake.md` (with `ANTHROPIC_API_KEY` set) produces a passing `snake.twe`.
- `twec mcp` accepts canned JSON-RPC handshake + `verify` request from `cargo test --test mcp`.
- `twec corpus --json` enumerates all examples with complete headers.

**Tier 3**
- `twec eval snake --provider claude` returns a JSON scorecard with `pass: true`.
- `twec mutate --rules all --out corpus/` produces ≥500 (broken, verify_json, fix) triples.
- `???`-containing program parses, verifies with a `kind: "hole"` warning, errors clearly at runtime if executed.

**Whole phase**
- `cargo test` — all pass (current 810; Phase 33 adds ~40-60).
- `cargo clippy --release --all-targets -- -D warnings` — zero warnings.
- `cargo build --release` clean with and without `--features llm-loop`.
- VM bench within 5% of pre-Phase-33 baseline (new code is non-hot-path).
- Smoke: `survive_beta`, `crystal_hunter`, `visual_fire`, `pong_net` all run unchanged.

---

## What's deliberately *not* in scope

- **Fine-tuning the model itself.** Phase 33 generates the corpus. Training is a separate workstream that depends on Tier 1+2 contracts being stable enough not to drift the training distribution.
- **An in-Twe `llm.ask(prompt)` builtin.** That's "LLMs *in* games," not "LLMs *authoring* Twe" — different problem. Belongs to the future studio/engine work.
- **A web playground / hosted IDE.** Studio territory.
- **Removing `help` in favor of `fix`.** Deferred to v3 — give v2 a full release cycle first to surface real consumer needs.
- **Constrained generation against Anthropic's API.** They don't currently expose grammar constraints. OpenAI-first; Claude when the API supports it.
- **HTTP transport for MCP.** stdio-only in Phase 33; HTTP is a follow-on if a client needs it.

---

## How this connects to the larger roadmap

Phase 33 stands on top of:

- The **gradual three-tier type system** ([docs/02-type-system.md](docs/02-type-system.md)) — Tier 3 "verified" mode is what `twec verify` runs in.
- The **JSON AST emitter** ([src/ast_json.rs](src/ast_json.rs)) — round-trippable since Phase 3.
- **Replay/record** ([src/replay.rs](src/replay.rs)) — Phase 29's deterministic harness becomes Session 7's eval engine.
- The **single-source builtin registry** ([src/stdlib.rs:222](src/stdlib.rs#L222)) — Session 3's manifest extraction.

Phase 33 sets up:

- **A future Twe Studio** with Claude/GPT API integration. The Studio becomes another MCP client (Session 5) consuming the same tools any external agent gets.
- **A Twe-fine-tuned LLM.** Sessions 6 (annotated examples), 7 (eval scorecard as reward), and 8 (auto-generated error→fix corpus) produce the training data. The eval harness measures progress.
- **External contributions to evaluation.** Anyone can add a suite to `eval/` and run it against any provider — the leaderboard is public infrastructure.

See [docs/05-roadmap.md](docs/05-roadmap.md) for the full phase plan and
[CLAUDE.md](CLAUDE.md) for the project's locked decisions.

---

## License

This plan, like the rest of Twe, is MIT-licensed. See [LICENSE](LICENSE).
