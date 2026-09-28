//! web3d-M0: plain data shared between the script-side 3D state in
//! `stdlib` (lights, mesh animation) and the wgpu renderer in `play3d`.
//!
//! Split out of `play3d.rs` in web3d-M0 so `stdlib` compiled on wasm32;
//! since web3d-M2 the kernel renderer builds on wasm32 too, so the GPU
//! layout derives are unconditional. Moves with the kernel when the
//! workspace splits.

/// Phase 20: per-frame lighting uniform. Up to 8 point lights +
/// one directional sun + a global ambient. Padded to vec4-aligned
/// fields per std140 / wgsl uniform layout rules. Disabled lights
/// have `radius = 0.0` so the shader can early-out cheaply.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointLightU {
    /// xyz = world-space position. w padding.
    pub pos: [f32; 4],
    /// xyz = light color (linear-ish, treat as gain). w = radius.
    /// `radius == 0.0` means "slot disabled".
    pub color_radius: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
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

/// One queued 3D primitive. Phase 6 session 7 added the `Primitive`
/// tag — a single render queue can now mix cubes and spheres,
/// dispatched as separate instanced draw calls by `kernel::render`.
#[derive(Debug, Clone, Copy)]
pub struct DrawCall3d {
    pub primitive: Primitive,
    pub at: [f32; 3],
    pub color: [f32; 4],
    pub size: f32,
    /// Phase 17 session 3: interned texture path id, or 0 for the
    /// white fallback (an untextured / tint-only draw). Applies to
    /// cube / sphere / mesh uniformly.
    pub texture: u32,
    /// web3d-M3: rotation about +Y in radians (0 faces +Z), from a
    /// `look:`'s `facing`. The immediate-mode draw builtins pass 0.
    pub yaw: f32,
}

/// The mesh shape behind a `DrawCall3d`. Each variant has its own
/// vertex/index buffer in `play3d`; a frame's queue is partitioned
/// per-primitive and each subset becomes one instanced draw call.
///
/// `Mesh(id)` carries an interned-path id assigned by
/// `Env::intern_mesh_path`. The render side keeps a parallel
/// `HashMap<u32, GpuMesh>` cache; first sight of an id triggers a
/// lazy `.glb` load + GPU upload. v0.2 session 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primitive {
    Cube,
    Sphere,
    Mesh(u32),
}
