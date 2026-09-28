# web3d-M3 baseline: what 5,000 entities cost today

**Date:** 2026-09-28
**Milestone:** web3d-M3 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md))

M3's exit criterion is 5,000 seeking enemies at 60 fps in Chrome, with the script tick at most 4 ms in wasm and at most 300 ns per update natively. Before changing the entity model, this note measures the current one on the exit benchmark, so the milestone's work can be ordered by evidence and the closeout has a before/after pair.

## The benchmark

`examples/swarm_3d.twe` is written in today's Twe. Each `Enemy` is an entity instance whose `update(dt)` steers it toward the player; when it arrives, it respawns at the rim. The top-level `on render():` draws each enemy with `cube()` in a loop over `entities.of(Enemy)`.

It has to draw them that way because **3D ignores per-entity `render()` methods**: `eval::render_frame3d` runs only the top-level handler. A 3D game today has to iterate its entities by hand. That is the gap the planned `look:` block closes.

## Numbers

**Native:** `cargo test --release --test perf_probe swarm_3d -- --ignored --nocapture`, min of 120 frames.

| Cost per frame | Native | Target |
|---|---|---|
| `update` tick | **6.50 ms = 1,299 ns/update** | ≤ 300 ns/update |
| script render (the loop + 5,000 `cube()` calls) | **3.56 ms = 713 ns/entity** | — |

**Chrome 152:** `twec build --target web`, measured with the web runtime's new `frame_stats()` export over four 2-second windows.

| Cost | Chrome (wasm) | Target |
|---|---|---|
| `update` tick | **7.3–8.3 ms per tick** | ≤ 4 ms |
| ticks per frame | 1.2–1.5 (the fixed step runs >1 tick once a frame exceeds 16.7 ms) | 1 |
| script render | **10.7–12.1 ms per frame** | — |
| kernel (cull, instance upload, encoding) | 0.37–0.39 ms per frame | — |
| **frame rate** | **39.5–49 fps** | 60 |

## What the numbers say

1. **The renderer is not the bottleneck.** The kernel's CPU time is 0.4 ms. Nearly all of the frame is script.
2. **Script-side drawing is the largest single cost in the browser** (about 11 ms), and it is disproportionately slow in wasm: 3× native, against 1.2× for `update`. Named-argument builtin calls are the likely culprit. Either way, `look:` removes this cost rather than tuning it: the kernel reads positions from entity storage, and no script runs per drawn entity.
3. **`update` needs about 2× in wasm and about 4× natively.** The work is:
   - archetype (SoA) storage, where field reads and writes hit columns with no instance lookup;
   - slot-resolved locals (deferred from M1);
   - an unboxed `vec3` path, since today every `vec3(...)` allocates a heap tuple.
4. **The fixed step turns slow frames into slower frames.** Once script time passes 16.7 ms, frames run extra ticks. Getting under budget has a compounding payoff.

## M3 work order

1. **`look:` and archetype storage together.** `look:` needs somewhere columnar to read positions from, and it is the biggest browser win. Design note first (a language contract change).
2. **Update speed:** column-backed field access, slot locals, unboxed `vec3`. Re-measure against the targets above.
3. **Materials from `visual` blocks and HUD text.** These don't affect the benchmark; they are what the M4 slice needs.

## Also found

- **Browsers cache the runtime across updates.** A rebuilt `twe_web.js` / `twe_web_bg.wasm` was served stale from the HTTP cache, so a page can load new JS against an old wasm. Content-hashed file names in `twec build --target web` go on the M4 list (before the itch.io upload).
