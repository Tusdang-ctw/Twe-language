pub mod ast;
pub mod ast_json;
pub mod bytecode;
pub mod compiler;
pub mod eval;
pub mod heap;
pub mod infer;
pub mod json;
pub mod lexer;
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
