# web3d-M5 session 1: the `twe-llm` crate

**Date:** 2026-10-01
**Milestone:** web3d-M5 ([plan](2026-10-01-web3d-m5-plan.md))
**Code:**
- `crates/twe-llm/` (new): `lib.rs`, `anthropic.rs`, `openai.rs`, `command.rs`, `fixture.rs`, `edit.rs`, `pricing.rs`, `http.rs`;
- `src/llm_loop.rs` (rewritten onto it);
- `src/cli.rs`: `llm-loop` flags, the shared `ProviderFlags`;
- `Cargo.toml`: workspace member, `llm-http` feature.

## Why

Before the benchmark can measure models, there has to be one way to call them. There were two:
- `src/llm_loop.rs`: a string-in, string-out trait, a shell-command provider and a fixture.
- Studio's `llm.rs`: its own async Anthropic and Ollama calls, and its own SEARCH/REPLACE edit protocol.

Neither reported what a call cost, and they had drifted apart. The benchmark needs tokens, cache hits, cost and stop reasons for every sample.

## What shipped

**`crates/twe-llm`** knows nothing about Twe syntax and doesn't depend on `twec`, so Studio can use it without pulling in the engine.

**`Provider`:** one blocking call, `complete(&Request) -> Result<Reply, Error>`.
- **`Request`:** a system prompt (stable, so cacheable), a user/assistant conversation, and a token ceiling.
- **`Reply`:** the text, the stop reason (end of turn, `max_tokens`, refusal), token usage split into uncached input, output, cache reads and cache writes, and the cost in dollars when the model's prices are known.

**Errors are infrastructure failures** (configuration, HTTP status, transport, an unreadable answer). Callers stop on them rather than counting them against the model. A refusal is a reply, not an error.

**The providers:**
- **`anthropic::Anthropic`** (Messages API):
  - the system prompt is marked for prompt caching;
  - `effort` is the only tuning knob, since current models reject `temperature`;
  - server-side fallbacks are deliberately off, because a benchmark must know which model answered;
  - credentials come from `ANTHROPIC_API_KEY` or an OAuth `ANTHROPIC_AUTH_TOKEN`.
- **`openai::OpenAiCompatible`:** chat completions for Ollama (the default base URL) and llama.cpp's server; cached prompt tokens are split out.
- **`CommandProvider`:** any program, with the flattened conversation on stdin.
- **`FixtureProvider`:** queued replies for tests; records every request.

**HTTP is optional.** It is blocking `ureq` behind the off-by-default `http` feature (`twec --features llm-http`), with four retries on 429, 5xx and dropped connections (backoff, or `retry-after`). Request bodies and response parsing are plain functions, always compiled and tested. A build without the feature fails the call with an explanation.

**`pricing`:** list prices per model (2026-09-25), and `cost(model, usage)`. Unknown models cost `None`, never zero.

**`edit`** is the edit protocol, ported from Studio:
- **SEARCH/REPLACE blocks** are matched as whole lines, ignoring trailing whitespace. One change from Studio: a SEARCH that matches more than one place is an error naming the block; Studio silently edited the first match.
- **Or a whole file** in a fenced block, by language tag, then untagged.
- **`protocol_instructions(lang)`** is the prompt text, kept next to the parser so the two can't drift.

**`twec llm-loop`** runs on it:
- **The rounds are one conversation.** A failed round appends the model's reply and the verify JSON, or why its edit didn't apply; before, each retry restated the program in a fresh prompt.
- **Replies are edits** against the current file, which can start from `--starter`.
- **The Twe primer is the system prompt** unless `--no-primer`.
- **Providers:**
  - `--provider anthropic --model M [--effort E]`;
  - `--provider openai --model M [--base-url URL]`;
  - `--command CMD [--arg A]*`.
- **Each round's trace line** (version 2) adds the model, stop reason, tokens, cost and edit error. The summary line prints totals.

## Verification

- **`twe-llm`:** 17 tests, including:
  - the edit protocol (exact once, trailing whitespace and CRLF, appends and deletions, ordering, unclosed blocks);
  - request bodies and response parsing for both APIs;
  - pricing (dated snapshots, unknown models).
- **The real HTTP path against a local server:** checks the headers and body, retries a 529, then reads the reply. Disabling retries makes the test fail.
- **`twec`:** the loop's tests cover:
  - a conversation that converges with a SEARCH/REPLACE fix;
  - an edit that doesn't apply, sent back with the unchanged file;
  - summed usage and cost.

  The integration tests now check the conversation and trace version 2.
- **CLI:** checked by hand. A Python command provider passes in one round; `--provider anthropic` in a build without HTTP explains what's missing.
- **Not tested live:** this machine has no Anthropic credentials. The first real call happens in session 9, with the maintainer's key.
- **Clippy** (`-D warnings`) is clean for `twec` (all targets, and with `llm-http`), `twe-llm` (with `http`, all targets), wasm32 and `twe-web`.

## Not done here

- **Studio still uses its own `llm.rs`.** Moving it onto `twe-llm` is session 7, where its async calls go through `spawn_blocking`.
- **`twec eval`'s stdout grading is unchanged.** Session 2 replaces it with behavioural grading.
