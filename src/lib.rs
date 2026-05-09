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
