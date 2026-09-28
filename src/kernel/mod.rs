//! web3d-M2: the engine kernel — rendering (and, as M2 proceeds,
//! world / assets / physics) with no knowledge of the Twe language.
//! Host shells (`play3d.rs` natively, the web shell in the browser)
//! adapt interpreter state into kernel inputs. Becomes the
//! `twe-kernel` crate when the workspace splits.

pub mod render;
