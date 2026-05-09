// Phase 30 session 1: WASM builds need a different entry point —
// they cannot call cli::run() because the CLI tooling modules
// (build, cli, play3d, play_visual) are excluded from wasm32 targets.
// The WASM entry launches the generic "Twe player" which fetches
// main.twe from the same origin as the HTML page and runs it.

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    twec::cli::run();
}

#[cfg(target_arch = "wasm32")]
fn main() {
    twec::play::launch_wasm();
}
