// Phase 35 session 1: API stability snapshot tooling. Builds a
// canonical, hashable JSON document of every public-API surface
// (stdlib manifest + keywords + tool versions) and supports
// diffing two snapshots. Pure Rust, no platform deps.
pub mod api_snapshot;
pub mod ast;
pub mod ast_json;
pub mod bytecode;
pub mod compiler;
pub mod eval;
// Phase 33 session 1: portable grammar export. Pure Rust, no
// platform deps — compiles on every target.
pub mod grammar;
pub mod heap;
pub mod infer;
pub mod json;
pub mod lexer;
// Phase 33 session 4: end-to-end LLM authoring loop. Pure Rust, no
// network deps in the binary — providers shell out to user-configured
// commands or live in third-party crates that depend on `twec`.
pub mod llm_loop;
// Phase 33 session 5: stdio MCP server. Pure Rust, reuses existing
// json + parser + verify entry points. No new deps.
pub mod mcp;
// Phase 33 session 6: examples-as-corpus header parser. Pure file IO.
pub mod corpus;
// Phase 33 session 7: replay-based LLM evaluation harness. Reuses
// `eval::run_with_frames` for deterministic execution; one suite =
// (prompt.md, expected.txt, config.toml) on disk.
pub mod llm_eval;
// Phase 33 session 8: error → fix corpus generator. Auto-mutates
// `tests/programs/*.twe` and captures the resulting (broken,
// verify_json, fix) triples for fine-tune training data.
pub mod mutator;
pub mod lsp;
pub mod module;
pub mod parser;
pub mod play;
pub mod printer;
pub mod profile;
pub mod replay;
pub mod save;
pub mod stdlib;
pub mod steam;
pub mod tagged_value;
pub mod types;
pub mod value;
pub mod verify;
pub mod visual_check;
pub mod visual_wgsl;
pub mod vm;
pub mod window_focus;

// bundle.rs compiles on all targets; its zstd / std::fs paths are
// gated internally with #[cfg(not(target_arch = "wasm32"))].
pub mod bundle;

// Phase 31: lockstep multiplayer over UDP. Native-only — wasm32 has
// no UDP socket access, browser multiplayer would route via
// WebRTC/WebSocket in a separate follow-on.
#[cfg(not(target_arch = "wasm32"))]
pub mod net;

// Phase 32: open-world 3D foundation. Spatial partitioning + chunked
// streaming + (later sessions) LOD + occlusion culling. Native-only —
// the structures themselves would compile on WASM but they share fate
// with the 3D rendering path which is desktop-only.
#[cfg(not(target_arch = "wasm32"))]
pub mod spatial;
#[cfg(not(target_arch = "wasm32"))]
pub mod streaming;
#[cfg(not(target_arch = "wasm32"))]
pub mod lod;
#[cfg(not(target_arch = "wasm32"))]
pub mod terrain;
#[cfg(not(target_arch = "wasm32"))]
pub mod cull;
#[cfg(not(target_arch = "wasm32"))]
pub mod instance;

// Phase 30 session 1: modules that depend on native-only crates
// (wgpu, winit, rapier3d, gltf, gilrs, arboard, zstd, image) are
// excluded from wasm32 builds. The 2D macroquad backend (play.rs)
// and bundle.rs compile on all targets; the 3D/visual backends and
// the build/CLI tooling are desktop-only for Phase 30.
#[cfg(not(target_arch = "wasm32"))]
pub mod build;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
#[cfg(not(target_arch = "wasm32"))]
pub mod physics3d;
#[cfg(not(target_arch = "wasm32"))]
pub mod play3d;
#[cfg(not(target_arch = "wasm32"))]
pub mod play_visual;
