// The `twec` CLI tooling (build, cli, play, play3d) is native-only, so
// the binary's real entry point is gated to non-wasm targets.
//
// web3d-M6: the browser runtime is the separate `twe-web` crate, not
// the `twec` binary — `twec build --target web` ships that prebuilt
// runtime with a game's scripts and assets. The `twec` binary no longer
// runs in the browser; its wasm entry point is an empty `main` so the
// crate still type-checks for `wasm32-unknown-unknown` (a CI gate).

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    twec::cli::run();
}

#[cfg(target_arch = "wasm32")]
fn main() {}
