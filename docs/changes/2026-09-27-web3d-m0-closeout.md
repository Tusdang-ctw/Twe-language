# web3d-M0 closeout — truth and foundation safety

**Date:** 2026-09-27
**Milestone:** web3d-M0 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md))
**Status:** **closed**. All five exit criteria met; two plan items were deliberately folded into M1 (see "Deviations").

## Exit criteria

| Criterion | Result | Evidence |
|---|---|---|
| Every program passes under `TWE_GC_STRESS=1` in debug builds | **Met** | `cargo test --test gc_stress` (debug): every `tests/programs/*.twe` and `examples/*.twe`, 0 and 10 frames, normal vs stress: all agree |
| GC fuzz clean for 1k seeds | **Met** | `TWE_GC_FUZZ_SEEDS=1000 cargo test --release --test gc_fuzz`: clean in 1.7 s. Now a CI step |
| wasm32 check green | **Met** | `cargo clippy --release --target wasm32-unknown-unknown -- -D warnings` clean, with and without `--features experimental`. New `wasm-check` CI job |
| Clippy clean | **Met** | native + wasm32, default + experimental, all `-D warnings` |
| Pivot note merged | **Met** | [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md); `CLAUDE.md` v1.0 thesis marked reopened; roadmap points at the new line |

Tests: **1,046 pass** in the default build, 0 fail (1,039 at the start of M0). `cargo fmt --check` passes for the first time in months.

## What shipped (commits `a4b2135` … this closeout)

1. **GC soundness** ([`2026-09-27-web3d-m0-gc-soundness.md`](2026-09-27-web3d-m0-gc-soundness.md)). A reproduced use-after-free that crashed the process (`STATUS_HEAP_CORRUPTION`) from safe script code is fixed. The changes:
   - collection only at call depth 0, plus a per-tick safepoint;
   - `heap::RootScope`;
   - a young list for the incremental sweep;
   - missing roots added;
   - `TWE_GC_STRESS` poison mode;
   - `tests/gc_stress.rs`;
   - `tests/gc_fuzz.rs`, a seeded generator of valid, allocation-heavy programs. It was mutation-tested: its first version couldn't detect a removed root because it only built flat int lists. The fixed version catches that on seed 0.
2. **Experimental gate.** `console`, `achievements` / `cloud_save` / `friends`, `mmo`, `workshop`, `rollback`, `world` and `terrain` moved to `src/stdlib/experimental.rs` behind `--features experimental`. That's 2,037 lines out of `stdlib.rs`.
   - Their demos moved to `examples/experimental/`.
   - Corpus and mutator skip `experimental/` dirs.
   - The manifest test asserts the default build doesn't advertise them to LLMs.
3. **Truth pass.** The README status now says what works, what is experimental, and that browser 3D doesn't exist yet. Also updated: the builtin count (286), `docs/06` §7.20 notes, `PARTNER.md`, and `CHANGELOG`.
4. **Shipped-build fixes:**
   - `load` / `load_atlas` / `sound.load` now resolve through the bundle, so a bundled exe works without loose assets.
   - The embedded loop now calls `steam::init()`.
   - New `quit()` and `quit_on_escape(flag)`. Escape was hard-wired to close the window, which made every Escape pause menu (`pause_menu_demo`, `survive_beta`) unreachable, and there was no way to quit from script.
   - 2D draw / UI builtins now raise a clear error outside the 2D runtime instead of aborting (`text()` in `play3d`).
5. **The wasm32 build compiles again.** It had 32 errors, broken silently during the 3D phases.
   - New `render3d_types` module.
   - `physics.*` is native-only.
   - Web saves, which never worked, now return an explicit error, and the `quad-url` dependency is removed. It wrote URL query params, not localStorage.
6. **Two pre-existing test flakes, root-caused.** Together they made roughly 10% of full runs fail.
   - The native clipboard corrupted the heap when used concurrently with other test threads (9/80 → 0/100 after isolating it in `tests/clipboard.rs`).
   - The process-wide pause flag let one test freeze others; it is now per-thread.
   - The clipboard test also no longer overwrites the developer's real clipboard unless `TWE_TEST_CLIPBOARD=1`.
7. **Studio** (`D:\IT\twe-engine`, `92d38ec`): Stop now kills the game process. Dropping the `Child` never terminated it.
8. **Hygiene:** `cargo fmt --all` as its own formatting-only commit, listed in `.git-blame-ignore-revs`. Crash `.replay` files are gitignored.

## Deviations from the plan

- **Bytecode VM not gated in M0.** Gating it means removing the VM's `HeapBody` variants and mark paths, a large change whose only purpose would be deletion a few weeks later. It is already opt-in (`--vm bytecode`), frozen, and no longer a parity target for new semantics. **Folded into M1:** delete at M1 exit, as planned. Its known GC hazard (unrooted builtin argument vectors) is documented; don't ship games on it.
- **Heap cap not implemented.** It existed to bound memory when a long-running function allocates without collecting (a side effect of depth-0 safepoints). **Folded into M1:** slot-based frames put locals on a scanned stack, which lets safepoints return inside functions and removes the need for a cap.

## Found along the way (not in the plan)

- The CI format check had been failing across about 48 files; fixed.
- `examples/crystal_hunter_web.twe` called `quit()`, which didn't exist; it works now.
- `joystick()` compiled out its touch scan on wasm32; the virtual stick now works with browser touch.

## Carried into M1

Lexical scoping via a resolver pass with slot frames, the module name-resolution fix, fiber slot frames, the cheap performance wins (no AST clones per tick, no per-call `saved_params`, interned builtin ids), VM deletion, and moving safepoints back inside functions.
