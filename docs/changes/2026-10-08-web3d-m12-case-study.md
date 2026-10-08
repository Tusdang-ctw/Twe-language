# web3d-M12: the same-game case study (human-authoring axis)

**Date:** 2026-10-08
**Line:** web3d creation-parity (M8–M12), **M12 capstone — session 1**
**Status:** the human-authoring half shipped; the LLM half (parked M5 live
runs) still needs the maintainer's API key + budget.

## What M12 is, and what this session did

M12 is "the measured proof" for the thesis *the best way for a human or an
AI to build a browser 3D game.* It has two axes:

1. **Human authoring** — build the *same* game in Twe and in Three.js,
   measure the difference. ← **this session.**
2. **LLM authoring** — the parked M5 benchmark (pass@1 vs a Python baseline,
   with a pre-registered kill line). Blocked on credentials + budget;
   untouched here.

This session delivers axis 1 as `examples/threejs-parity/`.

## Deliverable

`examples/threejs-parity/`:
- `twe/` — a pointer to the existing reference game `examples/action3d`
  (229 lines of Twe, one file; no copy, to keep one source of truth).
- `threejs/` — a **faithful, idiomatic, runnable** Three.js
  re-implementation of that exact game: `GLTFLoader` hero +
  `AnimationMixer`, a by-hand locomotion blend space, IBL via
  `RGBELoader`+`PMREMGenerator`, a `DirectionalLight` with a configured
  shadow frustum, an `EffectComposer` (GTAO + UnrealBloom), the 8 chasing
  enemies, the chase camera, and the rope bridge on **the same rapier
  engine** (`@dimforge/rapier3d-compat`). Two procedural `visual` blocks
  become `onBeforeCompile` GLSL patches on `MeshStandardMaterial`.
- `README.md` — the side-by-side write-up, the measured table, the
  feature-by-feature friction table, and an explicit honest read.

## Verification (both sides actually work — not a strawman)

- **Twe** `action3d`: runs native (`twec play3d`) + web
  (`twec build --target web`), verifies 0/0, covered by the suite.
- **Three.js**: `npm install` + `vite build` clean (27 modules, no errors);
  a headless **real-Chrome** smoke (`vite preview` + playwright-core, the
  M8 harness) loads the page, initialises the rapier wasm, builds the full
  scene (IBL/shadows/GTAO/bloom/skinned hero/enemies/jointed bridge) and
  runs its render loop **with zero JS errors**, reaching `status: ready`
  and advancing frames; `/assets/hero.glb` and `/assets/sky.hdr` serve 200
  (the lone 404 is `/favicon.ico`). A pixel-level visual diff is the
  GPU+eyes maintainer step (the same visual-playtest deferral `action3d`
  already carries).

## The measured result (objective, counted)

| | Twe | Three.js |
|---|---:|---:|
| Game code | **229 lines, 1 file** | 452 lines, 2 files (+30-line HUD HTML) |
| Config/scaffold files | 1 (`twe.toml`) | 4 |
| Runtime deps | **0** | 3 direct / 13 npm packages |
| Build | `twec build` (one binary) | Node + npm + Vite |
| Shipped JS | — (one wasm engine) | 2.76 MB (941 KB gzip) |

Roughly **2× the code, 13 dependencies vs 0, and a build toolchain vs one
command** — the gaps concentrate exactly on Twe's wedges (first-class game
concepts, one-obvious-way, determinism, a single self-contained artifact):
the blend space is one call vs ~25 lines of hand-weighting; a procedural
surface is a type-checked `visual` block vs stringly-typed `onBeforeCompile`
GLSL; IBL/sun/AO/bloom/HUD are one-liners vs assembled pipelines.

## The honest read (in the README, summarised here)

- Twe wins clearly on **authorability** for a self-contained game of this
  kind, with counted numbers on a real running game.
- Three.js wins, not closely, on **ecosystem / maturity / flexibility /
  tooling / training corpus** — and `onBeforeCompile`, painful as it is,
  still expresses shaders Twe's `visual` block cannot.
- This settles the authorability wedge; it does **not** settle "the best
  way" on its own — that also rests on the unmeasured LLM axis (M5) and the
  ecosystem axis. The defensible claim today is narrow and real.

## Remaining for M12

- The parked **M5 live runs** (pass@1 vs Python + the kill-line verdict) —
  maintainer's API key + budget.
- An optional pixel/visual side-by-side capture of the two games (GPU).
