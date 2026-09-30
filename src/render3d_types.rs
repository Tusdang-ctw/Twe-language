//! web3d-M0: plain data shared between the script-side 3D state in
//! `stdlib` (lights, mesh animation) and the wgpu renderer in `play3d`.
//!
//! Split out of `play3d.rs` in web3d-M0 so `stdlib` compiled on wasm32;
//! since web3d-M2 the kernel renderer builds on wasm32 too, so the GPU
//! layout derives are unconditional. Moves with the kernel when the
//! workspace splits.

/// web3d-M7: the most point and spot lights a scene holds (clustered
/// shading draws each pixel with only the lights near it).
pub const MAX_LIGHTS: usize = 1024;

/// A point or spot light, as scripts set it (`light.*`) and the
/// renderer's light list stores it. Disabled slots have `radius = 0`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointLightU {
    /// xyz = world-space position. w = 1 if the light should cast
    /// shadows (`light.shadow`); in the renderer's copy, the shadow
    /// cube it was given + 1 (0 = none).
    pub pos: [f32; 4],
    /// xyz = light color (linear-ish, treat as gain). w = radius.
    /// `radius == 0.0` means "slot disabled".
    pub color_radius: [f32; 4],
    /// web3d-M7 spot lights (`light.cone`): xyz = the direction it
    /// shines (unit), w = cosine of the cone's half-angle; w ≤ -1 for
    /// a point light.
    pub cone: [f32; 4],
    /// x = cosine of the half-angle where the edge's fade begins.
    pub params: [f32; 4],
}

impl PointLightU {
    /// A point light (no cone).
    pub fn point(at: [f32; 3], color: [f32; 3], radius: f32) -> Self {
        PointLightU {
            pos: [at[0], at[1], at[2], 0.0],
            color_radius: [color[0], color[1], color[2], radius],
            cone: [0.0, -1.0, 0.0, -2.0],
            params: [-2.0, 0.0, 0.0, 0.0],
        }
    }
}

/// Phase 20: per-frame lighting uniform: the global ambient and the
/// directional sun. (web3d-M7: point and spot lights moved to a list,
/// `PointLightU`, clustered by the renderer.)
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LightsUniform {
    /// xyz = ambient color (RGB), w padding.
    pub ambient: [f32; 4],
    /// xyz = normalized direction TOWARD the sun (i.e. light comes
    /// from this direction). w = sun intensity (0..1+).
    pub sun_dir: [f32; 4],
}

impl LightsUniform {
    pub fn new() -> Self {
        Self {
            ambient: [0.20, 0.20, 0.22, 0.0],
            sun_dir: [0.4, 0.85, 0.35, 1.0],
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
    /// web3d-M3: material id (`Env::intern_material`) from a look's
    /// `material:`, or 0 for the plain lit surface.
    pub material: u32,
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

/// web3d-M3: a HUD element drawn over the 3D scene by `text()` /
/// `rect()` in a 3D `on render():`. Coordinates are the 2D runtime's
/// 640×480 canvas (scaled to the target); `(x, y)` of text is its
/// baseline start. Colours are sRGB `[r, g, b, a]`.
#[derive(Debug, Clone)]
pub enum HudItem {
    Text {
        text: String,
        x: f32,
        y: f32,
        size: f32,
        color: [f32; 4],
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
}
