# web3d-M5 session 7: ablation arms and Studio on `twe-llm`

**Date:** 2026-10-01
**Milestone:** web3d-M5 ([plan](2026-10-01-web3d-m5-plan.md); previous: [bench v1](2026-10-01-web3d-m5-bench-v1.md))
**Code:**
- `crates/twe-llm/src/openai.rs` (`grammar`);
- `src/cli.rs` (`--gbnf on`);
- `D:\IT\twe-engine` commit `b0f4d06` (Studio's `llm.rs` and `Cargo.toml`).

## The ablation arms

Every arm the plan names is now a flag on `twec bench run`, so session 9 runs the matrix with no new code:

| Arm | Flags | What it isolates |
|---|---|---|
| Twe, full | (default) | primer, `twec verify` feedback, smoke run, 4 rounds |
| No feedback | `--rounds 1` | one attempt, no tools |
| No verify | `--no-verify` | the smoke run alone: what verify's structured fixes add |
| No primer | `--no-primer` | what grounding adds |
| Constrained decoding | `--provider openai --gbnf on` | llama.cpp decoding limited to Twe's GBNF |
| Python | `--lang python` | the baseline: Python's tools, no primer |

**Constrained decoding.** The OpenAI-compatible provider gains `grammar`, which is sent as llama.cpp's `grammar` request field. The model can then only produce text in Twe's grammar, so it replies with bare source, which the loop already takes as a whole file. The provider id gets `+gbnf`, so constrained runs are never mixed up with plain ones. This arm needs a local llama.cpp server, which this machine doesn't have; only the request body is tested.

## Studio on `twe-llm`

Studio's AI loop (`D:\IT\twe-engine\studio\src-tauri\src\commands\llm.rs`) now uses `twe-llm`:
- for model calls, the Anthropic and OpenAI-compatible providers, run on tokio's blocking pool;
- for the edit protocol, `edit::parse_reply` and `edit::apply`.

Its own reqwest calls and SEARCH/REPLACE code, which had drifted from `twec`'s, are gone, and reqwest is no longer a dependency. The dependency is a path (`../../../twe-language/crates/twe-llm`), because the two repositories sit side by side.

**Behaviour changes:**
- **Ambiguous edits are sent back.** A SEARCH that matches more than one place goes back to the model; before, Studio silently edited the first match.
- **Deletions are clean.** A deleted line no longer leaves an empty line behind.
- **Ollama** is reached through its OpenAI-compatible endpoint.
- **Refusals** are reported as such.
- **Model choices are unchanged.** Studio still offers Sonnet 4.6 and Haiku 4.5.

**Verification:**
- Studio builds.
- Its tests pass, including a new one that its prompt's edit format applies through `twe-llm` and that an ambiguous SEARCH is rejected.
- Clippy reports 7 errors, all pre-existing in other Studio files (8 before this change).

## Verification

- `twe-llm`: 16 tests (the request body with and without a grammar).
- `twec`: builds; the full suite passes (see the commit).
