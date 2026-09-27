//! web3d-M0: plain data shared between the script-side 3D state in
//! `stdlib` (lights, mesh animation) and the wgpu renderer in `play3d`.
//!
//! Split out of `play3d.rs` so `stdlib` compiles on wasm32, where the
//! native renderer is configured out. The GPU layout derives
//! (`bytemuck::Pod` / `Zeroable`) apply only where `play3d` exists.
//! These types move into the `twe-kernel` crate in web3d-M2.

/// Phase 20: per-frame lighting uniform. Up to 8 point lights +
/// one directional sun + a global ambient. Padded to vec4-aligned
/// fields per std140 / wgsl uniform layout rules. Disabled lights
/// have `radius = 0.0` so the shader can early-out cheaply.
#[repr(C)]
#[derive(Copy, Clone)]
#[cfg_attr(not(target_arch = "wasm32"), derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct PointLightU {
    /// xyz = world-space position. w padding.
    pub pos: [f32; 4],
    /// xyz = light color (linear-ish, treat as gain). w = radius.
    /// `radius == 0.0` means "slot disabled".
    pub color_radius: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone)]
#[cfg_attr(not(target_arch = "wasm32"), derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct LightsUniform {
    /// xyz = ambient color (RGB), w padding.
    pub ambient: [f32; 4],
    /// xyz = normalized direction TOWARD the sun (i.e. light comes
    /// from this direction). w = sun intensity (0..1+).
    pub sun_dir: [f32; 4],
    pub point_lights: [PointLightU; 8],
}

impl LightsUniform {
    pub fn new() -> Self {
        Self {
            ambient: [0.20, 0.20, 0.22, 0.0],
            sun_dir: [0.4, 0.85, 0.35, 1.0],
            point_lights: [PointLightU {
                pos: [0.0; 4],
                color_radius: [0.0; 4],
            }; 8],
        }
    }
}

impl Default for LightsUniform {
    fn default() -> Self {
        Self::new()
    }
}

/// Phase 24: minimal animation snapshot read from the script-side
/// `mesh_anim` state. The render path passes this to
/// `compute_skinned_joint_matrices`, which samples the named clip
/// at `time` and writes joint matrices.
#[derive(Clone, Default, Debug)]
pub struct AnimSnapshot {
    pub clip: String,
    pub time: f32,
    pub blend_clip: Option<String>,
    pub blend_t: f32,
}
