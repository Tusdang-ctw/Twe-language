# web3d-M2 closeout: kernel extraction and WebGPU

**Date:** 2026-09-28
**Milestone:** web3d-M2 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md))
**Status:** **closed.** All exit criteria are met. The workspace crate split and several plan items that were not exit criteria are deferred, with reasons (see "Deferred").

## Exit criteria

| Criterion | Result | Evidence |
|---|---|---|
| `hello_3d` at 60 fps in Chrome via `twec build --target web` | **Met: 142–145 fps** (display refresh, 144 Hz) | `twec build --target web examples/hello_3d.twe`, served locally in Chrome 152. Held for 48 s with the window visible; median frame 6.9 ms, no long animation frames. |
| The same kernel runs natively | **Met** | `twec play3d` runs through `kernel::render::Renderer` (it was crashing on its first frame since 2026-06-01; fixed in `e61ee54`). `tests/kernel_render.rs` renders `hello_3d` headless through the same kernel and checks the pixels. |
| The wasm is built in CI | **Met in the workflow; first run pending a push** | The `wasm-check` job runs clippy, a release build of `crates/twe-web` and a pinned `wasm-bindgen` 0.2.120, then `tests/web_runtime.rs` against the built wasm, and uploads the runtime. The same commands were run locally. `main` has not been pushed, so GitHub has not run the job yet. |
| All tests pass | **Met** | 931 pass (native, `cargo test --workspace`). Clippy is clean natively, with `experimental`, for the wasm32 lib and for `twe-web` on wasm32. `cargo fmt --check` is clean. |

## What shipped (commits `e61ee54` … this closeout)

1. **The renderer kernel** (`src/kernel/render.rs`, moved out of `play3d.rs`).
   - `Renderer` takes a `RenderSnapshot` (camera, lights, shadow, post-FX, draws, asset paths, animation poses) and an `AssetSource` (request/poll).
   - It knows nothing about Twe: its only in-crate import is the plain-data `render3d_types`.
   - `Renderer::new_headless` + `read_pixels` give the first test that actually renders.
2. **One frame path for every host** (`src/host3d.rs`).
   - Reading the camera and snapshot from a Twe `Env`, then applying key state, is shared by the native winit shell, the browser shell and the render test.
3. **The browser shell** (`crates/twe-web`, a wasm-bindgen cdylib).
   - Fetches `main.twe` and runs it.
   - Creates a WebGPU surface on `#twe-canvas`.
   - Drives the fixed-step sim from `requestAnimationFrame`.
   - Loads assets via `fetch`.
   - Reads keyboard input from DOM events.
   - Shows a "WebGPU required" message where it is missing.
   - The crate is empty on non-wasm targets.
4. **WebGPU fitness**
   - **wgpu 22 → 30.** Chrome rejects wgpu 22's `maxInterStageShaderComponents` limit.
   - **Bind groups 5 → 4.** Camera and lights now share group 0, so the renderer runs on WebGPU's *default* limits.
   - **Shadow PCF uses `textureSampleCompareLevel`.** Chrome's Tint rejects implicit-derivative sampling in non-uniform control flow, which gave a black canvas; naga had accepted the shader. A guard test pins it.
5. **A host clock** (`src/clock.rs`). `std::time::Instant` panics on wasm32. It had been breaking the incremental GC sweep in *every* web build, 2D included.
6. **`twec build --target web`**
   - **Output:** a servable folder with the prebuilt runtime, `index.html` titled after the game, `env.js`, the script and its `assets/`.
   - **Runtime lookup:** `$TWE_WEB_RUNTIME`, then `web-runtime/` next to `twec`, then the source checkout's `target/web-runtime/`.
   - **No Rust toolchain needed on the author's machine.**
   - **Replaces:** Phase 38's placeholder `wasm32-3d` (kept as an alias).
   - **Single `.twe` files:** `twec build` now accepts one, since every example is one.
7. **`env.js` + `tests/web_runtime.rs`.** macroquad still leaves 14 `env` imports in the runtime until M6. An import map points them at no-op stubs. The test reads the built wasm's import section, so a new import fails CI instead of a page load; I confirmed it fails when an export is removed.
8. **M0 carry-over.**
   - The `linux-server` / `ios` / `android` build targets are now refused unless twec is built with `--features experimental`.
   - M0 planned this but neither did it nor recorded it as deferred. It surfaced while writing this note.

## Deviations from the plan

- **No `web-time` dependency.** A 30-line host-installable clock (`src/clock.rs`) does the job. It is one less crate, and the web shell installs `performance.now()`.
- **No winit on the web.** The plan listed a raw `web-sys` canvas as the cut line if winit-on-web slipped. I took it up front: the shell needs a canvas, `requestAnimationFrame` and key events, which `web-sys` covers directly without a second event loop to reason about. The native shell stays on winit.
- **The wgpu upgrade came at the end, not the start.** Chrome's rejection of wgpu 22 only showed up once the shell existed. It was one mechanical pass plus two API follow-ups.

## Deferred

- **Workspace crate split** (`twe-lang` / `twe-kernel` / `twe-native`).
  - Only `crates/twe-web` is its own crate. The kernel is a module with a clean boundary (above), so the split is a pure move.
  - It moves to **M3**: the SoA `World` is kernel state, and creating `twe-kernel` when that code lands avoids moving it twice.
- **Kernel-owned `cull` / `spatial` / `instance` / physics state, and the render thread-locals.**
  - `host3d` still reads lights, shadow, post-FX and animation state from `stdlib` thread-locals.
  - `spatial` / `cull` / `lod` / `instance` are still singletons, and `lod` / `streaming` sit behind `experimental`.
  - This moves to **M3**, where the slice renderer consumes `cull::Frustum` and per-archetype instance data. That is the first code that needs them as owned state.
- **Multi-file games in bundled and web builds.**
  - `import` reads modules from the filesystem.
  - Bundles carry only `main.twe` plus `assets/`, so a multi-file game fails in a `.exe` run away from its source tree, and in the browser.
  - Both targets need one fix: bundle modules and resolve imports through `bundle::read_asset_bytes`. It moves to **M4**, where `survive3d` may be multi-file.
- **Web input beyond 11 keys** (mouse, gamepad, full key set). This moves to **M4** with the per-tick input-command stream.
- **Shipping the runtime in release archives** (`web-runtime/` next to `twec` in the cargo-dist output). The lookup path exists; the release workflow doesn't populate it yet. This is release engineering, needed before M4's itch.io upload.
- **A preload manifest and asset handles.** `AssetSource` requests assets lazily on first draw. This moves to **M4**, when the slice has enough assets to show pop-in.
- **Browsers other than Chrome** are untested. Firefox and Safari WebGPU checks move to M4.
- **`wasm-demo.yml`** still deploys the 2D flappy demo to Pages. The 3D slice replaces it in M4.

## Found along the way

- **`twec play3d` had crashed on its first frame for four months** (the macroquad `THREAD_ID` assertion via `get_time`). No test rendered anything. There now is one, plus a guard that the 3D shell never calls into macroquad.
- **The broken `Instant` in the web GC sweep** (item 5) meant 2D web builds were also crashing. The Phase 30 build path had no runtime test to catch it.
- **A comment was wrong in the dangerous direction.** It called naga's accept-set a *superset* of what wgpu accepts at runtime; the black canvas proved otherwise. The comment is corrected, and the guard test covers the case naga misses.
- **Measurement hazard.** An automated Chrome window that the OS considers covered throttles WebGPU pages to 1 fps even while `visibilityState` reads "visible". A plain WebGPU clear loop shows the same drop, so that isn't a Twe regression. Measure fps only with the window visible.
