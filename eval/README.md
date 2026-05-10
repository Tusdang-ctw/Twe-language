# Twe Eval Suites

Phase 33 session 7. Each subdirectory is one labelled benchmark for
the LLM authoring loop. Suites are graded by `twec eval`:

```sh
# Score a hand-written reference against the suite (regression test).
twec eval print_hello --source generated/hello.twe

# All suites at once.
twec eval --source-dir generated/

# Just emit the JSON scorecard for downstream tooling.
twec eval --json
```

## Suite layout

```
eval/<name>/
  prompt.md      # the LLM authoring task (free-form Markdown)
  expected.txt   # canonical stdout after running for N frames
  config.toml    # frames + dt + match_mode (optional; defaults applied)
```

`config.toml` keys (all optional):

| key          | default      | meaning |
|--------------|--------------|---------|
| `frames`     | `60`         | how many `on update(dt)` ticks before grading |
| `dt`         | `0.0166...`  | seconds per frame (1/60 default) |
| `match_mode` | `substring`  | one of `substring` / `exact` / `lines` |

## Match modes

- `substring` — expected text appears anywhere in actual stdout. Default
  because most LLM-generated programs include extra prints.
- `exact` — actual stdout (trimmed) equals expected (trimmed). Use for
  tight regressions where stdout diff is the spec.
- `lines` — every non-blank expected line appears in actual stdout in
  order. Loose enough to tolerate prologue / epilogue, strict enough
  to catch missing structure.

## Adding a suite

1. `mkdir eval/my_task`
2. Write `prompt.md` (the LLM task — pretend you're talking to a
   contributor; the model will see it verbatim).
3. Write `expected.txt` (the canonical output a passing program emits).
4. Optionally tune `config.toml`.

That's it. No Rust changes, no test wiring — `tests/llm_eval.rs`
auto-discovers every suite.

## Scorecard schema (JSON v1)

`twec eval --json` emits:

```json
{
  "tool": "twec-eval",
  "version": 1,
  "summary": { "total": N, "passed": K, "failed": N - K },
  "scores": [
    {
      "suite": "...",
      "passed": true | false,
      "stage": "lex" | "parse" | "run" | "match",
      "source_lines": N,
      "source_bytes": N,
      "actual_output": "...",
      "message": null | "..."
    }
  ]
}
```
