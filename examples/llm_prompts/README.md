# LLM seed prompts

Seed prompts for `twec llm-loop`. Each `.md` file is a self-contained
authoring task. Run one with:

```sh
twec llm-loop --command claude --arg "code" --arg "-p" \
  --prompt examples/llm_prompts/snake.md \
  --max-rounds 5 \
  --out generated/snake.twe \
  --trace-dir traces/
```

The `--command CMD --arg ARG --arg ARG ...` form points the loop at any
process that reads a prompt on stdin and writes the model's reply on
stdout. Any LLM you can wrap in a shell command works — Claude CLI, a
Python script, a `curl` wrapper, or `llama-cli --grammar twe.gbnf` for
constrained local generation.

Each round's prompt + reply + structured `verify` JSON is appended to
the trace directory as one JSONL line. Those traces are the seed corpus
for the future Twe-fine-tuned model (Phase 33 §"Tier 3 — fine-tune").

## Authoring contract

Every prompt should:

1. State the task in one sentence.
2. List the constraints (input modalities, expected behaviors).
3. Show the expected output shape (a single `twe` fenced block).
4. Reference the contracts the loop enforces — `twec verify` runs after
   each round and feeds JSON v2 diagnostics back, so the model knows
   to apply structured fixes.

Adding a new prompt: copy the shape of `snake.md` (smallest example
that exercises a state machine) and `orbit.md` (smallest entity loop).
