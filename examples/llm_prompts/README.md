# LLM seed prompts

Seed prompts for `twec llm-loop`. Each `.md` file is a self-contained
authoring task. Run one against a model API (needs a build with
`--features llm-http`):

```sh
# with ANTHROPIC_API_KEY set
twec llm-loop --provider anthropic --model claude-sonnet-5-5 --effort high \
  --prompt examples/llm_prompts/snake.md \
  --max-rounds 5 \
  --out generated/snake.twe \
  --trace-dir traces/

# a local model through Ollama (or any OpenAI-compatible server, with --base-url)
twec llm-loop --provider openai --model qwen2.5-coder:14b --prompt examples/llm_prompts/snake.md
```

The loop can also go through any program that reads a prompt on stdin
and prints the reply, with no HTTP feature needed:
- `--command claude --arg -p`;
- a Python script;
- a `curl` wrapper;
- `llama-cli --grammar twe.gbnf`, for constrained local generation.

The Twe primer (`docs/llm-primer.md`) is the system prompt unless you
pass `--no-primer`. `--starter FILE` starts from an existing file, which
the model then changes with SEARCH/REPLACE blocks (see
`crates/twe-llm/src/edit.rs`).

Each round's prompt, reply, structured `verify` JSON, token usage and
cost are appended to the trace directory as one JSONL line. These
traces are the seed corpus for a future Twe-tuned model.

## Authoring contract

Every prompt should:

1. State the task in one sentence.
2. List the constraints (input modalities, expected behaviors).
3. Show the expected output shape (a single `twe` fenced block).
4. Reference the contracts the loop enforces: `twec verify` runs after
   each round and feeds JSON v2 diagnostics back, so the model knows
   to apply structured fixes.

To add a prompt, copy the shape of `snake.md` (the smallest example
that exercises a state machine) or `orbit.md` (the smallest entity
loop).
