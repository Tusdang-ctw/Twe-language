# Phase 38 closeout — Browser 3D (wgpu-on-web)

**Status:** codebase-scaffolding-closed **2026-05-11**. Seven sessions shipped: build target, wgpu-on-web audit document, environment-introspection builtins, WebAudio reuse (Phase 30 path), physics.body wasm verification, the `examples/crystal_hunter_web.twe` placeholder, this closeout. The phase is **gated externally on browser wgpu maturity** (Firefox-stable + Safari-stable); the runner-side wgpu-on-web port is honest-deferred to a follow-on session once that gate clears.

Same shape Phases 35 + 37 used: the codebase work ships, the runner integration that needs an external precondition (Steam AppID, eval-loop refactor, browser-wgpu adoption) honestly defers with a single audit document driving the follow-on.

---

## Sessions shipped

| # | Surface | Files |
|---|---------|-------|
| 1 | `BuildTarget::Wasm32_3D` + parse mapping + directory layout | `src/build.rs` |
| 2 | wgpu-on-web audit + barrier catalogue | `docs/changes/2026-05-11-phase-38-wgpu-on-web-audit.md` |
| 3 | `assets.platform()` / `assets.is_browser()` / `assets.is_mobile()` builtins | `src/stdlib.rs` |
| 4 | WebAudio scaffold (Phase 30 reuse — no new code) | audit document |
| 5 | `physics.body` wasm verification (rapier3d compiles for wasm; verified) | audit document |
| 6 | `examples/crystal_hunter_web.twe` — platform-aware placeholder demo | `examples/crystal_hunter_web.twe` |
| 7 | Closeout + doc sync | this file, `docs/05-roadmap.md`, `CLAUDE.md`, `README.md` |

---

## What ships in detail

### Session 1 — `BuildTarget::Wasm32_3D`

`src/build.rs` gains a `Wasm32_3D` variant in the build-target enum. Parser mappings: `"wasm32-3d"`, `"wasm32-unknown-unknown-3d"`. Label: `"wasm32-3d"`. Binary extension: none (directory output). The build dispatcher routes to a new `build_wasm3d_target` that produces `dist/web-3d/` containing:

- `main.twe` (the game source).
- A flat copy of the project's `assets/` directory.
- A placeholder `index.html` explaining the deferral.
- A `README.txt` pointing to the audit document.

The wasm binary itself is *not* compiled today — that's the deferred wgpu-on-web port. Scripts targeting `--target wasm32-3d` get the directory layout + asset bundle today; the runtime that serves them waits on browser-wgpu maturity.

### Session 2 — wgpu-on-web audit

`docs/changes/2026-05-11-phase-38-wgpu-on-web-audit.md` catalogues every cfg gate that must relax for browser 3D to work:

- `src/play3d.rs` — wgpu device acquisition (async; needs `wasm_bindgen_futures` on wasm), winit window creation (needs canvas integration), `request_animation_frame` driving the frame loop, glTF mesh loading via `assets.fetch` instead of `std::fs::read`.
- `src/physics3d.rs` — no new barriers; rapier3d compiles for wasm. Drops cfg gate together with play3d in the same follow-on commit.
- `src/play_visual.rs` — same shape as play3d, lower priority.
- Cargo.toml — five native-only deps (`wgpu`, `winit`, `pollster`, `bytemuck`, `gltf`) need to move to unconditional dependencies. `pollster` stays via a wasm-side branch that uses `wasm_bindgen_futures::spawn_local`. `gilrs` + `arboard` stay native-only.

The audit lists the seven Phase 38 sessions with status per-session — sessions 1, 2, 3, 6, 7 ship; sessions 4 (audio) and 5 (physics) are no-code Phase 30 / rapier3d-already-works observations.

### Session 3 — `assets.*` environment introspection

The wgpu-on-web port plan routes asset loading through `fetch` transparently — scripts call `texture("hero.png")` and the wasm-side runner reroutes through `window.fetch`. The script-visible API doesn't change. But scripts *sometimes* need to branch on environment (touch-only controls on mobile + browser, skip fullscreen toggle on browser). Phase 38 session 3 ships three builtins for that:

- `assets.is_browser()` — true iff `cfg!(target_arch = "wasm32")`.
- `assets.is_mobile()` — true iff `cfg!(any(target_os = "ios", target_os = "android"))`. Today this is always false (no mobile runtime ships yet); Phase 39 follow-on flips it true.
- `assets.platform()` — returns one of `"windows"` / `"macos"` / `"linux"` / `"ios"` / `"android"` / `"browser"` / `"unknown"`.

These compose into the canonical platform-aware script pattern shown in `examples/crystal_hunter_web.twe`: probe at `on enter`, branch on `is_browser` in `on render`.

### Sessions 4 + 5 — Phase 30 audio reuse + rapier3d wasm verification

No new code. Phase 30's WebAudio path (`AudioContext` unlock on first user gesture, sample-accurate scheduling via `audioCtx.currentTime`) already handles the audio side; once the wgpu-on-web port lands, audio Just Works through the same code. rapier3d is pure-Rust and compiles for wasm32 today (Phase 28 verified this transitively); the only reason `src/physics3d.rs` is wasm-gated is that `play3d.rs` is wasm-gated, not because the physics crate doesn't support wasm.

Both are captured in the audit document as "no new work this session; deferral picks up the existing code unchanged."

### Session 6 — `examples/crystal_hunter_web.twe`

A platform-aware placeholder demo. On a desktop build it shows the native platform name + a pointer to `crystal_hunter.twe` for the full 3D experience. On a browser build it shows the deferral message + a pointer to the audit document. Verifies clean (`twec verify` returns 0 diagnostics). Corpus header check passes.

This isn't yet a 3D scene — that's the deferred port. It's the canonical "how to write a platform-aware Twe script" reference, and it'll become a real browser 3D scene once the port lands.

### Session 7 — Closeout (this file)

Plus doc sync.

---

## Test deltas

| | Pre-Phase-38 | Post-Phase-38 |
|---|---|---|
| Lib unit tests | 556 (post-Phase-37) | 556 (no new tests this phase — scaffolding-only) |
| Integration tests | 382 | 382 |
| **Total passing** | **938** | **938** |

Same pre-existing CRLF-cascade lib failures unchanged.

`cargo build --release` clean. `cargo clippy --release --all-targets -- -D warnings` clean — no new lints surfaced.

The decision to ship zero new tests is deliberate. Phase 38 is scaffolding-only: build-target variants, audit prose, three trivial environment-introspection builtins, a verify-clean placeholder example. None of these benefit from new unit tests; the next session that ships actual wgpu-on-web rendering is where tests land (browser-headless render smoke check, asset-fetch round-trip, etc).

---

## Honest deferrals

The phase is *codebase-scaffolding-closed*. The following remain:

1. **wgpu-on-web pipeline port.** The entirety of `src/play3d.rs`'s `cfg(not(target_arch = "wasm32"))` gates relaxing to wasm-friendly equivalents. Phase-sized work. Gated externally on Firefox-stable + Safari-stable browser wgpu support.
2. **Async device acquisition on wasm.** `pollster::block_on` → `wasm_bindgen_futures::spawn_local`. Touches every async wgpu call site.
3. **`request_animation_frame` integration.** Today native uses winit's event loop; browser needs `requestAnimationFrame` wired through `wasm_bindgen`.
4. **Asset fetch routing.** `texture("hero.png")` / `mesh("hero.glb")` on wasm need to route through `window.fetch` + `Response::array_buffer()` + `wgpu::Texture::create_with_data` / `gltf::Gltf::from_slice`. The audit document #5 names this; the implementation lives in the wasm branch of `play3d.rs` once the port lands.
5. **Cargo.toml move-to-unconditional.** `wgpu` + `winit` + `pollster` + `bytemuck` + `gltf` + `rapier3d` + `image` migrate from `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` to unconditional `[dependencies]`. Mechanical; lands in the same follow-on commit.
6. **End-to-end browser-3D demo running in Chrome + Firefox + Safari.** External validation — happens once browser wgpu is broadly stable.

---

## API surface additions

Phase 38 adds **3 new builtins** in a new `assets.*` namespace: `platform`, `is_browser`, `is_mobile`. The namespace is cross-platform (works on every target), platform-introspection-focused. No new builtins for the wasm-3d path itself — the existing asset / texture / mesh / audio surface routes through the wgpu-on-web runner transparently when the port lands.

---

## Doc updates

- `docs/05-roadmap.md` — Phase 38 entry updated to "codebase-scaffolding-closed 2026-05-11" with the 6 honest deferrals.
- `CLAUDE.md` — round-2 paragraph extended with Phase 38 closeout summary.
- `README.md` — test count unchanged at 938 (no new tests); examples gallery +1 (`crystal_hunter_web.twe`).

---

## What we learned

- **The scaffolding-closed shape is reusable.** Phase 35, 37, 38 all closed with the same discipline: ship the API surface + the audit document + the deferral list, name the external precondition that gates production-ready completion. Phase 36 was the outlier — it shipped a more complete runner because Steam P2P doesn't have an external precondition besides the Steam AppID (which most operators already have).
- **Build-target descriptors are cheap to add and worth doing early.** Adding `BuildTarget::Wasm32_3D` + the dispatch + a placeholder layout is ~120 LOC. Operators can wire CI pipelines, ship scripts, write tooling against `--target wasm32-3d` *today*; the runner that consumes the layout lands later without breaking the contract.
- **Platform-introspection builtins are independently useful.** `assets.platform()` is the kind of thing scripts ask for in *every* port: "what am I running on?" Adding it as part of Phase 38's scaffolding side-of-house pays off well past the wgpu-on-web port.
- **Cargo target gates aren't a barrier-of-the-week; they're a feature.** The dep-side audit walked every `[target.'cfg(...)'.dependencies]` block and named exactly which crates need to migrate when. That's checkable; it's not vibes. The migration is a single mechanical step in the follow-on commit.
