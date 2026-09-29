# Design note: the render graph (web3d-M7, session 1)

**Date:** 2026-09-29
**Milestone:** web3d-M7 ([plan](2026-09-28-web3d-m7-graphics-beyond-threejs.md), [sessions](2026-09-29-web3d-m7-plan.md))
**Code:** `src/kernel/graph.rs`; the frame in `src/kernel/render.rs` (`Renderer::render`, `FramePass`)

## Why now

At the end of M4 a frame was three hand-wired passes: shadow cascades, the lit scene, then tonemap + HUD. Every pass knew which renderer field held its target, and resizing meant remembering to reallocate each one.

M7 adds many passes:
- a depth prepass and GTAO;
- a multi-level bloom chain;
- TAA with a history buffer;
- depth of field and motion blur;
- screen-space reflections;
- culling and particle compute passes;
- transparency.

Wired by hand, the pass count multiplies the ways a target can be stale, wrongly sized or read before it is written. The M7 plan made a render graph the first task.

## Design

A pure planner, after the FrameGraph design (Yuriy O'Donnell, GDC 2017), kept deliberately small.

1. **Declare.** Each frame the renderer builds a `FrameGraph<P>`:
   - `create(TextureDesc)` declares a transient texture (label, extent, format, mips, layers, samples);
   - `import(name, output)` declares a renderer-owned texture (swapchain image, shadow atlas, later a TAA history);
   - `add_pass(p, name, reads, writes)` declares a pass; each access is `Sample`, `Attach` or `Storage`.
2. **Compile** (`FrameGraph::compile → Plan`):
   - **Order** is declaration order: simple, deterministic, and what authors read.
   - **Validation:** reading a transient before anything writes it is an error that names the pass and texture, not a black frame.
   - **Culling:** walking backwards, a pass lives only if it writes an output or something a later live pass reads.
   - **Allocation:**
     - Usage flags are derived from how passes access each texture.
     - Transients with matching descriptions and non-overlapping lifetimes share one *slot* (greedy aliasing).
3. **Execute.** A `TexturePool` backs the plan's slots.
   - Textures persist across frames and are reallocated only when the target size or a slot's description changes.
   - Each allocation carries a *generation*, so bind groups built on a view (the tonemap's) rebuild exactly when it changes.
   - The renderer runs `plan.passes` by matching on its own `FramePass` enum.

### Choices

- **No closures in the graph.** Pass bodies stay in the renderer as `match` arms, so they borrow renderer state normally. The alternative, graph-owned boxed closures that capture it, fights Rust's lifetimes and makes the planner untestable without a GPU. The planner has five unit tests that need no device.
- **Declaration order, not a topological sort.** Frames are written in the order they run. A sort would only add a second way to be surprised.
- **Imported textures may be read unwritten.** They hold last frame's data, which is exactly what TAA history needs.
- **No barriers or queues.** wgpu tracks usage itself, and the browser has one queue. The graph's job is lifetimes, allocation and culling.

## The port

`Renderer::render` now declares:
- `shadow cascade` × 3, writing the imported shadow map; omitted when shadows are off;
- `main`, reading the shadow map and writing transient `hdr colour` (Rgba16Float) and `depth`;
- `tonemap + hud`, reading `hdr colour` and writing the imported target.

The renderer's `depth_view`, `hdr_texture`, `hdr_view` and `hdr_size` fields, and `ensure_hdr_target` / `create_depth_view`, are gone. The pool owns the targets.

**Verification:** all seven GPU render tests (`tests/kernel_render.rs`) produce PNGs **byte-identical** to the images rendered before the port. Clippy is clean natively, for wasm32 and for `twe-web`.

## Not yet

- **Mip and layer sub-resources** (a bloom chain in one texture, per-cascade layers as graph resources): added when the bloom and shadow sessions need them.
- **Compute passes:** `Access::Storage` exists for them; the first user is GPU culling.
- **Per-pass GPU timings** (timestamp queries): come with the benchmark harness, where frame-time breakdowns are published.
