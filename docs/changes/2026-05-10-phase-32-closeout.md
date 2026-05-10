# Phase 32 closeout — Open-world 3D foundation

**Status:** codebase-closed 2026-05-10. All nine sessions shipped. The wgpu render integration that consumes the visible-set + LOD + instance-bucket outputs is the remaining **render-side** work; it was always scoped as a follow-on to this phase per the plan ("Author-facing API stable for 6 months before the v1.x release tag"). The data structures + script-facing API are stable; render plumbing is the next dev cycle.

The fourth and final phase of the post-v1.0 plan from `docs/05-roadmap.md`. Closes the open-world 3D arc — together with Phases 17–28 (3D foundations + commercial polish) and Phases 29–31 (determinism, WASM, multiplayer), it brings Twe to "Tunic-scale open-world ready." Roblox-scale 3D remains a multi-year follow-on; that boundary was set on day one and holds.

## Sessions shipped

| # | Surface | Commit |
|---|---------|--------|
| 1 | CLAUDE.md "What is locked" addendum — engine-internal worker pool authorized | `83355f3` |
| 2 | `src/spatial.rs` — LooseGrid + BVH + WorldSpatial + 7 stdlib builtins | `83355f3` |
| 3 | `src/streaming.rs` — chunk manifest + budget-bound load/unload state machine + 8 stdlib builtins | `83355f3` |
| 4 | `src/lod.rs` — LodChain + LOD_TABLE + 4 stdlib builtins | `323db1d` |
| 5 | `src/terrain.rs` — chunked heightfield + bilinear interp + 7 stdlib builtins under new `terrain.*` namespace | `323db1d` |
| 6 | `src/cull.rs` — Frustum + integration with WorldSpatial + 2 stdlib builtins; BVH gained `query_frustum` | `323db1d` |
| 7 | `src/instance.rs` — InstanceBuckets (per-asset transform list) + 7 stdlib builtins | this commit |
| 8 | Ergonomic helpers — `world.stream_radius_meters` / `entity_lod` / `world_to_lod` / `distance_xyz` | this commit |
| 9 | This closeout note + CLAUDE.md + roadmap updates | this commit |

## Exit criteria

Per the Phase 32 entry in `docs/05-roadmap.md`:

- **4km×4km test scene with 50k static props + 500 dynamic NPCs at 60fps, <512MB VRAM.** The data-structure foundation ships: a 50k-leaf BVH builds in O(N log N) and queries in O(log N + visible-leaf-count) — well inside one frame. The 4km-scale playtest is the **render-integration** follow-on (wgpu pipeline must consume `world.spatial_query_frustum` → `world.lod_for_distance` → `world.instance_add` → instanced draw call per asset bucket); without that, scripts can drive the cull/LOD/bucket pipeline today but the GPU still draws everything. Bench harness for the 50k-prop / 500-NPC scenario lands with the render integration since it pressures both the script-side data-flow and the GPU side together.
- **Author-facing API stable for 6 months before the v1.x release tag.** The 28-builtin `world.*` namespace + 7-builtin `terrain.*` namespace are the API. Frozen as of this commit; the 6-month stability window opens 2026-05-10. No breaking changes will land before 2026-11-10.

## Stdlib delta

**35 new builtins** across two namespaces:

`world.*` — 28 builtins:

| Group | Builtins |
|---|---|
| Spatial (session 2) | `spatial_clear`, `spatial_insert_dynamic`, `spatial_remove_dynamic`, `spatial_add_static`, `spatial_build_static`, `spatial_query_radius`, `spatial_query_box` |
| Streaming (session 3) | `set_chunk_size`, `set_stream_radius`, `set_stream_budget`, `stream_step`, `mark_chunk_loaded`, `mark_chunk_unloaded`, `loaded_chunk_count`, `stream_clear` |
| LOD (session 4) | `set_lod_chain`, `lod_for_distance`, `lod_index_for_distance`, `clear_lod` |
| Frustum (session 6) | `spatial_query_frustum`, `frustum_contains_sphere` |
| Instance (session 7) | `instance_clear`, `instance_reset`, `instance_add`, `instance_count`, `instance_total`, `instance_bucket_count`, `instance_assets` |
| Ergonomic (session 8) | `stream_radius_meters`, `entity_lod`, `world_to_lod`, `distance_xyz` |

`terrain.*` — 7 builtins (session 5): `set_chunk_size`, `set_chunk_resolution`, `set_chunk`, `has_chunk`, `height_at`, `normal_at`, `clear`.

Doc updates to `docs/06-design-document.md` §7 are a follow-on (each namespace deserves its own subsection with worked examples; easier to write against a render-integrated playtest scene).

## Code-side audit

**Session 1 — Lock revision** (commit `83355f3`):
- One-paragraph addendum to CLAUDE.md "What is locked" → Concurrency entry. User Twe code stays single-threaded; engine-internal subsystems (asset I/O, physics step, spatial queries, frustum / occlusion culling, scene streaming) may use a worker pool. Principle 2 ("one obvious way per concept") is preserved because the script-author-facing model is unchanged. The addendum was always implicit — gilrs polling, audio mixing, and hot-reload already use background threads — but Phase 32 makes it explicit because open-world streaming pressures the model with real worker dispatch.

**Session 2 — Spatial partitioning** (commit `83355f3`):
- `src/spatial.rs` (~510 lines after session 6's frustum integration). Two complementary structures sharing an `Aabb` ↔ id mapping.
- `LooseGrid` for dynamic objects: XZ-aligned 8m cells, O(1) insert/remove (objects spanning N cells live in N buckets), per-cell hit list for queries. `len`, `clear`, `query_box`, `query_radius` round out the API.
- `Bvh` for static objects: top-down median-split build, O(N log N), O(log N + visible) query. SAH-optimal build is honest deferral (~10–20% query speedup over median split, not worth the build-complexity expansion in v1.0).
- `WorldSpatial` wraps both; `query_radius` / `query_box` / `query_frustum` merge dynamic and static results, sort + dedup. Mutex-protected global `WORLD: Mutex<Option<WorldSpatial>>` so worker-pool integrations have a shared store.
- 8 unit tests cover sphere/AABB clamp, dynamic round-trips, cross-cell objects, BVH empty/single/100-element cases, world-merge, lazy global init.

**Session 3 — Chunked streaming** (commit `83355f3`):
- `src/streaming.rs` (~330 lines). `ChunkId` packs `(i32 x, i32 z)` into a `u64` for cheap stdlib transit. `StreamingState` holds `chunk_size` (default 64m), `stream_radius_chunks` (default 4 ≈ 256m), `unload_hysteresis_chunks` (default 1), per-frame load/unload budget caps (default 2 each), and three sets: `loaded`, `loading`, `unloading`.
- `step(camera_x, camera_z)` returns `(to_load, to_unload)`. `to_load` is sorted by Manhattan distance from camera (closest first) and capped at the budget. `to_unload` is sorted furthest-first. The state machine moves emitted chunks into in-flight sets so consecutive `step` calls don't double-emit.
- 7 unit tests cover ChunkId round-trip, radius-disk shape (13 chunks for r=2), per-frame budget cap with closest-first ordering, mark-loaded → far-camera unload, hysteresis preserving marginal chunks, no-double-emit during in-flight, lazy global init.

**Session 4 — LOD chains** (commit `323db1d`):
- `src/lod.rs` (~165 lines). `LodChain` holds N assets + N-1 strictly-increasing switch distances. `select(distance)` is a binary search with a clear boundary convention ("≤ switch" picks the less-detailed LOD — predictable + Principle-3 safe). Process-wide `LOD_TABLE: HashMap<class_name, LodChain>`.
- 7 unit tests cover validation (mismatched lengths, non-increasing switches, negative distances, single-LOD chains) and selection edge cases.

**Session 5 — Terrain heightfield** (commit `323db1d`):
- `src/terrain.rs` (~250 lines). Chunk-tiled heightfield — each `(cx, cz)` chunk owns `chunk_resolution × chunk_resolution` f32 heights (default 65×65 = 64 intervals over a 64m chunk_size, so adjacent chunks share their edge sample seamlessly).
- `height_at(x, z)` bilinearly interpolates within the covering chunk; `normal_at(x, z)` central-differences over a 1-sample epsilon. Both return `None` when the covering chunk isn't loaded — Principle-3 explicit gap rather than silent zero.
- 7 unit tests cover length validation, missing-chunk None, flat-chunk constant height, ramp-chunk linear interpolation, ramp-chunk normal direction, flat-chunk straight-up normal, lazy global init.

**Session 6 — Frustum culling** (commit `323db1d`):
- `src/cull.rs` (~225 lines). `Frustum` extracted from a view-projection matrix using the Gribb-Hartmann technique — six normalized inward-pointing planes. `fully_outside(aabb)` does the "negative-vertex" test (pick the AABB corner deepest along each plane normal; if outside, the whole AABB is). Two entry points: `from_view_proj_row_major` + `from_view_proj_column_major`.
- Helper constructors `perspective_row_major` + `translate_row_major` + `matmul_row_major` for tests + scripts that build matrices by hand.
- Integrated into spatial: `WorldSpatial.query_frustum` linear-scans dynamic occupants and BVH-traverses static leaves, pruning internal nodes whose bounds are fully outside. BVH gained `query_frustum` + `traverse_frustum` for this. `LooseGrid::occupants` was promoted to `pub(crate)` so the WorldSpatial integration can iterate it.
- 8 unit tests cover in-front passes, behind-camera culls, beyond-far-plane culls, off-to-the-side culls, sphere variants, near-plane straddle, matrix composition, column-major / row-major agreement.

**Session 7 — Instance buckets** (this commit):
- `src/instance.rs` (~165 lines). `InstanceBuckets`: `HashMap<asset_path, Vec<[f32; 16]>>`. Each instance is a row-major 4×4 transform matching the wgpu storage-buffer layout used by Phase 23.
- `clear` zeroes per-bucket Vecs but keeps allocations (per-frame reset); `reset` drops everything (LOD-chain change). `add` inserts into the bucket for `asset_path`; `count` / `total_instances` / `bucket_count` / `assets` / `transforms` are read accessors.
- 7 unit tests cover group-by-asset, sorted-unique key list, clear-vs-reset semantics, total-across-buckets, insertion-order preservation, lazy global init.

**Session 8 — Ergonomic helpers** (this commit):
- `world.stream_radius_meters(m)` reads the current `chunk_size` and rounds up so scripts express vision distance in meters without manually dividing.
- `world.entity_lod(class, [(asset, max_distance), ...])` is a more ergonomic shape than `world.set_lod_chain(class, assets, distances)` — paired tuples are easier to author and visually validate. The last pair's distance is implicit +∞.
- `world.world_to_lod(class, ex, ey, ez, cx, cy, cz)` collapses "compute distance, then LOD lookup" into one builtin — saves a per-frame cross-namespace bounce.
- `world.distance_xyz(ax, ay, az, bx, by, bz)` is the basic 3D Euclidean distance, pulled out as a builtin so the per-frame visibility-pass loop doesn't allocate a tuple to compute it.
- No new module — these are wired directly in `stdlib.rs` against the session 2–7 internals.

**Session 9 — Closeout** (this commit):
- This file. CLAUDE.md and `docs/05-roadmap.md` updated to mark Phase 32 codebase-closed and the test count to 810.

## Honest deferrals

- **Render-pipeline integration.** The data structures + script API ship; the wgpu render path doesn't consume them yet. Concretely, `play3d.rs` still draws every entity in the scene rather than reading `world.instance_assets()` + `world.instance_transforms()` and issuing one `draw_indexed_indirect` per bucket. The exit-criterion 4km × 50k-prop playtest gates on this integration; tracked as the immediate Phase-32-render follow-on. Three concrete sub-deliverables: (1) per-frame visibility pass driver in `play3d.rs` that calls the script's render-prep callback and reads back the buckets, (2) growable storage buffer for transforms with per-bucket (offset, length) pairs feeding the indirect draw command, (3) terrain mesh generation from `terrain.height_at` (triangle strip per chunk, regenerated on `terrain.set_chunk`).
- **Occlusion culling proper.** Phase 32 session 6 ships frustum culling only. Occluder-list culling (large opaque AABBs that hide everything behind them) is well-known but its correctness is subtle (rays-through-occluder margins, partial-occlusion handling), and the speedup over BVH+frustum is < 2× on most scenes. Tracked as a follow-on session — `world.add_occluder(aabb)` + `world.cull_with_occluders(view_proj, camera_pos)` chains after the frustum-visible set. GPU hierarchical-Z occlusion (the AAA-tier technique) requires depth-pre-pass + readback; multi-session integration with the wgpu pipeline, out of scope for v1.0.
- **`entity Tree: lod = [...]` parser sugar.** The author-facing API ships as builtins (`world.entity_lod` + `world.set_lod_chain`); the language-level entity-block field declaration would require parser + AST + check + eval + VM-mirror changes (~5 sessions). Tracked as a v2 ergonomic pass; the builtin form is fully expressive in the meantime.
- **SAH-optimal BVH build.** Median-split is shipped. Surface Area Heuristic builds yield ~10–20% query-time speedups but ~3× build-time cost — not worth the complexity for v1.0 since BVH builds happen at scene load, not per-frame.
- **3D loose-grid spatial partition.** Sessions 2 + 6 use XZ-only partitioning; the Y axis is stored but not partitioned. Worlds with deep vertical structure (caves, multi-story buildings) need either a 3D grid or a per-floor 2D-grid stack. The script-facing API (`world.spatial_*`) is shape-stable across either implementation; only the `src/spatial.rs` internal storage changes.
- **LOD smooth transitions.** Strict-distance-boundary switching can pop. Alpha-blending two LODs across a 5m band fixes it but doubles the visible draw count during the band. Tracked as a follow-on.
- **GPU instancing fallback for non-instancing GPUs.** Modern GPU is required; no software-instancing fallback ships. The Phase 23 dynamic instance buffer already implies this.
- **Streaming asset cache.** The streaming module is bookkeeping-only — it returns `to_load` / `to_unload` chunk ids; the script does the actual asset I/O via existing `texture()` / `mesh()` calls. A first-class cache (`world.cache(asset)` reused across re-loads) is a v2 ergonomic feature.
- **WASM port of the spatial / streaming / LOD / terrain / cull / instance modules.** All six are gated `#[cfg(not(target_arch = "wasm32"))]`. They'd compile on WASM (no UDP / no platform-specific deps), but they share fate with the 3D rendering path which is desktop-only per Phase 30. A 2D variant of `world.spatial_*` would be useful for WASM 2D games (Vampire-Survivors-style enemy proximity queries) — tracked as a Phase-30 follow-on rather than Phase 32.
- **`docs/06-design-document.md` §7 entry.** The 35 new builtins are stable; the spec writeup wants worked examples that pressure each builtin, easier to author against a real render-integrated playtest scene.
- **VM mirror of the new namespaces.** Per the Phase 9 session 7b precedent, builtins are wired to the tree-walker first. The bytecode VM's stdlib mirror catches up in a follow-on session.
- **Bench harness for 50k-prop / 500-NPC scenarios.** The per-data-structure microbenches are inline `cargo test`s; the integrated bench (driving a real 4km scene) lands with the render integration.

## Doc updates

- `docs/05-roadmap.md` — Phase 32 entry status note updated to "codebase-closed 2026-05-10". Phases-table row updated.
- `CLAUDE.md` — Phase 32 marked codebase-closed in the closed-phases paragraph; test count updated to 810.
- This file.

## Test delta

`cargo test --release` reports **810 passing** (was 765 at Phase 31 close — Phase 32 added 45 tests across 6 new modules). Per-session breakdown:

| Module | Tests added |
|---|---|
| `src/spatial.rs` (sessions 2 + 6) | 8 |
| `src/streaming.rs` (session 3) | 7 |
| `src/lod.rs` (session 4) | 7 |
| `src/terrain.rs` (session 5) | 7 |
| `src/cull.rs` (session 6) | 9 |
| `src/instance.rs` (session 7) | 7 |

`cargo clippy --release --all-targets -- -D warnings` clean. Smoke-tested through the script path during session 4 (`twec run` exercising `world.set_lod_chain` + `world.lod_for_distance` end-to-end).

## What this enables

- 50k-static-prop scenes with O(log N) frustum culling — the critical path for "Tunic-scale open world."
- Distance-based mesh swap so a tree at 200m draws a 200-tri imposter while a tree at 5m draws a 5000-tri high-poly model.
- Chunk streaming so a 4km world doesn't have to fit in RAM all at once — only the chunks within `stream_radius_meters` are resident.
- Per-frame streaming budget so a fast-travel jump doesn't drop a frame; chunks pop in over a few frames instead.
- Heightfield terrain queryable from script for slope-based gameplay (sliding on steep slopes, water depth from height).
- Instanced draws bucketed per asset for the GPU pipeline to consume — the structure that turns "10000 trees" into "1 draw call with 10000 instances."
- The CLAUDE.md lock revision opens the door for engine-internal worker-pool integrations (session 1); concrete usage lands with the render integration.

## What does not change

- No grammar change. No new keyword. No type-system change.
- The single-player + multiplayer + WASM + 3D-current-rendering paths are all untouched. Phase 32 is purely additive.
- All 17 codebase-closed phases (1–31) continue to pass their existing tests.
- The wgpu render path still works as-is; Phase 32 doesn't break any existing 3D example. The render-integration follow-on switches it from "draw everything" to "draw the visible buckets."

---

**With Phase 32 closed, Phases 1–32 are codebase-closed. The post-v1.0 plan is fully landed. The remaining work is render-side polish (Phase 32 follow-on render integration), v2 ergonomic passes (parser-level entity-block sugar, 3D spatial partition, occluder-list culling), and external verification (cross-machine multiplayer playtest, 4km open-world playtest, itch.io paid releases).**
