# web3d-M3 closeout: entities at scale, `look:`, materials, HUD

**Date:** 2026-09-28
**Milestone:** web3d-M3 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md))
**Status:** **closed.** The frame-rate exit criterion is met with ~2.3× headroom. The wasm script-tick criterion is **not met** (4.3 ms against 4 ms). The native per-update target is restated as unreachable on the tree-walker. The plan's columnar-storage design was replaced by the user's "interpreter first" decision. Details below.

## Exit criteria

| Criterion | Result | Evidence |
|---|---|---|
| `examples/swarm_3d.twe`: 5,000 seeking enemies at 60 fps in Chrome | **Met: 134–141 fps** (144 Hz display cap) | `frame_stats()` in Chrome; baseline 39–49 fps ([baseline note](2026-09-28-web3d-m3-baseline.md)) |
| Script tick ≤ 4 ms in wasm | **Not met: 4.3 ms median** (best 3.05 ms) | `node web/bench.mjs examples/swarm_3d.twe` (reported by CI); baseline ~7.5–8.3 ms per tick in Chrome |
| ≤ 300 ns per update natively | **Restated: 668 ns** (from 2,459) | This target assumed columnar storage. Measurement showed per-entity dispatch was ~100 ns of the cost, so the user chose interpreter work instead ([decision](2026-09-28-web3d-m3-interpreter-first.md)); a tree-walker won't reach 300 ns on this path |
| WASM speed tracked in CI | **Met** | `wasm-check` job runs the Node benchmark on every PR |

## What shipped

1. **`look:` blocks, a new language construct** ([design](2026-09-28-web3d-m3-look-block.md), `docs/06` §4.9a):
   - an entity declares `mesh`, `tint`, `scale`, `facing` and `material`, and the engine draws every live instance with no per-entity script;
   - wired through the lexer, parser, formatter, AST JSON, resolver, inference, `verify` (with rename fixes), GBNF/EBNF, tree-sitter, TextMate, and the LLM primer.
2. **`facing`**: per-instance yaw on the GPU, shadows included, plus `math.atan2`.
3. **`material`**: `visual` blocks become mesh surfaces. Their `pixel(uv, time)` is compiled into a per-material pipeline, lit and tinted, with an alpha cut-out. The Phase 9 procedural-shader system now reaches the 3D game.
4. **HUD**: `text()` and `rect()` in 3D draw over the scene in 2D-canvas coordinates, using a bundled font atlas.
5. **Interpreter speed, 3.7× natively** (eleven measured steps; [progress table](2026-09-28-web3d-m3-interpreter-first.md)):
   - locals resolved to slots, with globals and fields read through cached positions;
   - cheaper calls;
   - inline small tuples, one allocation per `vec3`;
   - the GC sweep reading the clock every 256 objects instead of per object (an 18% browser-only cost).
6. **Measurement tooling:**
   - `frame_stats()` in the web runtime;
   - a headless wasm benchmark (`web/bench.mjs`), run in CI;
   - CPU profiling of the wasm build through Node / Chrome DevTools, possible because the wasm keeps its function names.
7. **Correctness fixes found on the way:**
   - the web build rendered without its final gamma curve on non-sRGB canvases (now drawn through an sRGB view);
   - the tree-sitter grammar hadn't built since Phase 13;
   - `then` was missing from the grammar export;
   - `twec play` exited 0 on every startup error;
   - a look key's module wasn't GC-marked;
   - material pipelines are cached by WGSL source, so a stale pipeline can't survive a reload.

## Deviations from the plan

- **Columnar (SoA) archetype storage and `EntityRef` handles were not built.** The user decided on measurements (dispatch was ~4% of per-update cost). Everything that depended on that design moves with it:
  - `self` as an immediate `EntityRef` and the stale-reference errors;
  - `entities.near` via the kernel's `LooseGrid`;
  - `alive(e)`;
  - subclass-inclusive `entities.of`.

  They move to M4, **as needed by `survive3d`**; they are not required by default.
- **Deferred spawn/despawn semantics** (spawns visible next tick, despawn at the end of a system) were not needed without system-style updates. The existing semantics stand.
- **No single-cascade shadow or compute culling:** the existing CSM and CPU frustum culling were sufficient (kernel CPU time 0.4 ms per frame at 5,000 entities).
- **No declarative-behaviour note:** the plan wrote one only if performance missed by more than 2×, and it didn't.

## Deferred

- **The last ~7% of the wasm tick.** It needs compiling the AST to closures, which means re-implementing how suspended fibers resume (by AST statement index). It re-enters only if the M4 slice profiles as interpreter-bound.
- **The workspace crate split** (`twe-lang` / `twe-kernel` / `twe-native`). Its trigger was the columnar `World`, which wasn't built. The kernel boundary holds without it: `kernel/` imports only plain render types. It moves to **M6**, when retiring macroquad reshapes the crate anyway.
- **Kernel-owned `cull` / `spatial` / `instance`.** The kernel culls with its own frustum code; the Phase 32 modules stay behind `experimental`. `entities.near` (M4, if needed) would be the first real consumer of a spatial index.
- **UI widgets and the other 2D drawing calls in 3D.** They give a clear error for now; buttons for the level-up picker are M4 work. The full set arrives with the 2D-on-kernel port (M6).
- **Transparency** beyond the cut-out, and **emissive materials**.
- **A visual check of the HUD and colour change in a visible Chrome window.** They were verified by a native GPU render test and an error-free Chrome run; the browser window was occluded during the session.
