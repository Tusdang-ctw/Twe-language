# web3d-M8 closeout: browser truth

**Opened/closed:** 2026-10-04
**Plan:** [creation-parity roadmap](2026-10-04-creation-parity-plan.md), M8

M8 was the honesty milestone: everything Twe claims to run in a browser is now *captured running in a browser*, pixel-diffed against the native renderer — shown, not asserted. The browser tooling that was unavailable through M6 turned out to be reachable: `playwright-core` drives the installed Chrome/Edge/Firefox directly (the thing that was "disconnected" earlier was a different browser).

## Exit criteria — met

| Criterion | Result |
|---|---|
| A built 2D + 3D game captured in headless/real Chrome, diffed to native within tolerance, FPS recorded | **Met.** pong (2D) and hello_3d (3D) built with `twec build --target web`, run in real Chrome via WebGPU (Intel gen-12lp), captured deterministically at frame 60, mean diff vs native **0.18 / 0.11 of 255** (0 % of pixels off). hello_3d ran at **503 fps** (`bench/graphics/fps.mjs`). |
| Confirmed on Edge, Firefox, Safari (or a graceful "needs WebGPU" message) | **Met.** Chrome, Edge, Firefox all pixel-match native (**0.11 / 255** on hello_3d). Playwright's **WebKit has no WebGPU adapter on Windows** (Safari's WebGPU needs Metal/macOS) → the game shows its graceful "needs WebGPU" message, the documented fallback. A real-Safari pass needs a Mac (honest deferral). |
| Web alpha blends match native (gamma, not linear) | **Met.** The web 2D path drew to the surface's sRGB view (linear blend); an alpha-heavy scene (`dungeon_demo`: ambient darkness + torch glows) was **13.2 / 255, 18.9 % off**. `Renderer::render_2d` now draws through a **non-sRGB (base-format) view** with a second, base-format canvas, so it blends in gamma space like the native 2D shell: **0.25 / 255, 0.1 % off**. |
| Typed text on web | **Met.** The web keydown handler accumulates printable `event.key` characters and feeds them through `stdlib::set_typed_text` each frame. Verified live in Chrome: typing into a `text_input` shows "You typed: hello twe". |
| A `TWE_CAPTURE` hook on the 3D shell | **Met.** `twec play3d` (and the embedded 3D `.exe`) honour `TWE_CAPTURE=N:path`, rendering N deterministic ticks off-window through a headless renderer and writing the PNG — the 2D shell already did. This also produced the native 3D references for the browser diff. |
| The intermittent `STATUS_HEAP_CORRUPTION` flake fixed | **Met (by removal).** 0 crashes in 32 runs (12 debug + 20 release) after M6. The flake "needed the interpreter and renderer together under concurrency, pointing at a process-global the bisection couldn't separate" — and M6 deleted macroquad, whose miniquad layer carries exactly that kind of process-global native/GL state, now gone from the test binary. The conditions no longer exist; reopen if it returns. |

## What shipped

- **`src/play3d.rs`** — `TWE_CAPTURE` via a headless render path (reuses `Renderer::new_headless` + `host3d::render_frame` + `read_pixels`).
- **`crates/twe-web/src/lib.rs`** — typed-text capture on keydown; a `?capture=N` URL mode that renders N deterministic ticks (no input), then sets `window.tweCaptured` for the headless browser.
- **`src/kernel/render.rs`** — a second, base-format `canvas_2d`; `render_2d` draws 2D games through the non-sRGB surface view for gamma-space blending (the 3D HUD keeps the sRGB canvas). A base-format view is added to the surface's `view_formats`.
- **`bench/graphics/browser-capture.mjs`** — serve a `dist/web`, open `index.html?capture=N` in Chrome/Edge/Firefox/WebKit via `playwright-core`, screenshot `#twe-canvas`; exit 3 on no-WebGPU. (No image dep: diffing is a separate step.)
- **`.github/workflows/browser-capture.yml`** — builds the runtime + a 2D/3D demo, captures across browsers, diffs vs native, uploads the gallery. Best-effort on CI WebGPU (runner GPU availability varies); the verified browsers must stay within tolerance.

## Verification

`cargo test` 1,086 passed; clippy `-D warnings` clean for native, `wasm32` and `-p twe-web`; `cargo build --release` zero warnings. Browser results above measured locally on real Chrome/Edge/Firefox.

## Honest deferrals

- **Safari/WebKit WebGPU** can't be verified on Windows (needs Metal/macOS). The graceful fallback is confirmed; a real-device pass is a Mac follow-on.
- **The CI workflow's capture step** is best-effort on the runner's GPU; local verification is the recorded evidence.
- **Doc drift noticed:** `src/heap.rs` carries `#![allow(unsafe_code)]` (since the Phase 8.5 tracing GC) but wasn't in CLAUDE.md's exception list; added there.

## Next

M9 (asset pipeline parity) and M10 (Studio maturity) — parallelizable.
