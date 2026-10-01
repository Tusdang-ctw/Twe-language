# web3d-M5 session 4: `twec bench run`

**Date:** 2026-10-01
**Milestone:** web3d-M5 ([plan](2026-10-01-web3d-m5-plan.md); previous: [bench v0](2026-10-01-web3d-m5-bench-v0.md))
**Code:**
- `src/bench_run.rs` (new);
- `src/llm_loop.rs`: `run_loop_checked`, optional verify feedback, per-round verify and check results;
- `src/bench.rs`: the smoke task;
- `src/cli.rs`: `bench run` / `regrade`;
- `tests/bench_run.rs` (new);
- `bench/README.md`;
- `.gitignore`.

## What shipped

**One sample is one conversation through `llm_loop`:**
- **The system prompt:** the primer, every stdlib builtin with its parameters, and the run rules (60 ticks per second, how input arrives, that the names under Interface are read by a test script). It is about 6,000 tokens and cached.
- **The user message:** the task and its interface.
- **Feedback after each round,** for up to 4 rounds:
  - **`twec verify`'s diagnostics** when it finds errors;
  - **a smoke run** otherwise: 2 s of game time with no input, in a child process. A load error, crash or hang goes back with its message.
- **The final program** is graded on the task's checks in a child process.

**Why a smoke run is part of the feedback.** A developer runs their game, and the Python baseline (session 5) gets the same smoke run with Python's own checker in place of `twec verify`. The smoke run never uses the task's input script or checks, so the model gets no hints about the grading.

**What's recorded:**
- **Per sample (`samples.jsonl`):** the grade (stage, failed checks), rounds, whether round 1 parsed, round 1's and the final verify error counts, truncation, tokens split by cache use, cost, wall time and cache hits.
- **Per round:** the transcript.
- **Every final program.**

**The scorecard (`summary.md` and `summary.json`):**
- pass@k for k up to n, by the unbiased estimator (Chen et al. 2021);
- pass@1 per tier;
- 95% percentile-bootstrap intervals over tasks (10,000 resamples, fixed seed);
- round-1 syntax and verify-clean rates, final verify-clean rate, rounds to pass, truncation, final stages;
- tokens and cost.

**Operations:**
- **Caching.** Replies are cached by a 128-bit hash of the provider, the sample number and the whole request. Re-running resumes, retrying only samples whose call failed; a repeated run makes no calls.
- **Regrading.** `twec bench regrade` grades a run's programs again with the current grader, without a model.
- **Consistency.** A run directory refuses a resume with different settings.
- **Stopping.** A run stops on a configuration error (no credentials, a 4xx other than 429) or five provider failures in a row.
- **Parallelism.** `--jobs` workers each build their own provider.

**Cost, estimated before any run.** At Sonnet 5.5 prices, a sample of about 1.5 rounds with 2,000 output tokens plus thinking per round costs roughly $0.08. Session 9 replaces this with measured numbers before the maintainer approves the full matrix.

## Also decided

**`twec eval` is deprecated** in favour of `twec bench` (Principle 2: one way to grade). It keeps working; the CHANGELOG records the deprecation.

## Verification

- **Unit tests** cover:
  - pass@k against its definition (n = 5, c = 2: 0.4 and 0.7);
  - the bootstrap interval (it brackets the mean, is deterministic, and collapses for constant data);
  - the stability of cache keys;
  - the system prompt's contents with and without the primer.
- **`tests/bench_run.rs`** uses stand-in models that answer with the reference solutions; no network. It covers:
  - a run where round 1 has a typo: 6 samples pass in 2 rounds, round 1 is never verify-clean, the summary says so;
  - resuming a finished run (no calls), and a fresh run directory served wholly from the cache;
  - refusing different settings;
  - regrading a run after one program is broken (1 of 6 fails);
  - a single round without feedback, where the same typo fails at `load`;
  - a provider that always fails, which stops the run after 5 attempts with each failure recorded.
- **By hand, through the CLI,** with a command "model" that serves the pilot's programs:
  - plain: 40 of 40 pass;
  - a typo in round 1: every sample needs 2 rounds;
  - a load-time crash in round 1: the smoke run sends back "list index 5 out of bounds" and round 2 passes.
- **Clippy:** see the commit.
