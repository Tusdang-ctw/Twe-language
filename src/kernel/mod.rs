//! web3d-M2: the engine kernel — rendering (and, as M2 proceeds,
//! world / assets / physics) with no knowledge of the Twe language.
//! Host shells (`play3d.rs` natively, the web shell in the browser)
//! adapt interpreter state into kernel inputs. Becomes the
//! `twe-kernel` crate when the workspace splits.

pub(crate) mod ao;
pub mod canvas;
pub(crate) mod clusters;
pub(crate) mod environment;
pub(crate) mod gpu_cull;
pub(crate) mod gpu_profile;
pub mod graph;
pub(crate) mod ktx2_bc;
pub(crate) mod material;
pub mod particles;
pub(crate) mod post;
pub mod quality;
pub mod render;
pub(crate) mod ssr;
pub(crate) mod taa;
pub(crate) mod volumetric;

/// Low-end track (G5): free the texture behind a view that is being
/// replaced, now. wgpu's browser backend frees nothing when a texture or
/// buffer is dropped: the GPU memory waits for a JavaScript garbage
/// collection. A resize or a quality change replaces every screen-sized
/// target at once, and on an integrated GPU's shared memory the stale
/// ones halved Chrome's frame rate until a collection. Work already
/// submitted with the texture still completes (WebGPU and wgpu both
/// defer the free); callers bump their generation so no bind group
/// built on the old view is used again.
pub(crate) fn release_view(view: Option<&wgpu::TextureView>) {
    if let Some(v) = view {
        v.texture().destroy();
    }
}
