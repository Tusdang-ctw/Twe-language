# Contributing to Twe

Thank you for your interest in contributing. Twe is a game-first programming language being built carefully and deliberately — we value correctness, clarity, and shipping things that work over moving fast and breaking things.

## Before you start

Read [`CLAUDE.md`](CLAUDE.md) — it is the canonical description of the project's identity, locked decisions, and quality bars. Any contribution that contradicts a locked decision will be declined unless it comes with an explicit discussion of why the decision should be revisited.

Read [`docs/01-examples.md`](docs/01-examples.md) — the eleven example programs are the spec. If a change doesn't serve one of those examples (or the v1.0 thesis of "ship a Vampire-Survivors-class commercial 2D game on Twe"), it probably doesn't belong in the codebase.

## How to contribute

### Bug reports

Open a GitHub issue. Include:

1. The `.twe` file (or minimal reproduction) that triggers the bug.
2. The exact command you ran (`twec play`, `twec run --frames 10`, etc.).
3. What you expected vs. what happened (error message, wrong output, crash).
4. Your OS + `twec version` output.

### Feature requests

Open a GitHub issue tagged `design`. Describe the game mechanic or workflow the feature enables — if you can't point at one of the eleven examples or at a specific game you're trying to ship, the feature is likely out of scope for v1.0.

### Pull requests

1. **One thing per PR.** Don't mix a bug fix with a refactor. Don't mix two unrelated features.
2. **Tests first.** Every new Twe surface must have a corresponding `.twe` program in `tests/programs/` and a snapshot test in `tests/eval.rs`. A PR that adds a feature without a test will not be merged.
3. **Docs in the same commit.** Grammar change → update `docs/06-design-document.md §3`. New stdlib function → update `§7`. Design pivot → add a `docs/changes/` note.
4. **Clean build.** `cargo build --release` zero warnings. `cargo clippy -- -D warnings` clean. `cargo test` all green.
5. **Commit format:** `phase-N: <verb> <what>`. Use the current active phase from `CLAUDE.md`.

### What we will not accept

- Parser generators (PEG, ANTLR, nom, pest). Parser is hand-written recursive descent. This is locked.
- `async`/`await` or OS-thread concurrency. Cooperative fibers only.
- New crate dependencies without justification. Every new `Cargo.toml` entry needs a comment.
- Features not implied by the eleven examples or the v1.0 thesis.
- Code with `#![allow(unsafe_code)]` outside `src/tagged_value.rs`.

## Development setup

```
git clone https://github.com/your-org/twe-language
cd twe-language
cargo build          # debug build
cargo test           # run the full test suite (~730 tests)
cargo run -- play examples/pong.twe    # test an example
```

Rust stable is required. No nightly-only features; no pinned toolchain beyond whatever `rust-toolchain.toml` specifies (currently stable).

## v1.0 LTS policy

Once v1.0 ships:

- The public Twe language surface (keywords, stdlib API, file format) is frozen. Additions are allowed; removals require a `@deprecated` cycle of ≥ 12 months before removal.
- The `twec` CLI surface is frozen. New subcommands may be added; existing flags may not be removed without a deprecation cycle.
- Security and crash fixes backport to the `v1.x` branch for a minimum of 12 months after v1.0 ships.
- Breaking changes to the Rust public API (`twec::` crate) are allowed on minor version bumps, not patch bumps.

## Governance

The project is currently maintained by its original author. New core contributors are added by invitation after sustained high-quality contribution. Decisions on locked questions (see `CLAUDE.md`) require explicit discussion and a maintainer sign-off.
