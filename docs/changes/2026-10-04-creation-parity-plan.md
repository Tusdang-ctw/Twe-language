# Creation-parity roadmap: better than Three.js at *making* browser 3D games (M8–M12)

**Opened:** 2026-10-04 (after M6 closed; all of M0–M7 done)
**Supersedes as the active line:** the web3d pivot plan ([pivot note](2026-09-27-web3d-pivot.md)), whose milestones M0–M7 are all closed.

This note is canonical for the next milestone line, the way the pivot note was for M0–M7: milestones, exit criteria, cut lines, and scope.

## Why this exists

The pivot roadmap (M0–M7) answered "can Twe render a browser 3D game": one wgpu kernel draws 2D and 3D, native and web; `survive3d` plays in Chrome; graphics are at or below Three.js's image error on all 12 benchmark scenes ([M7 closeout](2026-09-30-web3d-m7-closeout.md)); macroquad is gone ([M6 closeout](2026-10-04-web3d-m6-closeout.md)). The open question is now **"can a person or an AI *build* a 3D game better in Twe than in Three.js?"**

## The claim, stated honestly

"Better than Three.js at *all* aspects" is **not** the target — it is not a well-formed engineering goal, and chasing it is the scope-creep the brief's anti-patterns forbid. Three.js's real moat is ecosystem + training-corpus + maturity: person-years of community work, not code-months. Twe wins by being *decisively* better on its wedges — AI authorability, code-only procedural visuals, one-obvious-way simplicity, determinism, tiny self-contained builds — **for its creators** (AI-first authorship + the solo/indie human). The target of this line:

> **The best way for a human or an AI to make and ship a browser 3D game — measured head-to-head against Three.js.**

## Decisions (locked with the maintainer 2026-10-04)

- **UGC is out of this roadmap entirely.** Hosted browser UGC (Roblox-class) needs the seven unmet sandbox blockers ([pre-reqs](2026-05-11-mmo-sandboxing-pre-reqs.md)) + an authoritative server runtime — multi-year, team-sized. A separate, later, explicitly-out-of-scope track; standalone/Workshop sharing is also out for now.
- **The measured LLM moat (M5) is deferred to the end** (capstone M12). Build the engineering milestones first; run the paid benchmark last.
- **All four major browsers** (Chrome, Edge, Firefox, Safari) are the committed bar, with an honest Chromium-first cut line.

## Grounded current state (2026-10-04 audit)

- **Works / verified:** native + Chrome 3D; one kernel for 2D + 3D, native and web; glTF PBR + 7 KHR extensions + skinning + animation; PNG/JPEG/HDR textures; `twec build --target web` ships a prebuilt runtime; the full M5 bench harness (60 tasks, Python baseline, 549-triple corpus) built but un-run; Studio (Tauri + React, Monaco, LSP, AI loop on `twe-llm`).
- **Stub / deferred this line must close:** no live-browser frame capture (M6 debt); web alpha blends in linear not gamma space; typed text on web; no 3D `.exe`/window capture hook; KTX2 / Basis / meshopt / Draco / anisotropy / morph-targets not loaded; Studio has no debugger and no profiler; an intermittent `STATUS_HEAP_CORRUPTION` in `tests/kernel_render.rs` (~1/7 under concurrency, open).

## Milestones

### M8 — Browser truth (all four browsers) — **CLOSED 2026-10-04** ([closeout](2026-10-04-web3d-m8-closeout.md))

Everything Twe claims to run in a browser is *captured running in a browser*. Nothing downstream is credible until this is shown, not asserted. **Done:** pong (2D) + hello_3d (3D) pixel-match native in real Chrome/Edge/Firefox (0.1–0.2/255); WebKit→graceful fallback (Safari WebGPU needs a Mac); gamma-blend parity fixed (dungeon 13.2→0.25); web typed text verified live; a 3D `TWE_CAPTURE` hook; the heap-corruption flake gone after the M6 macroquad removal (0/32 runs).

**Exit:** a CI Playwright job loads a built 2D game and a built 3D game in headless Chrome, captures a frame, diffs it against the native reference within tolerance, and records an FPS (probe WebGPU on a bare page first — it throttles to 1 fps when occluded); the capture confirmed on Edge, Firefox, Safari (or a documented graceful "needs WebGPU" message); web alpha blends match native via a non-sRGB surface view (verified on an alpha-heavy scene); typed text on web wired (`text_input` types in a browser); a `TWE_CAPTURE` hook on the 3D shell via surface readback; the `STATUS_HEAP_CORRUPTION` test flake root-caused and fixed.
**Artifact:** a CI-published gallery — the same game in each browser with its FPS.
**Cut line:** if Firefox/Safari WebGPU is too unstable, ship Chromium-verified + the graceful fallback + a tracked issue.
**Key files:** `crates/twe-web/src/lib.rs`, `src/kernel/canvas.rs`, `src/kernel/render.rs`, `src/play3d.rs`, `tests/kernel_render.rs`, `.github/workflows/`, `bench/graphics/fps.mjs`.

### M9 — Asset pipeline parity

A glTF that Three.js's `GLTFLoader` loads, Twe loads too — compressed textures and geometry included.

**Exit:** KTX2/Basis textures render (prefer build-time transcode so the runtime stays pure-Rust / wasm-safe); meshopt and Draco geometry load; `KHR_materials_anisotropy` applied; morph targets sampled (currently skipped); multi-skin; a `twec asset info <file>` subcommand (+ VRAM estimate vs the 512 MB budget) and a Studio `AssetBrowser` preview.
**Artifact:** a demo loading a Draco + KTX2 Blender export that Three.js also loads, side by side.
**Cut line:** if a decoder is C++/wasm-hostile, transcode at build time and document that the runtime doesn't decode it — never break `cargo build`.
**Key files:** `src/kernel/render.rs`, `src/kernel/material.rs`, `src/build.rs`, `src/cli.rs`, Studio `AssetBrowser.tsx`.

### M10 — Studio maturity (human authoring)

The editor becomes a place a human prefers to "VS Code + a pile of Three.js libraries." Studio has viewport/editor/AI/inspector/scene-tree/state-graph/console but no debugger and no profiler.

**Exit:** a debugger (breakpoints, step, variable/entity inspection on a running game); a profiler panel (frame-time + GC + draw-call, reusing `twec profile` + the F3 HUD); one-click publish (desktop `.exe` + web, wrapping `twec build`); live scene/inspector editing without a full reload.
**Artifact:** a screen-capture flow — open, edit, breakpoint, step, profile, publish to web + exe.
**Cut line:** if a full debug adapter slips, ship breakpoint + inspect (no step-into) and track the rest.
**Key files (twe-engine):** new `studio/src/components/Debugger/`, `Profiler/`, `useLspClient.ts`, `usePlay.ts`. **(twe-language):** `src/eval.rs`, `src/cli.rs`, `src/lsp.rs`.

### M11 — Game-feature depth

The 3D features a real game needs that Twe still lacks, scoped by a chosen reference game, not a checklist.

**Exit (choose by the reference game):** animation blending + state machines (Twe samples channels but doesn't blend); physics joints + CCD (deferred in Phase 18); the eval-side rollback rewind engine (deferred in Phase 37) only if a target game is net-twitch; a polished reference 3D game (extend `survive3d` or a small Tunic-like slice), playable in a browser, that stands next to a comparable Three.js game.
**Artifact:** that reference 3D game, in a browser.
**Cut line:** WebXR is a stretch, not committed; net rollback only if a target game needs it.
**Key files:** `src/kernel/render.rs`, `src/physics3d.rs`, `src/rollback.rs` + `src/eval.rs`, `examples/`.

### M12 — The measured proof (capstone; includes the deferred M5 live runs)

Substantiate — or honestly refute — the claim, with numbers.

**Exit:** run the parked M5 live benchmark (a ~$2 pilot then the full matrix on approval; publish `bench/results.md` with CIs + ablations; deliver the kill-line verdict — if Twe+verify doesn't beat Python on pass@1, drop the LLM-moat claim + a root-cause note); a head-to-head case study building the *same* 3D game in Twe and Three.js (time, LOC, friction — human and LLM); a public write-up stating exactly where Twe wins and loses.
**Artifact:** the published results + the two same-game builds (`examples/threejs-parity/`).
**Cut line:** none — the honesty gate. If the thesis fails, the deliverable is the root-cause and a reframed claim.
**Key files:** `bench/`, `crates/twe-llm`, new `examples/threejs-parity/`, docs.

## Order

M8 first (close the honesty debt + the all-browsers requirement). M9 and M10 parallelize (assets vs editor). M11 depends on nothing but is scoped by its reference game. M12 last (M5 deferred), as the capstone.

## Out of scope (explicit)

- Hosted browser UGC / Roblox-class platform (seven sandbox blockers + server runtime + accounts/moderation/economy). Standalone/Workshop sharing also out for now.
- Mobile / console targets — the `experimental` scaffolding stays as-is.
- "Feature parity with Three.js at all aspects" — not a target (see the honest reframe).

## Rules

`web3d-mN:` commit prefix continues (M8+). Each milestone ends with a `docs/changes/<date>-web3d-mN-closeout.md`. Every milestone ships a runnable artifact; nothing is "closed" until its exit criteria are met. Clippy clean for native + `wasm32` + `-p twe-web`; `cargo test` green.
