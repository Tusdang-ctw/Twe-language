# web3d-M10 closeout — Studio maturity

**Closed:** 2026-10-05 (opened 2026-10-04). Plan: [`2026-10-04-web3d-m10-plan.md`](2026-10-04-web3d-m10-plan.md). Line: creation-parity (M8–M12), [`2026-10-04-creation-parity-plan.md`](2026-10-04-creation-parity-plan.md).

> **The milestone, in one line:** the editor becomes a place a human prefers to "VS Code + a pile of Three.js libraries."

M10 is cross-repo: the **language hooks** are in `twe-language`; the **Studio UI** is in the sibling `twe-engine` (Tauri v2 + React + Monaco + zustand).

## What shipped (against the exit criteria)

| Exit criterion | Status |
|---|---|
| A debugger — breakpoints, step, variable/entity inspection | **Done** |
| A profiler panel — frame-time + GC + draw-call | **Done** (frame-time + sim/render/kernel; see note) |
| One-click publish — desktop `.exe` + web | **Done** |
| Live scene/inspector editing without a full reload | **Partial** — save → Viewport hot-reload (no-reload value patch deferred) |

Six sessions; details + commit hashes in the plan. The shape:

**Language side (`twe-language`, sessions 1–3, all committed earlier this line):**
- `src/debug.rs` + the `twec debug` stdio protocol (breakpoints / step / scope inspection; only user globals shown).
- `profile::FrameSample` ring + `samples_json` (native `play3d`), and the web build posts `frame_stats` to the parent when embedded (`9e1e710`).
- `twec build --json` (one-object artifact result on stdout).

**Studio side (`twe-engine`, sessions 4–6):**
- **Debugger panel** (`34b227b`): `commands/debug.rs` bridges the stdio protocol (mirrors `lsp.rs`); a Monaco breakpoint gutter + paused-line highlight; a "Debug" tab with step controls + a variables tree.
- **Profiler panel** (`8d5a363`): a "Perf" tab graphing the Viewport game's live frame stats (FPS + sim/render/kernel sparklines), fed by the web build's postMessage.
- **One-click Publish** (`8d5a363`): a Toolbar button that runs `twec build --json` and reveals the artifact.

## Verification

- **Language side:** `twec debug` has `tests/debug.rs` (2, via the binary with piped stdin); `profile` ring + JSON unit tests; `build --json` has `tests/build.rs::build_json_emits_machine_readable_result`. Native + wasm32 clippy clean; the full suite green.
- **Studio side:** frontend `npx tsc --noEmit` clean, `npm run build` (vite production bundle) clean; `src-tauri` `cargo check` clean on Windows.

## The profiler note (honest scope)

The Studio Viewport runs the game as the **web** build (`twec build --target web` served over `twe://` in the webview), not native `play3d`. So the profiler feed is the web runtime's `frame_stats` (frames, ticks/frame, tick ms, script-render ms, kernel ms, GPU-culled share), posted to the parent. Session 2's native `FrameSample` ring (frame/sim/render ms + GC ms + bytes-alive + draw-calls) is the native-`play3d` profiling path and is not what the Viewport panel reads. A draw-call count in the web feed and a GC line are follow-ons.

## Deferrals (need the maintainer / a GUI)

Code-complete + compile-verified; what remains needs a running GUI or is a follow-on:
- **Running the Studio and driving the panels** (set a breakpoint, step, watch the profiler, publish) — a maintainer GUI step. It needs a `twec` built with the M10 subcommands (`cargo build --release` in `twe-language`, or the bundled sidecar).
- **A no-reload live value patch** (edit an inspector field applied to the running game without a rebuild) — the hot-reload path is the baseline; a live patch is a follow-on.
- **GC + draw-call lines in the web profiler feed**, and a native-`play3d` profiler that reads session 2's ring.
- **Step-into vs step-over fidelity across the resumable-fiber path** (state on-entry bodies that `wait`) — the hook covers `eval_stmt`; the fiber path is a documented follow-on from session 1.

## Gates

`twe-language`: `cargo build` + `cargo clippy --all-targets -- -D warnings` clean on default + `wasm32`; suite green. `twe-engine`: `tsc` + `npm run build` + `cargo check` clean.

## Next

M10 closes. Per the creation-parity order (M8 → M9 ∥ M10 → M11 → M12), **M11 (game-feature depth)** is next: animation blending + state machines, physics joints/CCD, and a polished reference 3D game — scoped by the chosen reference game, not a checklist. Then **M12** (the measured proof + the parked M5 live runs).
