//! web3d-M2: the Twe renderer kernel — wgpu, no Twe.
//!
//! Extracted from `play3d.rs`. It knows nothing about the interpreter:
//! a host shell (the native winit loop in `play3d.rs`, or the browser
//! shell) gathers a [`RenderSnapshot`] each frame — camera, lights,
//! draw list, post-FX, id -> path tables — and supplies an
//! [`AssetSource`] that loads meshes and textures. The same code draws
//! through Vulkan / Metal / DX12 natively and WebGPU in the browser.
//! Moves to the `twe-kernel` crate when the workspace splits.

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::kernel::graph::{Access, Extent, FrameGraph, TextureDesc, TexturePool};
use crate::kernel::environment::EnvUniform;
use crate::kernel::gpu_cull::{CullDraw, CullGroup};
use crate::kernel::material::{
    upload_rgba, ImageData, MaterialData, MaterialKit, BASE, SLOTS,
};

pub use crate::render3d_types::{AnimSnapshot, DrawCall3d, LightsUniform, PointLightU, Primitive, MAX_LIGHTS};

/// Camera placement for a frame.
#[derive(Debug, Clone, Copy)]
pub struct Camera3d {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    /// web3d-M7: vertical field of view (radians) and clip planes.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera3d {
    /// The game camera's lens: 60° vertical, 0.1 m to 100 m.
    pub fn new(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Self {
        Camera3d {
            eye,
            target,
            up,
            fov_y: 60_f32.to_radians(),
            near: 0.1,
            far: 100.0,
        }
    }
}

/// Sun-shadow settings for a frame.
#[derive(Debug, Clone, Copy)]
pub struct ShadowSettings {
    pub enabled: bool,
    /// Half-side of the shadow frustum, in world units.
    pub extent: f32,
}

/// web3d-M7: the curve that maps HDR scene light to the display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tonemapper {
    /// A straight clamp (blown highlights clip).
    None,
    /// ACES Filmic (Hill's RRT + ODT fit), as Three.js and the glTF
    /// references apply it. The default.
    Aces,
    /// AgX (Sobotka), Blender 4's default: hue-preserving, highlights
    /// desaturate toward white instead of skewing.
    AgX,
    /// Khronos PBR Neutral: base colours below ~0.8 pass through
    /// almost unchanged (product and material viewers).
    Neutral,
}

impl Tonemapper {
    /// The id the tonemap shader switches on.
    fn id(self) -> f32 {
        match self {
            Tonemapper::None => 0.0,
            Tonemapper::Aces => 1.0,
            Tonemapper::AgX => 2.0,
            Tonemapper::Neutral => 3.0,
        }
    }
}

/// Post-processing and culling settings for a frame.
#[derive(Debug, Clone, Copy)]
pub struct PostFx {
    pub tonemapper: Tonemapper,
    /// web3d-M7: temporal anti-aliasing (on top of the main pass's MSAA).
    pub taa: bool,
    pub vignette: f32,
    pub vignette_color: [f32; 3],
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    pub frustum_cull: bool,
    /// web3d-M7: exposure compensation in stops (EV; the image is
    /// scaled by 2^exposure before the tonemap).
    pub exposure: f32,
    /// web3d-M7: adapt the exposure to the frame's brightness (the
    /// `exposure` stops are then added on top).
    pub auto_exposure: bool,
    /// web3d-M7: ambient occlusion (GTAO) strength, 0 = off; 1 is
    /// physically based.
    pub ao: f32,
    /// web3d-M7: how far (world units) occluders reach.
    pub ao_radius: f32,
    /// web3d-M7: depth of field — the distance in focus (world units)
    /// and the lens's f-number; off when either is 0.
    pub dof_focus: f32,
    pub dof_f_stop: f32,
    /// web3d-M7: camera motion blur, as the fraction of the frame the
    /// shutter is open (0.5 = a film camera's 180°); 0 = off.
    pub motion_blur: f32,
    /// web3d-M7: screen-space reflections, as a multiplier on how much
    /// of a surface's environment reflection they replace; 0 = off.
    pub ssr: f32,
}

impl Default for PostFx {
    fn default() -> Self {
        PostFx {
            tonemapper: Tonemapper::Aces,
            taa: false,
            vignette: 0.0,
            vignette_color: [0.0; 3],
            bloom_intensity: 0.0,
            bloom_threshold: 1.0,
            frustum_cull: true,
            exposure: 0.0,
            auto_exposure: false,
            ao: 0.0,
            ao_radius: 0.5,
            dof_focus: 0.0,
            dof_f_stop: 0.0,
            motion_blur: 0.0,
            ssr: 0.0,
        }
    }
}

/// Everything the renderer needs from the host for one frame.
/// web3d-M7: image-based lighting for a frame.
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentSettings<'a> {
    /// Asset path of an equirectangular HDR map (Radiance `.hdr`).
    pub path: &'a str,
    /// Multiplier on the environment's light (and backdrop).
    pub intensity: f32,
    /// Draw the environment behind the scene instead of `background`.
    pub backdrop: bool,
}

/// web3d-M7: exponential height fog for a frame. Density falls off
/// with height above y = 0: `density · e^(-falloff · y)` per world unit
/// (falloff 0 = the same density everywhere).
#[derive(Debug, Clone, Copy)]
pub struct FogSettings {
    pub density: f32,
    pub falloff: f32,
    /// sRGB, like script colours.
    pub color: [f32; 3],
    /// web3d-M7: light the fog per point in a froxel volume — shafts
    /// through the sun's shadows, halos around point lights — instead
    /// of the closed-form integral.
    pub volumetric: bool,
}

/// web3d-M7: a colour-grading look for a frame.
#[derive(Debug, Clone, Copy)]
pub struct LutSettings<'a> {
    /// Asset path of a `.cube` 3D LUT (display-referred, [0, 1] domain).
    pub path: &'a str,
    /// 0 = ungraded, 1 = fully graded.
    pub strength: f32,
}

pub struct RenderSnapshot<'a> {
    pub camera: Camera3d,
    /// web3d-M7: colour grading, applied after the tonemap curve.
    pub lut: Option<LutSettings<'a>>,
    /// web3d-M7: point and spot lights (up to `MAX_LIGHTS`; the rest are
    /// ignored), clustered for shading.
    pub point_lights: &'a [PointLightU],
    /// web3d-M7: height fog.
    pub fog: Option<FogSettings>,
    /// web3d-M7: GPU particle programs, this frame's emissions, and
    /// particles simulated on the CPU to draw.
    pub particles: crate::kernel::particles::ParticleFrame<'a>,
    /// web3d-M7: image-based lighting; `None` lights with the uniform
    /// ambient colour instead.
    pub environment: Option<EnvironmentSettings<'a>>,
    /// web3d-M7: what the scene is drawn over (linear RGB), where no
    /// geometry covers the frame.
    pub background: [f32; 3],
    pub lights: LightsUniform,
    pub shadow: ShadowSettings,
    pub post: PostFx,
    pub draws: &'a [DrawCall3d],
    /// web3d-M7 follow-up: `Some(g)` when `draws` are the same every
    /// frame `g` is sent (a world of unchanged looks). The renderer keeps
    /// the instances it built for `g`; while [`Renderer::retained_generation`]
    /// is `g`, the host may send `g` with empty `draws` and the renderer
    /// reuses them. `None`: build from `draws`, keep nothing.
    pub draws_generation: Option<u64>,
    /// `Primitive::Mesh(id)` -> asset path.
    pub mesh_paths: &'a [String],
    /// `DrawCall3d::texture` id -> asset path (id 0 = untextured).
    pub texture_paths: &'a [String],
    /// web3d-M3: simulation time in seconds (materials animate on it).
    pub time: f32,
    /// web3d-M3: WGSL for each material id (`twe_pixel` from
    /// `visual_wgsl::compile_material`); index 0 is unused.
    pub materials: &'a [String],
    /// web3d-M3: HUD text and rectangles, drawn over the scene.
    pub hud: &'a [crate::render3d_types::HudItem],
    /// Animation state for a skinned mesh id.
    pub anim: &'a dyn Fn(u32) -> AnimSnapshot,
}

/// Which kind of asset a request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Mesh,
    Texture,
    /// web3d-M7: an HDR environment map (equirect `.hdr`); delivered as
    /// raw bytes like a texture.
    Environment,
    /// web3d-M7: a `.cube` colour-grading LUT (raw bytes).
    Lut,
}

/// A finished asset load.
pub enum AssetReady {
    /// A parsed `.glb` (parse it with [`parse_glb_bytes`]).
    Mesh(u32, Result<LoadedGlb, String>),
    /// Encoded image bytes (PNG / JPEG).
    Texture(u32, Result<Vec<u8>, String>),
    /// web3d-M7: encoded HDR environment bytes.
    Environment(u32, Result<Vec<u8>, String>),
    /// web3d-M7: `.cube` LUT text.
    Lut(u32, Result<Vec<u8>, String>),
}

/// How the renderer gets asset data. Requests are fire-and-forget;
/// finished loads are collected with `poll` each frame, so a host can
/// load on worker threads (native) or with `fetch` (web).
pub trait AssetSource {
    fn request(&mut self, kind: AssetKind, id: u32, path: &str);
    fn poll(&mut self) -> Vec<AssetReady>;
}

/// Where the kernel reports non-fatal errors (a missing asset).
fn log_error(msg: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("error: {msg}");
    #[cfg(target_arch = "wasm32")]
    web_sys_log(msg);
}

#[cfg(target_arch = "wasm32")]
fn web_sys_log(msg: &str) {
    // The web shell installs a console hook; until then errors are
    // dropped rather than panicking.
    let _ = msg;
}

/// Phase 23: starting capacity for the instance buffer. The buffer
/// grows on demand (doubling on full) — the old fixed 4096 cap is
/// gone. 4096 stays as the initial size because a typical scene
/// (a few hundred draws) fits comfortably without any reallocation.
const INITIAL_INSTANCE_CAPACITY: u64 = 4096;

/// Per-vertex data — one upload at startup, never changes. The unit
/// cube's twenty-four vertices (four per face) live here; the
/// per-face normal drives Lambertian shading in the fragment
/// stage, so each face shades according to its angle to the
/// directional light.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    /// Model-space position in [-0.5, 0.5]³ (a unit cube centered
    /// at origin). The vertex shader translates and scales by the
    /// instance.
    position: [f32; 3],
    /// Per-face outward normal. Same value for all four corners
    /// of a face. Used for Lambertian diffuse shading.
    normal: [f32; 3],
    /// Texture coordinate. Phase 17 session 2: cube/sphere ship
    /// `[0.0, 0.0]` because they don't carry meaningful UVs (the
    /// fallback white texture sampled at any uv produces the same
    /// pixel). glb-loaded meshes write the `TEXCOORD_0` accessor
    /// here when present, `[0.0, 0.0]` otherwise.
    uv: [f32; 2],
    /// Phase 24 (GPU skinning): four joint indices into the
    /// per-mesh joint matrix UBO. For unskinned meshes (cube,
    /// sphere, glb without a skin), all four values are 0 and
    /// joint 0 of the bound UBO is an identity matrix — so the
    /// skin pass is a no-op.
    joints: [u16; 4],
    /// Four bone weights matching `joints`. For unskinned meshes,
    /// `weights[0] = 1.0` and the rest are 0.0, which means the
    /// vertex is fully driven by joint 0 (the identity matrix in
    /// the unskinned UBO). Skinned meshes from glTF write the
    /// `WEIGHTS_0` accessor here, normalized to sum to 1.0.
    weights: [f32; 4],
    /// web3d-M7: second UV set (glTF `TEXCOORD_1`; occlusion maps often
    /// use it).
    uv1: [f32; 2],
    /// web3d-M7: glTF `TANGENT` (xyz, w = handedness). All zero when
    /// the model has none: the shader then builds the normal-map frame
    /// from screen-space derivatives.
    tangent: [f32; 4],
    /// web3d-M7: glTF `COLOR_0` (linear RGBA), multiplied into base colour.
    color: [f32; 4],
}

impl Vertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        4 => Float32x2,
        5 => Uint16x4,
        6 => Float32x4,
        8 => Float32x2,
        9 => Float32x4,
        10 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Per-instance data — written once per frame from `env.render_queue3d`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Instance {
    position: [f32; 3],
    size: f32,
    /// sRGB (the vertex shader decodes it), alpha for translucency.
    color: [f32; 4],
    /// web3d-M3: (sin yaw, cos yaw) — rotation about +Y, applied by the
    /// vertex shaders. web3d-M7: z = the instance's GPU-cull group, w = 1
    /// for a skinned mesh (only those blend joints).
    rot: [f32; 4],
}

impl Instance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        2 => Float32x4, // packed (position.xyz, size)
        3 => Float32x4,
        7 => Float32x4, // rotation (sin yaw, cos yaw, _, _)
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Per-frame camera uniform.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    /// web3d-M3: x = simulation time (s), for materials.
    time: [f32; 4],
    /// web3d-M7: the eye position (xyz), for view-dependent shading.
    eye: [f32; 4],
    /// web3d-M7: inverse of `view_proj` (the backdrop's view rays).
    inv_view_proj: [[f32; 4]; 4],
}

/// web3d-M7: the height fog uniform (see `FogSettings`).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct FogUniform {
    /// density, falloff, base height, enabled (1/0).
    params: [f32; 4],
    /// Linear fog colour (rgb), _.
    color: [f32; 4],
    /// Direction toward the sun (xyz), forward-scattering strength.
    sun: [f32; 4],
    /// The background colour (rgb, linear) the backdrop fogs over when
    /// there is no environment backdrop; w = camera far distance.
    background: [f32; 4],
}

// web3d-M0: `PointLightU`, `LightsUniform` and `AnimSnapshot` live in
// `crate::render3d_types` (always compiled) so the script-side state in
// `stdlib` builds on wasm32 too; re-exported here for existing paths.

/// Phase 28 session 2: cascaded shadow maps. Three concentric
/// orthographic projections from the sun direction, each rendered
/// into one layer of a 2D-array depth texture. The fragment
/// shader picks a cascade per pixel by comparing the pixel's
/// view-space depth against `split_distances`.
///
/// Cascade 0 is the tightest (highest texel density, used near
/// the camera target); cascade 2 is the loosest (covers the far
/// scene). `split_distances` are camera-space forward distances
/// (positive looking away from the camera) where the cutoffs sit.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShadowUniform {
    /// One light-space view-projection matrix per cascade.
    pub light_space_matrices: [[[f32; 4]; 4]; CASCADE_COUNT],
    /// xyz = view-space z thresholds: pixels with view_z < x use
    /// cascade 0; < y use cascade 1; otherwise cascade 2. w pad.
    pub split_distances: [f32; 4],
    /// xyz unused, w = 1.0 if shadows are enabled this frame.
    pub flags: [f32; 4],
    /// web3d-M7, per cascade: (world units across the map, light-space
    /// depth range in world units, _, _), for soft-shadow sizing.
    pub cascade_params: [[f32; 4]; CASCADE_COUNT],
}

impl ShadowUniform {
    pub fn disabled() -> Self {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Self {
            light_space_matrices: [id; CASCADE_COUNT],
            split_distances: [0.0; 4],
            flags: [0.0; 4],
            cascade_params: [[0.0; 4]; CASCADE_COUNT],
        }
    }
}

impl Default for ShadowUniform {
    fn default() -> Self {
        Self::disabled()
    }
}

/// Phase 28 session 2: per-pass uniform for the shadow depth
/// pass. Holds the active cascade's matrix; rewritten between
/// passes so a single `vs_shadow` runs once per cascade.
/// web3d-M7: every depth-only pass (sun cascades, point-light faces,
/// the camera prepass) uses it, and `params.x` carries simulation time
/// for materials that displace their vertices.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShadowPassUniform {
    pub light_space_matrix: [[f32; 4]; 4],
    pub params: [f32; 4],
}

impl ShadowPassUniform {
    pub fn identity() -> Self {
        Self::new(
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            0.0,
        )
    }

    pub fn new(light_space_matrix: [[f32; 4]; 4], time: f32) -> Self {
        Self {
            light_space_matrix,
            params: [time, 0.0, 0.0, 0.0],
        }
    }
}

/// Phase 25: dimension of the shadow map. 2048 is a balanced
/// default — sharp enough for indoor / mid-range outdoor scenes,
/// not so big that 16 MB of GPU memory is allocated for it.
/// Phase 28 session 2: with `CASCADE_COUNT` layers the budget is
/// CASCADE_COUNT × 16 MB = 48 MB of depth memory at 2048².
pub const SHADOW_MAP_SIZE: u32 = 2048;

/// Phase 28 session 2: number of shadow cascades. Three is the
/// standard sweet spot for outdoor scenes — far enough to cover
/// 100m view distance with one cascade per ~5×, no waste from
/// over-cascading.
pub const CASCADE_COUNT: usize = 3;

/// Phase 24: joint matrix array bound at @group(3). Each skinned
/// mesh uploads its computed per-joint matrices (joint world ×
/// inverse-bind matrix) to a per-mesh instance of this UBO each
/// frame. Unskinned meshes bind a shared "all identity" instance
/// of this UBO so the skin pass collapses to a no-op.
///
/// 128 mat4 = 8 KB. Comfortably under the default 64 KB UBO size
/// limit. 128 covers Mixamo-class characters (typically ≤80 joints)
/// with headroom for complex rigs.
pub const MAX_JOINTS: usize = 128;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct JointsUniform {
    pub matrices: [[[f32; 4]; 4]; MAX_JOINTS],
}

impl JointsUniform {
    pub fn identity() -> Self {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Self {
            matrices: [id; MAX_JOINTS],
        }
    }
}

impl Default for JointsUniform {
    fn default() -> Self {
        Self::identity()
    }
}

pub(crate) const SHADER_SRC: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    // x = simulation time in seconds (materials animate on it).
    time: vec4<f32>,
    // web3d-M7: xyz = eye position.
    eye: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// web3d-M7: the surface's material (kernel/material.rs) at group 1:
// glTF metallic-roughness factors, alpha mode, a UV transform per
// texture, and five textures with their samplers. Built-in shapes and
// script-textured draws bind the plain material (white, dielectric).
struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,       // rgb = emissive (strength applied)
    params: vec4<f32>,         // metallic, roughness, normal scale, occlusion strength
    alpha: vec4<f32>,          // mode (0 opaque, 1 mask, 2 blend), cutoff, double-sided
    // Per texture slot: (offset.xy, rotation, uv set), (scale.xy, has texture, _).
    xf: array<vec4<f32>, 18>,
    // web3d-M7: glTF extension parameters and the extension slot of each
    // texture role (kernel/material.rs `MaterialUniform`).
    ext: array<vec4<f32>, 6>,
    roles: array<vec4<f32>, 3>,
};
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var t_base: texture_2d<f32>;
@group(1) @binding(2) var t_metal_rough: texture_2d<f32>;
@group(1) @binding(3) var t_normal: texture_2d<f32>;
@group(1) @binding(4) var t_occlusion: texture_2d<f32>;
@group(1) @binding(5) var t_emissive: texture_2d<f32>;
@group(1) @binding(6) var t_ext0: texture_2d<f32>;
@group(1) @binding(7) var t_ext1: texture_2d<f32>;
@group(1) @binding(8) var t_ext2: texture_2d<f32>;
@group(1) @binding(9) var t_ext3: texture_2d<f32>;
@group(1) @binding(10) var s_base: sampler;
@group(1) @binding(11) var s_metal_rough: sampler;
@group(1) @binding(12) var s_normal: sampler;
@group(1) @binding(13) var s_occlusion: sampler;
@group(1) @binding(14) var s_emissive: sampler;
@group(1) @binding(15) var s_ext0: sampler;
@group(1) @binding(16) var s_ext1: sampler;
@group(1) @binding(17) var s_ext2: sampler;
@group(1) @binding(18) var s_ext3: sampler;

// Phase 20: lighting uniform — global ambient and the directional sun.
struct Lights {
    ambient: vec4<f32>,
    sun_dir: vec4<f32>,        // xyz=normalized dir TOWARD light, w=intensity
};

@group(0) @binding(1) var<uniform> lights: Lights;

// web3d-M7: point and spot lights, clustered (kernel/clusters.rs): the
// light list, each cluster's count and indices, and the grid.
struct PointLight {
    pos: vec4<f32>,           // xyz, w = shadow cube + 1 (0 = none)
    color_radius: vec4<f32>,  // xyz=color, w=radius (0 = disabled)
    cone: vec4<f32>,          // spot: xyz = direction, w = cos(half-angle); w <= -1: point
    params: vec4<f32>,        // x = cos of the fade's inner angle
};
struct Clusters {
    view: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    grid: vec4<f32>,          // x, y, z, light count
    depth: vec4<f32>,         // near, far, slice scale, slice bias
    screen: vec4<f32>,
};
@group(0) @binding(11) var<storage, read> point_lights: array<PointLight>;
@group(0) @binding(12) var<storage, read> light_grid: array<u32>;
@group(0) @binding(13) var<uniform> clusters: Clusters;

// The cluster holding a pixel at view depth `z`.
fn cluster_of(frag: vec2<f32>, z: f32) -> u32 {
    let g = vec2<u32>(clusters.grid.xy);
    let tile = min(vec2<u32>(frag / (clusters.screen.xy / clusters.grid.xy)), g - vec2<u32>(1u));
    let slice = u32(clamp(floor(log(max(z, 1e-4)) * clusters.depth.z - clusters.depth.w), 0.0, clusters.grid.z - 1.0));
    return tile.x + g.x * (tile.y + g.y * slice);
}

// web3d-M7: image-based lighting (kernel/environment.rs). `sh` is the
// cosine-convolved irradiance SH9; params = (intensity, max specular
// LOD, has environment, backdrop). The DFG table is always bound.
struct Env {
    sh: array<vec4<f32>, 9>,
    params: vec4<f32>,
};
@group(0) @binding(2) var<uniform> env: Env;
@group(0) @binding(3) var t_env_specular: texture_cube<f32>;
@group(0) @binding(4) var t_env_equirect: texture_2d<f32>;
@group(0) @binding(5) var t_dfg: texture_2d<f32>;
@group(0) @binding(6) var s_env: sampler;
@group(0) @binding(7) var s_clamp: sampler;
// web3d-M7: screen-space ambient occlusion (kernel/ao.rs), one texel
// per pixel; a 1x1 white texture when AO is off.
@group(0) @binding(8) var t_ao: texture_2d<f32>;

// web3d-M7: exponential height fog (Quilez, "better fog"). Density
// a·e^(-b·(y - h0)) integrates in closed form along the view ray:
// optical depth = a·e^(-b·(o.y - h0))·(1 - e^(-b·d.y·t)) / (b·d.y).
// The light it scatters toward the eye is its colour, brightened
// looking toward the sun (forward scattering).
struct Fog {
    params: vec4<f32>,
    color: vec4<f32>,
    sun: vec4<f32>,
    background: vec4<f32>,
};
@group(0) @binding(9) var<uniform> fog: Fog;
// web3d-M7: the opaque scene behind transmissive surfaces, with mips
// (kernel/post.rs `Transmission`); black when nothing transmits.
@group(0) @binding(10) var t_transmission: texture_2d<f32>;

fn fog_amount(p: vec3<f32>) -> f32 {
    if (fog.params.w < 0.5) {
        return 0.0;
    }
    let o = camera.eye.xyz;
    let v = p - o;
    let t = length(v);
    let dy = v.y / max(t, 1e-5);
    let a = fog.params.x;
    let b = fog.params.y;
    let k = a * exp(-b * (o.y - fog.params.z));
    let bdt = b * dy * t;
    var depth = k * t;
    if (abs(bdt) > 1e-4) {
        depth = k * (1.0 - exp(-bdt)) / (b * dy);
    }
    return 1.0 - exp(-max(depth, 0.0));
}

fn fog_light(dir: vec3<f32>) -> vec3<f32> {
    let lobe = pow(max(dot(dir, fog.sun.xyz), 0.0), 8.0) * fog.sun.w;
    return fog.color.rgb * (1.0 + lobe);
}

fn apply_fog(c: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let f = fog_amount(p);
    if (f <= 0.0) {
        return c;
    }
    return mix(c, fog_light(normalize(p - camera.eye.xyz)), f);
}

// Phase 24: per-mesh joint matrix UBO. Up to 128 joints per
// skinned mesh. Unskinned meshes (cube, sphere, glb without a
// skin) bind a default UBO whose joint 0 is identity, and their
// vertices use joints=[0,0,0,0], weights=[1,0,0,0] — so the
// skin matrix collapses to identity and the vertex passes
// through unchanged. Skinned meshes upload computed joint
// matrices each frame (driven by `mesh_anim.advance` + glTF
// animation channels).
struct Joints {
    matrices: array<mat4x4<f32>, 128>,
};
@group(2) @binding(0) var<uniform> joints_u: Joints;

// Phase 28 session 2: cascaded shadow maps. `light_space_matrices`
// holds one mat4 per cascade; `split_distances` xyz are the
// view-space depth thresholds for cascade selection. `flags.w`
// carries the runtime enable bit (0 means short-circuit the
// shadow lookup, e.g. when the script disables shadows via
// `sun.shadow(false)`).
struct Shadow {
    light_space_matrices: array<mat4x4<f32>, 3>,
    split_distances: vec4<f32>,
    flags: vec4<f32>,
    // web3d-M7: per cascade (world units across the map, depth range).
    cascade_params: array<vec4<f32>, 3>,
};
@group(3) @binding(0) var<uniform> shadow_u: Shadow;
@group(3) @binding(1) var t_shadow: texture_depth_2d_array;
@group(3) @binding(2) var s_shadow: sampler_comparison;
// web3d-M7: point-light shadow cubes (layer = light slot's pos.w - 1).
@group(3) @binding(3) var t_point_shadow: texture_depth_cube_array;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) joints: vec4<u32>,   // u16x4 zero-extended
    @location(6) weights: vec4<f32>,
    @location(8) uv1: vec2<f32>,
    @location(9) tangent: vec4<f32>,
    @location(10) color: vec4<f32>,
};

struct InstanceInput {
    @location(2) inst_pos_size: vec4<f32>, // xyz = position, w = uniform scale
    @location(3) inst_color: vec4<f32>,
    @location(7) inst_rot: vec4<f32>,      // (sin yaw, cos yaw, _, _)
};

// web3d-M3: rotate about +Y by the instance's yaw (0 faces +Z).
fn yaw_rotate(v: vec3<f32>, rot: vec4<f32>) -> vec3<f32> {
    return vec3<f32>(rot.y * v.x + rot.x * v.z, v.y, -rot.x * v.x + rot.y * v.z);
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    // web3d-M7: the instance tint with its alpha (translucency).
    @location(1) base_color: vec4<f32>,
    @location(2) tex_coord: vec2<f32>,
    @location(3) world_pos: vec3<f32>,
    // Phase 28 session 2: view-space forward depth, used to pick
    // the shadow cascade. For the standard reverse-Z perspective
    // matrix in this codebase, clip.w equals view-space `-z`, i.e.
    // positive distance away from the eye.
    @location(4) view_z: f32,
    // web3d-M7: second UV set, tangent (w = handedness, 0 = none) and
    // vertex colour.
    @location(5) tex_coord1: vec2<f32>,
    @location(6) world_tangent: vec4<f32>,
    @location(7) vertex_color: vec4<f32>,
    // web3d-M7: the instance's scale (volume thickness is in mesh units).
    @location(8) model_scale: f32,
};

fn srgb_to_linear3(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    return vertex_out(vert, inst);
}

fn vertex_out(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    // Phase 24: linear blend skinning. The four joint indices select
    // four mat4 from the joint UBO, weighted by `weights`. For
    // unskinned meshes joint 0 is identity and weights = (1,0,0,0),
    // so skin_mat collapses to identity.
    // web3d-M7: only skinned meshes (rot.w = 1) blend joints.
    var skin_mat = mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
    if (inst.inst_rot.w > 0.5) {
        skin_mat = vert.weights.x * joints_u.matrices[vert.joints.x]
            + vert.weights.y * joints_u.matrices[vert.joints.y]
            + vert.weights.z * joints_u.matrices[vert.joints.z]
            + vert.weights.w * joints_u.matrices[vert.joints.w];
    }
    let skinned_pos = (skin_mat * vec4<f32>(vert.position, 1.0)).xyz;
    let skinned_normal = (skin_mat * vec4<f32>(vert.normal, 0.0)).xyz;
    let skinned_tangent = (skin_mat * vec4<f32>(vert.tangent.xyz, 0.0)).xyz;
    let model_pos = yaw_rotate(skinned_pos, inst.inst_rot) * inst.inst_pos_size.w
        + inst.inst_pos_size.xyz;
    var out: VertexOutput;
    out.tex_coord1 = vert.uv1;
    out.world_tangent = vec4<f32>(yaw_rotate(skinned_tangent, inst.inst_rot), vert.tangent.w);
    out.vertex_color = vert.color;
    out.clip_position = camera.view_proj * vec4<f32>(model_pos, 1.0);
    out.world_normal = yaw_rotate(skinned_normal, inst.inst_rot);
    // web3d-M7: instance colours arrive sRGB (as scripts write them) and
    // are decoded here, per vertex, rather than per instance on the CPU.
    out.base_color = vec4<f32>(srgb_to_linear3(inst.inst_color.rgb), inst.inst_color.a);
    out.model_scale = inst.inst_pos_size.w;
    out.tex_coord = vert.uv;
    out.world_pos = model_pos;
    // Reverse-Z: clip.w is positive view-space distance forward.
    out.view_z = out.clip_position.w;
    return out;
}

// web3d-M7: soft sun shadows (PCSS, Fernando 2005).
//
// Cascades are picked by view depth. A blocker search over a Poisson
// disk finds the average depth of whatever shadows this point; the gap
// between it and the receiver sets the penumbra (a wider gap, a softer
// edge: contact-hardening), and a rotated Poisson PCF filters over it.
// The disk is rotated per pixel (interleaved gradient noise) so the
// pattern breaks up into fine noise, which TAA averages away.
//
// SUN_TAN is the tangent of the sun's apparent radius: how quickly
// shadows soften with distance from their caster. 0.04 (≈2.3°) is a
// little softer than the real sun, which reads better at game scale.
const SUN_TAN: f32 = 0.04;

fn sample_shadow(world_pos: vec3<f32>, view_z: f32, n: vec3<f32>, frag: vec2<f32>) -> f32 {
    if (shadow_u.flags.w < 0.5) {
        return 1.0;
    }
    var cascade: i32 = 2;
    if (view_z < shadow_u.split_distances.x) {
        cascade = 0;
    } else if (view_z < shadow_u.split_distances.y) {
        cascade = 1;
    } else if (view_z >= shadow_u.split_distances.z) {
        // Beyond the last cascade: no shadow.
        return 1.0;
    }
    let params = shadow_u.cascade_params[cascade];
    let world_per_uv = params.x;
    let depth_range = params.y;
    let dim = vec2<f32>(textureDimensions(t_shadow));
    let texel_world = world_per_uv / dim.x;
    // Normal offset: step off the surface by about a texel so it can't
    // shadow itself (acne), scaled with the cascade.
    let p = world_pos + n * (texel_world * 1.5);
    let lp = shadow_u.light_space_matrices[cascade] * vec4<f32>(p, 1.0);
    let suv = vec3<f32>(lp.x / lp.w * 0.5 + 0.5, lp.y / lp.w * -0.5 + 0.5, lp.z / lp.w);
    if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0 || suv.z < 0.0 || suv.z > 1.0) {
        return 1.0;
    }
    let receiver = suv.z - 0.5 * texel_world / depth_range;

    var disk = array<vec2<f32>, 16>(
        vec2<f32>(-0.9420, -0.3991), vec2<f32>(0.9456, -0.7689),
        vec2<f32>(-0.0942, -0.9294), vec2<f32>(0.3450, 0.2939),
        vec2<f32>(-0.9159, 0.4577), vec2<f32>(-0.8154, -0.8791),
        vec2<f32>(-0.3828, 0.2768), vec2<f32>(0.9748, 0.7565),
        vec2<f32>(0.4432, -0.9751), vec2<f32>(0.5374, -0.4737),
        vec2<f32>(-0.2650, -0.4190), vec2<f32>(0.7920, 0.1909),
        vec2<f32>(-0.2419, 0.9971), vec2<f32>(-0.8141, 0.9144),
        vec2<f32>(0.1998, 0.7864), vec2<f32>(0.1438, -0.1410),
    );
    let angle = 6.2831853 * fract(52.9829189 * fract(dot(frag, vec2<f32>(0.06711056, 0.00583715))));
    let rot = mat2x2<f32>(cos(angle), sin(angle), -sin(angle), cos(angle));

    // Blocker search: casters up to ~4 m away can soften this point.
    let search = clamp(4.0 * SUN_TAN / world_per_uv, 2.0 / dim.x, 24.0 / dim.x);
    let max_texel = vec2<i32>(dim) - vec2<i32>(1);
    var blockers = 0.0;
    var blocker_depth = 0.0;
    for (var i: i32 = 0; i < 16; i = i + 1) {
        let q = suv.xy + rot * disk[i] * search;
        let t = clamp(vec2<i32>(q * dim), vec2<i32>(0), max_texel);
        let d = textureLoad(t_shadow, t, cascade, 0);
        if (d < receiver) {
            blockers = blockers + 1.0;
            blocker_depth = blocker_depth + d;
        }
    }
    if (blockers < 0.5) {
        return 1.0;
    }
    let gap = (receiver - blocker_depth / blockers) * depth_range;
    let filter_radius = clamp(gap * SUN_TAN / world_per_uv, 1.5 / dim.x, 32.0 / dim.x);

    // PCF over the penumbra. `...CompareLevel`: this runs in non-uniform
    // control flow, where WebGPU rejects implicit-derivative sampling.
    var lit = 0.0;
    for (var i: i32 = 0; i < 16; i = i + 1) {
        let q = suv.xy + rot * disk[i] * filter_radius;
        lit = lit + textureSampleCompareLevel(t_shadow, s_shadow, q, cascade, receiver);
    }
    return lit / 16.0;
}

// web3d-M7: a point light's shadow from its cube map. The stored depth
// is each face camera's perspective depth, so the receiver's is
// rebuilt from its distance along the dominant axis; five taps around
// the direction soften the edge.
fn point_shadow(layer: i32, world_pos: vec3<f32>, light_pos: vec3<f32>, far: f32, n: vec3<f32>) -> f32 {
    let near = 0.05;
    let to_point = world_pos - light_pos;
    let dist = length(to_point);
    // Normal offset grows with distance (texels widen with it).
    let d = to_point + n * (0.01 * dist + 0.01);
    let a = abs(d);
    let major = max(a.x, max(a.y, a.z));
    let receiver = far * (major - near) / (major * (far - near)) - 0.0005;
    var side = vec3<f32>(0.0, 1.0, 0.0);
    if (abs(d.y) > 0.9 * length(d)) {
        side = vec3<f32>(1.0, 0.0, 0.0);
    }
    let t1 = normalize(cross(d, side)) * (0.015 * major);
    let t2 = normalize(cross(d, t1)) * (0.015 * major);
    var lit = textureSampleCompareLevel(t_point_shadow, s_shadow, d, layer, receiver);
    lit = lit + textureSampleCompareLevel(t_point_shadow, s_shadow, d + t1, layer, receiver);
    lit = lit + textureSampleCompareLevel(t_point_shadow, s_shadow, d - t1, layer, receiver);
    lit = lit + textureSampleCompareLevel(t_point_shadow, s_shadow, d + t2, layer, receiver);
    lit = lit + textureSampleCompareLevel(t_point_shadow, s_shadow, d - t2, layer, receiver);
    return lit / 5.0;
}

// ---- web3d-M7: physically based shading -------------------------------
//
// glTF 2.0 metallic-roughness (spec Appendix B): Lambert diffuse plus a
// Cook-Torrance specular lobe with the GGX distribution, the
// height-correlated Smith visibility term and Schlick's Fresnel, and
// multiscatter energy compensation (Fdez-Agüera 2019, as in Filament)
// so rough metals don't lose energy. Light units: a light of intensity
// 1 makes a white Lambert surface facing it reflect 1 (irradiance x pi),
// which keeps the game's existing lights at their old brightness.
//
// Indirect light comes from the environment map when the frame has one
// (image-based lighting, kernel/environment.rs), else from the ambient
// colour treated as a uniform environment.

const PI: f32 = 3.14159265;

struct Surface {
    albedo: vec3<f32>,
    metallic: f32,
    roughness: f32,   // perceptual
    n: vec3<f32>,
    occlusion: f32,
    emissive: vec3<f32>,
    // web3d-M7 glTF extensions. The dielectric specular lobe's f0 and
    // f90 (KHR_materials_ior + KHR_materials_specular; 0.04 and 1 by
    // default); KHR_materials_clearcoat (strength, perceptual roughness,
    // its own normal); KHR_materials_sheen (colour, roughness);
    // KHR_materials_iridescence (strength, film IOR, film thickness nm).
    specular_f0: vec3<f32>,
    specular_f90: f32,
    clearcoat: f32,
    clearcoat_roughness: f32,
    clearcoat_n: vec3<f32>,
    sheen_color: vec3<f32>,
    sheen_roughness: f32,
    iridescence: f32,
    iridescence_ior: f32,
    iridescence_thickness: f32,
    // KHR_materials_transmission + KHR_materials_volume: how much light
    // passes through, the IOR it refracts by, the volume's thickness
    // (mesh units) and its absorption (colour reached at `distance`;
    // distance 0 = none).
    transmission: f32,
    ior: f32,
    thickness: f32,
    attenuation_color: vec3<f32>,
    attenuation_distance: f32,
};

// A surface without extension layers (the plain surface and `visual`
// materials).
fn surface_plain(albedo: vec3<f32>, metallic: f32, roughness: f32, n: vec3<f32>, occlusion: f32, emissive: vec3<f32>) -> Surface {
    return Surface(albedo, metallic, roughness, n, occlusion, emissive, vec3<f32>(0.04), 1.0, 0.0, 0.0, n, vec3<f32>(0.0), 0.0, 0.0, 1.3, 0.0, 0.0, 1.5, 0.0, vec3<f32>(1.0), 0.0);
}

fn d_ggx(noh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let f = noh * noh * (a2 - 1.0) + 1.0;
    return a2 / (PI * f * f);
}

fn v_smith_ggx_correlated(nov: f32, nol: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = nol * sqrt(nov * nov * (1.0 - a2) + a2);
    let gl = nov * sqrt(nol * nol * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

fn f_schlick(f0: vec3<f32>, voh: f32) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - voh, 5.0);
}

fn f_schlick90(f0: vec3<f32>, f90: f32, voh: f32) -> vec3<f32> {
    return f0 + (vec3<f32>(f90) - f0) * pow(1.0 - voh, 5.0);
}

// web3d-M7: the sheen lobe — the "Charlie" distribution (Estevez &
// Kulla 2017) with Neubelt & Pettineo's visibility, as the glTF sheen
// extension specifies.
fn d_charlie(noh: f32, roughness: f32) -> f32 {
    let inv_a = 1.0 / max(roughness * roughness, 1e-6);
    let sin2 = max(1.0 - noh * noh, 0.0078125);
    return (2.0 + inv_a) * pow(sin2, inv_a * 0.5) / (2.0 * PI);
}

fn v_neubelt(nov: f32, nol: f32) -> f32 {
    return clamp(1.0 / (4.0 * (nol + nov - nol * nov)), 0.0, 1.0);
}

fn max3(c: vec3<f32>) -> f32 {
    return max(c.r, max(c.g, c.b));
}

// web3d-M7: thin-film iridescence (Belcour & Barla 2017, "A Practical
// Extension to Microfacet Theory for the Modeling of Varying
// Iridescence"), as the glTF iridescence extension and the Khronos
// sample viewer evaluate it: the Fresnel of an air / film / base stack,
// with the interference integrated against the CIE sensitivity curves
// (a Gaussian fit in Fourier space).
fn iridescence_sensitivity(opd: f32, shift: vec3<f32>) -> vec3<f32> {
    let phase = 2.0 * PI * opd * 1.0e-9;
    let val = vec3<f32>(5.4856e-13, 4.4201e-13, 5.2481e-13);
    let pos = vec3<f32>(1.6810e+06, 1.7953e+06, 2.2084e+06);
    let variance = vec3<f32>(4.3278e+09, 9.3046e+09, 6.6121e+09);
    var xyz = val * sqrt(2.0 * PI * variance) * cos(pos * phase + shift) * exp(-phase * phase * variance);
    xyz.x = xyz.x + 9.7470e-14 * sqrt(2.0 * PI * 4.5282e+09) * cos(2.2399e+06 * phase + shift.x) * exp(-4.5282e+09 * phase * phase);
    xyz = xyz / 1.0685e-7;
    let xyz_to_rec709 = mat3x3<f32>(
        vec3<f32>(3.2404542, -0.9692660, 0.0556434),
        vec3<f32>(-1.5371385, 1.8760108, -0.2040259),
        vec3<f32>(-0.4985314, 0.0415560, 1.0572252),
    );
    return xyz_to_rec709 * xyz;
}

fn ior_to_f0(transmitted: f32, incident: f32) -> f32 {
    let r = (transmitted - incident) / (transmitted + incident);
    return r * r;
}

fn eval_iridescence(outside_ior: f32, film_ior_in: f32, cos1: f32, thickness: f32, base_f0: vec3<f32>) -> vec3<f32> {
    // The film fades to the outside medium as it thins to nothing.
    let film_ior = mix(outside_ior, film_ior_in, smoothstep(0.0, 0.03, thickness));
    let sin2_sq = (outside_ior / film_ior) * (outside_ior / film_ior) * (1.0 - cos1 * cos1);
    let cos2_sq = 1.0 - sin2_sq;
    if (cos2_sq < 0.0) {
        return vec3<f32>(1.0); // total internal reflection
    }
    let cos2 = sqrt(cos2_sq);
    // First interface (outside -> film).
    let r0 = ior_to_f0(film_ior, outside_ior);
    let r12 = f_schlick(vec3<f32>(r0), cos1).x;
    let t121 = 1.0 - r12;
    var phi12 = 0.0;
    if (film_ior < outside_ior) {
        phi12 = PI;
    }
    let phi21 = PI - phi12;
    // Second interface (film -> base).
    let sqrt_f0 = sqrt(clamp(base_f0, vec3<f32>(0.0), vec3<f32>(0.9999)));
    let base_ior = (vec3<f32>(1.0) + sqrt_f0) / (vec3<f32>(1.0) - sqrt_f0);
    let r1 = ((base_ior - vec3<f32>(film_ior)) / (base_ior + vec3<f32>(film_ior))) * ((base_ior - vec3<f32>(film_ior)) / (base_ior + vec3<f32>(film_ior)));
    let r23 = f_schlick(r1, cos2);
    let phi23 = select(vec3<f32>(0.0), vec3<f32>(PI), base_ior < vec3<f32>(film_ior));
    // Phase shift and the compound terms.
    let opd = 2.0 * film_ior * thickness * cos2;
    let phi = vec3<f32>(phi21) + phi23;
    let r123 = clamp(r12 * r23, vec3<f32>(1e-5), vec3<f32>(0.9999));
    let sqrt_r123 = sqrt(r123);
    let rs = t121 * t121 * r23 / (vec3<f32>(1.0) - r123);
    var i = vec3<f32>(r12) + rs;
    var cm = rs - vec3<f32>(t121);
    for (var m: i32 = 1; m <= 2; m = m + 1) {
        cm = cm * sqrt_r123;
        i = i + cm * 2.0 * iridescence_sensitivity(f32(m) * opd, f32(m) * phi);
    }
    return max(i, vec3<f32>(0.0));
}

// Irradiance from the environment's SH9 at normal n.
fn sh_irradiance(n: vec3<f32>) -> vec3<f32> {
    let x = n.x;
    let y = n.y;
    let z = n.z;
    var e = env.sh[0].xyz * 0.282095;
    e = e + env.sh[1].xyz * (0.488603 * y) + env.sh[2].xyz * (0.488603 * z) + env.sh[3].xyz * (0.488603 * x);
    e = e + env.sh[4].xyz * (1.092548 * x * y) + env.sh[5].xyz * (1.092548 * y * z);
    e = e + env.sh[6].xyz * (0.315392 * (3.0 * z * z - 1.0));
    e = e + env.sh[7].xyz * (1.092548 * x * z) + env.sh[8].xyz * (0.546274 * (x * x - y * y));
    return max(e, vec3<f32>(0.0));
}

// Everything a light's contribution needs about the surface and the
// view, computed once per pixel.
struct Lobes {
    n: vec3<f32>,
    v: vec3<f32>,
    nov: f32,
    a: f32,
    f0: vec3<f32>,
    f90: f32,
    diffuse_color: vec3<f32>,
    energy: vec3<f32>,
    // Iridescent Fresnel (at the view angle) and its strength.
    irid: vec3<f32>,
    irid_w: f32,
    // Sheen colour, roughness, and the base layer's scale under it.
    sheen_color: vec3<f32>,
    sheen_roughness: f32,
    sheen_scale: f32,
    // Clearcoat strength, normal, n·v, alpha, and the base layer's
    // scale under it.
    cc: f32,
    cc_n: vec3<f32>,
    cc_nov: f32,
    cc_a: f32,
    cc_atten: f32,
};

// One light's reflected radiance: the base layer (Lambert + GGX, with
// iridescence), sheen over it, clearcoat over both.
fn surface_light(p: Lobes, l: vec3<f32>, radiance: vec3<f32>) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    let h = normalize(p.v + l);
    let voh = clamp(dot(p.v, h), 0.0, 1.0);
    let nol = clamp(dot(p.n, l), 0.0, 1.0);
    if (nol > 0.0) {
        let noh = clamp(dot(p.n, h), 0.0, 1.0);
        var f = f_schlick90(p.f0, p.f90, voh);
        if (p.irid_w > 0.0) {
            f = mix(f, p.irid, p.irid_w);
        }
        let spec = d_ggx(noh, p.a) * v_smith_ggx_correlated(p.nov, nol, p.a) * f * p.energy;
        // Light the specular lobe reflects doesn't reach the diffuse
        // layer (glTF's fresnel_mix; web3d-M7 follow-up).
        out = (p.diffuse_color * (vec3<f32>(1.0) - f) / PI + spec) * (nol * PI);
        if (p.sheen_scale < 1.0) {
            let sheen = p.sheen_color * d_charlie(noh, p.sheen_roughness) * v_neubelt(p.nov, nol);
            out = out * p.sheen_scale + sheen * (nol * PI);
        }
    }
    if (p.cc > 0.0) {
        out = out * p.cc_atten;
        let cc_nol = clamp(dot(p.cc_n, l), 0.0, 1.0);
        if (cc_nol > 0.0) {
            let cc_noh = clamp(dot(p.cc_n, h), 0.0, 1.0);
            let cc_f = f_schlick(vec3<f32>(0.04), voh).x * p.cc;
            out = out + vec3<f32>(d_ggx(cc_noh, p.cc_a) * v_smith_ggx_correlated(p.cc_nov, cc_nol, p.cc_a) * cc_f * (cc_nol * PI));
        }
    }
    return out * radiance;
}

// web3d-M7: this pixel's screen-space ambient occlusion (computed at
// half resolution; a 1×1 white texture when off).
fn screen_ao(frag: vec2<f32>) -> f32 {
    let dim = textureDimensions(t_ao);
    let p = min(vec2<u32>(frag * 0.5), dim - vec2<u32>(1u));
    return textureLoad(t_ao, p, 0).r;
}

// Jimenez 2016's multi-bounce fit: light bounced between occluders
// brightens occlusion on bright surfaces (and tints it on coloured
// ones). Exactly 1 when unoccluded.
fn ao_multibounce(x: f32, albedo: vec3<f32>) -> vec3<f32> {
    if (x >= 0.999) {
        return vec3<f32>(1.0);
    }
    let a = 2.0404 * albedo - 0.3324;
    let b = -4.7951 * albedo + 0.6417;
    let c = 2.7552 * albedo + 0.6903;
    return max(vec3<f32>(x), ((x * a + b) * x + c) * x);
}

// Light leaving a surface point toward the eye: ambient environment,
// the shadowed sun, up to 8 point lights, and emission. Shared by glTF
// materials, the plain surface and `visual` materials.
// web3d-M7: what screen-space reflections need of the surface a pixel
// shows, written by `shade` and output as the main pass's second target
// (`SURFACE_FORMAT`): the world normal (octahedral, 0..1), roughness,
// and the weight the environment's specular reflection has in the pixel
// (0 = nothing to reflect).
var<private> g_surface: vec4<f32>;

fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    var q = p;
    if (n.z < 0.0) {
        q = (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
    }
    return q * 0.5 + 0.5;
}

struct SurfaceOut {
    @location(0) color: vec4<f32>,
    @location(1) surface: vec4<f32>,
};

// The prefiltered environment's level for a roughness: mip i holds
// roughness (i / max)² (kernel/environment.rs).
fn env_lod(roughness: f32) -> f32 {
    return sqrt(roughness) * env.params.y;
}

fn shade(in: VertexOutput, s: Surface) -> vec3<f32> {
    let n = s.n;
    let v = normalize(camera.eye.xyz - in.world_pos);
    let nov = max(dot(n, v), 1e-4);
    let roughness = clamp(s.roughness, 0.03, 1.0);
    var p: Lobes;
    p.n = n;
    p.v = v;
    p.nov = nov;
    p.a = roughness * roughness;
    p.f0 = mix(s.specular_f0, s.albedo, s.metallic);
    p.f90 = mix(s.specular_f90, 1.0, s.metallic);
    p.diffuse_color = s.albedo * (1.0 - s.metallic);
    // Transmission takes the place of diffuse reflection (metals don't
    // transmit).
    let transmission = s.transmission * (1.0 - s.metallic);
    p.diffuse_color = p.diffuse_color * (1.0 - transmission);
    // The split sum's DFG terms (A, B): specular albedo = f0·A + f90·B.
    let ab = textureSampleLevel(t_dfg, s_clamp, vec2<f32>(nov, roughness), 0.0).xy;
    p.energy = vec3<f32>(1.0) + p.f0 * (1.0 / max(ab.x + ab.y, 1e-4) - 1.0);
    var specular_albedo = (p.f0 * ab.x + p.f90 * ab.y) * p.energy;
    // Iridescence replaces the base layer's Fresnel by the thin film's.
    p.irid = p.f0;
    p.irid_w = 0.0;
    if (s.iridescence > 0.0) {
        // Over the dielectric base and over the metal base (the
        // albedo), blended by metalness, as Three.js does; one film
        // over the blended F0 (web3d-M7 follow-up) put strong colour on
        // half-metallic glass that Cycles and Three.js keep neutral.
        let irid_dielectric = eval_iridescence(1.0, s.iridescence_ior, nov, s.iridescence_thickness, s.specular_f0);
        let irid_metal = eval_iridescence(1.0, s.iridescence_ior, nov, s.iridescence_thickness, s.albedo);
        p.irid = mix(irid_dielectric, irid_metal, s.metallic);
        p.irid_w = s.iridescence;
        // The film's Fresnel as an equivalent F0 (Schlick inverted at
        // this angle), through the same split sum as any F0 (Three.js's
        // computeMultiscatteringIridescence; web3d-M7 follow-up).
        let x5 = min(pow(1.0 - nov, 5.0), 0.9999);
        let irid_f0 = clamp((p.irid - p.f90 * x5) / (1.0 - x5), vec3<f32>(0.0), vec3<f32>(1.0));
        let fr = mix(p.f0, irid_f0, s.iridescence);
        specular_albedo = (fr * ab.x + p.f90 * ab.y) * p.energy;
    }
    // Sheen scales the layer under it by the energy it reflects (the
    // DFG table's third channel integrates the sheen lobe).
    p.sheen_color = s.sheen_color;
    p.sheen_roughness = clamp(s.sheen_roughness, 0.07, 1.0);
    p.sheen_scale = 1.0;
    var sheen_e = 0.0;
    if (max3(s.sheen_color) > 0.0) {
        sheen_e = textureSampleLevel(t_dfg, s_clamp, vec2<f32>(nov, p.sheen_roughness), 0.0).z;
        p.sheen_scale = 1.0 - max3(s.sheen_color) * sheen_e;
    }
    // Clearcoat: a dielectric (IOR 1.5) GGX layer on top; what it
    // reflects doesn't reach the layers below.
    p.cc = s.clearcoat;
    p.cc_n = s.clearcoat_n;
    p.cc_nov = max(dot(s.clearcoat_n, v), 1e-4);
    let cc_roughness = clamp(s.clearcoat_roughness, 0.03, 1.0);
    p.cc_a = cc_roughness * cc_roughness;
    p.cc_atten = 1.0 - s.clearcoat * f_schlick(vec3<f32>(0.04), p.cc_nov).x;

    // Indirect light; occlusion applies here only (glTF: it describes
    // indirect light), on the specular lobe through Lagarde's fit. The
    // material's occlusion and screen-space AO (web3d-M7) combine: the
    // product on diffuse (with multi-bounce), the minimum on specular.
    // web3d-M7 follow-up: what the specular layer reflects (single and
    // multiple scattering) doesn't reach the diffuse layer, as Three.js
    // and the glTF reference BRDF have it; before this, every surface
    // was a few percent too bright under environment light.
    let diffuse_share = 1.0 - max3(specular_albedo);
    let ssao = screen_ao(in.clip_position.xy);
    let diffuse_ao = s.occlusion * ao_multibounce(ssao, s.albedo);
    let occlusion = min(s.occlusion, ssao);
    var color: vec3<f32>;
    var irradiance: vec3<f32>;
    var cc_light = vec3<f32>(0.0);
    if (env.params.z > 0.5) {
        let r = reflect(-v, n);
        let prefiltered = textureSampleLevel(t_env_specular, s_env, r, env_lod(roughness)).rgb;
        let spec_ao = clamp(pow(nov + occlusion, exp2(-16.0 * roughness - 1.0)) - 1.0 + occlusion, 0.0, 1.0);
        irradiance = textureSampleLevel(t_env_specular, s_env, n, env.params.y).rgb * env.params.x;
        color = p.diffuse_color * diffuse_share * irradiance * diffuse_ao + prefiltered * specular_albedo * spec_ao * env.params.x;
        if (s.clearcoat > 0.0) {
            let rc = reflect(-v, s.clearcoat_n);
            let abc = textureSampleLevel(t_dfg, s_clamp, vec2<f32>(p.cc_nov, cc_roughness), 0.0).xy;
            let pc = textureSampleLevel(t_env_specular, s_env, rc, env_lod(cc_roughness)).rgb;
            cc_light = pc * (0.04 * abc.x + abc.y) * s.clearcoat * spec_ao * env.params.x;
        }
    } else {
        // No environment: the ambient colour as a uniform one.
        irradiance = lights.ambient.rgb;
        color = lights.ambient.rgb * (p.diffuse_color * diffuse_share * diffuse_ao + specular_albedo * occlusion);
        if (s.clearcoat > 0.0) {
            let abc = textureSampleLevel(t_dfg, s_clamp, vec2<f32>(p.cc_nov, cc_roughness), 0.0).xy;
            cc_light = lights.ambient.rgb * (0.04 * abc.x + abc.y) * s.clearcoat * occlusion;
        }
    }
    // The environment's specular weight, for reflections to replace.
    var reflect_w = dot(specular_albedo, vec3<f32>(0.2126, 0.7152, 0.0722)) * occlusion;
    if (env.params.z > 0.5) {
        reflect_w = reflect_w * env.params.x;
    }
    g_surface = vec4<f32>(oct_encode(n), roughness, clamp(reflect_w, 0.0, 1.0));
    // web3d-M7: what's behind, refracted through the volume (Snell,
    // exiting after `thickness`), blurred by roughness (a mip of the
    // opaque scene per roughness, narrowed as IOR nears 1), absorbed
    // over the path (Beer-Lambert), tinted by the base colour, and what
    // the specular lobe reflects doesn't pass.
    if (transmission > 0.0) {
        let refracted = refract(-v, n, 1.0 / max(s.ior, 1.0001));
        let exit = in.world_pos + refracted * (s.thickness * in.model_scale);
        let clip = camera.view_proj * vec4<f32>(exit, 1.0);
        let uv = vec2<f32>(clip.x / clip.w * 0.5 + 0.5, 0.5 - clip.y / clip.w * 0.5);
        let size = vec2<f32>(textureDimensions(t_transmission));
        let lod = log2(max(size.x, 1.0)) * roughness * clamp(s.ior * 2.0 - 2.0, 0.0, 1.0);
        let source = textureSampleLevel(t_transmission, s_clamp, uv, lod);
        var behind = source.rgb;
        // Where nothing was drawn (the source's alpha is coverage), a
        // refracted ray still reaches the environment even when the
        // camera's background isn't it (as in a path tracer).
        if (env.params.z > 0.5 && env.params.w < 0.5) {
            let sky = textureSampleLevel(t_env_specular, s_env, refracted, env_lod(roughness)).rgb * env.params.x;
            behind = mix(sky, source.rgb, source.a);
        }
        if (s.attenuation_distance > 0.0) {
            let path = length(exit - in.world_pos);
            behind = behind * pow(max(s.attenuation_color, vec3<f32>(1e-4)), vec3<f32>(path / s.attenuation_distance));
        }
        color = color + transmission * behind * s.albedo * (vec3<f32>(1.0) - specular_albedo);
    }
    // Sheen from the environment: its lobe is broad, so it takes the
    // irradiance, scaled by the lobe's integral.
    if (p.sheen_scale < 1.0) {
        color = color * p.sheen_scale + s.sheen_color * sheen_e * irradiance * diffuse_ao;
    }
    if (s.clearcoat > 0.0) {
        color = color * p.cc_atten + cc_light;
    }

    // Directional sun. w = intensity; 0 disables the sun. Shadows
    // attenuate only the sun.
    if (lights.sun_dir.w > 0.0) {
        let l = normalize(lights.sun_dir.xyz);
        let shadow = sample_shadow(in.world_pos, in.view_z, n, in.clip_position.xy);
        color = color + surface_light(p, l, vec3<f32>(lights.sun_dir.w * shadow));
    }
    // Point and spot lights: only those listed for this pixel's cluster
    // (web3d-M7). A smooth-edged falloff reaches zero at the radius
    // (game-friendly, predictable to tune); a spot light's cone fades
    // between its inner and outer angles.
    if (clusters.grid.w > 0.0) {
        let base = cluster_of(in.clip_position.xy, in.view_z) * 129u;
        let count = light_grid[base];
        for (var k: u32 = 0u; k < count; k = k + 1u) {
            let pl = point_lights[light_grid[base + 1u + k]];
            let r = pl.color_radius.w;
            let to_light = pl.pos.xyz - in.world_pos;
            let dist = length(to_light);
            if (r <= 0.0 || dist >= r) {
                continue;
            }
            let l = to_light / dist;
            var spot = 1.0;
            if (pl.cone.w > -1.5) {
                spot = smoothstep(pl.cone.w, pl.params.x, dot(-l, pl.cone.xyz));
                if (spot <= 0.0) {
                    continue;
                }
            }
            let t = 1.0 - (dist / r);
            var visible = 1.0;
            let layer = i32(pl.pos.w + 0.5) - 1;
            if (layer >= 0) {
                visible = point_shadow(layer, in.world_pos, pl.pos.xyz, r, n);
            }
            color = color + surface_light(p, l, pl.color_radius.rgb * (t * t * visible * spot));
        }
    }
    return apply_fog(color + s.emissive * p.cc_atten, in.world_pos);
}

// A material texture slot's UV: the chosen UV set through the slot's
// KHR_texture_transform (offset, rotation, scale).
fn material_uv(in: VertexOutput, slot: u32) -> vec2<f32> {
    let a = material.xf[2u * slot];
    let b = material.xf[2u * slot + 1u];
    var uv = in.tex_coord;
    if (a.w > 0.5) {
        uv = in.tex_coord1;
    }
    let c = cos(a.z);
    let s = sin(a.z);
    return vec2<f32>(
        b.x * c * uv.x + b.y * s * uv.y + a.x,
        -b.x * s * uv.x + b.y * c * uv.y + a.y,
    );
}

// web3d-M7: an extension texture by role (kernel/material.rs ROLE_*):
// the texel of the extension slot the material assigned the role, or
// white when the role has no texture.
fn ext_texel(ext: array<vec4<f32>, 4>, role: u32) -> vec4<f32> {
    var texels = ext;
    let slot = i32(material.roles[role / 4u][role % 4u]);
    if (slot < 0) {
        return vec4<f32>(1.0);
    }
    return texels[slot];
}

@fragment
fn fs_main(in: VertexOutput, @builtin(front_facing) front: bool) -> SurfaceOut {
    // Every texture read and derivative first, in uniform control flow
    // (WebGPU requires it for implicit-derivative sampling).
    let normal_uv = material_uv(in, 2u);
    let base_t = textureSample(t_base, s_base, material_uv(in, 0u));
    let mr_t = textureSample(t_metal_rough, s_metal_rough, material_uv(in, 1u));
    let normal_t = textureSample(t_normal, s_normal, normal_uv);
    let occlusion_t = textureSample(t_occlusion, s_occlusion, material_uv(in, 3u));
    let emissive_t = textureSample(t_emissive, s_emissive, material_uv(in, 4u));
    let ext = array<vec4<f32>, 4>(
        textureSample(t_ext0, s_ext0, material_uv(in, 5u)),
        textureSample(t_ext1, s_ext1, material_uv(in, 6u)),
        textureSample(t_ext2, s_ext2, material_uv(in, 7u)),
        textureSample(t_ext3, s_ext3, material_uv(in, 8u)),
    );
    let dp1 = dpdx(in.world_pos);
    let dp2 = dpdy(in.world_pos);
    let duv1 = dpdx(normal_uv);
    let duv2 = dpdy(normal_uv);

    let base = material.base_color * base_t * in.vertex_color * in.base_color;
    if (material.alpha.x > 0.5 && material.alpha.x < 1.5 && base.a < material.alpha.y) {
        discard;
    }

    // The geometric normal (flipped for back faces of double-sided
    // surfaces) and a tangent frame from the vertex tangents or, when
    // the model has none, screen-space derivatives (Schüler 2013, as
    // Three.js does). Normal maps (the base layer's and clearcoat's)
    // perturb it within that frame.
    var ng = normalize(in.world_normal);
    var t: vec3<f32>;
    var b: vec3<f32>;
    if (abs(in.world_tangent.w) > 0.5) {
        t = normalize(in.world_tangent.xyz - ng * dot(ng, in.world_tangent.xyz));
        b = cross(ng, t) * in.world_tangent.w;
    } else {
        let dp2perp = cross(dp2, ng);
        let dp1perp = cross(ng, dp1);
        t = dp2perp * duv1.x + dp1perp * duv2.x;
        b = dp2perp * duv1.y + dp1perp * duv2.y;
        let scale = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
        // web3d-M7 follow-up: WGSL's `dpdy` runs down the screen (GLSL's
        // `dFdy` runs up), which turns the whole derived frame over;
        // glTF's v axis running down the texture turns the bitangent
        // back (three.js flips it for glTF, issue 11438). Net: the
        // tangent is negated. Wrong, it bent DamagedHelmet's visor
        // reflections the wrong way (ꟻLIP 0.081 → 0.059).
        t = -t * scale;
        b = b * scale;
    }
    if (!front) {
        t = -t;
        b = -b;
        ng = -ng;
    }
    var n = ng;
    if (material.xf[5].z > 0.5) {
        let tn = normal_t.xyz * 2.0 - 1.0;
        n = normalize(t * (tn.x * material.params.z) + b * (tn.y * material.params.z) + ng * tn.z);
    }

    var s = surface_plain(
        base.rgb,
        clamp(material.params.x * mr_t.b, 0.0, 1.0),
        clamp(material.params.y * mr_t.g, 0.0, 1.0),
        n,
        1.0 + material.params.w * (occlusion_t.r - 1.0),
        material.emissive.rgb * emissive_t.rgb,
    );
    // web3d-M7: glTF material extensions (kernel/material.rs).
    let ior = material.ext[0].x;
    let specular = material.ext[0].y * ext_texel(ext, 0u).a;
    let specular_color = material.ext[1].rgb * ext_texel(ext, 1u).rgb;
    let r0 = ior_to_f0(ior, 1.0);
    s.specular_f0 = min(vec3<f32>(r0) * specular_color, vec3<f32>(1.0)) * specular;
    s.specular_f90 = specular;
    s.clearcoat = clamp(material.ext[0].z * ext_texel(ext, 2u).r, 0.0, 1.0);
    s.clearcoat_roughness = clamp(material.ext[0].w * ext_texel(ext, 3u).g, 0.0, 1.0);
    s.clearcoat_n = ng;
    if (material.roles[1].x >= 0.0) {
        let cn = ext_texel(ext, 4u).xyz * 2.0 - 1.0;
        let cs = material.ext[1].w;
        s.clearcoat_n = normalize(t * (cn.x * cs) + b * (cn.y * cs) + ng * cn.z);
    }
    s.sheen_color = material.ext[2].rgb * ext_texel(ext, 5u).rgb;
    s.sheen_roughness = material.ext[2].w * ext_texel(ext, 6u).a;
    s.iridescence = material.ext[3].w * ext_texel(ext, 9u).r;
    s.iridescence_ior = material.ext[4].w;
    s.transmission = clamp(material.ext[3].x * ext_texel(ext, 7u).r, 0.0, 1.0);
    s.ior = ior;
    s.thickness = material.ext[3].y * ext_texel(ext, 8u).g;
    s.attenuation_distance = material.ext[3].z;
    s.attenuation_color = material.ext[4].rgb;
    let film = material.ext[5];
    s.iridescence_thickness = film.y;
    if (material.roles[2].z >= 0.0) {
        s.iridescence_thickness = mix(film.x, film.y, ext_texel(ext, 10u).g);
    }
    // web3d-M7: coverage for the transparent pass (the opaque passes
    // don't blend, so it's ignored there): a glTF BLEND material's
    // alpha, else the draw's tint alpha.
    var alpha = in.base_color.a;
    if (material.alpha.x > 1.5) {
        alpha = base.a;
    }
    // A transmissive surface already carries what's behind it.
    if (s.transmission > 0.0 && material.alpha.x < 1.5) {
        alpha = 1.0;
    }
    let lit = shade(in, s);
    return SurfaceOut(vec4<f32>(lit, alpha), g_surface);
}

// web3d-M7 session 16: an untextured script draw (`cube`, `sphere`, a
// look without a material): the plain material's surface — the
// instance colour, roughness 0.5, not metal — without `fs_main`'s nine
// texture reads and tangent frame, which it would spend on 1×1 white
// textures. Same result; about half the cost on an integrated GPU.
@fragment
fn fs_script(in: VertexOutput, @builtin(front_facing) front: bool) -> SurfaceOut {
    var n = normalize(in.world_normal);
    if (!front) {
        n = -n;
    }
    let s = surface_plain(in.base_color.rgb * in.vertex_color.rgb, 0.0, 0.5, n, 1.0, vec3<f32>(0.0));
    return SurfaceOut(vec4<f32>(shade(in, s), in.base_color.a), g_surface);
}
"#;

/// web3d-M7: the environment as the backdrop — a fullscreen triangle at
/// the far plane, drawn after opaque geometry where nothing covers it.
pub(crate) const SKY_SHADER_SRC: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    time: vec4<f32>,
    eye: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
};
struct Env {
    sh: array<vec4<f32>, 9>,
    params: vec4<f32>,
};
struct Fog {
    params: vec4<f32>,
    color: vec4<f32>,
    sun: vec4<f32>,
    background: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(2) var<uniform> env: Env;
@group(0) @binding(4) var t_env_equirect: texture_2d<f32>;
@group(0) @binding(6) var s_env: sampler;
@group(0) @binding(9) var<uniform> fog: Fog;

// Height fog along a ray to the far plane (the main shader's integral
// at t = far).
fn sky_fog(d: vec3<f32>) -> f32 {
    if (fog.params.w < 0.5) {
        return 0.0;
    }
    let t = fog.background.w;
    let b = fog.params.y;
    let k = fog.params.x * exp(-b * (camera.eye.y - fog.params.z));
    let bdt = b * d.y * t;
    var depth = k * t;
    if (abs(bdt) > 1e-4) {
        depth = k * (1.0 - exp(-bdt)) / (b * d.y);
    }
    return 1.0 - exp(-max(depth, 0.0));
}

struct SkyOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: SkyOut;
    out.pos = vec4<f32>(p[i], 1.0, 1.0);
    out.ndc = p[i];
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let far = camera.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let d = normalize(far.xyz / far.w - camera.eye.xyz);
    let uv = vec2<f32>(atan2(d.z, d.x) / (2.0 * 3.14159265) + 0.5, 0.5 - asin(clamp(d.y, -1.0, 1.0)) / 3.14159265);
    // The environment, or (fog without one) the background colour.
    var c = fog.background.rgb;
    if (env.params.w > 0.5) {
        c = textureSampleLevel(t_env_equirect, s_env, uv, 0.0).rgb * env.params.x;
    }
    let lobe = pow(max(dot(d, fog.sun.xyz), 0.0), 8.0) * fog.sun.w;
    return vec4<f32>(mix(c, fog.color.rgb * (1.0 + lobe), sky_fog(d)), 1.0);
}
"#;

/// web3d-M3 / M7: fragment entry for a `visual` block used as a mesh
/// material. The visual's `twe_surface(uv, time, pos, normal)` gives
/// albedo (times the instance tint), normal, roughness, metalness and
/// emission, lit like any surface; albedo alpha below 0.5 is cut out,
/// so a visual can shape the mesh (a flame on a quad).
///
/// Colours arrive as scripts write them, sRGB (as tints do), and are
/// decoded here. Emission may exceed 1 (brighter than white, for
/// bloom): its brightest channel above 1 is an intensity, and the
/// colour under it is decoded.
const MATERIAL_FS: &str = r#"
fn srgb_intensity_to_linear(c: vec3<f32>) -> vec3<f32> {
    let k = max(max3(max(c, vec3<f32>(0.0))), 1.0);
    return srgb_to_linear3(max(c, vec3<f32>(0.0)) / k) * k;
}

@fragment
fn fs_material(in: VertexOutput, @builtin(front_facing) front: bool) -> SurfaceOut {
    var n = normalize(in.world_normal);
    if (!front) {
        n = -n;
    }
    let m = twe_surface(in.tex_coord, camera.time.x, in.world_pos, n);
    if (m.albedo.a < 0.5) {
        discard;
    }
    // A zero normal (normalized to NaN) falls back to the mesh's.
    var sn = m.normal;
    if (!(dot(sn, sn) > 0.5)) {
        sn = n;
    }
    let albedo = in.base_color.rgb * srgb_to_linear3(clamp(m.albedo.rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
    let s = surface_plain(albedo, m.metalness, m.roughness, sn, 1.0, srgb_intensity_to_linear(m.emission));
    let lit = shade(in, s);
    return SurfaceOut(vec4<f32>(lit, 1.0), g_surface);
}
"#;

/// web3d-M7: a displaced vertex. The visual's `twe_displace` moves the
/// point in world space; the normal is rebuilt from the displaced
/// surface around it — displacement sampled at two points a centimetre
/// away along the surface — so lighting follows the new shape. Shared
/// by the main vertex stage and the depth passes' (which use only the
/// position).
const DISPLACE_WGSL: &str = r#"
struct TweDisplaced {
    pos: vec3<f32>,
    normal: vec3<f32>,
};

fn twe_displaced(p: vec3<f32>, n: vec3<f32>, uv: vec2<f32>, time: f32) -> TweDisplaced {
    var a = cross(n, vec3<f32>(0.0, 1.0, 0.0));
    if (dot(a, a) < 1e-4) {
        a = cross(n, vec3<f32>(1.0, 0.0, 0.0));
    }
    a = normalize(a);
    // (a, b, n) is right-handed, so (pa - p0) × (pb - p0) ≈ n.
    let b = cross(n, a);
    let e = 0.01;
    let p0 = p + twe_displace(uv, time, p, n);
    let pa = p + a * e + twe_displace(uv, time, p + a * e, n);
    let pb = p + b * e + twe_displace(uv, time, p + b * e, n);
    var m = cross(pa - p0, pb - p0);
    if (!(dot(m, m) > 1e-14)) {
        m = n;
    }
    return TweDisplaced(p0, normalize(m));
}
"#;

/// web3d-M7: the main passes' vertex entry for a displacing material.
const MATERIAL_VS: &str = r#"
@vertex
fn vs_material(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    var out = vertex_out(vert, inst);
    let d = twe_displaced(out.world_pos, normalize(out.world_normal), out.tex_coord, camera.time.x);
    out.world_pos = d.pos;
    out.world_normal = d.normal;
    out.clip_position = camera.view_proj * vec4<f32>(d.pos, 1.0);
    out.view_z = out.clip_position.w;
    return out;
}
"#;

/// web3d-M7: the depth passes' vertex entry for a displacing material.
const MATERIAL_DEPTH_VS: &str = r#"
@vertex
fn vs_shadow_displaced(vert: VertexInput, inst: InstanceInput) -> @builtin(position) vec4<f32> {
    let w = world_vertex(vert, inst);
    let p = w.pos + twe_displace(vert.uv, shadow_u.params.x, w.pos, normalize(w.normal));
    return shadow_u.light_space_matrix * vec4<f32>(p, 1.0);
}
"#;

/// web3d-M7: whether a material's WGSL (`visual_wgsl::compile_material`)
/// displaces its vertices — it defines `twe_displace` exactly then.
pub fn material_displaces(material: &str) -> bool {
    material.contains("fn twe_displace(")
}

/// The full shader for a material: the main shader, the visual's
/// `twe_surface` (from `visual_wgsl::compile_material`), and the
/// material fragment entry, plus the displacing vertex entry
/// `vs_material` when the material displaces. Public so tests can
/// validate it.
pub fn material_shader_source(material: &str) -> String {
    if material_displaces(material) {
        format!("{SHADER_SRC}\n{material}\n{MATERIAL_FS}\n{DISPLACE_WGSL}\n{MATERIAL_VS}")
    } else {
        format!("{SHADER_SRC}\n{material}\n{MATERIAL_FS}")
    }
}

/// web3d-M7: a displacing material's depth-only shader (entry
/// `vs_shadow_displaced`), for the sun cascades, point-light faces and
/// the camera prepass. Public so tests can validate it.
pub fn material_depth_source(material: &str) -> String {
    format!("{SHADOW_SHADER_SRC}\n{material}\n{MATERIAL_DEPTH_VS}")
}

/// Phase 26 / web3d-M7: the display pass. Reads the HDR frame and
/// writes the display (sRGB) target:
///
/// 1. **exposure**: the manual stops, plus the adapted exposure when
///    auto exposure is on (kernel/post.rs);
/// 2. **bloom**: the multi-level chain (kernel/post.rs), added before
///    the curve so glare shoulders like the highlights it comes from;
/// 3. **the curve**: none, ACES, AgX or Khronos PBR Neutral;
/// 4. **vignette**, toward a tint colour (black by default).
pub(crate) const TONEMAP_SHADER_SRC: &str = r#"
struct Params {
    /// x = curve (0 none, 1 ACES, 2 AgX, 3 PBR Neutral),
    /// y = bloom scale (intensity / levels; 0 = off),
    /// z = exposure multiplier (2^stops), w = vignette strength.
    flags: vec4<f32>,
    /// xyz = vignette tint; w = auto exposure on (1/0).
    vignette_color: vec4<f32>,
    /// x = grading strength (0 = off), y = LUT size.
    lut: vec4<f32>,
};

@group(0) @binding(0) var t_hdr: texture_2d<f32>;
@group(0) @binding(1) var s_hdr: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var t_bloom: texture_2d<f32>;
// x = adapted log2 exposure (kernel/post.rs).
@group(0) @binding(4) var<storage, read> exposure_state: vec4<f32>;
// web3d-M7: colour grading, a 3D LUT in display (sRGB-encoded) space.
@group(0) @binding(5) var t_lut: texture_3d<f32>;

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn from_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

struct VOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle — covers [-1, -1] to [3, 3] in clip space,
// the visible screen window is the [-1, 1] subset of it. Avoids
// the diagonal seam of a quad.
@vertex
fn vs_fullscreen(@builtin(vertex_index) idx: u32) -> VOut {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var uv = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );
    var out: VOut;
    out.clip_pos = vec4<f32>(pos[idx], 0.0, 1.0);
    out.uv = uv[idx];
    return out;
}

// ACES Filmic as Three.js, the glTF Sample Viewer and the Khronos
// Render Fidelity references apply it — Stephen Hill's fit of the RRT +
// ODT, in AP1 with the sRGB <-> AP1 matrices, after a 1/0.6 exposure.
fn rrt_odt_fit(v: vec3<f32>) -> vec3<f32> {
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return a / b;
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    // sRGB => XYZ => D65_2_D60 => AP1 => RRT_SAT (columns)
    let aces_in = mat3x3<f32>(
        vec3<f32>(0.59719, 0.07600, 0.02840),
        vec3<f32>(0.35458, 0.90834, 0.13383),
        vec3<f32>(0.04823, 0.01566, 0.83777),
    );
    // ODT_SAT => XYZ => D60_2_D65 => sRGB (columns)
    let aces_out = mat3x3<f32>(
        vec3<f32>(1.60475, -0.10208, -0.00327),
        vec3<f32>(-0.53108, 1.10813, -0.07276),
        vec3<f32>(-0.07367, -0.00605, 1.07602),
    );
    let c = aces_out * rrt_odt_fit(aces_in * (x / 0.6));
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

// AgX (Troy Sobotka), in the form Three.js r160+ and Filament ship:
// Rec.2020 working space, the inset matrix, a log2 encoding over
// [-12.47, 4.03] EV, the default-contrast sigmoid (a 6th-order fit),
// then the outset matrix and back to Rec.709. Matrices are columns.
fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx(x: vec3<f32>) -> vec3<f32> {
    let srgb_to_2020 = mat3x3<f32>(
        vec3<f32>(0.6274, 0.0691, 0.0164),
        vec3<f32>(0.3293, 0.9195, 0.0880),
        vec3<f32>(0.0433, 0.0113, 0.8956),
    );
    let rec2020_to_srgb = mat3x3<f32>(
        vec3<f32>(1.6605, -0.1246, -0.0182),
        vec3<f32>(-0.5876, 1.1329, -0.1006),
        vec3<f32>(-0.0728, -0.0083, 1.1187),
    );
    let inset = mat3x3<f32>(
        vec3<f32>(0.856627153315983, 0.137318972929847, 0.11189821299995),
        vec3<f32>(0.0951212405381588, 0.761241990602591, 0.0767994186031903),
        vec3<f32>(0.0482516061458583, 0.101439036467562, 0.811302368396859),
    );
    let outset = mat3x3<f32>(
        vec3<f32>(1.1271005818144368, -0.1413297634984383, -0.14132976349843826),
        vec3<f32>(-0.11060664309660323, 1.157823702216272, -0.11060664309660294),
        vec3<f32>(-0.016493938717834573, -0.016493938717834257, 1.2519364065950405),
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var c = inset * (srgb_to_2020 * x);
    c = max(c, vec3<f32>(1e-10));
    c = clamp((log2(c) - min_ev) / (max_ev - min_ev), vec3<f32>(0.0), vec3<f32>(1.0));
    c = agx_contrast(c);
    c = outset * c;
    c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
    c = rec2020_to_srgb * c;
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

// Khronos PBR Neutral (Khronos 3D Commerce, 2024): identity below the
// compression start, apart from a small toe offset; above it a
// highlight roll-off that desaturates toward white.
fn pbr_neutral(x: vec3<f32>) -> vec3<f32> {
    let start = 0.8 - 0.04;
    let desaturation = 0.15;
    let m = min(x.r, min(x.g, x.b));
    var offset = 0.04;
    if (m < 0.08) {
        offset = m - 6.25 * m * m;
    }
    var c = x - vec3<f32>(offset);
    let peak = max(c.r, max(c.g, c.b));
    if (peak < start) {
        return c;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    c = c * (new_peak / peak);
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(c, vec3<f32>(new_peak), g);
}

@fragment
fn fs_tonemap(in: VOut) -> @location(0) vec4<f32> {
    var hdr = textureSample(t_hdr, s_hdr, in.uv).rgb;
    let bloom = textureSample(t_bloom, s_hdr, in.uv).rgb;
    if (params.flags.y > 0.0) {
        hdr = hdr + bloom * params.flags.y;
    }
    var exposure = params.flags.z;
    if (params.vignette_color.w > 0.5) {
        exposure = exposure * exp2(exposure_state.x);
    }
    hdr = hdr * exposure;
    var col: vec3<f32>;
    let curve = params.flags.x;
    if (curve > 2.5) {
        col = clamp(pbr_neutral(hdr), vec3<f32>(0.0), vec3<f32>(1.0));
    } else if (curve > 1.5) {
        col = agx(hdr);
    } else if (curve > 0.5) {
        col = aces(hdr);
    } else {
        col = clamp(hdr, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    // web3d-M7: grade through the LUT (sampled at texel centres), in
    // the display encoding grading tools author LUTs in.
    if (params.lut.x > 0.0) {
        let n = params.lut.y;
        let coord = to_srgb(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0))) * ((n - 1.0) / n) + 0.5 / n;
        let graded = from_srgb(textureSampleLevel(t_lut, s_hdr, coord, 0.0).rgb);
        col = mix(col, graded, params.lut.x);
    }
    // Phase 28 session 4: colour-tinted vignette. Smooth radial
    // lerp from the LDR color toward `vignette_color` — strength 0
    // disables, strength 1 fully replaces the corner pixels with
    // the tint color. Black tint = classic vignette darkening.
    if (params.flags.w > 0.001) {
        let centered = in.uv - vec2<f32>(0.5, 0.5);
        let r2 = dot(centered, centered) * 4.0; // 0 at center, 1 at corner
        let t = clamp(r2 * params.flags.w, 0.0, 0.85);
        col = mix(col, params.vignette_color.rgb, t);
    }
    return vec4<f32>(col, 1.0);
}
"#;

/// Phase 26: HDR linear-light intermediate format for the main
/// pass when tone mapping is on. Rgba16Float gives ~5 stops of
/// HDR headroom while staying well within mainstream GPU support.
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// web3d-M7: samples per pixel in the main pass (MSAA). The multisampled
/// colour resolves into the single-sample HDR target the tonemap reads.
pub(crate) const MSAA_SAMPLES: u32 = 4;
/// web3d-M7: the main pass's second target, the surface record for
/// screen-space reflections (see `g_surface` in the shader).
pub(crate) const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Phase 25: separate shadow shader — depth-only, no fragment
/// stage needed, just the vertex pass that emits clip-space
/// positions in *light space* so the depth buffer captures
/// distance-to-sun. Reuses the same vertex layout (position +
/// joints + weights) so a single vertex buffer per mesh feeds
/// both passes.
///
/// Phase 28 session 2: takes a per-cascade `ShadowPass` uniform
/// (just the active cascade's matrix) instead of the full
/// `ShadowUniform`. The render loop rebinds a different bind
/// group between cascades; one shader execution per cascade.
pub(crate) const SHADOW_SHADER_SRC: &str = r#"
struct ShadowPass {
    light_space_matrix: mat4x4<f32>,
    // x = simulation time (displacing materials).
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> shadow_u: ShadowPass;

struct Joints {
    matrices: array<mat4x4<f32>, 128>,
};
@group(1) @binding(0) var<uniform> joints_u: Joints;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
};

struct InstanceInput {
    @location(2) inst_pos_size: vec4<f32>,
    @location(3) inst_color: vec4<f32>,
    @location(7) inst_rot: vec4<f32>,
};

// Same rotation as the main shader's, so shadows match the geometry.
fn yaw_rotate(v: vec3<f32>, rot: vec4<f32>) -> vec3<f32> {
    return vec3<f32>(rot.y * v.x + rot.x * v.z, v.y, -rot.x * v.x + rot.y * v.z);
}

// web3d-M7: a vertex in world space, with its normal (for displacing
// materials).
struct WorldVertex {
    pos: vec3<f32>,
    normal: vec3<f32>,
};

@vertex
fn vs_shadow(vert: VertexInput, inst: InstanceInput) -> @builtin(position) vec4<f32> {
    return shadow_u.light_space_matrix * vec4<f32>(world_vertex(vert, inst).pos, 1.0);
}

fn world_vertex(vert: VertexInput, inst: InstanceInput) -> WorldVertex {
    // web3d-M7: only skinned meshes (rot.w = 1) blend joints.
    var skin_mat = mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
    if (inst.inst_rot.w > 0.5) {
        skin_mat = vert.weights.x * joints_u.matrices[vert.joints.x]
            + vert.weights.y * joints_u.matrices[vert.joints.y]
            + vert.weights.z * joints_u.matrices[vert.joints.z]
            + vert.weights.w * joints_u.matrices[vert.joints.w];
    }
    let skinned_pos = (skin_mat * vec4<f32>(vert.position, 1.0)).xyz;
    let skinned_normal = (skin_mat * vec4<f32>(vert.normal, 0.0)).xyz;
    let model_pos = yaw_rotate(skinned_pos, inst.inst_rot) * inst.inst_pos_size.w
        + inst.inst_pos_size.xyz;
    return WorldVertex(model_pos, yaw_rotate(skinned_normal, inst.inst_rot));
}
"#;

/// web3d-M7: the depth prepass for alpha-masked glTF primitives (the
/// opaque ones reuse `vs_shadow` with the camera's matrix): cut out
/// where base-colour alpha is below the cutoff, as the main pass does,
/// so leaves and fences don't occlude like solid cards.
pub(crate) const PREPASS_MASK_SRC: &str = r#"
struct Pass {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> pass_u: Pass;

struct Joints {
    matrices: array<mat4x4<f32>, 128>,
};
@group(1) @binding(0) var<uniform> joints_u: Joints;

struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    params: vec4<f32>,
    alpha: vec4<f32>,
    xf: array<vec4<f32>, 10>,
};
@group(2) @binding(0) var<uniform> material: Material;
@group(2) @binding(1) var t_base: texture_2d<f32>;
@group(2) @binding(10) var s_base: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) joints: vec4<u32>,
    @location(6) weights: vec4<f32>,
    @location(8) uv1: vec2<f32>,
    @location(10) color: vec4<f32>,
};

struct InstanceInput {
    @location(2) inst_pos_size: vec4<f32>,
    @location(7) inst_rot: vec4<f32>,
};

struct MaskOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) uv1: vec2<f32>,
    @location(2) alpha: f32,
};

fn yaw_rotate(v: vec3<f32>, rot: vec4<f32>) -> vec3<f32> {
    return vec3<f32>(rot.y * v.x + rot.x * v.z, v.y, -rot.x * v.x + rot.y * v.z);
}

@vertex
fn vs_mask(vert: VertexInput, inst: InstanceInput) -> MaskOut {
    // web3d-M7: only skinned meshes (rot.w = 1) blend joints.
    var skin_mat = mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
    if (inst.inst_rot.w > 0.5) {
        skin_mat = vert.weights.x * joints_u.matrices[vert.joints.x]
            + vert.weights.y * joints_u.matrices[vert.joints.y]
            + vert.weights.z * joints_u.matrices[vert.joints.z]
            + vert.weights.w * joints_u.matrices[vert.joints.w];
    }
    let skinned_pos = (skin_mat * vec4<f32>(vert.position, 1.0)).xyz;
    let model_pos = yaw_rotate(skinned_pos, inst.inst_rot) * inst.inst_pos_size.w
        + inst.inst_pos_size.xyz;
    var out: MaskOut;
    out.pos = pass_u.view_proj * vec4<f32>(model_pos, 1.0);
    out.uv = vert.uv;
    out.uv1 = vert.uv1;
    out.alpha = vert.color.a;
    return out;
}

@fragment
fn fs_mask(in: MaskOut) {
    // Base-colour slot's UV set and KHR_texture_transform.
    let a = material.xf[0];
    let b = material.xf[1];
    var uv = in.uv;
    if (a.w > 0.5) {
        uv = in.uv1;
    }
    let c = cos(a.z);
    let s = sin(a.z);
    let t = vec2<f32>(b.x * c * uv.x + b.y * s * uv.y + a.x, -b.x * s * uv.x + b.y * c * uv.y + a.y);
    let alpha = material.base_color.a * textureSample(t_base, s_base, t).a * in.alpha;
    if (alpha < material.alpha.y) {
        discard;
    }
}
"#;

/// The lit-surface pipeline with fragment entry `fs_entry`: `fs_main`
/// (the plain surface) or a material's `fs_material` (web3d-M3).
fn surface_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    (vs_entry, fs_entry): (&str, &str),
    double_sided: bool,
    surface: bool,
) -> wgpu::RenderPipeline {
    let cull = if double_sided { None } else { Some(wgpu::Face::Back) };
    surface_pipeline_with(device, layout, shader, (vs_entry, fs_entry), cull, false, surface)
}

/// web3d-M7: the lit-surface pipelines of the main passes, without or
/// with the surface record target (`surface`) that reflections read.
/// Frames without reflections use the first set, so they don't pay for
/// a second multisampled target.
struct LitPipelines {
    opaque: wgpu::RenderPipeline,
    /// web3d-M7 session 16: untextured script draws (`fs_script`).
    script: wgpu::RenderPipeline,
    /// Back faces drawn too (glTF `doubleSided` materials).
    double: wgpu::RenderPipeline,
    /// The transparent pass, front faces and back faces.
    blend_front: wgpu::RenderPipeline,
    blend_back: wgpu::RenderPipeline,
}

impl LitPipelines {
    fn new(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule, surface: bool) -> Self {
        let entries = ("vs_main", "fs_main");
        let blend = |cull| surface_pipeline_with(device, layout, shader, entries, Some(cull), true, surface);
        LitPipelines {
            opaque: surface_pipeline(device, layout, shader, entries, false, surface),
            script: surface_pipeline(device, layout, shader, ("vs_main", "fs_script"), false, surface),
            double: surface_pipeline(device, layout, shader, entries, true, surface),
            blend_front: blend(wgpu::Face::Back),
            blend_back: blend(wgpu::Face::Front),
        }
    }
}

/// web3d-M7: the lit surface culling `cull`, either opaque (depth
/// written) or blended over what's drawn (alpha blending, depth tested
/// but not written) for the transparent pass.
fn surface_pipeline_with(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    (vs_entry, fs_entry): (&str, &str),
    cull: Option<wgpu::Face>,
    blend: bool,
    surface: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fs_entry),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs_entry),
            buffers: &[Some(Vertex::layout()), Some(Instance::layout())],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            // Phase 26: main pipeline always targets HDR. The
            // tonemap pass converts to the swapchain's sRGB.
            targets: &[
                Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(if blend {
                        wgpu::BlendState::ALPHA_BLENDING
                    } else {
                        wgpu::BlendState::REPLACE
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                // web3d-M7: the surface record for reflections; blended
                // surfaces leave the one beneath.
                surface.then_some(wgpu::ColorTargetState {
                    format: SURFACE_FORMAT,
                    blend: None,
                    write_mask: if blend {
                        wgpu::ColorWrites::empty()
                    } else {
                        wgpu::ColorWrites::ALL
                    },
                }),
            ],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            // web3d-M7: glTF `doubleSided` materials draw both faces.
            cull_mode: cull,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(!blend),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        // web3d-M7: the main pass renders at MSAA_SAMPLES samples.
        multisample: wgpu::MultisampleState {
            count: MSAA_SAMPLES,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

/// web3d-M4: script colours are sRGB (as authors pick them, and as
/// `color.*` names them); lighting runs in linear light. Decoding here
/// is what makes a dark tint look dark and a saturated one saturated —
/// the same convention as Three.js's colour management.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// web3d-M3: draw-group keys. A built-in shape groups by (texture,
/// material); a mesh by (mesh id, texture, material). Each group is one
/// instanced draw over an `InstanceRange` of the instance buffer.
type SurfaceKey = (u32, u32);
type MeshKey = (u32, u32, u32);
type InstanceRange = (u32, u32);

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

// ---------- Cube geometry ----------
//
// 24 vertices, 4 per face, so each face carries one outward
// normal uniformly. Indices wind CCW from outside; the pipeline's
// `Face::Back` cull drops interior triangles. The fragment shader
// computes Lambertian diffuse from these normals against a fixed
// directional light.

const N_FRONT: [f32; 3] = [0.0, 0.0, 1.0];
const N_BACK: [f32; 3] = [0.0, 0.0, -1.0];
const N_RIGHT: [f32; 3] = [1.0, 0.0, 0.0];
const N_LEFT: [f32; 3] = [-1.0, 0.0, 0.0];
const N_TOP: [f32; 3] = [0.0, 1.0, 0.0];
const N_BOTTOM: [f32; 3] = [0.0, -1.0, 0.0];

#[rustfmt::skip]
// Phase 17 session 2: per-face UVs run (0,0) at top-left to (1,1)
// at bottom-right of each face, so a 2x3 atlas of face textures
// could decorate the cube. For untextured cubes the fallback
// white texture means the value is irrelevant.
const UV_TL: [f32; 2] = [0.0, 0.0];
const UV_TR: [f32; 2] = [1.0, 0.0];
const UV_BR: [f32; 2] = [1.0, 1.0];
const UV_BL: [f32; 2] = [0.0, 1.0];

/// Phase 24: default joint indices for unskinned vertices. All
/// four reference joint 0 of the bound joint UBO. Combined with
/// `UNSKINNED_W` and the identity-joint UBO, the skin pass is a
/// no-op for static geometry.
const UNSKINNED_J: [u16; 4] = [0, 0, 0, 0];
/// Phase 24: default joint weights for unskinned vertices. Full
/// weight on joint 0 (which is the identity matrix in the
/// unskinned UBO).
const UNSKINNED_W: [f32; 4] = [1.0, 0.0, 0.0, 0.0];
/// web3d-M7: defaults for the built-in shapes' extra attributes.
const NO_UV1: [f32; 2] = [0.0, 0.0];
const NO_TANGENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

const CUBE_VERTICES: &[Vertex] = &[
    // +z (front)
    Vertex {
        position: [-0.5, -0.5, 0.5],
        normal: N_FRONT,
        uv: UV_BL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, -0.5, 0.5],
        normal: N_FRONT,
        uv: UV_BR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, 0.5, 0.5],
        normal: N_FRONT,
        uv: UV_TR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [-0.5, 0.5, 0.5],
        normal: N_FRONT,
        uv: UV_TL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    // -z (back)
    Vertex {
        position: [0.5, -0.5, -0.5],
        normal: N_BACK,
        uv: UV_BL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [-0.5, -0.5, -0.5],
        normal: N_BACK,
        uv: UV_BR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [-0.5, 0.5, -0.5],
        normal: N_BACK,
        uv: UV_TR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, 0.5, -0.5],
        normal: N_BACK,
        uv: UV_TL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    // +x (right)
    Vertex {
        position: [0.5, -0.5, 0.5],
        normal: N_RIGHT,
        uv: UV_BL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, -0.5, -0.5],
        normal: N_RIGHT,
        uv: UV_BR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, 0.5, -0.5],
        normal: N_RIGHT,
        uv: UV_TR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, 0.5, 0.5],
        normal: N_RIGHT,
        uv: UV_TL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    // -x (left)
    Vertex {
        position: [-0.5, -0.5, -0.5],
        normal: N_LEFT,
        uv: UV_BL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [-0.5, -0.5, 0.5],
        normal: N_LEFT,
        uv: UV_BR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [-0.5, 0.5, 0.5],
        normal: N_LEFT,
        uv: UV_TR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [-0.5, 0.5, -0.5],
        normal: N_LEFT,
        uv: UV_TL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    // +y (top)
    Vertex {
        position: [-0.5, 0.5, 0.5],
        normal: N_TOP,
        uv: UV_BL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, 0.5, 0.5],
        normal: N_TOP,
        uv: UV_BR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, 0.5, -0.5],
        normal: N_TOP,
        uv: UV_TR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [-0.5, 0.5, -0.5],
        normal: N_TOP,
        uv: UV_TL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    // -y (bottom)
    Vertex {
        position: [-0.5, -0.5, -0.5],
        normal: N_BOTTOM,
        uv: UV_BL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, -0.5, -0.5],
        normal: N_BOTTOM,
        uv: UV_BR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [0.5, -0.5, 0.5],
        normal: N_BOTTOM,
        uv: UV_TR,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
    Vertex {
        position: [-0.5, -0.5, 0.5],
        normal: N_BOTTOM,
        uv: UV_TL,
        joints: UNSKINNED_J,
        weights: UNSKINNED_W,
        uv1: NO_UV1,
        tangent: NO_TANGENT,
        color: WHITE,
    },
];

#[rustfmt::skip]
const CUBE_INDICES: &[u16] = &[
    0,  1,  2,    0,  2,  3,    // +z
    4,  5,  6,    4,  6,  7,    // -z
    8,  9,  10,   8,  10, 11,   // +x
    12, 13, 14,   12, 14, 15,   // -x
    16, 17, 18,   16, 18, 19,   // +y
    20, 21, 22,   20, 22, 23,   // -y
];

// ---------- Sphere geometry (Phase 6 session 7) ----------
//
// Procedural UV-sphere of radius 0.5 centred at the origin (so a
// `sphere(size: 1.0)` matches `cube(size: 1.0)` visually — both
// span [-0.5, 0.5] in their model-space bounding box). Generated
// at startup, uploaded once. CCW winding from outside so the
// existing back-face-cull pipeline drops interior triangles.
//
// Latitude × longitude segments chosen for "looks round at a few
// hundred pixels" — denser meshes are nice but cost vertex
// throughput. 16 × 24 = 384 vertices, 720 triangles. Indexed as
// u16, comfortably under the 65k cap.

const SPHERE_LAT_SEGMENTS: u32 = 16;
const SPHERE_LON_SEGMENTS: u32 = 24;

fn sphere_mesh() -> (Vec<Vertex>, Vec<u16>) {
    let lat = SPHERE_LAT_SEGMENTS;
    let lon = SPHERE_LON_SEGMENTS;
    let pi = std::f32::consts::PI;
    let mut vertices: Vec<Vertex> = Vec::with_capacity(((lat + 1) * (lon + 1)) as usize);
    for i in 0..=lat {
        let v = i as f32 / lat as f32;
        let theta = v * pi; // [0, π] from +y down to -y
        let sin_t = theta.sin();
        let cos_t = theta.cos();
        for j in 0..=lon {
            let u = j as f32 / lon as f32;
            let phi = u * 2.0 * pi; // [0, 2π] around y axis
            let sin_p = phi.sin();
            let cos_p = phi.cos();
            // Unit-sphere position; normal is the same (radial).
            // Scale to radius 0.5 so the bounding box matches the
            // unit cube's [-0.5, 0.5]³.
            let nx = sin_t * cos_p;
            let ny = cos_t;
            let nz = sin_t * sin_p;
            vertices.push(Vertex {
                position: [nx * 0.5, ny * 0.5, nz * 0.5],
                normal: [nx, ny, nz],
                // Standard UV-sphere mapping: longitude → u, latitude → v.
                uv: [u, v],
                joints: UNSKINNED_J,
                weights: UNSKINNED_W,
                uv1: NO_UV1,
                tangent: NO_TANGENT,
                color: WHITE,
            });
        }
    }
    let mut indices: Vec<u16> = Vec::with_capacity((lat * lon * 6) as usize);
    let stride = lon + 1;
    for i in 0..lat {
        for j in 0..lon {
            // Two triangles per quad, counter-clockwise seen from
            // outside: going down a row (d) is -y, one step round (b)
            // is +phi, and (b - a) x (d - a) points outward.
            // web3d-M7: the winding was clockwise, which went unseen
            // until session 3's back-face culling drew every sphere
            // inside-out (the far hemisphere's inner face).
            let a = (i * stride + j) as u16;
            let b = (i * stride + j + 1) as u16;
            let c = ((i + 1) * stride + j + 1) as u16;
            let d = ((i + 1) * stride + j) as u16;
            indices.push(a);
            indices.push(c);
            indices.push(d);
            indices.push(a);
            indices.push(b);
            indices.push(c);
        }
    }
    (vertices, indices)
}

// ---------- App / state ----------

pub struct Renderer {
    /// The window / canvas surface, or `None` for a headless renderer
    /// that draws into `offscreen` (tests, thumbnails).
    surface: Option<wgpu::Surface<'static>>,
    /// Headless colour target, created when there is no surface.
    offscreen: Option<wgpu::Texture>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// web3d-M7: the lit-surface pipelines, and (built on the first
    /// frame with reflections) the set writing the surface record.
    lit: LitPipelines,
    lit_ssr: Option<LitPipelines>,
    /// The main shader, for `lit_ssr`.
    main_shader: wgpu::ShaderModule,
    /// Whether this frame writes the surface record.
    ssr_frame: std::cell::Cell<bool>,
    /// web3d-M7: height fog.
    fog_buffer: wgpu::Buffer,
    /// web3d-M3: the main pipeline's layout, reused by material
    /// pipelines.
    pipeline_layout: wgpu::PipelineLayout,
    /// Material pipelines keyed by their WGSL, not by id: ids restart
    /// with every program (and on hot reload), the source doesn't lie.
    /// Index 1: with the surface record (see `lit_ssr`).
    material_pipelines: [HashMap<String, wgpu::RenderPipeline>; 2],
    /// web3d-M7: the depth-only passes' layout (sun cascades, point
    /// faces and the prepass share it), and the depth pipelines of each
    /// displacing material, keyed like `material_pipelines`.
    depth_layout: wgpu::PipelineLayout,
    displaced_depth: HashMap<String, DisplacedDepth>,
    /// web3d-M7 follow-up: see [`RetainedDraws`].
    retained: Option<RetainedDraws>,
    /// web3d-M3: the sRGB format frames are drawn in — the surface's
    /// own format, or an sRGB view of it (browsers often give a
    /// WebGPU canvas a non-sRGB format).
    target_format: wgpu::TextureFormat,
    hud: super::hud::Hud,
    cube_vertex_buffer: wgpu::Buffer,
    cube_index_buffer: wgpu::Buffer,
    cube_index_count: u32,
    sphere_vertex_buffer: wgpu::Buffer,
    sphere_index_buffer: wgpu::Buffer,
    sphere_index_count: u32,
    instance_buffer: wgpu::Buffer,
    /// Phase 23: current capacity in instances. The buffer grows
    /// (by doubling) when a frame's instance count exceeds this.
    instance_capacity: u64,
    camera_buffer: wgpu::Buffer,
    /// Bind group 0: camera (0), lights (1), and web3d-M7's
    /// environment (2–7). Rebuilt when the environment changes.
    frame_bind_group: wgpu::BindGroup,
    frame_bgl: wgpu::BindGroupLayout,
    /// web3d-M7: image-based lighting state.
    env: EnvState,
    /// web3d-M7: temporal anti-aliasing (history, jitter, resolve).
    taa: crate::kernel::taa::Taa,
    /// web3d-M7: the camera depth prepass (for ambient occlusion).
    prepass: Prepass,
    /// web3d-M7: ambient occlusion, bloom and auto exposure.
    ao: crate::kernel::ao::Ao,
    bloom: crate::kernel::post::Bloom,
    exposure: crate::kernel::post::AutoExposure,
    /// web3d-M7 session 8: depth of field, motion blur and grading.
    dof: crate::kernel::post::Dof,
    motion: crate::kernel::post::MotionBlur,
    lut: LutState,
    /// The AO and transmission textures the frame bind group was built
    /// with (`Ao::key`, `Transmission::key`), and a frame counter (AO
    /// pattern rotation).
    frame_ao_key: (u64, u64),
    /// web3d-M7: the opaque scene behind transmissive surfaces.
    transmission: crate::kernel::post::Transmission,
    /// web3d-M7: GPU-driven culling and indirect draws.
    gpu_cull: crate::kernel::gpu_cull::GpuCull,
    /// web3d-M7: GPU particles.
    particles: crate::kernel::particles::Particles,
    /// web3d-M7: screen-space reflections and volumetric fog.
    ssr: crate::kernel::ssr::Ssr,
    volumetric: crate::kernel::volumetric::Volumetric,
    /// web3d-M7: the clustered point / spot light list.
    clusters: crate::kernel::clusters::Clusters,
    frame_index: std::cell::Cell<u32>,
    /// web3d-M7 follow-up: per-pass GPU times (`TWE_GPU_PROFILE`).
    gpu_profile: Option<std::cell::RefCell<crate::kernel::gpu_profile::GpuProfile>>,
    /// web3d-M7 follow-up: whether the last frame culled on the GPU
    /// (diagnostics: the web shell's `frame_stats`).
    last_culled: std::cell::Cell<bool>,
    /// web3d-M7: point-light shadow cubes.
    point_shadows: PointShadows,
    /// Phase 20: lighting uniform buffer, written once per frame from
    /// the snapshot.
    lights_buffer: wgpu::Buffer,
    /// Phase 24: bind group layout for joint matrix UBOs. Reused
    /// for every skinned mesh's per-mesh joint UBO, plus the
    /// identity-default UBO bound for unskinned meshes.
    joints_bgl: wgpu::BindGroupLayout,
    /// Phase 24: shared identity-only joint UBO + bind group, bound
    /// at @group(3) for any draw call without a skin. Joint 0 is
    /// the identity matrix; combined with the unskinned vertex
    /// defaults (joints=0, weights=[1,0,0,0]) the skin matrix is
    /// identity and the vertex passes through unchanged.
    identity_joints_bind_group: wgpu::BindGroup,
    /// Phase 25: depth-only render pipeline used for the shadow
    /// pass. Reuses the same vertex + instance layouts as the
    /// main pipeline so vertex buffers are shared between passes.
    shadow_pipeline: wgpu::RenderPipeline,
    /// Per-frame shadow lookup uniform: 3 cascade matrices +
    /// split distances + enable flag. Bound at @group(4) of the
    /// main pipeline; the depth pass uses `shadow_pass_bgs`
    /// instead. Phase 28 session 2.
    shadow_buffer: wgpu::Buffer,
    /// Phase 28 session 2: per-cascade pass uniform buffers + bind
    /// groups. Each holds a single mat4 (the cascade's light-space
    /// matrix). The depth pass binds index `i` while rendering
    /// cascade `i`'s layer of the shadow array.
    shadow_pass_buffers: [wgpu::Buffer; CASCADE_COUNT],
    shadow_pass_bgs: [wgpu::BindGroup; CASCADE_COUNT],
    /// Combined bind group used by the main pipeline at @group(4):
    /// shadow lookup uniform + shadow texture array view +
    /// comparison sampler.
    shadow_combined_bg: wgpu::BindGroup,
    /// 2D-array depth texture; one layer per cascade. Phase 28
    /// session 2.
    #[allow(dead_code)]
    shadow_texture: wgpu::Texture,
    /// Per-cascade depth-attachment views (one layer each) for
    /// the three shadow passes.
    shadow_layer_views: [wgpu::TextureView; CASCADE_COUNT],
    /// Phase 26: post-FX pipeline — fullscreen triangle that
    /// reads the HDR offscreen texture, applies ACES tone
    /// mapping + optional vignette, writes to the swapchain.
    /// Only used when `postfx.tonemap(true)`. The pipeline is
    /// always created (cheap); the offscreen target is too.
    tonemap_pipeline: wgpu::RenderPipeline,
    /// Bind group layout reused when the offscreen texture
    /// resizes (the layout itself is stable; only the texture
    /// view binding changes).
    tonemap_bgl: wgpu::BindGroupLayout,
    /// Linear sampler for the HDR offscreen lookup.
    tonemap_sampler: wgpu::Sampler,
    /// Per-frame tonemap params buffer (vignette strength).
    tonemap_params_buffer: wgpu::Buffer,
    /// web3d-M7: the render graph's transient targets (HDR colour,
    /// depth, …), kept across frames.
    pool: TexturePool,
    /// Bind group over the HDR target + sampler + params + bloom +
    /// exposure, tagged with the HDR texture's pool generation and the
    /// bloom texture's key.
    tonemap_bind_group: Option<((u64, u64, u64), wgpu::BindGroup)>,
    /// Lazy-loaded `.glb` mesh GPU resources, keyed by the
    /// `Env::mesh_paths` interned id (the `u32` payload of
    /// `Primitive::Mesh`). Populated on first sight of a new id in
    /// `env.render_queue3d`. v0.2 session 1.
    mesh_cache: HashMap<u32, GpuMesh>,
    /// Ids whose load already failed once. Skip the file I/O and
    /// the stderr noise on every subsequent frame; the user can
    /// fix the path and hot-reload to retry.
    mesh_load_failures: HashSet<u32>,
    /// Phase 28 session 5: in-flight async .glb loads. The render
    /// loop spawns a worker thread for any uncached mesh referenced
    /// this frame, polls `is_finished()` next frame, and does the
    /// GPU upload on the main thread when the worker completes. The
    /// map is keyed by the same id used for the mesh cache.
    /// Mesh / texture ids requested from the `AssetSource` and not yet
    /// delivered.
    mesh_pending: HashSet<u32>,
    texture_pending: HashSet<u32>,
    /// web3d-M7: the group-1 material layout, default textures and
    /// samplers, and the plain material the built-in shapes and
    /// untextured draws use.
    materials: MaterialKit,
    plain_material: wgpu::BindGroup,
    /// Lazy-loaded textures keyed by `Env::texture_paths` interned id:
    /// the plain material with that texture as its base colour.
    texture_cache: HashMap<u32, wgpu::BindGroup>,
    texture_load_failures: HashSet<u32>,
}

/// Per-mesh GPU buffers loaded from a `.glb` file. v0.2 session 1.
/// Stored in `RenderState::mesh_cache` keyed by the
/// `Env::mesh_paths` interned id.
struct GpuMesh {
    /// Phase 26: bounding sphere radius in mesh-local space, used
    /// for per-instance frustum culling. For unskinned meshes it's
    /// `max(|v|)` over all vertices; for skinned meshes it's a
    /// loose multiplier of the rest-pose AABB to account for
    /// limb extension during animation.
    bound_radius: f32,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    /// `.glb` accessors can use u8 / u16 / u32; we widen everything
    /// to u32 on load so the pipeline only needs one branch.
    index_format: wgpu::IndexFormat,
    /// web3d-M7: one draw range per glTF primitive, each with its
    /// material (used when the script gives no texture or material).
    submeshes: Vec<GpuSubmesh>,
    /// Phase 24: skinning data extracted from the glTF skin (if
    /// any). When `Some`, the mesh has per-vertex joint indices +
    /// weights bound, the per-mesh joint UBO + bind group below
    /// are populated, and the render flow updates the UBO each
    /// frame from the active animation clip. When `None`, the
    /// shared `identity_joints_bind_group` is used at slot 3.
    skin: Option<MeshSkin>,
}

/// web3d-M7: a glTF primitive's index range and material.
struct GpuSubmesh {
    first: u32,
    count: u32,
    material: wgpu::BindGroup,
    double_sided: bool,
    /// web3d-M7: alpha-masked (the depth prepass cuts it out too).
    masked: bool,
    /// web3d-M7: alpha-blended: drawn in the transparent pass, sorted
    /// by `centroid` (mesh space).
    blend: bool,
    centroid: [f32; 3],
    /// web3d-M7: transmissive (`KHR_materials_transmission`): drawn in
    /// the transparent pass, over a copy of the opaque scene.
    transmissive: bool,
}

impl GpuSubmesh {
    /// Drawn in the sorted transparent pass rather than the opaque one.
    fn sorted(&self) -> bool {
        self.blend || self.transmissive
    }
}

/// Phase 24: per-mesh skin data + GPU resources. Built at glb load
/// time, retained for the mesh's lifetime. The animation channels
/// own time/value samples in CPU memory; the joint UBO is updated
/// each frame from the script-driven `mesh_anim` state.
struct MeshSkin {
    /// glTF node indices that drive each skin joint (in joint
    /// order). `joint_node_indices.len() ≤ MAX_JOINTS`.
    joint_node_indices: Vec<usize>,
    /// 4×4 inverse bind matrices (column-major) per joint. Same
    /// length as `joint_node_indices`. Multiplied with the joint's
    /// world transform each frame to produce the skin matrix.
    inverse_bind_matrices: Vec<[[f32; 4]; 4]>,
    /// Full set of glTF nodes referenced by the document, in
    /// insertion order. Each entry holds rest-pose TRS plus the
    /// list of child node indices, so the animation sampler can
    /// rebuild the world transform of any joint by walking down
    /// from the skeleton root.
    nodes: Vec<GltfNodeData>,
    /// Top-level scene node indices (the "roots" of the hierarchy
    /// the animation walks). The skin's joints are reachable from
    /// these.
    scene_roots: Vec<usize>,
    /// Animation clips by name. A script-driven `mesh_anim.play(h,
    /// "walk")` selects the clip whose `name` matches.
    clips: HashMap<String, AnimClip>,
    /// Per-mesh joint matrix UBO, written once per frame when an
    /// animation is active. Bound at @group(3) for skinned draws.
    joint_buffer: wgpu::Buffer,
    joint_bind_group: wgpu::BindGroup,
}

/// Phase 24: a single glTF node's rest-pose TRS + children. Stored
/// at glb load time so animation sampling can override TRS each
/// frame and walk children to compute world transforms.
#[derive(Clone, Debug)]
struct GltfNodeData {
    translation: [f32; 3],
    rotation: [f32; 4], // (x, y, z, w) glTF order
    scale: [f32; 3],
    children: Vec<usize>,
}

/// Phase 24: a single animation clip = a set of channels over a
/// fixed duration. Sampled at any time `t ∈ [0, duration]` to
/// produce per-node TRS overrides.
#[derive(Clone, Debug)]
struct AnimClip {
    duration: f32,
    channels: Vec<AnimChannel>,
}

/// One animation channel = (target node, target property, samples).
/// `times` is monotonic; `values` runs in the same step (vec3 for
/// T/S, vec4 for R). Linear interpolation between bracketing
/// keyframes; rotations use shortest-path slerp.
#[derive(Clone, Debug)]
struct AnimChannel {
    target_node: usize,
    property: AnimProperty,
    times: Vec<f32>,
    values: AnimValues,
}

#[derive(Clone, Copy, Debug)]
enum AnimProperty {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Debug)]
enum AnimValues {
    Vec3(Vec<[f32; 3]>),
    Vec4(Vec<[f32; 4]>),
}

impl Renderer {
    /// Create the renderer for `surface` (made by the host shell from
    /// its window or canvas). Async because adapter / device requests
    /// are async on the web; native hosts drive it with `pollster`.
    pub async fn new(
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Renderer, String> {
        init_renderer(instance, Some(surface), width, height).await
    }

    /// A renderer with no window: frames draw into an offscreen
    /// `Rgba8UnormSrgb` texture, read back with [`Renderer::read_pixels`].
    /// Used by the kernel's render tests.
    pub async fn new_headless(width: u32, height: u32) -> Result<Renderer, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        init_renderer(&instance, None, width, height).await
    }

    /// Reconfigure for a new drawable size (the graph's targets follow
    /// on the next frame).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        match &self.surface {
            Some(surface) => surface.configure(&self.device, &self.config),
            None => self.offscreen = Some(create_offscreen(&self.device, &self.config)),
        }
    }

    /// Drop every cached / pending mesh and texture (hot reload).
    pub fn clear_asset_caches(&mut self) {
        self.mesh_cache.clear();
        self.mesh_load_failures.clear();
        self.mesh_pending.clear();
        self.texture_cache.clear();
        self.texture_load_failures.clear();
        self.texture_pending.clear();
    }
}

async fn init_renderer(
    instance: &wgpu::Instance,
    surface: Option<wgpu::Surface<'static>>,
    width: u32,
    height: u32,
) -> Result<Renderer, String> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power_preference(),
            compatible_surface: surface.as_ref(),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|e| format!("no compatible wgpu adapter found (WebGPU unavailable?): {e}"))?;
    // web3d-M2: request only the WebGPU default limits (4 bind groups,
    // etc.) so the renderer runs on any conformant device — browsers
    // and mobile GPUs included. The main pipeline uses exactly 4 groups.
    let required_limits = wgpu::Limits::default();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("twe-kernel device"),
            // web3d-M7 follow-up: timestamps, only for `TWE_GPU_PROFILE`.
            required_features: crate::kernel::gpu_profile::wanted_features(&adapter),
            required_limits,
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|e| e.to_string())?;
    let config = match &surface {
        Some(surface) => {
            let caps = surface.get_capabilities(&adapter);
            let format = caps
                .formats
                .iter()
                .find(|f| f.is_srgb())
                .copied()
                .unwrap_or_else(|| caps.formats[0]);
            wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: wgpu::SurfaceColorSpace::Auto,
                width: width.max(1),
                height: height.max(1),
                present_mode: caps.present_modes[0],
                alpha_mode: caps.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            }
        }
        // Headless: the config only carries size + format.
        None => wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        },
    };
    let mut config = config;
    // web3d-M3: always draw through an sRGB view. The tonemap and HUD
    // write linear colour and rely on the target's sRGB encoding; a
    // non-sRGB surface (common for browser canvases) gets an sRGB view
    // format instead of silently skipping the gamma curve.
    let target_format = config.format.add_srgb_suffix();
    if target_format != config.format {
        config.view_formats.push(target_format);
    }
    let surface_format = target_format;
    if let Some(surface) = &surface {
        surface.configure(&device, &config);
    }
    let offscreen = surface
        .is_none()
        .then(|| create_offscreen(&device, &config));

    // Vertex + index buffers, uploaded once. Cube is a const,
    // sphere is generated at startup (the procedural mesh is
    // small and the cost amortises across the program's lifetime).
    let cube_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d cube vertices"),
        contents: bytemuck::cast_slice(CUBE_VERTICES),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let cube_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d cube indices"),
        contents: bytemuck::cast_slice(CUBE_INDICES),
        usage: wgpu::BufferUsages::INDEX,
    });
    let (sphere_verts, sphere_idxs) = sphere_mesh();
    let sphere_index_count = sphere_idxs.len() as u32;
    let sphere_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d sphere vertices"),
        contents: bytemuck::cast_slice(&sphere_verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let sphere_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d sphere indices"),
        contents: bytemuck::cast_slice(&sphere_idxs),
        usage: wgpu::BufferUsages::INDEX,
    });

    // Phase 23: dynamic instance buffer. Starts at
    // INITIAL_INSTANCE_CAPACITY but grows by doubling whenever a
    // frame needs more than the current capacity. Removes the
    // hard 4096 cap from the original implementation; large open
    // scenes can push thousands of draws without surface error.
    let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twec-play3d instances"),
        size: INITIAL_INSTANCE_CAPACITY * std::mem::size_of::<Instance>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let instance_capacity = INITIAL_INSTANCE_CAPACITY;

    // Camera uniform.
    let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twec-play3d camera"),
        size: std::mem::size_of::<CameraUniform>() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Phase 20: lights uniform. web3d-M2: shares bind group 0 with the
    // camera (both change once per frame), so the main pipeline fits in
    // the WebGPU minimum of 4 bind groups. Initial contents are
    // overwritten by the first frame's snapshot.
    let lights_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twe-kernel lights"),
        size: std::mem::size_of::<LightsUniform>() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let uniform_entry = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let texture_entry = |binding: u32, dim: wgpu::TextureViewDimension| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    };
    let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    };
    let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twe-kernel frame bgl"),
        entries: &[
            // web3d-M3: fragments read `camera.time` (materials).
            uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
            // web3d-M7: the fog volume (kernel/volumetric.rs) reads the
            // lights and clusters in a compute pass.
            uniform_entry(1, wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE),
            // web3d-M7: the environment.
            uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            texture_entry(3, wgpu::TextureViewDimension::Cube),
            texture_entry(4, wgpu::TextureViewDimension::D2),
            texture_entry(5, wgpu::TextureViewDimension::D2),
            sampler_entry(6),
            sampler_entry(7),
            // web3d-M7: screen-space ambient occlusion.
            texture_entry(8, wgpu::TextureViewDimension::D2),
            // web3d-M7: height fog.
            uniform_entry(9, wgpu::ShaderStages::FRAGMENT),
            // web3d-M7: the transmission source.
            texture_entry(10, wgpu::TextureViewDimension::D2),
            // web3d-M7: clustered lights (list, grid, parameters).
            wgpu::BindGroupLayoutEntry {
                binding: 11,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 12,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            uniform_entry(13, wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE),
        ],
    });
    let fog_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twe-kernel fog uniform"),
        contents: bytemuck::bytes_of(&FogUniform::zeroed()),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let env = EnvState::new(&device, &queue, &frame_bgl);
    let taa = crate::kernel::taa::Taa::new(&device);
    let ao = crate::kernel::ao::Ao::new(&device, &queue);
    let bloom = crate::kernel::post::Bloom::new(&device, &queue);
    let exposure = crate::kernel::post::AutoExposure::new(&device);
    let transmission = crate::kernel::post::Transmission::new(&device, &queue);
    let gpu_cull = crate::kernel::gpu_cull::GpuCull::new(&device);
    let particles = crate::kernel::particles::Particles::new(&device);
    let ssr = crate::kernel::ssr::Ssr::new(&device);
    let dof = crate::kernel::post::Dof::new(&device);
    let motion = crate::kernel::post::MotionBlur::new(&device);
    let lut = LutState {
        view: crate::kernel::post::identity_lut(&device, &queue),
        size: 2,
        requested: None,
        loaded: None,
        generation: 0,
    };
    let clusters = crate::kernel::clusters::Clusters::new(&device);
    let frame_bind_group = frame_bind_group(
        &device,
        &frame_bgl,
        &FrameBuffers {
            camera: &camera_buffer,
            lights: &lights_buffer,
            fog: &fog_buffer,
            clusters: &clusters,
        },
        &env,
        [ao.view(false), transmission.view(false)],
    );

    // Phase 24: joint UBO bind group layout, plus a shared
    // identity-only UBO bound for unskinned draws (cube, sphere,
    // glb without a skin). Skinned meshes own a per-mesh UBO of
    // the same layout written each frame from `mesh_anim`.
    let joints_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twec-play3d joints bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let identity_joints_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d identity joints"),
        contents: bytemuck::bytes_of(&JointsUniform::identity()),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let identity_joints_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twec-play3d identity joints bg"),
        layout: &joints_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: identity_joints_buffer.as_entire_binding(),
        }],
    });

    // web3d-M7: materials (kernel/material.rs) at group 1.
    let mut materials = MaterialKit::new(&device, &queue);
    let plain_material = materials.bind_group(&device, &MaterialData::plain(), [None; SLOTS]);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("twec-play3d shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
    });
    // Phase 25: shadow uniform bgl (just the matrix + flags). The
    // shadow pipeline binds this at @group(0) as its only "camera".
    let shadow_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twec-play3d shadow uniform bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    // Combined shadow bgl used by the main pipeline: shadow
    // lookup uniform + 2D-array depth texture + comparison
    // sampler. Phase 28 session 2: texture is now D2Array (one
    // layer per cascade) so the fragment shader can pick a layer
    // per pixel.
    let shadow_combined_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twec-play3d shadow combined bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            // web3d-M7: point-light shadow cubes.
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::CubeArray,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("twec-play3d pipeline layout"),
        bind_group_layouts: &[
            Some(&frame_bgl),
            Some(&materials.layout),
            Some(&joints_bgl),
            Some(&shadow_combined_bgl),
        ],
        immediate_size: 0,
    });
    let lit = LitPipelines::new(&device, &pipeline_layout, &shader, false);
    let volumetric = crate::kernel::volumetric::Volumetric::new(&device, &frame_bgl, &shadow_combined_bgl);

    // Phase 28 session 2: cascaded shadow maps. The shadow texture
    // is a 2D array with `CASCADE_COUNT` layers; each shadow pass
    // renders one cascade into its own layer. The main fragment
    // shader samples the array, picking a layer per pixel by
    // view-space depth.
    let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("twec-play3d shadow map"),
        size: wgpu::Extent3d {
            width: SHADOW_MAP_SIZE,
            height: SHADOW_MAP_SIZE,
            depth_or_array_layers: CASCADE_COUNT as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    // Per-cascade depth-attachment views — each targets a single
    // array layer for the corresponding shadow pass.
    let shadow_layer_views: [wgpu::TextureView; CASCADE_COUNT] = std::array::from_fn(|i| {
        shadow_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("twec-play3d shadow layer view"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: i as u32,
            array_layer_count: Some(1),
            ..Default::default()
        })
    });
    // Full-array view sampled by the main fragment shader.
    let shadow_array_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("twec-play3d shadow array view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_array_layer: 0,
        array_layer_count: Some(CASCADE_COUNT as u32),
        ..Default::default()
    });
    // Lookup uniform: full ShadowUniform with 3 matrices + splits.
    let shadow_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twec-play3d shadow lookup uniform"),
        size: std::mem::size_of::<ShadowUniform>() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &shadow_buffer,
        0,
        bytemuck::bytes_of(&ShadowUniform::disabled()),
    );
    // Per-cascade pass uniforms — small, just one mat4 each.
    let shadow_pass_buffers: [wgpu::Buffer; CASCADE_COUNT] = std::array::from_fn(|i| {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twec-play3d shadow pass uniform"),
            size: std::mem::size_of::<ShadowPassUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let _ = i;
        queue.write_buffer(&buf, 0, bytemuck::bytes_of(&ShadowPassUniform::identity()));
        buf
    });
    let shadow_pass_bgs: [wgpu::BindGroup; CASCADE_COUNT] = std::array::from_fn(|i| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twec-play3d shadow pass bg"),
            layout: &shadow_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_pass_buffers[i].as_entire_binding(),
            }],
        })
    });
    // Comparison sampler — PCF expects this. Linear filter does
    // hardware 2x2 PCF on top of our manual 3x3 = a 5x5-ish kernel.
    let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("twec-play3d shadow sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("twec-play3d shadow shader"),
        source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER_SRC.into()),
    });
    let point_shadows = PointShadows::new(&device, &shadow_uniform_bgl, &joints_bgl, &shadow_shader);
    let prepass = Prepass::new(&device, &shadow_uniform_bgl, &joints_bgl, &materials.layout, &shadow_shader);
    let gpu_profile = crate::kernel::gpu_profile::GpuProfile::new(&device, &queue).map(std::cell::RefCell::new);
    let shadow_combined_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twec-play3d shadow combined bg"),
        layout: &shadow_combined_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&shadow_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&point_shadows.cube_array),
            },
        ],
    });
    // Shadow pipeline: depth-only. Binds shadow uniform at @group(0)
    // (acting as the camera) and joints UBO at @group(1) (so
    // skinned characters cast correct shadows).
    let shadow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("twec-play3d shadow pipeline layout"),
        bind_group_layouts: &[Some(&shadow_uniform_bgl), Some(&joints_bgl)],
        immediate_size: 0,
    });
    let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("twec-play3d shadow pipeline"),
        layout: Some(&shadow_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shadow_shader,
            entry_point: Some("vs_shadow"),
            buffers: &[Some(Vertex::layout()), Some(Instance::layout())],
            compilation_options: Default::default(),
        },
        fragment: None, // depth-only pass
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            // Front-face culling for the shadow pass mitigates
            // self-shadowing acne on flat ground meshes.
            cull_mode: Some(wgpu::Face::Front),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    // Phase 26: tonemap pipeline. The fragment samples the HDR
    // offscreen texture (group 0 binding 0), runs ACES, optionally
    // applies vignette from `params` (binding 2), and writes sRGB
    // to the swapchain.
    let tonemap_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twec-play3d tonemap bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // web3d-M7: the bloom chain's top level.
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // web3d-M7: the adapted exposure.
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // web3d-M7: the colour-grading LUT.
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let tonemap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("twec-play3d tonemap sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    // Tonemap params, three vec4s (48 B): see `TONEMAP_SHADER_SRC`.
    let tonemap_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twec-play3d tonemap params"),
        size: 48,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &tonemap_params_buffer,
        0,
        bytemuck::cast_slice(&[0.0_f32; 12]),
    );
    let tonemap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("twec-play3d tonemap shader"),
        source: wgpu::ShaderSource::Wgsl(TONEMAP_SHADER_SRC.into()),
    });
    let tonemap_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("twec-play3d tonemap pipeline layout"),
        bind_group_layouts: &[Some(&tonemap_bgl)],
        immediate_size: 0,
    });
    let tonemap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("twec-play3d tonemap pipeline"),
        layout: Some(&tonemap_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &tonemap_shader,
            entry_point: Some("vs_fullscreen"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &tonemap_shader,
            entry_point: Some("fs_tonemap"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let hud = super::hud::Hud::new(&device, &queue, target_format);
    Ok(Renderer {
        surface,
        offscreen,
        device,
        queue,
        config,
        lit,
        lit_ssr: None,
        main_shader: shader,
        ssr_frame: std::cell::Cell::new(false),
        fog_buffer,
        pipeline_layout,
        material_pipelines: [HashMap::new(), HashMap::new()],
        depth_layout: shadow_pipeline_layout,
        displaced_depth: HashMap::new(),
        retained: None,
        target_format,
        hud,
        cube_vertex_buffer,
        cube_index_buffer,
        cube_index_count: CUBE_INDICES.len() as u32,
        sphere_vertex_buffer,
        sphere_index_buffer,
        sphere_index_count,
        instance_buffer,
        instance_capacity,
        camera_buffer,
        frame_bind_group,
        frame_bgl,
        env,
        taa,
        prepass,
        ao,
        bloom,
        exposure,
        dof,
        motion,
        lut,
        frame_ao_key: (0, 0),
        transmission,
        gpu_cull,
        particles,
        ssr,
        volumetric,
        clusters,
        frame_index: std::cell::Cell::new(0),
        gpu_profile,
        last_culled: std::cell::Cell::new(false),
        point_shadows,
        lights_buffer,
        joints_bgl,
        identity_joints_bind_group,
        shadow_pipeline,
        shadow_buffer,
        shadow_pass_buffers,
        shadow_pass_bgs,
        shadow_combined_bg,
        shadow_texture,
        shadow_layer_views,
        tonemap_pipeline,
        tonemap_bgl,
        tonemap_sampler,
        tonemap_params_buffer,
        pool: TexturePool::default(),
        tonemap_bind_group: None,
        mesh_cache: HashMap::new(),
        mesh_load_failures: HashSet::new(),
        mesh_pending: HashSet::new(),
        texture_pending: HashSet::new(),
        materials,
        plain_material,
        texture_cache: HashMap::new(),
        texture_load_failures: HashSet::new(),
    })
}

/// Phase 17 session 3: PNG/JPEG texture loader (a script's
/// `texture(path)`). Decodes via the `image` crate and uploads as sRGB
/// colour with mips; the caller wraps it in a material.
fn upload_texture_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bytes: &[u8],
) -> Result<wgpu::TextureView, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let texture = upload_rgba(device, queue, "twec-play3d texture", &rgba, w, h, true);
    Ok(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

// ---------- glTF 2.0 mesh loader ----------
//
// v0.2 session 1. Pulls position + normal + indices out of the first
// primitive of the first mesh in a `.glb`. Returns CPU-side data the
// caller uploads to GPU. Multi-primitive scenes, node transforms,
// materials, and textures are all follow-ons — design notes in
// `notes/future-phases.md` "Carried into v0.2".

/// Phase 24: skin + animation data extracted from a glb at load
/// time, ready to be paired with GPU resources by the caller.
/// Decoupled from `MeshSkin` (which holds the GPU buffer) so
/// tests can exercise the parser without a wgpu device.
pub(crate) struct LoadedSkinData {
    joint_node_indices: Vec<usize>,
    inverse_bind_matrices: Vec<[[f32; 4]; 4]>,
    nodes: Vec<GltfNodeData>,
    scene_roots: Vec<usize>,
    clips: HashMap<String, AnimClip>,
}

/// A decoded `.glb`, ready for GPU upload. Opaque to hosts: they get
/// one from [`parse_glb_bytes`] and hand it back via
/// [`AssetReady::Mesh`].
pub struct LoadedGlb(Box<GlbData>);

impl LoadedGlb {
    /// web3d-M7: a sphere around every vertex (bounding-box centre, then
    /// the farthest vertex), in the model's own units. The graphics
    /// harness places cameras and clip planes with it.
    pub fn bounding_sphere(&self) -> ([f32; 3], f32) {
        let verts = &self.0.vertices;
        if verts.is_empty() {
            return ([0.0; 3], 0.0);
        }
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in verts {
            for k in 0..3 {
                lo[k] = lo[k].min(v.position[k]);
                hi[k] = hi[k].max(v.position[k]);
            }
        }
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let r = verts
            .iter()
            .map(|v| {
                let d = sub(v.position, c);
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .fold(0.0, f32::max);
        (c, r)
    }
}

/// The glb loader's output: one vertex / index buffer for the whole
/// model, split into per-primitive draw ranges with their materials
/// (web3d-M7), the images those materials use, and skin + animation
/// data (Phase 24).
struct GlbData {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    submeshes: Vec<Submesh>,
    materials: Vec<MaterialData>,
    images: Vec<ImageData>,
    skin: Option<LoadedSkinData>,
}

/// web3d-M7: one glTF primitive's range of `GlbData::indices`.
#[derive(Clone, Copy, Debug)]
struct Submesh {
    first: u32,
    count: u32,
    material: usize,
}

/// Inner loader exposed for tests — drives the gltf crate against
/// an in-memory byte slice instead of a path so we can exercise
/// the decode path without shipping binary fixtures.
pub fn parse_glb_bytes(bytes: &[u8]) -> Result<LoadedGlb, String> {
    let (doc, buffers, images) = gltf::import_slice(bytes).map_err(|e| e.to_string())?;
    if doc.meshes().count() == 0 {
        return Err("glb has no meshes".to_string());
    }

    // Phase 19: walk the scene graph and flatten all nodes into a
    // single (vertices, indices) buffer. Each node's accumulated
    // transform (parent-multiplied) bakes into vertex positions and
    // normals at load time — except for skinned primitives, whose
    // positions stay in mesh-local space (the skin pass at render
    // time resolves them via the joint matrices). Phase 24.
    // web3d-M7: every material of the document, then glTF's default
    // material for primitives that name none; every image as RGBA8.
    let mut materials: Vec<MaterialData> =
        doc.materials().map(|m| MaterialData::from_gltf(&m, &doc)).collect();
    materials.push(MaterialData {
        metallic: 1.0,
        roughness: 1.0,
        ..MaterialData::plain()
    });
    let mut out = GlbData {
        vertices: Vec::new(),
        indices: Vec::new(),
        submeshes: Vec::new(),
        materials,
        images: images.iter().map(ImageData::from_gltf).collect(),
        skin: None,
    };

    // Use the default scene if present, otherwise scene 0.
    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .ok_or_else(|| "glb has no scenes".to_string())?;

    for node in scene.nodes() {
        flatten_node(&node, mat4_identity(), &buffers, &mut out);
    }

    // Phase 5 fallback: if the document has meshes but no scene
    // graph (rare but legal for raw mesh files), pull the first
    // primitive of the first mesh directly. Preserves backward
    // compatibility with the pre-Phase-19 single-primitive loader.
    if out.vertices.is_empty() {
        let mesh = doc.meshes().next().unwrap();
        let primitive = mesh
            .primitives()
            .next()
            .ok_or_else(|| "first mesh has no primitives".to_string())?;
        flatten_primitive(&primitive, mat4_identity(), false, &buffers, &mut out);
    }

    if out.vertices.is_empty() {
        return Err("glb has zero vertices across all primitives".to_string());
    }

    // Phase 24: extract skin + animation data, if any. We pick the
    // first skin referenced anywhere in the document; multi-skin
    // documents are rare in practice and outside MVP scope.
    out.skin = extract_skin_data(&doc, &buffers, &scene);
    Ok(LoadedGlb(Box::new(out)))
}

/// Phase 24: extract skin + animation channels from the document.
/// Returns `None` for unskinned glb files (the common case for
/// static props / level geometry). When `Some`, the caller pairs
/// this with a per-mesh joint UBO in `MeshSkin`.
fn extract_skin_data(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    scene: &gltf::Scene<'_>,
) -> Option<LoadedSkinData> {
    let skin = doc.skins().next()?;
    let joint_node_indices: Vec<usize> = skin.joints().map(|n| n.index()).collect();
    if joint_node_indices.is_empty() {
        return None;
    }
    let reader = skin.reader(|b| Some(&buffers[b.index()]));
    let inverse_bind_matrices: Vec<[[f32; 4]; 4]> = match reader.read_inverse_bind_matrices() {
        Some(it) => it.collect(),
        None => vec![mat4_identity(); joint_node_indices.len()],
    };
    let inverse_bind_matrices = if inverse_bind_matrices.len() == joint_node_indices.len() {
        inverse_bind_matrices
    } else {
        vec![mat4_identity(); joint_node_indices.len()]
    };

    // Capture all nodes' rest-pose TRS + child indices. We index
    // by node index, so any animation channel can target any node
    // by `node.index()`.
    let mut nodes: Vec<GltfNodeData> = Vec::with_capacity(doc.nodes().count());
    for node in doc.nodes() {
        let (t, r, s) = node.transform().decomposed();
        nodes.push(GltfNodeData {
            translation: t,
            rotation: r,
            scale: s,
            children: node.children().map(|c| c.index()).collect(),
        });
    }

    let scene_roots: Vec<usize> = scene.nodes().map(|n| n.index()).collect();

    // Walk all animations; group channels by clip name.
    let mut clips: HashMap<String, AnimClip> = HashMap::new();
    for (i, anim) in doc.animations().enumerate() {
        let name = anim
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("anim_{i}"));
        let mut duration: f32 = 0.0;
        let mut channels: Vec<AnimChannel> = Vec::new();
        for chan in anim.channels() {
            let target_node = chan.target().node().index();
            let property = match chan.target().property() {
                gltf::animation::Property::Translation => AnimProperty::Translation,
                gltf::animation::Property::Rotation => AnimProperty::Rotation,
                gltf::animation::Property::Scale => AnimProperty::Scale,
                gltf::animation::Property::MorphTargetWeights => continue,
            };
            let sampler_reader = chan.reader(|b| Some(&buffers[b.index()]));
            let times: Vec<f32> = match sampler_reader.read_inputs() {
                Some(it) => it.collect(),
                None => continue,
            };
            if let Some(&t_max) = times.last() {
                if t_max > duration {
                    duration = t_max;
                }
            }
            let values = match sampler_reader.read_outputs() {
                Some(gltf::animation::util::ReadOutputs::Translations(it)) => {
                    AnimValues::Vec3(it.collect())
                }
                Some(gltf::animation::util::ReadOutputs::Scales(it)) => {
                    AnimValues::Vec3(it.collect())
                }
                Some(gltf::animation::util::ReadOutputs::Rotations(it)) => {
                    AnimValues::Vec4(it.into_f32().collect())
                }
                _ => continue,
            };
            channels.push(AnimChannel {
                target_node,
                property,
                times,
                values,
            });
        }
        clips.insert(name, AnimClip { duration, channels });
    }

    Some(LoadedSkinData {
        joint_node_indices,
        inverse_bind_matrices,
        nodes,
        scene_roots,
        clips,
    })
}

/// Phase 19: 4×4 column-major identity matrix, used as the root
/// transform when walking a glTF scene graph.
fn mat4_identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Multiply two 4×4 column-major matrices: out = a * b. Glsl-style
/// (so `a` is the outer transform — applied second when transforming
/// a vector).
fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            out[j][i] =
                a[0][i] * b[j][0] + a[1][i] * b[j][1] + a[2][i] * b[j][2] + a[3][i] * b[j][3];
        }
    }
    out
}

/// Apply a 4×4 transform to a 3D point (w=1). Used when baking
/// node transforms into vertex positions during scene flattening.
fn mat4_transform_point(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
        m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
        m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
    ]
}

/// Apply a 4×4 transform to a 3D direction (w=0). Used for normals.
/// For non-uniform scale this is wrong (proper fix is the
/// inverse-transpose of the upper-left 3×3); for the typical
/// translation-rotation-uniform-scale node transforms in a
/// Blender export it's good enough.
fn mat4_transform_dir(m: [[f32; 4]; 4], d: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * d[0] + m[1][0] * d[1] + m[2][0] * d[2],
        m[0][1] * d[0] + m[1][1] * d[1] + m[2][1] * d[2],
        m[0][2] * d[0] + m[1][2] * d[1] + m[2][2] * d[2],
    ]
}

/// Phase 24: build a column-major TRS matrix from translation,
/// rotation (quaternion x,y,z,w), and scale. Matches the glTF 2.0
/// transform composition: M = T * R * S, applied to a point as
/// p' = T * (R * (S * p)).
fn mat4_from_trs(t: [f32; 3], r: [f32; 4], s: [f32; 3]) -> [[f32; 4]; 4] {
    let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    // Column-major: column 0 is the first array entry. Each column
    // is the basis-axis image (e.g. column 0 = R*S * (1,0,0)).
    [
        [
            (1.0 - 2.0 * (yy + zz)) * s[0],
            2.0 * (xy + wz) * s[0],
            2.0 * (xz - wy) * s[0],
            0.0,
        ],
        [
            2.0 * (xy - wz) * s[1],
            (1.0 - 2.0 * (xx + zz)) * s[1],
            2.0 * (yz + wx) * s[1],
            0.0,
        ],
        [
            2.0 * (xz + wy) * s[2],
            2.0 * (yz - wx) * s[2],
            (1.0 - 2.0 * (xx + yy)) * s[2],
            0.0,
        ],
        [t[0], t[1], t[2], 1.0],
    ]
}

/// Phase 24: linear interpolation between two vec3 keyframes.
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Phase 24: shortest-path quaternion slerp. Flips one operand
/// when the dot product is negative so the interpolation takes
/// the short arc — matches the standard glTF animation rule.
fn slerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut bb = b;
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        bb = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }
    if dot > 0.9995 {
        // Near-parallel: lerp + normalize is more stable than slerp.
        let r = [
            a[0] + (bb[0] - a[0]) * t,
            a[1] + (bb[1] - a[1]) * t,
            a[2] + (bb[2] - a[2]) * t,
            a[3] + (bb[3] - a[3]) * t,
        ];
        let len = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2] + r[3] * r[3]).sqrt();
        return [r[0] / len, r[1] / len, r[2] / len, r[3] / len];
    }
    let omega = dot.acos();
    let sin_o = omega.sin();
    let s_a = ((1.0 - t) * omega).sin() / sin_o;
    let s_b = (t * omega).sin() / sin_o;
    [
        a[0] * s_a + bb[0] * s_b,
        a[1] * s_a + bb[1] * s_b,
        a[2] * s_a + bb[2] * s_b,
        a[3] * s_a + bb[3] * s_b,
    ]
}

/// Phase 24: bracket a time `t` against a sorted keyframe `times`
/// vector and return `(i0, i1, alpha)` where `i0 < i1` are
/// keyframe indices and `alpha ∈ [0, 1]` is the local interp
/// parameter. Clamps to first/last when `t` is outside the range.
fn bracket_keyframe(times: &[f32], t: f32) -> (usize, usize, f32) {
    if times.is_empty() {
        return (0, 0, 0.0);
    }
    if t <= times[0] {
        return (0, 0, 0.0);
    }
    let last = times.len() - 1;
    if t >= times[last] {
        return (last, last, 0.0);
    }
    // Linear search is fine for typical animation channels
    // (≤30 keyframes for a 1-second walk cycle at 30fps). For
    // longer clips this becomes binary search.
    for i in 0..last {
        if t >= times[i] && t <= times[i + 1] {
            let span = times[i + 1] - times[i];
            let alpha = if span > 1e-6 {
                (t - times[i]) / span
            } else {
                0.0
            };
            return (i, i + 1, alpha);
        }
    }
    (last, last, 0.0)
}

/// Phase 24: sample one animation channel at time `t`, returning
/// either a vec3 (Translation/Scale) or a vec4 (Rotation). The
/// caller knows which property the channel targets and slots the
/// value into the corresponding TRS field of the channel's node.
enum SampledValue {
    Vec3([f32; 3]),
    Vec4([f32; 4]),
}

fn sample_channel(channel: &AnimChannel, t: f32) -> SampledValue {
    let (i0, i1, alpha) = bracket_keyframe(&channel.times, t);
    match (&channel.property, &channel.values) {
        (AnimProperty::Translation, AnimValues::Vec3(v))
        | (AnimProperty::Scale, AnimValues::Vec3(v)) => {
            let a = v.get(i0).copied().unwrap_or([0.0; 3]);
            let b = v.get(i1).copied().unwrap_or(a);
            SampledValue::Vec3(lerp3(a, b, alpha))
        }
        (AnimProperty::Rotation, AnimValues::Vec4(v)) => {
            let a = v.get(i0).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let b = v.get(i1).copied().unwrap_or(a);
            SampledValue::Vec4(slerp4(a, b, alpha))
        }
        // Mismatched property/value combos shouldn't happen for
        // well-formed glTF; fall back to identity.
        (AnimProperty::Translation | AnimProperty::Scale, _) => SampledValue::Vec3([0.0; 3]),
        (AnimProperty::Rotation, _) => SampledValue::Vec4([0.0, 0.0, 0.0, 1.0]),
    }
}

/// Phase 24: compute per-joint skin matrices at the current
/// animation time. Returns a `JointsUniform` ready to upload.
///
/// Algorithm (standard glTF skinning):
/// 1. Start each node at its rest-pose TRS.
/// 2. Override TRS on nodes targeted by the active clip's channels.
/// 3. Walk the scene roots, multiplying parent world × local TRS
///    matrix, to get every node's world transform.
/// 4. For each skin joint i, the skin matrix is
///    `world(joint_node[i]) * inverse_bind_matrix[i]`.
fn compute_skinned_joint_matrices(skin: &MeshSkin, anim: &AnimSnapshot) -> JointsUniform {
    let n_nodes = skin.nodes.len();

    // Start each node at its rest pose.
    let mut trs: Vec<([f32; 3], [f32; 4], [f32; 3])> = skin
        .nodes
        .iter()
        .map(|n| (n.translation, n.rotation, n.scale))
        .collect();

    // Apply primary clip overrides.
    if !anim.clip.is_empty() {
        if let Some(clip) = skin.clips.get(&anim.clip) {
            // Loop time within clip duration when the clip has any
            // duration. (Looping is the script's choice via
            // `mesh_anim.play(h, name, true)`; the renderer wraps
            // either way — non-looping holds the last frame because
            // `bracket_keyframe` clamps, which is acceptable.)
            let t_clip = if clip.duration > 0.0 {
                anim.time.rem_euclid(clip.duration)
            } else {
                0.0
            };
            apply_clip(&mut trs, clip, t_clip);
        }
    }

    // Optional blend: linearly interpolate the secondary clip on
    // top of the primary by `blend_t`. For TRS this is component-
    // wise lerp (translation/scale) and slerp (rotation).
    if let Some(blend_name) = &anim.blend_clip {
        if let Some(clip_b) = skin.clips.get(blend_name) {
            let t_clip = if clip_b.duration > 0.0 {
                anim.time.rem_euclid(clip_b.duration)
            } else {
                0.0
            };
            // Snapshot rest-pose-overlaid-with-A-only into `a_trs`
            // so we can lerp toward B without trampling.
            let a_trs = trs.clone();
            let mut b_trs: Vec<([f32; 3], [f32; 4], [f32; 3])> = skin
                .nodes
                .iter()
                .map(|n| (n.translation, n.rotation, n.scale))
                .collect();
            apply_clip(&mut b_trs, clip_b, t_clip);
            for i in 0..n_nodes {
                let t = anim.blend_t.clamp(0.0, 1.0);
                trs[i] = (
                    lerp3(a_trs[i].0, b_trs[i].0, t),
                    slerp4(a_trs[i].1, b_trs[i].1, t),
                    lerp3(a_trs[i].2, b_trs[i].2, t),
                );
            }
        }
    }

    // Walk hierarchy from scene roots, multiplying parent world *
    // local matrix, to get each node's world transform.
    let mut world: Vec<[[f32; 4]; 4]> = vec![mat4_identity(); n_nodes];
    let mut visited = vec![false; n_nodes];
    let mut stack: Vec<(usize, [[f32; 4]; 4])> = skin
        .scene_roots
        .iter()
        .map(|&i| (i, mat4_identity()))
        .collect();
    while let Some((idx, parent_world)) = stack.pop() {
        if idx >= n_nodes || visited[idx] {
            continue;
        }
        visited[idx] = true;
        let (t, r, s) = trs[idx];
        let local = mat4_from_trs(t, r, s);
        let w = mat4_mul(parent_world, local);
        world[idx] = w;
        for &child in &skin.nodes[idx].children {
            stack.push((child, w));
        }
    }

    // Build skin matrices = world(joint) * IBM. Cap at MAX_JOINTS
    // — anything beyond is clipped (a static error path that
    // would only fire for >128-joint rigs).
    let mut out = JointsUniform::identity();
    let n_joints = skin.joint_node_indices.len().min(MAX_JOINTS);
    for i in 0..n_joints {
        let node_idx = skin.joint_node_indices[i];
        let w = if node_idx < n_nodes {
            world[node_idx]
        } else {
            mat4_identity()
        };
        out.matrices[i] = mat4_mul(w, skin.inverse_bind_matrices[i]);
    }
    out
}

/// Phase 25: build the per-frame shadow uniform from sun direction,
/// camera target, the script-driven enable flag, and extent.
/// Returns a "disabled" uniform when shadows are off so the main
/// pass short-circuits the lookup. The light-space matrix is an
/// orthographic projection from the sun looking at `target`, sized
/// by the script-controlled extent (default 30m radius).
/// Phase 28 session 2: compute the cascaded shadow uniform.
///
/// Each cascade is a target-centered orthographic projection, with
/// extents scaled so cascade 0 is tight (highest texel density),
/// cascade 1 medium, cascade 2 wide. Split distances are camera-
/// space forward depths where the fragment shader switches between
/// cascades. A "tunic-scale" outdoor view of ~100m fits the
/// chosen splits — the user-supplied `shadow_extent` controls
/// cascade 1's size; cascade 0 shrinks by 4×, cascade 2 grows
/// by 4×.
///
/// Trade-off vs. fully-fitted CSM: the per-cascade ortho is
/// centered on `target` rather than tightly bounded to that
/// cascade's view-frustum slice. For third-person cameras
/// (target ≈ player position) this is fine; for free cameras far
/// from the focal target, expect lower-than-ideal cascade-0
/// resolution. A view-frustum-corner CSM upgrade is a follow-on
/// if someone pressures the gap.
/// web3d-M7: point lights that cast shadows (`light.shadow(h, true)`),
/// at most this many per frame, each a cube of this size.
pub(crate) const POINT_SHADOW_LIGHTS: usize = 4;
pub(crate) const POINT_SHADOW_SIZE: u32 = 512;
/// Near plane of the point-shadow face cameras (the far plane is the
/// light's radius).
const POINT_SHADOW_NEAR: f32 = 0.05;

/// The camera as cascade fitting needs it: a unit basis, the half-angle
/// tangents of the view, and the clip planes.
struct CascadeCamera {
    eye: [f32; 3],
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    tan_x: f32,
    tan_y: f32,
    near: f32,
    far: f32,
}

/// Cascade boundaries from `near` to `far`: the "practical" split
/// scheme (Zhang et al. 2006), a blend of logarithmic and uniform
/// splits (λ = 0.75) so near cascades stay dense without starving the
/// far ones.
fn cascade_splits(near: f32, far: f32) -> [f32; CASCADE_COUNT + 1] {
    const LAMBDA: f32 = 0.75;
    let mut out = [near; CASCADE_COUNT + 1];
    for (i, split) in out.iter_mut().enumerate().skip(1) {
        let k = i as f32 / CASCADE_COUNT as f32;
        let log = near * (far / near).powf(k);
        let uniform = near + (far - near) * k;
        *split = LAMBDA * log + (1.0 - LAMBDA) * uniform;
    }
    out
}

/// web3d-M7: sun shadow cascades fitted to the camera.
///
/// The view frustum from the near plane to `4 × extent` is split into
/// `CASCADE_COUNT` slices. Each slice is wrapped in a bounding sphere,
/// so the cascade's size doesn't change as the camera turns, and its
/// orthographic window is snapped to whole shadow-map texels in a fixed
/// light view, so shadow edges don't crawl as the camera moves. Casters
/// up to `extent` beyond a slice toward the sun still land in it.
fn compute_shadow_uniform(
    lights: &LightsUniform,
    cam: &CascadeCamera,
    shadow: ShadowSettings,
) -> ShadowUniform {
    if !shadow.enabled || lights.sun_dir[3] <= 0.0 {
        return ShadowUniform::disabled();
    }
    let sun = normalize([lights.sun_dir[0], lights.sun_dir[1], lights.sun_dir[2]]);
    let up_guess = if sun[1].abs() > 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    // A fixed light view through the origin, looking along the light.
    let light_view = look_at([0.0; 3], [-sun[0], -sun[1], -sun[2]], up_guess);
    let far = (shadow.extent * 4.0).min(cam.far).max(cam.near * 2.0);
    let splits = cascade_splits(cam.near, far);

    let mut light_space_matrices = [[[0.0; 4]; 4]; CASCADE_COUNT];
    let mut cascade_params = [[0.0; 4]; CASCADE_COUNT];
    for i in 0..CASCADE_COUNT {
        // The slice's eight corners, their centroid, and the sphere
        // around them.
        let mut corners = Vec::with_capacity(8);
        for d in [splits[i], splits[i + 1]] {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                corners.push([
                    cam.eye[0] + cam.forward[0] * d + cam.right[0] * sx * cam.tan_x * d + cam.up[0] * sy * cam.tan_y * d,
                    cam.eye[1] + cam.forward[1] * d + cam.right[1] * sx * cam.tan_x * d + cam.up[1] * sy * cam.tan_y * d,
                    cam.eye[2] + cam.forward[2] * d + cam.right[2] * sx * cam.tan_x * d + cam.up[2] * sy * cam.tan_y * d,
                ]);
            }
        }
        let mut center = [0.0f32; 3];
        for c in &corners {
            for k in 0..3 {
                center[k] += c[k] / 8.0;
            }
        }
        let mut radius = corners
            .iter()
            .map(|c| {
                let d = sub(*c, center);
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .fold(0.0f32, f32::max);
        // Quantise the radius so the texel size only changes in steps.
        radius = (radius * 16.0).ceil() / 16.0;
        let texel = 2.0 * radius / SHADOW_MAP_SIZE as f32;
        let c = mat4_transform_point(light_view, center);
        let (cx, cy) = ((c[0] / texel).floor() * texel, (c[1] / texel).floor() * texel);
        let dist = -c[2];
        let pull = radius + shadow.extent;
        let proj = ortho(
            cx - radius,
            cx + radius,
            cy - radius,
            cy + radius,
            dist - radius - pull,
            dist + radius,
        );
        light_space_matrices[i] = mul(proj, light_view);
        cascade_params[i] = [2.0 * radius, 2.0 * radius + pull, 0.0, 0.0];
    }
    ShadowUniform {
        light_space_matrices,
        split_distances: [splits[1], splits[2], splits[3], 0.0],
        flags: [0.0, 0.0, 0.0, 1.0],
        cascade_params,
    }
}

/// web3d-M7: the view-projection of face `face` of a point light's
/// shadow cube, reaching `far` (the light's radius). Each face camera's
/// (right, up, forward) is the standard cube-map layout that samplers
/// use (the same table `environment.rs` renders with), so a direction
/// sampled from the cube lands on the texel this camera drew.
fn point_face_view_proj(light: [f32; 3], face: usize, far: f32) -> [[f32; 4]; 4] {
    let (r, u, f): ([f32; 3], [f32; 3], [f32; 3]) = match face {
        0 => ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        1 => ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]),
        2 => ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        3 => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]),
        4 => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        _ => ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]),
    };
    let dot_light = |a: [f32; 3]| a[0] * light[0] + a[1] * light[1] + a[2] * light[2];
    // Rows (right, up, -forward), translated to the light: the camera
    // looks down its -z like every other view in this file.
    let view = [
        [r[0], u[0], -f[0], 0.0],
        [r[1], u[1], -f[1], 0.0],
        [r[2], u[2], -f[2], 0.0],
        [-dot_light(r), -dot_light(u), dot_light(f), 1.0],
    ];
    mul(
        perspective(std::f32::consts::FRAC_PI_2, 1.0, POINT_SHADOW_NEAR, far),
        view,
    )
}

/// Phase 26: extract the 6 frustum planes from a column-major
/// view-projection matrix. Each plane is (a, b, c, d) such that
/// a point (x, y, z) is *inside* when a*x + b*y + c*z + d ≥ 0.
/// Planes are normalized so the dot product is a signed distance.
///
/// Algorithm (Gribb-Hartmann): combine rows of `vp` to get
/// the planes. With column-major storage, accessing row r column c
/// is `vp[c][r]`.
fn extract_frustum_planes(vp: [[f32; 4]; 4]) -> [[f32; 4]; 6] {
    // Row vectors, indexed [c][r]:
    let row = |r: usize| [vp[0][r], vp[1][r], vp[2][r], vp[3][r]];
    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);
    // wgpu's NDC z is [0, 1]; near plane = row3 + row2 (z ≥ 0),
    // far = row3 - row2 (z ≤ 1).
    let raw = [
        [r3[0] + r0[0], r3[1] + r0[1], r3[2] + r0[2], r3[3] + r0[3]], // left
        [r3[0] - r0[0], r3[1] - r0[1], r3[2] - r0[2], r3[3] - r0[3]], // right
        [r3[0] + r1[0], r3[1] + r1[1], r3[2] + r1[2], r3[3] + r1[3]], // bottom
        [r3[0] - r1[0], r3[1] - r1[1], r3[2] - r1[2], r3[3] - r1[3]], // top
        [r2[0], r2[1], r2[2], r2[3]],                                 // near
        [r3[0] - r2[0], r3[1] - r2[1], r3[2] - r2[2], r3[3] - r2[3]], // far
    ];
    let mut out = [[0.0; 4]; 6];
    for (i, p) in raw.iter().enumerate() {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        if len > 1e-6 {
            out[i] = [p[0] / len, p[1] / len, p[2] / len, p[3] / len];
        } else {
            out[i] = *p;
        }
    }
    out
}

/// Phase 25: orthographic projection — column-major, matches WGSL
/// expectations. NDC z is [0, 1] for wgpu (D3D-style), so the
/// near→far range maps to that.
fn ortho(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let rl = right - left;
    let tb = top - bottom;
    let fnz = far - near;
    [
        [2.0 / rl, 0.0, 0.0, 0.0],
        [0.0, 2.0 / tb, 0.0, 0.0],
        [0.0, 0.0, -1.0 / fnz, 0.0],
        [-(right + left) / rl, -(top + bottom) / tb, -near / fnz, 1.0],
    ]
}

/// Phase 24: apply one animation clip's channels onto a TRS table
/// at time `t_clip`. Mutates the targeted nodes in place; nodes
/// not referenced by any channel keep their rest-pose values.
fn apply_clip(trs: &mut [([f32; 3], [f32; 4], [f32; 3])], clip: &AnimClip, t_clip: f32) {
    for ch in &clip.channels {
        if ch.target_node >= trs.len() {
            continue;
        }
        match (sample_channel(ch, t_clip), ch.property) {
            (SampledValue::Vec3(v), AnimProperty::Translation) => {
                trs[ch.target_node].0 = v;
            }
            (SampledValue::Vec3(v), AnimProperty::Scale) => {
                trs[ch.target_node].2 = v;
            }
            (SampledValue::Vec4(v), AnimProperty::Rotation) => {
                trs[ch.target_node].1 = v;
            }
            _ => {}
        }
    }
}

/// Recursively walk a glTF node's subtree, accumulate transforms,
/// and bake each primitive's vertices into the flattened output.
/// Phase 24: pass-through skin awareness — when a node has a
/// `node.skin()` reference, its primitives are flagged skinned so
/// `flatten_primitive` skips world-baking (the skin pass at render
/// time will resolve them via joint matrices).
fn flatten_node(
    node: &gltf::Node<'_>,
    parent_transform: [[f32; 4]; 4],
    buffers: &[gltf::buffer::Data],
    out: &mut GlbData,
) {
    let local = node.transform().matrix();
    let world = mat4_mul(parent_transform, local);
    let is_skinned = node.skin().is_some();
    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            flatten_primitive(&primitive, world, is_skinned, buffers, out);
        }
    }
    for child in node.children() {
        flatten_node(&child, world, buffers, out);
    }
}

/// Append one glTF primitive's vertices + indices to the flattened
/// output, transformed by `world` (or left in mesh-local space when
/// skinned). Index values are offset by the existing vertex count
/// so multiple primitives share one buffer.
/// Append one glTF primitive to the flattened model as a submesh with
/// its material: vertices transformed by `world` (or left in mesh-local
/// space when skinned), indices offset by the existing vertex count so
/// every primitive shares one buffer. Non-triangle primitives (points,
/// lines) are skipped.
fn flatten_primitive(
    primitive: &gltf::Primitive<'_>,
    world: [[f32; 4]; 4],
    is_skinned: bool,
    buffers: &[gltf::buffer::Data],
    out: &mut GlbData,
) {
    if primitive.mode() != gltf::mesh::Mode::Triangles {
        return;
    }
    let reader = primitive.reader(|b| Some(&buffers[b.index()]));
    let positions: Vec<[f32; 3]> = match reader.read_positions() {
        Some(p) => p.collect(),
        None => return,
    };
    if positions.is_empty() {
        return;
    }
    let count = positions.len();
    // An optional attribute, or `default` for every vertex when it is
    // absent or has the wrong length.
    fn or_default<T: Clone>(v: Option<Vec<T>>, count: usize, default: T) -> Vec<T> {
        match v {
            Some(v) if v.len() == count => v,
            _ => vec![default; count],
        }
    }
    let uvs = or_default(
        reader.read_tex_coords(0).map(|i| i.into_f32().collect()),
        count,
        [0.0, 0.0],
    );
    let uvs1 = or_default(
        reader.read_tex_coords(1).map(|i| i.into_f32().collect()),
        count,
        [0.0, 0.0],
    );
    let tangents = or_default(reader.read_tangents().map(|i| i.collect()), count, NO_TANGENT);
    let colors = or_default(
        reader.read_colors(0).map(|i| i.into_rgba_f32().collect()),
        count,
        WHITE,
    );
    // Phase 24: JOINTS_0 / WEIGHTS_0, or the identity skin.
    let joints = or_default(
        reader.read_joints(0).map(|i| i.into_u16().collect()),
        count,
        UNSKINNED_J,
    );
    let weights = or_default(
        reader.read_weights(0).map(|i| i.into_f32().collect()),
        count,
        UNSKINNED_W,
    );
    let mut indices: Vec<u32> = match reader.read_indices() {
        Some(idx) => idx.into_u32().collect(),
        None => (0..count as u32).collect(),
    };
    indices.truncate(indices.len() / 3 * 3);

    // web3d-M7: glTF says a primitive without normals is shaded with flat
    // normals. Give every triangle corner its own vertex carrying the
    // face normal (the loader used to point them all straight up, which
    // lit every such surface like a floor).
    let normals: Vec<[f32; 3]> = match reader.read_normals() {
        Some(n) => n.collect(),
        None => Vec::new(),
    };
    let (order, normals): (Vec<usize>, Vec<[f32; 3]>) = if normals.len() == count {
        ((0..count).collect(), normals)
    } else {
        let mut flat = Vec::with_capacity(indices.len());
        for tri in indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| positions[tri[k] as usize]);
            let n = normalize(cross(sub(b, a), sub(c, a)));
            flat.extend([n, n, n]);
        }
        let order = indices.iter().map(|&i| i as usize).collect();
        indices = (0..flat.len() as u32).collect();
        (order, flat)
    };

    // Phase 24: per glTF 2.0, a skinned mesh's node transform is not
    // applied (the joints carry it), so skinned primitives stay in
    // mesh-local space; unskinned ones bake the world transform.
    let base_index = out.vertices.len() as u32;
    for (k, &i) in order.iter().enumerate() {
        let (position, normal, tangent) = if is_skinned {
            (positions[i], normals[k], tangents[i])
        } else {
            let t = tangents[i];
            let tt = mat4_transform_dir(world, [t[0], t[1], t[2]]);
            (
                mat4_transform_point(world, positions[i]),
                mat4_transform_dir(world, normals[k]),
                [tt[0], tt[1], tt[2], t[3]],
            )
        };
        out.vertices.push(Vertex {
            position,
            normal,
            uv: uvs[i],
            joints: joints[i],
            weights: weights[i],
            uv1: uvs1[i],
            tangent,
            color: colors[i],
        });
    }

    let first = out.indices.len() as u32;
    out.indices.extend(indices.iter().map(|i| base_index + i));
    // web3d-M7: the primitive's material; glTF's default (the last
    // entry) when it names none.
    let material = primitive
        .material()
        .index()
        .unwrap_or(out.materials.len() - 1);
    out.submeshes.push(Submesh {
        first,
        count: out.indices.len() as u32 - first,
        material,
    });
}

/// Phase 28 session 5: GPU-upload portion of mesh load, split out
/// from the old synchronous mesh loader so the disk I/O + glb parse half can
/// run on a background worker thread. Takes pre-parsed CPU data
/// (a `LoadedGlb` from `load_glb`) and turns it into a renderable
/// `GpuMesh`. All wgpu calls happen on the calling thread, which
/// is the main render thread.
fn upload_loaded_glb(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    kit: &mut MaterialKit,
    joints_bgl: &wgpu::BindGroupLayout,
    loaded: LoadedGlb,
) -> GpuMesh {
    let GlbData {
        vertices,
        indices,
        submeshes,
        materials,
        images,
        skin: skin_data,
    } = *loaded.0;
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d mesh vertices"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d mesh indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    // web3d-M7: upload each image once per encoding it is used with
    // (colour slots sRGB, data slots linear), then a bind group per
    // material.
    let mut views: HashMap<(usize, bool), wgpu::TextureView> = HashMap::new();
    for m in &materials {
        for slot in &m.slots {
            let Some(slot) = slot else { continue };
            let key = (slot.image, slot.srgb);
            if views.contains_key(&key) {
                continue;
            }
            if let Some(img) = images.get(slot.image) {
                let t = upload_rgba(
                    device,
                    queue,
                    "twe-kernel glb texture",
                    &img.rgba,
                    img.width,
                    img.height,
                    key.1,
                );
                views.insert(key, t.create_view(&wgpu::TextureViewDescriptor::default()));
            }
        }
    }
    let bind_groups: Vec<wgpu::BindGroup> = materials
        .iter()
        .map(|m| {
            let mut slots = [None; SLOTS];
            for (i, slot) in m.slots.iter().enumerate() {
                slots[i] = slot.and_then(|s| views.get(&(s.image, s.srgb)));
            }
            kit.bind_group(device, m, slots)
        })
        .collect();
    let submeshes = submeshes
        .iter()
        .map(|sub| GpuSubmesh {
            first: sub.first,
            count: sub.count,
            material: bind_groups[sub.material].clone(),
            double_sided: materials[sub.material].double_sided,
            masked: materials[sub.material].alpha_mode == crate::kernel::material::AlphaMode::Mask,
            blend: materials[sub.material].alpha_mode == crate::kernel::material::AlphaMode::Blend,
            centroid: submesh_centroid(&vertices, &indices[sub.first as usize..(sub.first + sub.count) as usize]),
            transmissive: materials[sub.material].ext.transmission > 0.0,
        })
        .collect();
    // Phase 24: build per-mesh skin GPU resources when the glb
    // has a skinned mesh. Each skinned mesh owns its joint UBO,
    // updated each frame from the script-driven `mesh_anim`.
    let skin = skin_data.map(|sd| {
        let joint_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twec-play3d mesh joints"),
            size: std::mem::size_of::<JointsUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Initialize to identity so the first frame (before
        // animation runs) draws the rest pose correctly.
        queue.write_buffer(
            &joint_buffer,
            0,
            bytemuck::bytes_of(&JointsUniform::identity()),
        );
        let joint_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twec-play3d mesh joints bg"),
            layout: joints_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: joint_buffer.as_entire_binding(),
            }],
        });
        MeshSkin {
            joint_node_indices: sd.joint_node_indices,
            inverse_bind_matrices: sd.inverse_bind_matrices,
            nodes: sd.nodes,
            scene_roots: sd.scene_roots,
            clips: sd.clips,
            joint_buffer,
            joint_bind_group,
        }
    });
    // Phase 26: bound radius for frustum culling. For skinned
    // meshes, multiply by 1.5 to leave headroom for the joints
    // moving the surface beyond the rest-pose bound (e.g. an
    // outstretched arm).
    let mut max_sq: f32 = 0.0;
    for v in &vertices {
        let p = v.position;
        let d = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        if d > max_sq {
            max_sq = d;
        }
    }
    let mut bound_radius = max_sq.sqrt();
    if skin.is_some() {
        bound_radius *= 1.5;
    }
    if bound_radius < 0.001 {
        bound_radius = 0.5; // fallback for empty / degenerate meshes
    }

    GpuMesh {
        bound_radius,
        vertex_buffer,
        index_buffer,
        index_count: indices.len() as u32,
        index_format: wgpu::IndexFormat::Uint32,
        submeshes,
        skin,
    }
}

// ---------- Per-frame render ----------

impl Renderer {
    /// Draw one frame. `snap` is everything the frame needs from the
    /// host (camera, lights, draw list, post-FX, id -> path tables);
    /// `assets` loads meshes / textures referenced for the first time
    /// (native: worker threads + filesystem / bundle; web: `fetch`).
    pub fn render(
        &mut self,
        snap: &RenderSnapshot,
        assets: &mut dyn AssetSource,
    ) -> Result<(), String> {
        let state = self;
        let Camera3d {
            eye,
            target,
            up,
            fov_y,
            near,
            far,
        } = snap.camera;
        let aspect = state.config.width as f32 / state.config.height.max(1) as f32;
        let proj = perspective(fov_y, aspect, near, far);
        let view = look_at(eye, target, up);
        // web3d-M7: with TAA the projection is jittered by a sub-pixel
        // offset each frame; the resolve reprojects with the unjittered
        // matrices.
        let taa_on = snap.post.taa;
        // web3d-M7: screen-space reflections need the main passes to
        // write the surface record, through the pipelines that do.
        let ssr_on = snap.post.ssr > 0.0;
        state.ssr_frame.set(ssr_on);
        if ssr_on && state.lit_ssr.is_none() {
            state.lit_ssr = Some(LitPipelines::new(&state.device, &state.pipeline_layout, &state.main_shader, true));
        }
        let unjittered_view_proj = mul(proj, view);
        let view_proj = if taa_on {
            let j = state.taa.jitter(state.config.width, state.config.height);
            mul(crate::kernel::taa::jitter_projection(proj, j), view)
        } else {
            state.taa.reset();
            unjittered_view_proj
        };
        let camera_uniform = CameraUniform {
            view_proj,
            time: [snap.time, 0.0, 0.0, 0.0],
            eye: [eye[0], eye[1], eye[2], 1.0],
            inv_view_proj: invert4(view_proj),
        };
        state
            .queue
            .write_buffer(&state.camera_buffer, 0, bytemuck::bytes_of(&camera_uniform));
        // web3d-M7: the frame's particles, simulated and drawn with the
        // (jittered) camera the depth buffer is rendered with.
        let particles_on = state.particles.prepare(
            &state.device,
            &state.queue,
            &snap.particles,
            &crate::kernel::particles::ParticleCamera {
                view_proj,
                inv_view_proj: camera_uniform.inv_view_proj,
                eye,
                right: [view[0][0], view[1][0], view[2][0]],
                up: [view[0][1], view[1][1], view[2][1]],
            },
            (state.config.width, state.config.height),
            snap.time,
        );

        let lights_uniform = snap.lights;
        // web3d-M7: the frame's point and spot lights, with the shadow
        // budget: the POINT_SHADOW_LIGHTS shadow-casting lights nearest
        // the camera get a shadow cube (pos.w = cube + 1), the rest light
        // without shadows.
        let mut point_lights: Vec<PointLightU> = snap.point_lights.iter().take(MAX_LIGHTS).copied().collect();
        let mut wanting: Vec<usize> = Vec::new();
        for (i, pl) in point_lights.iter_mut().enumerate() {
            if pl.pos[3] > 0.5 && pl.color_radius[3] > 0.0 {
                wanting.push(i);
            }
            pl.pos[3] = 0.0;
        }
        let reach = |pl: &PointLightU| {
            let d = sub([pl.pos[0], pl.pos[1], pl.pos[2]], eye);
            dot(d, d).sqrt() - pl.color_radius[3]
        };
        wanting.sort_by(|&a, &b| reach(&point_lights[a]).total_cmp(&reach(&point_lights[b])));
        let mut point_shadow_lights: Vec<([f32; 3], f32)> = Vec::new();
        for &i in wanting.iter().take(POINT_SHADOW_LIGHTS) {
            let pl = &mut point_lights[i];
            point_shadow_lights.push(([pl.pos[0], pl.pos[1], pl.pos[2]], pl.color_radius[3]));
            pl.pos[3] = point_shadow_lights.len() as f32;
        }
        let light_count = point_lights.len() as u32;
        state.clusters.prepare(
            &state.queue,
            &point_lights,
            &crate::kernel::clusters::ClusterUniform::new(
                view,
                invert4(proj),
                near,
                far,
                (state.config.width, state.config.height),
                light_count,
            ),
        );
        for (light, (pos, radius)) in point_shadow_lights.iter().enumerate() {
            for face in 0..6 {
                let pass_uniform = ShadowPassUniform::new(point_face_view_proj(*pos, face, *radius), snap.time);
                state.queue.write_buffer(
                    &state.point_shadows.pass_buffers[light * 6 + face],
                    0,
                    bytemuck::bytes_of(&pass_uniform),
                );
            }
        }
        state
            .queue
            .write_buffer(&state.lights_buffer, 0, bytemuck::bytes_of(&lights_uniform));

        // web3d-M7: the colour-grading LUT — ask for a new one.
        let wanted_lut = snap.lut.map(|l| l.path.to_string());
        if wanted_lut.is_some() && wanted_lut != state.lut.requested {
            state.lut.requested = wanted_lut.clone();
            if let Some(path) = &wanted_lut {
                assets.request(AssetKind::Lut, 0, path);
            }
        }

        // web3d-M7: the frame's environment — ask for a new one, adopt
        // one that finished loading, and describe it to the shaders.
        let wanted = snap.environment.map(|e| e.path.to_string());
        if wanted.is_some() && wanted != state.env.requested {
            state.env.requested = wanted.clone();
            if let Some(path) = &wanted {
                assets.request(AssetKind::Environment, 0, path);
            }
        }
        let env_ready = snap.environment.is_some() && state.env.loaded == state.env.requested;
        let env_uniform = match snap.environment {
            Some(e) if env_ready => EnvUniform {
                sh: state.env.current.sh,
                params: [
                    e.intensity,
                    (crate::kernel::environment::SPECULAR_MIPS - 1) as f32,
                    1.0,
                    if e.backdrop { 1.0 } else { 0.0 },
                ],
            },
            _ => EnvUniform::none(),
        };
        state
            .queue
            .write_buffer(&state.env.uniform, 0, bytemuck::bytes_of(&env_uniform));

        let forward = normalize(sub(target, eye));
        let right = normalize(cross(forward, up));
        let cascade_camera = CascadeCamera {
            eye,
            forward,
            right,
            up: cross(right, forward),
            tan_y: (fov_y * 0.5).tan(),
            tan_x: (fov_y * 0.5).tan() * aspect,
            near,
            far,
        };
        let shadow_uniform = compute_shadow_uniform(&lights_uniform, &cascade_camera, snap.shadow);
        state
            .queue
            .write_buffer(&state.shadow_buffer, 0, bytemuck::bytes_of(&shadow_uniform));

        // web3d-M7 follow-up: the draws, or (when the host sent this
        // generation without them) the ones kept from it.
        let retained = state.retained.take();
        let kept = matches!(
            (snap.draws_generation, &retained),
            (Some(g), Some(r)) if snap.draws.is_empty() && r.generation == g
        );
        let kept_draws = retained.as_ref().filter(|_| kept).map(|r| r.draws.clone());
        let queue: &[DrawCall3d] = match &kept_draws {
            Some(d) => d,
            None => snap.draws,
        };
        let mut instances: Vec<Instance> = Vec::new();
        let cap = usize::MAX;

        // Asset requests: first sight of a mesh / texture id asks the
        // host's `AssetSource` to load it; finished loads are uploaded
        // on this thread. A mesh draws from the frame its upload lands.
        // (Kept draws were seen when they were sent: skip the walk.)
        for d in queue.iter().filter(|_| !kept) {
            if let Primitive::Mesh(id) = d.primitive {
                if !state.mesh_cache.contains_key(&id)
                    && !state.mesh_load_failures.contains(&id)
                    && state.mesh_pending.insert(id)
                {
                    match snap.mesh_paths.get(id as usize) {
                        Some(path) => assets.request(AssetKind::Mesh, id, path),
                        None => {
                            state.mesh_pending.remove(&id);
                            state.mesh_load_failures.insert(id);
                        }
                    }
                }
            }
            let tex = d.texture;
            if tex != 0
                && !state.texture_cache.contains_key(&tex)
                && !state.texture_load_failures.contains(&tex)
                && state.texture_pending.insert(tex)
            {
                match snap.texture_paths.get(tex as usize) {
                    Some(path) => assets.request(AssetKind::Texture, tex, path),
                    None => {
                        state.texture_pending.remove(&tex);
                        state.texture_load_failures.insert(tex);
                    }
                }
            }
        }
        for ready in assets.poll() {
            match ready {
                AssetReady::Mesh(id, result) => {
                    state.mesh_pending.remove(&id);
                    match result {
                        Ok(loaded) => {
                            let gpu_mesh = upload_loaded_glb(
                                &state.device,
                                &state.queue,
                                &mut state.materials,
                                &state.joints_bgl,
                                loaded,
                            );
                            state.mesh_cache.insert(id, gpu_mesh);
                        }
                        Err(e) => {
                            log_error(&format!("mesh load: {e}"));
                            state.mesh_load_failures.insert(id);
                        }
                    }
                }
                AssetReady::Lut(_, result) => {
                    let parsed = result.and_then(|bytes| {
                        let text = String::from_utf8(bytes).map_err(|_| "a .cube file is text".to_string())?;
                        crate::kernel::post::parse_cube(&text)
                    });
                    match parsed {
                        Ok(lut) => {
                            state.lut.view = crate::kernel::post::upload_lut(&state.device, &state.queue, &lut);
                            state.lut.size = lut.size;
                            state.lut.loaded = state.lut.requested.clone();
                            state.lut.generation += 1;
                        }
                        Err(e) => log_error(&format!("colour LUT `{}`: {e}", state.lut.requested.as_deref().unwrap_or(""))),
                    }
                }
                AssetReady::Environment(_, result) => {
                    let built = result.and_then(|bytes| {
                        crate::kernel::environment::decode_hdr(&bytes)
                            .map(|img| crate::kernel::environment::build(&state.device, &state.queue, &img))
                    });
                    match built {
                        Ok(env) => {
                            state.env.current = env;
                            state.env.loaded = state.env.requested.clone();
                            // Built without AO; the frame re-binds it below.
                            state.frame_bind_group = frame_bind_group(
                                &state.device,
                                &state.frame_bgl,
                                &FrameBuffers {
                                    camera: &state.camera_buffer,
                                    lights: &state.lights_buffer,
                                    fog: &state.fog_buffer,
                                    clusters: &state.clusters,
                                },
                                &state.env,
                                [state.ao.view(false), state.transmission.view(false)],
                            );
                            state.frame_ao_key = (0, 0);
                        }
                        Err(e) => log_error(&format!("environment load: {e}")),
                    }
                }
                AssetReady::Texture(id, result) => {
                    state.texture_pending.remove(&id);
                    let uploaded = result.and_then(|bytes| {
                        let view = upload_texture_bytes(&state.device, &state.queue, &bytes)?;
                        let mut slots = [None; SLOTS];
                        slots[BASE] = Some(&view);
                        Ok(state
                            .materials
                            .bind_group(&state.device, &MaterialData::plain(), slots))
                    });
                    match uploaded {
                        Ok(bg) => {
                            state.texture_cache.insert(id, bg);
                        }
                        Err(e) => {
                            log_error(&format!("texture load: {e}"));
                            state.texture_load_failures.insert(id);
                        }
                    }
                }
            }
        }

        let assets_key = (state.mesh_cache.len(), state.mesh_load_failures.len(), state.texture_cache.len());
        // Reuse outright: same generation, nothing new loaded.
        let reuse = retained.as_ref().filter(|r| kept && r.assets == assets_key);
        let (cube_ranges, sphere_ranges, mesh_ranges, cull_groups, opaque_draws, opaque_count, transparent) =
            if let Some(r) = reuse {
                (
                    r.cube_ranges.clone(),
                    r.sphere_ranges.clone(),
                    r.mesh_ranges.clone(),
                    r.cull_groups.clone(),
                    r.opaque_draws.clone(),
                    r.opaque_count,
                    Vec::new(),
                )
            } else {
        instances.reserve(queue.len());
        // Phase 17 session 3: group draws by (primitive, texture_id).
        // Each unique combination becomes its own instanced draw call
        // because group 1's bind group changes between textures.
        // Within a group, instance order = queue order (preserves any
        // back-to-front ordering the script established).
        // web3d-M3: the material joins the key — a material draws with
        // its own pipeline.
        let mut cube_groups: Vec<(SurfaceKey, Vec<&DrawCall3d>)> = Vec::new();
        let mut sphere_groups: Vec<(SurfaceKey, Vec<&DrawCall3d>)> = Vec::new();
        let mut mesh_groups: Vec<(MeshKey, Vec<&DrawCall3d>)> = Vec::new();
        // web3d-M7: a tint with alpha below 1 is translucent (drawn in
        // the sorted transparent pass). `visual` materials stay opaque.
        let translucent = |d: &DrawCall3d| d.color[3] < 0.999 && d.material == 0;
        for d in queue.iter().filter(|d| !translucent(d)) {
            match d.primitive {
                Primitive::Cube => {
                    let key = (d.texture, d.material);
                    match cube_groups.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, list)) => list.push(d),
                        None => cube_groups.push((key, vec![d])),
                    }
                }
                Primitive::Sphere => {
                    let key = (d.texture, d.material);
                    match sphere_groups.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, list)) => list.push(d),
                        None => sphere_groups.push((key, vec![d])),
                    }
                }
                Primitive::Mesh(id) => {
                    if !state.mesh_cache.contains_key(&id) {
                        continue;
                    }
                    let key = (id, d.texture, d.material);
                    match mesh_groups.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, list)) => list.push(d),
                        None => mesh_groups.push((key, vec![d])),
                    }
                }
            }
        }

        // Phase 26: extract frustum planes from the camera view-proj
        // for per-instance sphere culling. `frustum_culling_enabled`
        // is a script-controlled toggle (default on); disable for
        // benchmarking the cull path's contribution.
        // web3d-M7: culling moved to the GPU (kernel/gpu_cull.rs); every
        // instance is uploaded, colours still sRGB (the vertex shader
        // decodes them).
        let push_group =
            |group: &[&DrawCall3d], out: &mut Vec<Instance>, _mesh_radius: f32| -> (u32, u32) {
                let start = out.len() as u32;
                for d in group {
                    if out.len() >= cap {
                        break;
                    }
                    let (s, c) = d.yaw.sin_cos();
                    out.push(Instance {
                        position: d.at,
                        size: d.size,
                        color: d.color,
                        rot: [s, c, 0.0, 0.0],
                    });
                }
                let end = out.len() as u32;
                (start, end)
            };
        // Cube has corner-distance √3/2 ≈ 0.866 in mesh-local space.
        let cube_radius = 0.8660254;
        // Sphere has radius 0.5 in mesh-local space.
        let sphere_radius = 0.5;
        let cube_ranges: Vec<(SurfaceKey, InstanceRange)> = cube_groups
            .iter()
            .map(|(k, list)| (*k, push_group(list, &mut instances, cube_radius)))
            .collect();
        let sphere_ranges: Vec<(SurfaceKey, InstanceRange)> = sphere_groups
            .iter()
            .map(|(k, list)| (*k, push_group(list, &mut instances, sphere_radius)))
            .collect();
        let mesh_ranges: Vec<(MeshKey, InstanceRange)> = mesh_groups
            .iter()
            .map(|(k, list)| {
                let r = state
                    .mesh_cache
                    .get(&k.0)
                    .map(|m| m.bound_radius)
                    .unwrap_or(1.0);
                (*k, push_group(list, &mut instances, r))
            })
            .collect();
        // web3d-M7: the opaque draw list. Every group of instances (cube /
        // sphere / mesh with a texture and material) is a cull group; each
        // draw (a shape, a mesh, or one glTF primitive of a mesh) draws one
        // group's instances, directly or through the GPU cull.
        let mut cull_groups: Vec<CullGroup> = Vec::new();
        let mut opaque_draws: Vec<OpaqueDraw> = Vec::new();
        let mut add_group = |range: InstanceRange, radius: f32, instances: &mut [Instance]| -> u32 {
            let g = cull_groups.len() as u32;
            for inst in &mut instances[range.0 as usize..range.1 as usize] {
                inst.rot[2] = g as f32;
            }
            cull_groups.push(CullGroup {
                radius,
                base: range.0,
                _pad: [0; 2],
            });
            g
        };
        for ((tex, mat), range) in &cube_ranges {
            let group = add_group(*range, cube_radius, &mut instances);
            opaque_draws.push(OpaqueDraw {
                shape: OpaqueShape::Cube,
                surface: OpaqueSurface::Script { tex: *tex, mat: *mat },
                group,
                range: *range,
                indices: 0..state.cube_index_count,
            });
        }
        for ((tex, mat), range) in &sphere_ranges {
            let group = add_group(*range, sphere_radius, &mut instances);
            opaque_draws.push(OpaqueDraw {
                shape: OpaqueShape::Sphere,
                surface: OpaqueSurface::Script { tex: *tex, mat: *mat },
                group,
                range: *range,
                indices: 0..state.sphere_index_count,
            });
        }
        for ((id, tex, mat), range) in &mesh_ranges {
            let Some(mesh) = state.mesh_cache.get(id) else { continue };
            let group = add_group(*range, mesh.bound_radius, &mut instances);
            // Skinned meshes' instances blend joints (rot.w); nothing
            // else pays for it.
            if mesh.skin.is_some() {
                for inst in &mut instances[range.0 as usize..range.1 as usize] {
                    inst.rot[3] = 1.0;
                }
            }
            if *tex != 0 || *mat != 0 {
                // A script texture or a `visual` material covers the whole
                // mesh as one surface.
                opaque_draws.push(OpaqueDraw {
                    shape: OpaqueShape::Mesh(*id),
                    surface: OpaqueSurface::Script { tex: *tex, mat: *mat },
                    group,
                    range: *range,
                    indices: 0..mesh.index_count,
                });
            } else {
                // Each glTF primitive with its own material.
                for (si, sub) in mesh.submeshes.iter().enumerate().filter(|(_, s)| !s.sorted()) {
                    opaque_draws.push(OpaqueDraw {
                        shape: OpaqueShape::Mesh(*id),
                        surface: OpaqueSurface::Submesh(si),
                        group,
                        range: *range,
                        indices: sub.first..sub.first + sub.count,
                    });
                }
            }
        }
        let opaque_count = cull_groups.last().map_or(0, |g| {
            let last = opaque_draws.iter().rev().find(|d| d.group == cull_groups.len() as u32 - 1);
            last.map_or(g.base, |d| d.range.1)
        });

        // web3d-M7: the transparent pass's draws — translucent draws, one
        // instance each, and the blended glTF primitives of opaque mesh
        // instances — sorted back to front by distance to the eye.
        let mut transparent: Vec<TransparentDraw> = Vec::new();
        let dist2 = |p: [f32; 3]| {
            let v = sub(p, eye);
            dot(v, v)
        };
        let world_point = |inst: &Instance, local: [f32; 3]| {
            let (s, c) = (inst.rot[0], inst.rot[1]);
            [
                inst.position[0] + (c * local[0] + s * local[2]) * inst.size,
                inst.position[1] + local[1] * inst.size,
                inst.position[2] + (-s * local[0] + c * local[2]) * inst.size,
            ]
        };
        for ((id, tex, mat), range) in &mesh_ranges {
            if *tex != 0 || *mat != 0 {
                continue;
            }
            let Some(mesh) = state.mesh_cache.get(id) else { continue };
            for (si, sub) in mesh.submeshes.iter().enumerate().filter(|(_, s)| s.sorted()) {
                for i in range.0..range.1 {
                    transparent.push(TransparentDraw {
                        shape: TransparentShape::Mesh { id: *id, tex: 0, sub: Some(si) },
                        instance: i,
                        depth: dist2(world_point(&instances[i as usize], sub.centroid)),
                        double_sided: sub.double_sided,
                        solid: sub.transmissive && !sub.blend,
                    });
                }
            }
        }
        for d in queue.iter().filter(|d| translucent(d)) {
            let (radius, shape) = match d.primitive {
                Primitive::Cube => (cube_radius, TransparentShape::Cube(d.texture)),
                Primitive::Sphere => (sphere_radius, TransparentShape::Sphere(d.texture)),
                Primitive::Mesh(id) => match state.mesh_cache.get(&id) {
                    Some(m) => (m.bound_radius, TransparentShape::Mesh { id, tex: d.texture, sub: None }),
                    None => continue,
                },
            };
            let (start, end) = push_group(&[d], &mut instances, radius);
            if end == start {
                continue;
            }
            if let Primitive::Mesh(id) = d.primitive {
                if state.mesh_cache.get(&id).is_some_and(|m| m.skin.is_some()) {
                    instances[start as usize].rot[3] = 1.0;
                }
            }
            let inst = instances[start as usize];
            match shape {
                // A glTF mesh without a script texture: each primitive
                // sorts on its own centroid.
                TransparentShape::Mesh { id, tex: 0, .. } => {
                    for (si, sub) in state.mesh_cache[&id].submeshes.iter().enumerate() {
                        transparent.push(TransparentDraw {
                            shape: TransparentShape::Mesh { id, tex: 0, sub: Some(si) },
                            instance: start,
                            depth: dist2(world_point(&inst, sub.centroid)),
                            double_sided: sub.double_sided,
                            solid: false,
                        });
                    }
                }
                shape => transparent.push(TransparentDraw {
                    shape,
                    instance: start,
                    depth: dist2(inst.position),
                    double_sided: false,
                    solid: false,
                }),
            }
        }
        transparent.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        (cube_ranges, sphere_ranges, mesh_ranges, cull_groups, opaque_draws, opaque_count, transparent)
            };
        let instance_count = match reuse {
            Some(r) => r.instance_count,
            None => instances.len(),
        };
        // web3d-M7: GPU culling (frustum + hierarchical-Z occlusion) with
        // `postfx.frustum_cull` on and enough opaque instances for it to
        // pay: the depth pyramid costs per pixel, not per object, so a
        // small scene draws faster without it (measured in
        // tests/render_bench.rs: 300 cubes, +1.1 ms on an integrated GPU).
        // web3d-M7 follow-up: and only while it rejects enough to pay
        // (`gpu_cull::CullProbe`).
        let gpu_cull = state
            .gpu_cull
            .should_cull(snap.post.frustum_cull && opaque_count >= GPU_CULL_MIN_INSTANCES);
        state.last_culled.set(gpu_cull);
        // web3d-M7: transmissive surfaces need the opaque scene as a
        // texture: the main pass splits around a copy of it.
        let transmission_on = transparent.iter().any(|d| match d.shape {
            TransparentShape::Mesh { id, sub: Some(si), .. } => state
                .mesh_cache
                .get(&id)
                .and_then(|m| m.submeshes.get(si))
                .is_some_and(|s| s.transmissive),
            _ => false,
        });

        // web3d-M7: height fog, with the sun's direction for its glow.
        let fog_uniform = match snap.fog {
            Some(f) if f.density > 0.0 => {
                let sun_len = (lights_uniform.sun_dir[0].powi(2)
                    + lights_uniform.sun_dir[1].powi(2)
                    + lights_uniform.sun_dir[2].powi(2))
                .sqrt()
                .max(1e-6);
                FogUniform {
                    params: [f.density, f.falloff.max(0.0), 0.0, 1.0],
                    color: [
                        srgb_to_linear(f.color[0]),
                        srgb_to_linear(f.color[1]),
                        srgb_to_linear(f.color[2]),
                        0.0,
                    ],
                    sun: [
                        lights_uniform.sun_dir[0] / sun_len,
                        lights_uniform.sun_dir[1] / sun_len,
                        lights_uniform.sun_dir[2] / sun_len,
                        0.5 * lights_uniform.sun_dir[3].max(0.0),
                    ],
                    background: [snap.background[0], snap.background[1], snap.background[2], far],
                }
            }
            _ => FogUniform::zeroed(),
        };
        // web3d-M7: volumetric fog replaces the closed-form fog.
        let volumetric_on = fog_uniform.params[3] > 0.5 && snap.fog.is_some_and(|f| f.volumetric);
        let mut fog_uniform = fog_uniform;
        if volumetric_on {
            state.volumetric.prepare(
                &state.queue,
                &crate::kernel::volumetric::VolumeFrame {
                    inv_view_proj: camera_uniform.inv_view_proj,
                    eye,
                    forward,
                    density: fog_uniform.params[0],
                    falloff: fog_uniform.params[1],
                    far,
                    color: [fog_uniform.color[0], fog_uniform.color[1], fog_uniform.color[2]],
                    glow: fog_uniform.sun[3],
                    screen: (state.config.width, state.config.height),
                },
            );
            fog_uniform.params[3] = 0.0;
        }
        if ssr_on {
            state.ssr.prepare(
                &state.queue,
                (state.config.width, state.config.height),
                &crate::kernel::ssr::SsrFrame {
                    view_proj,
                    inv_view_proj: camera_uniform.inv_view_proj,
                    eye,
                    strength: snap.post.ssr,
                    frame: state.frame_index.get(),
                    env: [env_uniform.params[0], env_uniform.params[1], env_uniform.params[2]],
                    ambient: [lights_uniform.ambient[0], lights_uniform.ambient[1], lights_uniform.ambient[2]],
                },
            );
        }
        let fog_on = fog_uniform.params[3] > 0.5;
        state
            .queue
            .write_buffer(&state.fog_buffer, 0, bytemuck::bytes_of(&fog_uniform));

        if instance_count != 0 {
            // Phase 23: grow the instance buffer if this frame needs
            // more instances than the current capacity. Doubling keeps
            // amortized growth cost O(1); the realloc is rare in
            // practice (only on first frame past 4096, then 8192, etc.).
            let needed = instances.len() as u64;
            if needed > state.instance_capacity {
                let mut new_cap = state.instance_capacity.max(1);
                while new_cap < needed {
                    new_cap *= 2;
                }
                state.instance_buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("twec-play3d instances (grown)"),
                    size: new_cap * std::mem::size_of::<Instance>() as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                state.instance_capacity = new_cap;
            }
            state
                .queue
                .write_buffer(&state.instance_buffer, 0, bytemuck::cast_slice(&instances));
        }
        // web3d-M7 follow-up: keep what was built for a generation the
        // host will repeat (not with transparent draws: they sort by the
        // camera each frame).
        let reused = reuse.is_some();
        state.retained = match (snap.draws_generation, retained) {
            (Some(_), Some(r)) if reused => Some(r),
            (Some(g), old) if transparent.is_empty() => Some(RetainedDraws {
                generation: g,
                assets: assets_key,
                draws: match (old, &kept_draws) {
                    (_, Some(d)) => d.clone(),
                    _ => queue.into(),
                },
                instance_count,
                cube_ranges: cube_ranges.clone(),
                sphere_ranges: sphere_ranges.clone(),
                mesh_ranges: mesh_ranges.clone(),
                cull_groups: cull_groups.clone(),
                opaque_draws: opaque_draws.clone(),
                opaque_count,
                materials: {
                    let mut m: Vec<u32> = queue.iter().map(|d| d.material).filter(|&m| m != 0).collect();
                    m.sort_unstable();
                    m.dedup();
                    m
                },
            }),
            _ => None,
        };

        // 4d. Phase 24: update each skinned mesh's joint UBO from
        //     the script-driven animation state. Walks every mesh
        //     referenced this frame; for each that has a skin, looks
        //     up the active clip in `mesh_anim` state, samples TRS
        //     for every joint at the current time, builds skin
        //     matrices, uploads to the per-mesh joint UBO. Unskinned
        //     meshes skip this work.
        let mesh_ids_this_frame: HashSet<u32> =
            mesh_ranges.iter().map(|((id, _, _), _)| *id).collect();
        for mesh_id in mesh_ids_this_frame {
            let gpu_mesh = match state.mesh_cache.get(&mesh_id) {
                Some(m) => m,
                None => continue,
            };
            let skin = match &gpu_mesh.skin {
                Some(s) => s,
                None => continue,
            };
            let anim_state = (snap.anim)(mesh_id);
            let joints_uniform = compute_skinned_joint_matrices(skin, &anim_state);
            state
                .queue
                .write_buffer(&skin.joint_buffer, 0, bytemuck::bytes_of(&joints_uniform));
        }

        // web3d-M3: a pipeline per material used this frame, built on
        // first use from the visual's WGSL. An unknown id draws as the
        // plain surface. web3d-M7 session 16: per material, not per
        // draw — the pipeline maps are keyed by the WGSL source, and
        // hashing it for each of 100k draws cost 90 ms a frame.
        // (Kept draws: the materials recorded with them.)
        let mut used: Vec<u32> = match state.retained.as_ref().filter(|_| kept) {
            Some(r) => r.materials.clone(),
            None => queue.iter().map(|d| d.material).filter(|&m| m != 0).collect(),
        };
        used.sort_unstable();
        used.dedup();
        for m in used {
            let Some(pixel) = snap.materials.get(m as usize).filter(|_| m != 0) else {
                continue;
            };
            let variant = usize::from(ssr_on);
            let displaces = material_displaces(pixel);
            if !state.material_pipelines[variant].contains_key(pixel) {
                let module = state
                    .device
                    .create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("twe-kernel material"),
                        source: wgpu::ShaderSource::Wgsl(material_shader_source(pixel).into()),
                    });
                let pipeline = surface_pipeline(
                    &state.device,
                    &state.pipeline_layout,
                    &module,
                    (if displaces { "vs_material" } else { "vs_main" }, "fs_material"),
                    false,
                    ssr_on,
                );
                state.material_pipelines[variant].insert(pixel.clone(), pipeline);
            }
            // web3d-M7: a displacing material's shadows and prepass
            // depth follow its displaced shape.
            if displaces && !state.displaced_depth.contains_key(pixel) {
                let depth = DisplacedDepth::new(&state.device, &state.depth_layout, pixel);
                state.displaced_depth.insert(pixel.clone(), depth);
            }
        }

        // 5. Acquire the target (swapchain frame, or the headless
        //    offscreen texture) and draw.
        let frame = match &state.surface {
            Some(surface) => match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t)
                | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
                // Nothing to draw into this frame (minimised, busy
                // compositor): skip it.
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    return Ok(());
                }
                // The surface went stale (resize race, device change):
                // reconfigure and draw next frame.
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    surface.configure(&state.device, &state.config);
                    return Ok(());
                }
                wgpu::CurrentSurfaceTexture::Validation => {
                    return Err("surface configuration failed validation".to_string());
                }
            },
            None => None,
        };
        let view_target = match (&frame, &state.offscreen) {
            (Some(f), _) => f.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(state.target_format),
                ..Default::default()
            }),
            (None, Some(t)) => t.create_view(&wgpu::TextureViewDescriptor::default()),
            (None, None) => return Err("renderer has no target".to_string()),
        };
        state.hud.prepare(
            &state.device,
            &state.queue,
            snap.hud,
            state.config.width,
            state.config.height,
        );
        let mut encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("twec-play3d encoder"),
            });
        // Phase 28 session 2: cascaded shadow passes — depth-only
        // renders from the sun's POV, one per cascade, each into a
        // separate layer of the shadow array texture. Each pass binds
        // a different per-cascade uniform (`shadow_pass_bgs[i]`) so
        // `vs_shadow` projects geometry into the right cascade's
        // light space. Runs `CASCADE_COUNT` times (3) per frame; cost
        // scales with the visible draw call count, not the geometry
        // count, since each call uses the same vertex buffers.
        // web3d-M7: the frame is a render graph (`kernel/graph.rs`).
        // Passes declare what they read and write; the graph orders
        // and culls them, and the HDR colour and depth targets come
        // from its pool (reallocated only on resize).
        let shadows_on = shadow_uniform.flags[3] > 0.5 && instance_count != 0;
        let mut graph = FrameGraph::new();
        let target = graph.import("target", true);
        let shadow_map = graph.import("shadow map", false);
        let point_shadow_map = graph.import("point shadow cube array", false);
        let hdr = graph.create(TextureDesc::new("hdr colour", Extent::FULL, HDR_FORMAT));
        // web3d-M7: the main pass draws into multisampled colour and
        // depth, and resolves the colour into `hdr`.
        let hdr_msaa = graph.create(TextureDesc {
            samples: MSAA_SAMPLES,
            ..TextureDesc::new("hdr colour (msaa)", Extent::FULL, HDR_FORMAT)
        });
        let depth = graph.create(TextureDesc {
            samples: MSAA_SAMPLES,
            ..TextureDesc::new("depth (msaa)", Extent::FULL, DEPTH_FORMAT)
        });
        // web3d-M7: with reflections, the main passes also write the
        // surface record (multisampled, resolved).
        let surface = ssr_on.then(|| {
            let msaa = graph.create(TextureDesc {
                samples: MSAA_SAMPLES,
                ..TextureDesc::new("surface (msaa)", Extent::FULL, SURFACE_FORMAT)
            });
            (msaa, graph.create(TextureDesc::new("surface", Extent::FULL, SURFACE_FORMAT)))
        });
        let mut main_writes = vec![(hdr_msaa, Access::Attach), (depth, Access::Attach), (hdr, Access::Attach)];
        if let Some((msaa, resolved)) = surface {
            main_writes.push((msaa, Access::Attach));
            main_writes.push((resolved, Access::Attach));
        }
        if shadows_on {
            for cascade in 0..CASCADE_COUNT {
                graph.add_pass(
                    FramePass::Shadow(cascade),
                    "shadow cascade",
                    &[],
                    &[(shadow_map, Access::Attach)],
                );
            }
        }
        for light in 0..point_shadow_lights.len() {
            for face in 0..6 {
                graph.add_pass(
                    FramePass::PointShadow(light * 6 + face),
                    "point shadow face",
                    &[],
                    &[(point_shadow_map, Access::Attach)],
                );
            }
        }
        // web3d-M7: ambient occlusion needs the camera's depth before
        // the main pass shades: a depth prepass, then GTAO into the
        // persistent AO texture the main pass reads.
        let ao_on = snap.post.ao > 0.0 && instance_count != 0;
        let mut main_reads = vec![(shadow_map, Access::Sample), (point_shadow_map, Access::Sample)];
        let prepass_depth = if ao_on {
            let prepass_depth = graph.create(TextureDesc::new("prepass depth", Extent::FULL, DEPTH_FORMAT));
            let ao_texture = graph.import("ambient occlusion", false);
            graph.add_pass(FramePass::Prepass, "depth prepass", &[], &[(prepass_depth, Access::Attach)]);
            graph.add_pass(
                FramePass::Ao,
                "gtao + blur",
                &[(prepass_depth, Access::Sample)],
                &[(ao_texture, Access::Attach)],
            );
            main_reads.push((ao_texture, Access::Sample));
            Some(prepass_depth)
        } else {
            None
        };
        // web3d-M7: the light clusters, before anything shades.
        if light_count > 0 {
            let grid = graph.import("light clusters", false);
            graph.add_pass(FramePass::Clusters, "light clusters", &[], &[(grid, Access::Storage)]);
            main_reads.push((grid, Access::Sample));
        }
        // web3d-M7: with GPU culling, a compute pass picks the early set
        // before the main pass, and after it the depth pyramid, the late
        // cull and a second opaque pass run.
        if gpu_cull {
            let early_set = graph.import("culled early", false);
            graph.add_pass(FramePass::CullEarly, "cull early", &[], &[(early_set, Access::Storage)]);
            main_reads.push((early_set, Access::Sample));
        }
        graph.add_pass(
            FramePass::Main,
            "main",
            &main_reads,
            &main_writes,
        );
        if gpu_cull {
            let late_set = graph.import("culled late", false);
            graph.add_pass(
                FramePass::CullLate,
                "hi-z + cull late",
                &[(depth, Access::Sample)],
                &[(late_set, Access::Storage)],
            );
            graph.add_pass(
                FramePass::MainLate,
                "main (late)",
                &[(late_set, Access::Sample)],
                &main_writes,
            );
        }
        // web3d-M7: with transmission, the opaque frame is copied into the
        // transmission source, and a second pass draws the transparent
        // list over the same multisampled targets.
        if transmission_on {
            let source = graph.import("transmission source", false);
            graph.add_pass(
                FramePass::TransmissionCopy,
                "transmission source",
                &[(hdr, Access::Sample)],
                &[(source, Access::Attach)],
            );
            graph.add_pass(
                FramePass::MainTransparent,
                "transparent",
                &[(source, Access::Sample)],
                &main_writes,
            );
        }
        // web3d-M7: reflections, then fog, over the drawn scene.
        let reflection = if let Some((_, surface_resolved)) = surface {
            let reflection = graph.create(TextureDesc::new(
                "reflection",
                Extent::FULL,
                crate::kernel::ssr::REFLECTION_FORMAT,
            ));
            graph.add_pass(
                FramePass::Ssr,
                "ssr trace",
                &[(hdr, Access::Sample), (depth, Access::Sample), (surface_resolved, Access::Sample)],
                &[(reflection, Access::Attach)],
            );
            graph.add_pass(
                FramePass::SsrComposite,
                "ssr composite",
                &[(reflection, Access::Sample)],
                &[(hdr, Access::Attach)],
            );
            Some((reflection, surface_resolved))
        } else {
            None
        };
        if volumetric_on {
            let volume = graph.import("fog volume", false);
            graph.add_pass(
                FramePass::FogVolume,
                "fog volume",
                &[(shadow_map, Access::Sample)],
                &[(volume, Access::Storage)],
            );
            graph.add_pass(
                FramePass::FogApply,
                "fog apply",
                &[(volume, Access::Sample), (depth, Access::Sample)],
                &[(hdr, Access::Attach)],
            );
        }
        // web3d-M7: particles, after everything that draws the scene:
        // simulate (bouncing off its depth), draw into the WBOIT
        // targets, blend over `hdr`.
        let wboit = if particles_on {
            let pool = graph.import("particle pool", false);
            graph.add_pass(
                FramePass::ParticlesSim,
                "particles",
                &[(depth, Access::Sample)],
                &[(pool, Access::Storage)],
            );
            use crate::kernel::particles::{ACCUM_FORMAT, REVEAL_FORMAT};
            let accum = graph.create(TextureDesc::new("particle accum", Extent::FULL, ACCUM_FORMAT));
            let reveal = graph.create(TextureDesc::new("particle reveal", Extent::FULL, REVEAL_FORMAT));
            graph.add_pass(
                FramePass::ParticlesDraw,
                "particle draw",
                &[(pool, Access::Sample), (depth, Access::Sample)],
                &[(accum, Access::Attach), (reveal, Access::Attach)],
            );
            graph.add_pass(
                FramePass::ParticlesComposite,
                "particle composite",
                &[(accum, Access::Sample), (reveal, Access::Sample)],
                &[(hdr, Access::Attach)],
            );
            Some([accum, reveal])
        } else {
            None
        };
        // web3d-M7: TAA resolves `hdr` into the persistent history the
        // tonemap then reads.
        let tonemap_input = if taa_on {
            let history = graph.import("taa history", false);
            graph.add_pass(
                FramePass::Taa,
                "taa resolve",
                &[(hdr, Access::Sample), (depth, Access::Sample)],
                &[(history, Access::Attach)],
            );
            history
        } else {
            hdr
        };
        // web3d-M7: depth of field, then motion blur, each a new HDR
        // target read by the next step.
        let (width, height) = (state.config.width, state.config.height);
        let dof_on = snap.post.dof_focus > 0.0
            && instance_count != 0
            && state.dof.prepare(
                &state.device,
                &state.queue,
                width,
                height,
                &crate::kernel::post::DofFrame {
                    focus: snap.post.dof_focus,
                    f_stop: snap.post.dof_f_stop,
                    fov_y,
                    near,
                    far,
                },
            );
        let motion_on = snap.post.motion_blur > 0.0;
        if motion_on {
            state.motion.prepare(
                &state.device,
                &state.queue,
                width,
                height,
                &crate::kernel::post::MotionFrame {
                    view_proj: unjittered_view_proj,
                    inv_view_proj: invert4(unjittered_view_proj),
                    near,
                    far,
                    shutter: snap.post.motion_blur,
                },
            );
        } else {
            state.motion.reset();
        }
        let taa_history = taa_on.then_some(tonemap_input);
        let mut post_res = tonemap_input;
        let mut dof_io = None;
        let mut motion_io = None;
        if dof_on {
            let out = graph.create(TextureDesc::new("depth of field", Extent::FULL, HDR_FORMAT));
            graph.add_pass(
                FramePass::Dof,
                "depth of field",
                &[(post_res, Access::Sample), (depth, Access::Sample)],
                &[(out, Access::Attach)],
            );
            dof_io = Some((post_res, out));
            post_res = out;
        }
        if motion_on {
            let out = graph.create(TextureDesc::new("motion blur", Extent::FULL, HDR_FORMAT));
            graph.add_pass(
                FramePass::MotionBlur,
                "motion blur",
                &[(post_res, Access::Sample), (depth, Access::Sample)],
                &[(out, Access::Attach)],
            );
            motion_io = Some((post_res, out));
            post_res = out;
        }
        let tonemap_input = post_res;
        // web3d-M7: bloom and exposure measure the anti-aliased frame.
        let bloom_on = snap.post.bloom_intensity > 0.0;
        let auto_exposure = snap.post.auto_exposure;
        let mut tonemap_reads = vec![(tonemap_input, Access::Sample)];
        if bloom_on {
            let chain = graph.import("bloom chain", false);
            graph.add_pass(
                FramePass::Bloom,
                "bloom",
                &[(tonemap_input, Access::Sample)],
                &[(chain, Access::Attach)],
            );
            tonemap_reads.push((chain, Access::Sample));
        }
        if auto_exposure {
            let adapted = graph.import("adapted exposure", false);
            graph.add_pass(
                FramePass::Exposure,
                "exposure",
                &[(tonemap_input, Access::Sample)],
                &[(adapted, Access::Storage)],
            );
            tonemap_reads.push((adapted, Access::Sample));
        }
        graph.add_pass(
            FramePass::Tonemap,
            "tonemap + hud",
            &tonemap_reads,
            &[(target, Access::Attach)],
        );
        let plan = graph.compile().map_err(|e| e.to_string())?;
        state
            .pool
            .prepare(&state.device, &plan, state.config.width, state.config.height);
        if taa_on {
            state.taa.prepare(
                &state.device,
                &state.queue,
                state.config.width,
                state.config.height,
                unjittered_view_proj,
                invert4(unjittered_view_proj),
            );
        }
        if ao_on {
            let (device, queue) = (&state.device, &state.queue);
            state.prepass.write(queue, view_proj, snap.time);
            state.ao.prepare(
                device,
                queue,
                width,
                height,
                &crate::kernel::ao::AoFrame {
                    proj,
                    near,
                    far,
                    radius: snap.post.ao_radius.max(1e-4),
                    intensity: snap.post.ao,
                    // A still pattern without TAA (no flicker).
                    frame: if taa_on { state.frame_index.get() } else { 0 },
                },
            );
        }
        if transmission_on {
            state.transmission.prepare(&state.device, width, height);
        }
        if gpu_cull {
            let cull_draws: Vec<CullDraw> = opaque_draws
                .iter()
                .map(|d| CullDraw {
                    index_count: d.indices.end - d.indices.start,
                    first_index: d.indices.start,
                    base_vertex: 0,
                    group: d.group,
                })
                .collect();
            state.gpu_cull.prepare(
                &state.device,
                &state.queue,
                &crate::kernel::gpu_cull::CullFrame {
                    size: (width, height),
                    view_proj: unjittered_view_proj,
                    planes: extract_frustum_planes(unjittered_view_proj),
                    instances: opaque_count,
                },
                &cull_groups,
                &cull_draws,
            );
        }
        let frame_key = (state.ao.key(ao_on), state.transmission.key(transmission_on));
        if frame_key != state.frame_ao_key {
            state.frame_bind_group = frame_bind_group(
                &state.device,
                &state.frame_bgl,
                &FrameBuffers {
                    camera: &state.camera_buffer,
                    lights: &state.lights_buffer,
                    fog: &state.fog_buffer,
                    clusters: &state.clusters,
                },
                &state.env,
                [state.ao.view(ao_on), state.transmission.view(transmission_on)],
            );
            state.frame_ao_key = frame_key;
        }
        if bloom_on {
            state
                .bloom
                .prepare(&state.device, &state.queue, width, height, snap.post.bloom_threshold);
        }
        if auto_exposure {
            state.exposure.prepare(&state.queue, snap.time);
        } else {
            state.exposure.reset(&state.queue);
        }
        // The history written this frame alternates: with TAA the
        // tonemap's input is bound fresh every frame.
        let from_history = taa_history == Some(tonemap_input);
        let input_key = if from_history {
            u64::MAX
        } else {
            state.pool.generation(&plan, tonemap_input)
        };
        let key = (input_key, state.bloom.key(bloom_on), state.lut.generation);
        if from_history || state.tonemap_bind_group.as_ref().map(|(k, _)| *k) != Some(key) {
            let input = if from_history {
                state.taa.output()
            } else {
                state
                    .pool
                    .view(&plan, tonemap_input)
                    .ok_or("render graph: no tonemap input")?
            };
            let bg = tonemap_bind_group(state, input, state.bloom.view(bloom_on));
            state.tonemap_bind_group = Some((key, bg));
        }
        let lut_strength = match snap.lut {
            Some(l) if state.lut.loaded.is_some() && state.lut.loaded == state.lut.requested => {
                l.strength.clamp(0.0, 1.0)
            }
            _ => 0.0,
        };
        // Phase 26: fullscreen tonemap pass — reads the HDR offscreen,
        // applies ACES (or pass-through, per script flag) plus
        // optional vignette, writes to the swapchain. Always runs:
        // the main pipeline writes Rgba16Float, so this pass is the
        // step that gets that data into the user's sRGB display.
        let PostFx {
            tonemapper,
            vignette,
            vignette_color: [vc_r, vc_g, vc_b],
            bloom_intensity,
            exposure,
            ..
        } = snap.post;
        // The bloom chain's top level sums its levels: average them.
        let bloom_scale = if bloom_on {
            bloom_intensity / state.bloom.level_count() as f32
        } else {
            0.0
        };
        state.queue.write_buffer(
            &state.tonemap_params_buffer,
            0,
            bytemuck::cast_slice(&[
                tonemapper.id(),
                bloom_scale,
                exposure.exp2(),
                vignette,
                vc_r,
                vc_g,
                vc_b,
                if auto_exposure { 1.0 } else { 0.0 },
                lut_strength,
                state.lut.size as f32,
                0.0,
                0.0,
            ]),
        );
        let state = &*state;
        let main_color_view = state.pool.view(&plan, hdr).ok_or("render graph: no hdr target")?;
        let depth_view = state.pool.view(&plan, depth).ok_or("render graph: no depth target")?;
        let msaa_view = state
            .pool
            .view(&plan, hdr_msaa)
            .ok_or("render graph: no msaa target")?;
        // A graph texture's view: TAA's history, or a pooled target.
        let view_of = |r| {
            if taa_history == Some(r) {
                Some(state.taa.output())
            } else {
                state.pool.view(&plan, r)
            }
        };
        let post_input = view_of(tonemap_input).ok_or("render graph: no post input")?;
        // web3d-M7: the main passes' second target, the surface record
        // (resolved only when reflections read it).
        let surface_views = match surface {
            Some((msaa, resolved)) => Some((
                state.pool.view(&plan, msaa).ok_or("render graph: no surface target")?,
                state.pool.view(&plan, resolved).ok_or("render graph: no surface resolve")?,
            )),
            None => None,
        };
        let surface_target = |clear: bool, keep: bool| {
            let (view, resolved) = surface_views?;
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: Some(resolved),
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: if keep { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard },
                },
            })
        };
        for pass in &plan.passes {
            if let Some(profile) = &state.gpu_profile {
                profile.borrow_mut().mark(&mut encoder, format!("{pass:?}"));
            }
            match *pass {
                FramePass::Prepass => {
                    let view = prepass_depth
                        .and_then(|d| state.pool.view(&plan, d))
                        .ok_or("render graph: no prepass depth")?;
                    let mut ppass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("twe-kernel depth prepass"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    let ranges = (&cube_ranges[..], &sphere_ranges[..], &mesh_ranges[..]);
                    draw_prepass(&mut ppass, state, ranges, snap.materials);
                }
                FramePass::Ao => {
                    let view = prepass_depth
                        .and_then(|d| state.pool.view(&plan, d))
                        .ok_or("render graph: no prepass depth")?;
                    state.ao.record(&state.device, &mut encoder, view);
                }
                FramePass::Clusters => state.clusters.record(&mut encoder),
                FramePass::CullEarly => {
                    state
                        .gpu_cull
                        .record_early(&state.device, &mut encoder, &state.instance_buffer);
                }
                FramePass::CullLate => {
                    state
                        .gpu_cull
                        .record_late(&state.device, &mut encoder, &state.instance_buffer, depth_view);
                }
                FramePass::MainLate => {
                    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("twe-kernel main pass (late)"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: msaa_view,
                            depth_slice: None,
                            resolve_target: Some(main_color_view),
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: if transmission_on {
                                    wgpu::StoreOp::Store
                                } else {
                                    wgpu::StoreOp::Discard
                                },
                            },
                        }),
                        surface_target(false, transmission_on),
                    ],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: if taa_on || dof_on || motion_on || transmission_on || particles_on || ssr_on || volumetric_on {
                                    wgpu::StoreOp::Store
                                } else {
                                    wgpu::StoreOp::Discard
                                },
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    rpass.set_bind_group(0, &state.frame_bind_group, &[]);
                    rpass.set_bind_group(2, &state.identity_joints_bind_group, &[]);
                    rpass.set_bind_group(3, &state.shadow_combined_bg, &[]);
                    draw_opaque(&mut rpass, state, snap.materials, &opaque_draws, &cull_groups, OpaqueSource::Late);
                    finish_scene(&mut rpass, state, env_uniform.params[3] > 0.5 || fog_on, &transparent, transmission_on);
                }
                FramePass::Ssr => {
                    let (reflection, surface_resolved) = reflection.ok_or("render graph: ssr without targets")?;
                    let view = |r| state.pool.view(&plan, r).ok_or("render graph: no ssr target");
                    state.ssr.record_trace(
                        &state.device,
                        &mut encoder,
                        [main_color_view, depth_view, view(surface_resolved)?, &state.env.current.specular],
                        view(reflection)?,
                    );
                }
                FramePass::SsrComposite => {
                    let (reflection, _) = reflection.ok_or("render graph: ssr without targets")?;
                    let reflection = state.pool.view(&plan, reflection).ok_or("render graph: no ssr target")?;
                    state
                        .ssr
                        .record_composite(&state.device, &mut encoder, reflection, main_color_view);
                }
                FramePass::FogVolume => {
                    state
                        .volumetric
                        .record_volume(&mut encoder, &state.frame_bind_group, &state.shadow_combined_bg);
                }
                FramePass::FogApply => {
                    state
                        .volumetric
                        .record_apply(&state.device, &mut encoder, depth_view, main_color_view);
                }
                FramePass::ParticlesSim => {
                    state.particles.record_sim(&state.device, &mut encoder, depth_view);
                }
                FramePass::ParticlesDraw => {
                    let [accum, reveal] = wboit.ok_or("render graph: particles without targets")?;
                    let view = |r| state.pool.view(&plan, r).ok_or("render graph: no particle target");
                    state
                        .particles
                        .record_draw(&state.device, &mut encoder, [view(accum)?, view(reveal)?], depth_view);
                }
                FramePass::ParticlesComposite => {
                    let [accum, reveal] = wboit.ok_or("render graph: particles without targets")?;
                    let view = |r| state.pool.view(&plan, r).ok_or("render graph: no particle target");
                    state
                        .particles
                        .record_composite(&state.device, &mut encoder, view(accum)?, view(reveal)?, main_color_view);
                }
                FramePass::TransmissionCopy => {
                    state.transmission.record(&state.device, &mut encoder, main_color_view);
                }
                FramePass::MainTransparent => {
                    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("twe-kernel transparent pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: msaa_view,
                            depth_slice: None,
                            resolve_target: Some(main_color_view),
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Discard,
                            },
                        }),
                        surface_target(false, false),
                    ],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: if taa_on || dof_on || motion_on || particles_on || ssr_on || volumetric_on {
                                    wgpu::StoreOp::Store
                                } else {
                                    wgpu::StoreOp::Discard
                                },
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    draw_transparent(&mut rpass, state, &transparent);
                }
                FramePass::Dof => {
                    let (input, output) = dof_io.ok_or("render graph: dof without targets")?;
                    let input = view_of(input).ok_or("render graph: no dof input")?;
                    let output = view_of(output).ok_or("render graph: no dof output")?;
                    state.dof.record(&state.device, &mut encoder, input, depth_view, output);
                }
                FramePass::MotionBlur => {
                    let (input, output) = motion_io.ok_or("render graph: motion blur without targets")?;
                    let input = view_of(input).ok_or("render graph: no motion blur input")?;
                    let output = view_of(output).ok_or("render graph: no motion blur output")?;
                    state.motion.record(&state.device, &mut encoder, input, depth_view, output);
                }
                FramePass::Bloom => {
                    state.bloom.record(&state.device, &mut encoder, post_input);
                }
                FramePass::Exposure => {
                    state
                        .exposure
                        .record(&state.device, &mut encoder, post_input, width, height);
                }
                FramePass::Shadow(cascade) => {
                    // Push the active cascade's matrix into the per-pass
                    // uniform. queue.write_buffer is recorded as a copy
                    // command; sequential write_buffer + render_pass pairs
                    // execute in order at submission time.
                    let pass_uniform =
                        ShadowPassUniform::new(shadow_uniform.light_space_matrices[cascade], snap.time);
                    state.queue.write_buffer(
                        &state.shadow_pass_buffers[cascade],
                        0,
                        bytemuck::bytes_of(&pass_uniform),
                    );
                    let mut spass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("twec-play3d shadow pass"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &state.shadow_layer_views[cascade],
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    spass.set_bind_group(0, &state.shadow_pass_bgs[cascade], &[]);
                    spass.set_bind_group(1, &state.identity_joints_bind_group, &[]);
                    spass.set_vertex_buffer(1, state.instance_buffer.slice(..));
                    let ranges = (&cube_ranges[..], &sphere_ranges[..], &mesh_ranges[..]);
                    draw_depth(&mut spass, state, ranges, snap.materials, DepthPass::Cascade);
                }
                FramePass::PointShadow(layer) => {
                    let mut spass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("twe-kernel point shadow face"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &state.point_shadows.layer_views[layer],
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    spass.set_bind_group(0, &state.point_shadows.pass_bgs[layer], &[]);
                    spass.set_bind_group(1, &state.identity_joints_bind_group, &[]);
                    spass.set_vertex_buffer(1, state.instance_buffer.slice(..));
                    let ranges = (&cube_ranges[..], &sphere_ranges[..], &mesh_ranges[..]);
                    draw_depth(&mut spass, state, ranges, snap.materials, DepthPass::Point);
                }
                FramePass::Main => {
                    {
                        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("twec-play3d main pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: msaa_view,
                                depth_slice: None,
                                resolve_target: Some(main_color_view),
                                ops: wgpu::Operations {
                                    // With transmission, alpha 0 marks the
                                    // background (alpha resolves to
                                    // coverage) for refracted rays.
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: f64::from(snap.background[0]),
                                        g: f64::from(snap.background[1]),
                                        b: f64::from(snap.background[2]),
                                        a: if transmission_on { 0.0 } else { 1.0 },
                                    }),
                                    // Kept for the transparent pass when
                                    // it runs separately.
                                    store: if transmission_on || gpu_cull {
                                        wgpu::StoreOp::Store
                                    } else {
                                        wgpu::StoreOp::Discard
                                    },
                                },
                            }),
                            surface_target(true, transmission_on || gpu_cull),
                        ],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: depth_view,
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(1.0),
                                    store: if taa_on || dof_on || motion_on || transmission_on || gpu_cull || particles_on || ssr_on || volumetric_on {
                                        wgpu::StoreOp::Store
                                    } else {
                                        wgpu::StoreOp::Discard
                                    },
                                }),
                                stencil_ops: None,
                            }),
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        if instance_count != 0 {
                            rpass.set_pipeline(&state.lit().opaque);
                            rpass.set_bind_group(0, &state.frame_bind_group, &[]);
                            // Phase 24: bind the shared identity joint UBO as the
                            // default for unskinned draws (cube, sphere, glb
                            // without a skin). Skinned mesh draws override slot 3
                            // with their per-mesh joint bind group below.
                            rpass.set_bind_group(2, &state.identity_joints_bind_group, &[]);
                            // Phase 25: shadow combined bind group at slot 4 —
                            // shadow uniform + texture + comparison sampler. The
                            // shader short-circuits when flags.w == 0.
                            rpass.set_bind_group(3, &state.shadow_combined_bg, &[]);
                            // web3d-M7: the opaque draw list, directly or (GPU
                            // culling) the early set through indirect draws.
                            let source = if gpu_cull { OpaqueSource::Early } else { OpaqueSource::Direct };
                            draw_opaque(&mut rpass, state, snap.materials, &opaque_draws, &cull_groups, source);
                        }
                        // With GPU culling the late opaque pass finishes
                        // the scene; otherwise it's finished here.
                        if !gpu_cull {
                            finish_scene(&mut rpass, state, env_uniform.params[3] > 0.5 || fog_on, &transparent, transmission_on);
                        }
                    }
                }
                FramePass::Taa => {
                    state
                        .taa
                        .record(&state.device, &mut encoder, main_color_view, depth_view);
                }
                FramePass::Tonemap => {
                    if let Some((_, bg)) = &state.tonemap_bind_group {
                        let mut tmpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("twec-play3d tonemap pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view_target,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        tmpass.set_pipeline(&state.tonemap_pipeline);
                        tmpass.set_bind_group(0, bg, &[]);
                        tmpass.draw(0..3, 0..1);
                        // web3d-M3: the HUD, over the finished scene.
                        state.hud.draw(&mut tmpass);
                    }
                }
            }
        }

        if let Some(profile) = &state.gpu_profile {
            profile.borrow_mut().finish(&mut encoder);
        }
        state.queue.submit(Some(encoder.finish()));
        if let Some(profile) = &state.gpu_profile {
            profile.borrow_mut().collect(&state.device);
        }
        if taa_on {
            state.taa.advance();
        }
        state.frame_index.set(state.frame_index.get().wrapping_add(1));
        if let Some(frame) = frame {
            state.queue.present(frame);
        }
        Ok(())
    }

    /// web3d-M7 diagnostics (native only; blocks on the GPU): the last
    /// GPU-culled frame's (instances drawn early, drawn late, opaque
    /// instances in all), or `None` if it wasn't culled on the GPU.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn gpu_cull_counts(&self) -> Option<(u32, u32, u32)> {
        self.gpu_cull.last_counts(&self.device, &self.queue)
    }

    /// web3d-M7: this frame's lit-surface pipelines.
    /// web3d-M7: the depth pipeline for a draw with material `mat` in
    /// depth pass `which`: the displacing material's own, else the
    /// pass's plain one.
    fn depth_pipeline(&self, materials: &[String], mat: u32, which: DepthPass) -> &wgpu::RenderPipeline {
        let displaced = materials
            .get(mat as usize)
            .filter(|_| mat != 0)
            .and_then(|src| self.displaced_depth.get(src));
        match (which, displaced) {
            (DepthPass::Cascade, Some(d)) => &d.cascade,
            (DepthPass::Point, Some(d)) => &d.point,
            (DepthPass::Prepass, Some(d)) => &d.prepass,
            (DepthPass::Cascade, None) => &self.shadow_pipeline,
            (DepthPass::Point, None) => &self.point_shadows.pipeline,
            (DepthPass::Prepass, None) => &self.prepass.opaque,
        }
    }

    /// web3d-M7 follow-up: the `draws_generation` whose draws this
    /// renderer holds, if any (the host may then send it with no draws).
    /// web3d-M7 follow-up diagnostics: whether the last frame culled on
    /// the GPU.
    pub fn last_frame_culled(&self) -> bool {
        self.last_culled.get()
    }

    pub fn retained_generation(&self) -> Option<u64> {
        self.retained.as_ref().map(|r| r.generation)
    }

    fn lit(&self) -> &LitPipelines {
        match (&self.lit_ssr, self.ssr_frame.get()) {
            (Some(p), true) => p,
            _ => &self.lit,
        }
    }

    /// web3d-M7 diagnostics: how many particles the GPU pool holds.
    pub fn particle_capacity(&self) -> u32 {
        self.particles.capacity()
    }

    /// Read back the last headless frame as tightly packed RGBA8
    /// (sRGB) rows, top row first. Native only: it blocks on the GPU,
    /// which the browser does not allow.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_pixels(&self) -> Result<Vec<u8>, String> {
        let texture = self
            .offscreen
            .as_ref()
            .ok_or("read_pixels needs a headless renderer")?;
        let (w, h) = (self.config.width, self.config.height);
        let row = 4 * w;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twe-kernel readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            let start = y * padded as usize;
            out.extend_from_slice(&data[start..start + row as usize]);
        }
        Ok(out)
    }
}

/// web3d-M7: the colour-grading LUT: the current one (identity until a
/// `.cube` loads), its size, and which path was asked for / loaded.
struct LutState {
    view: wgpu::TextureView,
    size: u32,
    requested: Option<String>,
    loaded: Option<String>,
    /// Bumped when `view` changes (the tonemap bind group rebuilds).
    generation: u64,
}

/// web3d-M7: image-based lighting state: the current environment (a
/// black placeholder until one loads), its uniform, the DFG table, the
/// samplers, and the backdrop pipeline.
struct EnvState {
    uniform: wgpu::Buffer,
    current: crate::kernel::environment::GpuEnvironment,
    /// The path last requested from the asset source, and the one
    /// `current` was built from.
    requested: Option<String>,
    loaded: Option<String>,
    dfg: wgpu::TextureView,
    sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    /// Index 1: with the (unwritten) surface record target.
    sky_pipelines: [wgpu::RenderPipeline; 2],
}

impl EnvState {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, frame_bgl: &wgpu::BindGroupLayout) -> Self {
        let linear = |mode_u, label| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode_u,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel sky"),
            source: wgpu::ShaderSource::Wgsl(SKY_SHADER_SRC.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel sky layout"),
            bind_group_layouts: &[Some(frame_bgl)],
            immediate_size: 0,
        });
        let sky_pipeline = |surface: bool| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel sky"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    // The backdrop reflects nothing (the target's clear).
                    surface.then_some(wgpu::ColorTargetState {
                        format: SURFACE_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::empty(),
                    }),
                ],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            // At the far plane, only where the depth buffer is still clear.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        EnvState {
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel env uniform"),
                contents: bytemuck::bytes_of(&EnvUniform::none()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            current: crate::kernel::environment::placeholder(device, queue),
            requested: None,
            loaded: None,
            dfg: crate::kernel::environment::dfg_texture(device, queue),
            sampler: linear(wgpu::AddressMode::Repeat, "twe-kernel env sampler"),
            clamp_sampler: linear(wgpu::AddressMode::ClampToEdge, "twe-kernel clamp sampler"),
            sky_pipelines: [sky_pipeline(false), sky_pipeline(true)],
        }
    }
}

/// The frame group's buffers.
struct FrameBuffers<'a> {
    camera: &'a wgpu::Buffer,
    lights: &'a wgpu::Buffer,
    fog: &'a wgpu::Buffer,
    clusters: &'a crate::kernel::clusters::Clusters,
}

/// Bind group 0: camera, lights, the environment, fog, AO, the
/// transmission source and the clustered light list.
fn frame_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffers: &FrameBuffers<'_>,
    env: &EnvState,
    [ao, transmission]: [&wgpu::TextureView; 2],
) -> wgpu::BindGroup {
    let FrameBuffers {
        camera,
        lights,
        fog,
        clusters,
    } = *buffers;
    let view = |v| wgpu::BindingResource::TextureView(v);
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twe-kernel frame bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: env.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: view(&env.current.specular),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: view(&env.current.equirect),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: view(&env.dfg),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(&env.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(&env.clamp_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: view(ao),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: fog.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: view(transmission),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: clusters.lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: clusters.grid.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: clusters.uniform.as_entire_binding(),
            },
        ],
    })
}

/// Draw every opaque instance into a depth-only shadow pass (group 0 =
/// the pass's light matrix, group 1 = joints), skinned meshes with
/// their own joint matrices.
fn draw_depth(
    pass: &mut wgpu::RenderPass<'_>,
    state: &Renderer,
    (cubes, spheres, meshes): DepthRanges<'_>,
    materials: &[String],
    which: DepthPass,
) {
    let pipeline = |mat: u32| state.depth_pipeline(materials, mat, which);
    for ((_, mat), range) in cubes {
        if range.1 > range.0 {
            pass.set_pipeline(pipeline(*mat));
            pass.set_vertex_buffer(0, state.cube_vertex_buffer.slice(..));
            pass.set_index_buffer(state.cube_index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..state.cube_index_count, 0, range.0..range.1);
        }
    }
    for ((_, mat), range) in spheres {
        if range.1 > range.0 {
            pass.set_pipeline(pipeline(*mat));
            pass.set_vertex_buffer(0, state.sphere_vertex_buffer.slice(..));
            pass.set_index_buffer(state.sphere_index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..state.sphere_index_count, 0, range.0..range.1);
        }
    }
    for ((mesh_id, _, mat), range) in meshes {
        if range.1 <= range.0 {
            continue;
        }
        let Some(gpu_mesh) = state.mesh_cache.get(mesh_id) else {
            continue;
        };
        pass.set_pipeline(pipeline(*mat));
        if let Some(skin) = &gpu_mesh.skin {
            pass.set_bind_group(1, &skin.joint_bind_group, &[]);
        }
        pass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(gpu_mesh.index_buffer.slice(..), gpu_mesh.index_format);
        pass.draw_indexed(0..gpu_mesh.index_count, 0, range.0..range.1);
        if gpu_mesh.skin.is_some() {
            pass.set_bind_group(1, &state.identity_joints_bind_group, &[]);
        }
    }
}

/// web3d-M7 follow-up: a frame's opaque draw list as built, kept while
/// the host keeps sending the same `draws_generation`: the draws (to
/// rebuild from if an asset lands), and what was built from them, whose
/// instances stay in the instance buffer.
struct RetainedDraws {
    generation: u64,
    /// Mesh uploads, failures and textures when built: any change (a
    /// mesh arriving) rebuilds from `draws`.
    assets: (usize, usize, usize),
    draws: std::rc::Rc<[DrawCall3d]>,
    instance_count: usize,
    cube_ranges: Vec<(SurfaceKey, InstanceRange)>,
    sphere_ranges: Vec<(SurfaceKey, InstanceRange)>,
    mesh_ranges: Vec<(MeshKey, InstanceRange)>,
    cull_groups: Vec<CullGroup>,
    opaque_draws: Vec<OpaqueDraw>,
    opaque_count: u32,
    /// The `visual` materials the draws use (their pipelines are
    /// readied every frame: a switch of SSR needs the other variant).
    materials: Vec<u32>,
}

/// web3d-M7: the instance ranges a depth pass draws: cubes, spheres,
/// meshes.
type DepthRanges<'a> = (
    &'a [(SurfaceKey, InstanceRange)],
    &'a [(SurfaceKey, InstanceRange)],
    &'a [(MeshKey, InstanceRange)],
);

/// web3d-M7: which depth-only pass is drawing.
#[derive(Clone, Copy)]
enum DepthPass {
    Cascade,
    Point,
    Prepass,
}

/// web3d-M7: a displacing material's depth-only pipelines, each like
/// its pass's plain pipeline (culling, bias) but with the material's
/// displacing vertex stage, so shadows and ambient occlusion follow the
/// displaced shape.
struct DisplacedDepth {
    cascade: wgpu::RenderPipeline,
    point: wgpu::RenderPipeline,
    prepass: wgpu::RenderPipeline,
}

impl DisplacedDepth {
    fn new(device: &wgpu::Device, layout: &wgpu::PipelineLayout, material: &str) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel displaced depth"),
            source: wgpu::ShaderSource::Wgsl(material_depth_source(material).into()),
        });
        let shadow_bias = wgpu::DepthBiasState {
            constant: 2,
            slope_scale: 2.0,
            clamp: 0.0,
        };
        let pipeline = |label: &str, cull: Option<wgpu::Face>, bias: wgpu::DepthBiasState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_shadow_displaced"),
                    buffers: &[Some(Vertex::layout()), Some(Instance::layout())],
                    compilation_options: Default::default(),
                },
                fragment: None,
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias,
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        DisplacedDepth {
            cascade: pipeline("twe-kernel displaced shadow", Some(wgpu::Face::Front), shadow_bias),
            point: pipeline("twe-kernel displaced point shadow", None, shadow_bias),
            prepass: pipeline(
                "twe-kernel displaced prepass",
                Some(wgpu::Face::Back),
                wgpu::DepthBiasState::default(),
            ),
        }
    }
}

/// web3d-M7: the camera depth prepass: the camera's (jittered)
/// view-projection, and depth-only pipelines for single-sided,
/// double-sided and alpha-masked surfaces.
struct Prepass {
    buffer: wgpu::Buffer,
    bg: wgpu::BindGroup,
    opaque: wgpu::RenderPipeline,
    double: wgpu::RenderPipeline,
    masked: wgpu::RenderPipeline,
}

impl Prepass {
    fn new(
        device: &wgpu::Device,
        pass_bgl: &wgpu::BindGroupLayout,
        joints_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        shadow_shader: &wgpu::ShaderModule,
    ) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("twe-kernel prepass uniform"),
            contents: bytemuck::bytes_of(&ShadowPassUniform::identity()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel prepass bg"),
            layout: pass_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let depth_only = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel prepass layout"),
            bind_group_layouts: &[Some(pass_bgl), Some(joints_bgl)],
            immediate_size: 0,
        });
        let with_material = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel prepass mask layout"),
            bind_group_layouts: &[Some(pass_bgl), Some(joints_bgl), Some(material_bgl)],
            immediate_size: 0,
        });
        let mask_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel prepass mask"),
            source: wgpu::ShaderSource::Wgsl(PREPASS_MASK_SRC.into()),
        });
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        module: &wgpu::ShaderModule,
                        vs: &str,
                        fs: Option<&str>,
                        cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
                    buffers: &[Some(Vertex::layout()), Some(Instance::layout())],
                    compilation_options: Default::default(),
                },
                fragment: fs.map(|entry| wgpu::FragmentState {
                    module,
                    entry_point: Some(entry),
                    targets: &[],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        Prepass {
            opaque: pipeline(
                "twe-kernel prepass",
                &depth_only,
                shadow_shader,
                "vs_shadow",
                None,
                Some(wgpu::Face::Back),
            ),
            double: pipeline("twe-kernel prepass (double-sided)", &depth_only, shadow_shader, "vs_shadow", None, None),
            masked: pipeline(
                "twe-kernel prepass (masked)",
                &with_material,
                &mask_shader,
                "vs_mask",
                Some("fs_mask"),
                None,
            ),
            buffer,
            bg,
        }
    }

    fn write(&self, queue: &wgpu::Queue, view_proj: [[f32; 4]; 4], time: f32) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&ShadowPassUniform::new(view_proj, time)));
    }
}

/// web3d-M7: the geometry an opaque draw uses.
#[derive(Clone, Copy, Debug)]
enum OpaqueShape {
    Cube,
    Sphere,
    Mesh(u32),
}

/// web3d-M7: the surface an opaque draw is shaded with: a script
/// texture / `visual` material (plain surface when both are 0), or one
/// glTF primitive's own material.
#[derive(Clone, Copy, Debug)]
enum OpaqueSurface {
    Script { tex: u32, mat: u32 },
    Submesh(usize),
}

/// web3d-M7: one draw of the opaque list: shape, surface, index range,
/// and the cull group whose instances it draws (their range in the
/// instance buffer when drawn directly).
#[derive(Clone, Debug)]
struct OpaqueDraw {
    shape: OpaqueShape,
    surface: OpaqueSurface,
    group: u32,
    range: InstanceRange,
    indices: std::ops::Range<u32>,
}

/// Where the opaque list's instances come from.
#[derive(Clone, Copy, PartialEq)]
enum OpaqueSource {
    /// The instance buffer, every instance.
    Direct,
    /// The GPU cull's early or late set, through indirect draws.
    Early,
    Late,
}

/// Draw the opaque list. Expects bind groups 0 (frame), 2 (identity
/// joints) and 3 (shadows) bound.
fn draw_opaque(
    pass: &mut wgpu::RenderPass<'_>,
    state: &Renderer,
    materials: &[String],
    draws: &[OpaqueDraw],
    groups: &[CullGroup],
    source: OpaqueSource,
) {
    // A script texture's material (0 = the plain surface), and a
    // `visual` material's pipeline (0 = the plain surface).
    let texture_group = |tex: u32| -> &wgpu::BindGroup {
        if tex == 0 {
            return &state.plain_material;
        }
        state.texture_cache.get(&tex).unwrap_or(&state.plain_material)
    };
    let pipeline_for = |mat: u32| -> &wgpu::RenderPipeline {
        materials
            .get(mat as usize)
            .filter(|_| mat != 0)
            .and_then(|src| state.material_pipelines[usize::from(state.ssr_frame.get())].get(src))
            .unwrap_or(&state.lit().opaque)
    };
    if source == OpaqueSource::Direct {
        pass.set_vertex_buffer(1, state.instance_buffer.slice(..));
    }
    for (i, d) in draws.iter().enumerate() {
        if d.range.1 <= d.range.0 {
            continue;
        }
        let skin = match d.shape {
            OpaqueShape::Cube => {
                pass.set_vertex_buffer(0, state.cube_vertex_buffer.slice(..));
                pass.set_index_buffer(state.cube_index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                None
            }
            OpaqueShape::Sphere => {
                pass.set_vertex_buffer(0, state.sphere_vertex_buffer.slice(..));
                pass.set_index_buffer(state.sphere_index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                None
            }
            OpaqueShape::Mesh(id) => {
                let Some(mesh) = state.mesh_cache.get(&id) else { continue };
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), mesh.index_format);
                if let Some(skin) = &mesh.skin {
                    pass.set_bind_group(2, &skin.joint_bind_group, &[]);
                }
                Some(mesh)
            }
        };
        match d.surface {
            OpaqueSurface::Script { tex, mat } => {
                pass.set_pipeline(if tex == 0 && mat == 0 {
                    &state.lit().script
                } else {
                    pipeline_for(mat)
                });
                pass.set_bind_group(1, texture_group(tex), &[]);
            }
            OpaqueSurface::Submesh(si) => {
                let Some(sub) = skin.and_then(|m| m.submeshes.get(si)) else { continue };
                pass.set_pipeline(if sub.double_sided {
                    &state.lit().double
                } else {
                    &state.lit().opaque
                });
                pass.set_bind_group(1, &sub.material, &[]);
            }
        }
        match source {
            OpaqueSource::Direct => pass.draw_indexed(d.indices.clone(), 0, d.range.0..d.range.1),
            OpaqueSource::Early | OpaqueSource::Late => {
                let g = &groups[d.group as usize];
                state
                    .gpu_cull
                    .draw(pass, source == OpaqueSource::Late, i as u32, g, d.range.1 - d.range.0);
            }
        }
        if skin.is_some_and(|m| m.skin.is_some()) {
            pass.set_bind_group(2, &state.identity_joints_bind_group, &[]);
        }
    }
}

/// web3d-M7: the end of the main pass: the backdrop where nothing was
/// drawn (the environment, or with fog the fogged background), then the
/// transparent list unless transmission draws it in its own pass.
fn finish_scene(
    pass: &mut wgpu::RenderPass<'_>,
    state: &Renderer,
    backdrop: bool,
    transparent: &[TransparentDraw],
    transmission_on: bool,
) {
    if backdrop {
        pass.set_pipeline(&state.env.sky_pipelines[usize::from(state.ssr_frame.get())]);
        pass.set_bind_group(0, &state.frame_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    if !transparent.is_empty() && !transmission_on {
        draw_transparent(pass, state, transparent);
    }
}

/// web3d-M7: what a transparent draw draws.
#[derive(Clone, Copy, Debug)]
enum TransparentShape {
    Cube(u32),
    Sphere(u32),
    /// A mesh; `sub` is one glTF primitive (with its own material), or
    /// `None` for the whole mesh under a script texture `tex`.
    Mesh { id: u32, tex: u32, sub: Option<usize> },
}

/// web3d-M7: one draw of the transparent pass: a shape, the instance
/// (index into the instance buffer), and its squared distance to the
/// eye (the sort key).
#[derive(Clone, Copy, Debug)]
struct TransparentDraw {
    shape: TransparentShape,
    instance: u32,
    depth: f32,
    double_sided: bool,
    /// web3d-M7 follow-up: a transmissive (not blended) glTF primitive.
    /// It already carries what's behind it, so it's drawn like an opaque
    /// surface, depth written: a glass shell's far wall (a concave
    /// mirror) no longer draws over its near wall when the two sort
    /// alike, which showed IridescenceLamp's globe reflections upside
    /// down.
    solid: bool,
}

/// The average of a glTF primitive's vertex positions (mesh space).
fn submesh_centroid(vertices: &[Vertex], indices: &[u32]) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    for &i in indices {
        let p = vertices[i as usize].position;
        for k in 0..3 {
            sum[k] += f64::from(p[k]);
        }
    }
    let n = indices.len().max(1) as f64;
    [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32]
}

/// web3d-M7: the transparent pass: each sorted draw blended over the
/// frame, double-sided surfaces' back faces first. Expects the main
/// pass's bind groups 0 and 3 and the instance buffer bound.
fn draw_transparent(pass: &mut wgpu::RenderPass<'_>, state: &Renderer, draws: &[TransparentDraw]) {
    let texture_group = |tex: u32| -> &wgpu::BindGroup {
        if tex == 0 {
            return &state.plain_material;
        }
        state.texture_cache.get(&tex).unwrap_or(&state.plain_material)
    };
    pass.set_bind_group(0, &state.frame_bind_group, &[]);
    pass.set_bind_group(2, &state.identity_joints_bind_group, &[]);
    pass.set_bind_group(3, &state.shadow_combined_bg, &[]);
    pass.set_vertex_buffer(1, state.instance_buffer.slice(..));
    for d in draws {
        let instances = d.instance..d.instance + 1;
        let (range, skinned) = match d.shape {
            TransparentShape::Cube(tex) => {
                pass.set_bind_group(1, texture_group(tex), &[]);
                pass.set_vertex_buffer(0, state.cube_vertex_buffer.slice(..));
                pass.set_index_buffer(state.cube_index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                (0..state.cube_index_count, false)
            }
            TransparentShape::Sphere(tex) => {
                pass.set_bind_group(1, texture_group(tex), &[]);
                pass.set_vertex_buffer(0, state.sphere_vertex_buffer.slice(..));
                pass.set_index_buffer(state.sphere_index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                (0..state.sphere_index_count, false)
            }
            TransparentShape::Mesh { id, tex, sub } => {
                let Some(mesh) = state.mesh_cache.get(&id) else { continue };
                if let Some(skin) = &mesh.skin {
                    pass.set_bind_group(2, &skin.joint_bind_group, &[]);
                }
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), mesh.index_format);
                let range = match sub.and_then(|s| mesh.submeshes.get(s)) {
                    Some(s) => {
                        pass.set_bind_group(1, &s.material, &[]);
                        s.first..s.first + s.count
                    }
                    None => {
                        pass.set_bind_group(1, texture_group(tex), &[]);
                        0..mesh.index_count
                    }
                };
                (range, mesh.skin.is_some())
            }
        };
        if d.solid {
            pass.set_pipeline(if d.double_sided { &state.lit().double } else { &state.lit().opaque });
            pass.draw_indexed(range, 0, instances);
        } else {
            if d.double_sided {
                pass.set_pipeline(&state.lit().blend_back);
                pass.draw_indexed(range.clone(), 0, instances.clone());
            }
            pass.set_pipeline(&state.lit().blend_front);
            pass.draw_indexed(range, 0, instances);
        }
        if skinned {
            pass.set_bind_group(2, &state.identity_joints_bind_group, &[]);
        }
    }
}

/// Draw every instance into the depth prepass, each glTF primitive with
/// the pipeline its material needs (masked ones bind their material).
fn draw_prepass(pass: &mut wgpu::RenderPass<'_>, state: &Renderer, (cubes, spheres, meshes): DepthRanges<'_>, materials: &[String]) {
    let pp = &state.prepass;
    let pipeline = |mat: u32| state.depth_pipeline(materials, mat, DepthPass::Prepass);
    pass.set_bind_group(0, &pp.bg, &[]);
    pass.set_bind_group(1, &state.identity_joints_bind_group, &[]);
    pass.set_vertex_buffer(1, state.instance_buffer.slice(..));
    for (buffers, ranges) in [
        (
            (&state.cube_vertex_buffer, &state.cube_index_buffer, state.cube_index_count),
            cubes,
        ),
        (
            (&state.sphere_vertex_buffer, &state.sphere_index_buffer, state.sphere_index_count),
            spheres,
        ),
    ] {
        for ((_, mat), range) in ranges {
            if range.1 > range.0 {
                pass.set_pipeline(pipeline(*mat));
                pass.set_vertex_buffer(0, buffers.0.slice(..));
                pass.set_index_buffer(buffers.1.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..buffers.2, 0, range.0..range.1);
            }
        }
    }
    for ((mesh_id, tex, mat), range) in meshes {
        if range.1 <= range.0 {
            continue;
        }
        let Some(gpu_mesh) = state.mesh_cache.get(mesh_id) else {
            continue;
        };
        if let Some(skin) = &gpu_mesh.skin {
            pass.set_bind_group(1, &skin.joint_bind_group, &[]);
        }
        pass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(gpu_mesh.index_buffer.slice(..), gpu_mesh.index_format);
        if *tex != 0 || *mat != 0 {
            pass.set_pipeline(pipeline(*mat));
            pass.draw_indexed(0..gpu_mesh.index_count, 0, range.0..range.1);
        } else {
            for sub in gpu_mesh.submeshes.iter().filter(|s| !s.sorted()) {
                if sub.masked {
                    pass.set_pipeline(&pp.masked);
                    pass.set_bind_group(2, &sub.material, &[]);
                } else if sub.double_sided {
                    pass.set_pipeline(&pp.double);
                } else {
                    pass.set_pipeline(&pp.opaque);
                }
                pass.draw_indexed(sub.first..sub.first + sub.count, 0, range.0..range.1);
            }
        }
        if gpu_mesh.skin.is_some() {
            pass.set_bind_group(1, &state.identity_joints_bind_group, &[]);
        }
    }
}

/// web3d-M7: point-light shadow resources — a cube-array depth map
/// (POINT_SHADOW_LIGHTS cubes of POINT_SHADOW_SIZE² faces), one pass
/// uniform per face, and a depth pipeline without face culling (the
/// face cameras are mirrored to match cube-map sampling, which flips
/// winding).
struct PointShadows {
    layer_views: Vec<wgpu::TextureView>,
    pass_buffers: Vec<wgpu::Buffer>,
    pass_bgs: Vec<wgpu::BindGroup>,
    cube_array: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
}

impl PointShadows {
    fn new(
        device: &wgpu::Device,
        pass_bgl: &wgpu::BindGroupLayout,
        joints_bgl: &wgpu::BindGroupLayout,
        shader: &wgpu::ShaderModule,
    ) -> Self {
        let layers = (POINT_SHADOW_LIGHTS * 6) as u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("twe-kernel point shadow cubes"),
            size: wgpu::Extent3d {
                width: POINT_SHADOW_SIZE,
                height: POINT_SHADOW_SIZE,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let layer_views = (0..layers)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("twe-kernel point shadow face"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let pass_buffers: Vec<wgpu::Buffer> = (0..layers)
            .map(|_| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("twe-kernel point shadow pass"),
                    contents: bytemuck::bytes_of(&ShadowPassUniform::identity()),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                })
            })
            .collect();
        let pass_bgs = pass_buffers
            .iter()
            .map(|b| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("twe-kernel point shadow pass bg"),
                    layout: pass_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: b.as_entire_binding(),
                    }],
                })
            })
            .collect();
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel point shadow layout"),
            bind_group_layouts: &[Some(pass_bgl), Some(joints_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel point shadow"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_shadow"),
                buffers: &[Some(Vertex::layout()), Some(Instance::layout())],
                compilation_options: Default::default(),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        PointShadows {
            layer_views,
            pass_buffers,
            pass_bgs,
            cube_array: texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("twe-kernel point shadow cube array"),
                dimension: Some(wgpu::TextureViewDimension::CubeArray),
                ..Default::default()
            }),
            pipeline,
        }
    }
}

/// web3d-M7: the passes of a frame, as the render graph schedules them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FramePass {
    /// Depth from the sun, into one cascade layer of the shadow map.
    Shadow(usize),
    /// web3d-M7: depth from a point light into one face (light × 6 +
    /// face) of the point-shadow cube array.
    PointShadow(usize),
    /// web3d-M7: camera depth only (for ambient occlusion).
    Prepass,
    /// web3d-M7: GTAO and its blur, from the prepass depth.
    Ao,
    /// The lit scene into the HDR target.
    Main,
    /// web3d-M7: list each cluster's lights.
    Clusters,
    /// web3d-M7: GPU culling — the early set (frustum + visible last
    /// frame), then the depth pyramid and the late set.
    CullEarly,
    CullLate,
    /// web3d-M7: the late opaque draws, then the backdrop and
    /// transparent surfaces (when GPU culling splits the main pass).
    MainLate,
    /// web3d-M7: the opaque frame into the transmission source's mips.
    TransmissionCopy,
    /// web3d-M7: the transparent list over the opaque frame (when the
    /// main pass is split for transmission).
    MainTransparent,
    /// web3d-M7: trace screen-space reflections, then add them.
    Ssr,
    SsrComposite,
    /// web3d-M7: light and integrate the fog volume, then blend it.
    FogVolume,
    FogApply,
    /// web3d-M7: spawn and update the GPU particles.
    ParticlesSim,
    /// web3d-M7: draw the particles into the WBOIT targets.
    ParticlesDraw,
    /// web3d-M7: blend the particles over the frame.
    ParticlesComposite,
    /// web3d-M7: temporal anti-aliasing resolve into the history.
    Taa,
    /// web3d-M7: depth of field (bokeh gather + composite).
    Dof,
    /// web3d-M7: camera motion blur.
    MotionBlur,
    /// web3d-M7: the bloom chain, from the (anti-aliased) HDR frame.
    Bloom,
    /// web3d-M7: measure the frame and adapt the exposure.
    Exposure,
    /// HDR to the display (tonemap, bloom, vignette), then the HUD.
    Tonemap,
}

/// web3d-M7: the fewest opaque instances worth culling on the GPU
/// (below this they are drawn directly; the GPU clips what's outside
/// the view). See `render()`.
pub(crate) const GPU_CULL_MIN_INSTANCES: u32 = 4096;

/// web3d-M7: which GPU to ask for. Natively, a 3D game wants the
/// discrete GPU on machines with two (the wgpu default picks the
/// integrated one); `TWE_GPU_POWER=low` asks for the integrated,
/// power-saving one (benchmarking the weakest target, or a laptop on
/// battery). The browser picks the adapter itself (Chrome ignores the
/// hint on Windows and warns about it), so the web build doesn't ask.
fn power_preference() -> wgpu::PowerPreference {
    // In the browser the browser chooses. (web3d-M7 session 16 tried
    // "high-performance": Chrome on Windows ignores it and logs a
    // warning, crbug.com/369219127; the player picks the GPU in the
    // OS's graphics settings.)
    #[cfg(target_arch = "wasm32")]
    {
        wgpu::PowerPreference::None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if std::env::var("TWE_GPU_POWER").is_ok_and(|v| v.eq_ignore_ascii_case("low")) {
            wgpu::PowerPreference::LowPower
        } else {
            wgpu::PowerPreference::HighPerformance
        }
    }
}

/// Headless colour target matching `config`'s size + format.
fn create_offscreen(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("twe-kernel offscreen target"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// The tonemap pass's inputs: the HDR target, its sampler and the
/// per-frame params.
fn tonemap_bind_group(
    state: &Renderer,
    hdr: &wgpu::TextureView,
    bloom: &wgpu::TextureView,
) -> wgpu::BindGroup {
    let (device, layout, sampler, params) = (
        &state.device,
        &state.tonemap_bgl,
        &state.tonemap_sampler,
        &state.tonemap_params_buffer,
    );
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twec-play3d tonemap bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(hdr),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(bloom),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: state.exposure.state().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&state.lut.view),
            },
        ],
    })
}

/// Inverse of a 4×4 matrix (cofactor expansion; column-major, like the
/// rest of this file). Returns the identity for a singular matrix.
fn invert4(m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let a: Vec<f64> = m.iter().flatten().map(|v| f64::from(*v)).collect();
    let mut inv = [0.0f64; 16];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15] + a[9] * a[7] * a[14] + a[13] * a[6] * a[11] - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15] - a[8] * a[7] * a[14] - a[12] * a[6] * a[11] + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15] + a[8] * a[7] * a[13] + a[12] * a[5] * a[11] - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14] - a[8] * a[6] * a[13] - a[12] * a[5] * a[10] + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15] - a[9] * a[3] * a[14] - a[13] * a[2] * a[11] + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15] + a[8] * a[3] * a[14] + a[12] * a[2] * a[11] - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15] - a[8] * a[3] * a[13] - a[12] * a[1] * a[11] + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14] + a[8] * a[2] * a[13] + a[12] * a[1] * a[10] - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15] + a[5] * a[3] * a[14] + a[13] * a[2] * a[7] - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15] - a[4] * a[3] * a[14] - a[12] * a[2] * a[7] + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15] + a[4] * a[3] * a[13] + a[12] * a[1] * a[7] - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14] - a[4] * a[2] * a[13] - a[12] * a[1] * a[6] + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11] - a[5] * a[3] * a[10] - a[9] * a[2] * a[7] + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11] + a[4] * a[3] * a[10] + a[8] * a[2] * a[7] - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11] - a[4] * a[3] * a[9] - a[8] * a[1] * a[7] + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10] + a[4] * a[2] * a[9] + a[8] * a[1] * a[6] - a[8] * a[2] * a[5];
    let det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    if det.abs() < 1e-30 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
    }
    let mut out = [[0.0f32; 4]; 4];
    for (i, v) in inv.iter().enumerate() {
        out[i / 4][i % 4] = (v / det) as f32;
    }
    out
}

fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fovy * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, (near * far) / (near - far), 0.0],
    ]
}

fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k][row] * b[col][k];
            }
            out[col][row] = sum;
        }
    }
    out
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 {
        v
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    /// web3d-M7: the built-in shapes wind counter-clockwise seen from
    /// outside (the front faces the pipelines keep), so every
    /// triangle's geometric normal points away from the centre. The
    /// sphere was wound the other way and drew inside-out under
    /// back-face culling.
    #[test]
    fn built_in_shapes_face_outward() {
        let (sphere_vertices, sphere_indices) = sphere_mesh();
        for (name, vertices, indices) in [
            ("cube", CUBE_VERTICES, CUBE_INDICES),
            ("sphere", sphere_vertices.as_slice(), sphere_indices.as_slice()),
        ] {
            for tri in indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|k| vertices[tri[k] as usize].position);
                let n = cross(sub(b, a), sub(c, a));
                if dot(n, n) < 1e-12 {
                    continue; // degenerate (the sphere's poles)
                }
                let centroid = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
                assert!(dot(n, centroid) > 0.0, "{name}: triangle {tri:?} faces inward");
            }
        }
    }

    fn approx_mat(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> bool {
        (0..4).all(|c| (0..4).all(|r| approx(a[c][r], b[c][r])))
    }

    #[test]
    fn mul_identity_is_identity() {
        let id: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let p = perspective(60_f32.to_radians(), 1.0, 0.1, 100.0);
        assert!(approx_mat(mul(id, p), p));
        assert!(approx_mat(mul(p, id), p));
    }

    #[test]
    fn look_at_eye_at_origin_facing_minus_z_is_identity_axes() {
        let m = look_at([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        assert!(approx(m[0][0], 1.0));
        assert!(approx(m[1][1], 1.0));
        assert!(approx(m[2][2], 1.0));
        assert!(approx(m[3][3], 1.0));
    }

    #[test]
    fn perspective_maps_near_plane_to_zero_depth() {
        let near = 0.1;
        let p = perspective(60_f32.to_radians(), 1.0, near, 100.0);
        let point = [0.0, 0.0, -near, 1.0];
        let mut out = [0.0; 4];
        for r in 0..4 {
            for c in 0..4 {
                out[r] += p[c][r] * point[c];
            }
        }
        assert!(out[3] > 0.0);
        assert!(approx(out[2] / out[3], 0.0));
    }

    #[test]
    fn cross_basis_vectors() {
        // x × y = z
        let z = cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(approx(z[0], 0.0));
        assert!(approx(z[1], 0.0));
        assert!(approx(z[2], 1.0));
    }

    #[test]
    fn normalize_unit_vector_unchanged() {
        let v = normalize([0.0, 0.0, 1.0]);
        assert!(approx(v[2], 1.0));
    }

    #[test]
    fn normalize_zero_vector_safe() {
        // Don't divide by zero — return as-is rather than NaN.
        let v = normalize([0.0, 0.0, 0.0]);
        assert_eq!(v, [0.0, 0.0, 0.0]);
    }

    // ---------- v0.2 session 1: .glb loader ----------

    /// Build a minimal valid .glb in memory: one mesh, one
    /// primitive, three vertices forming a triangle, three u32
    /// indices, no normals (loader fills with up-vector). Used to
    /// exercise `parse_glb_bytes` without shipping binary fixtures.
    fn make_minimal_glb() -> Vec<u8> {
        let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let indices: [u32; 3] = [0, 1, 2];
        let pos_bytes = bytemuck::cast_slice::<f32, u8>(&positions).to_vec();
        let idx_bytes = bytemuck::cast_slice::<u32, u8>(&indices).to_vec();
        let bin: Vec<u8> = [pos_bytes.as_slice(), idx_bytes.as_slice()].concat();

        // POSITION accessors require `min`/`max` per the glTF spec
        // (used by culling / bounds checks). For our triangle:
        // min = [0, 0, 0], max = [1, 1, 0].
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}},{{"bufferView":1,"componentType":5125,"count":3,"type":"SCALAR"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":12}}],"buffers":[{{"byteLength":{}}}]}}"#,
            bin.len()
        );
        let mut json_bytes = json.into_bytes();
        // glTF chunk data must be 4-byte aligned. JSON pads with
        // spaces (0x20), BIN pads with null bytes (0x00).
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut bin_bytes = bin;
        while !bin_bytes.len().is_multiple_of(4) {
            bin_bytes.push(0);
        }

        let total_len: u32 = 12 + 8 + json_bytes.len() as u32 + 8 + bin_bytes.len() as u32;
        let mut out: Vec<u8> = Vec::with_capacity(total_len as usize);
        // 12-byte header.
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&total_len.to_le_bytes());
        // Chunk 0: JSON.
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        // Chunk 1: BIN. Type tag is "BIN\0".
        out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0x42, 0x49, 0x4E, 0x00]);
        out.extend_from_slice(&bin_bytes);
        out
    }

    /// Pack a glTF JSON (with `{bin_len}` for the buffer length) and a
    /// binary chunk into a .glb.
    fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut j = json.replace("{bin_len}", &bin.len().to_string()).into_bytes();
        while !j.len().is_multiple_of(4) {
            j.push(b' ');
        }
        let mut b = bin.to_vec();
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        let total = (12 + 8 + j.len() + 8 + b.len()) as u32;
        let mut out = b"glTF".to_vec();
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&total.to_le_bytes());
        out.extend_from_slice(&(j.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&j);
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&b);
        out
    }

    /// web3d-M7: each primitive becomes a submesh with its own glTF
    /// material; factors, alpha mode, double-sidedness, emissive
    /// strength and vertex colours are read; a primitive without a
    /// material gets glTF's default (metallic 1, roughness 1).
    #[test]
    fn parse_glb_reads_a_material_per_primitive() {
        let positions: [f32; 18] = [
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, // triangle 0
            2.0, 0.0, 0.0, 3.0, 0.0, 0.0, 2.0, 1.0, 0.0, // triangle 1
        ];
        let colors: [f32; 24] = [0.5; 24];
        let mut bin = bytemuck::cast_slice::<f32, u8>(&positions).to_vec();
        bin.extend_from_slice(bytemuck::cast_slice::<f32, u8>(&colors));
        let json = r#"{"asset":{"version":"2.0"},"extensionsUsed":["KHR_materials_emissive_strength"],
            "scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],
            "meshes":[{"primitives":[
                {"attributes":{"POSITION":0,"COLOR_0":2},"material":0},
                {"attributes":{"POSITION":1},"material":1},
                {"attributes":{"POSITION":1}}]}],
            "materials":[
                {"pbrMetallicRoughness":{"baseColorFactor":[1,0,0,1],"metallicFactor":0.25,"roughnessFactor":0.75},"doubleSided":true},
                {"emissiveFactor":[0,0,1],"extensions":{"KHR_materials_emissive_strength":{"emissiveStrength":4}},"alphaMode":"MASK","alphaCutoff":0.3}],
            "accessors":[
                {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},
                {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3","min":[2,0,0],"max":[3,1,0]},
                {"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"}],
            "bufferViews":[
                {"buffer":0,"byteOffset":0,"byteLength":36},
                {"buffer":0,"byteOffset":36,"byteLength":36},
                {"buffer":0,"byteOffset":72,"byteLength":48}],
            "buffers":[{"byteLength":{bin_len}}]}"#;
        let data = parse_glb_bytes(&glb(json, &bin)).expect("decode").0;
        assert_eq!(data.submeshes.len(), 3);
        let mats: Vec<usize> = data.submeshes.iter().map(|s| s.material).collect();
        assert_eq!(mats, [0, 1, 2], "the third primitive gets the default material");
        assert_eq!(
            data.submeshes.iter().map(|s| (s.first, s.count)).collect::<Vec<_>>(),
            [(0, 3), (3, 3), (6, 3)]
        );
        let red = &data.materials[0];
        assert_eq!(red.base_color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!((red.metallic, red.roughness), (0.25, 0.75));
        assert!(red.double_sided);
        let glow = &data.materials[1];
        assert_eq!(glow.emissive, [0.0, 0.0, 4.0], "emissive strength applied");
        assert_eq!(glow.alpha_mode, crate::kernel::material::AlphaMode::Mask);
        assert_eq!(glow.alpha_cutoff, 0.3);
        let default = &data.materials[2];
        assert_eq!((default.metallic, default.roughness), (1.0, 1.0));
        assert_eq!(data.vertices[0].color, [0.5; 4]);
        assert_eq!(data.vertices[3].color, [1.0; 4], "no COLOR_0: white");
    }

    #[test]
    fn parse_glb_extracts_positions_and_indices() {
        let bytes = make_minimal_glb();
        let GlbData {
            vertices, indices, ..
        } = *parse_glb_bytes(&bytes).expect("decode").0;
        assert_eq!(vertices.len(), 3);
        assert_eq!(indices, vec![0, 1, 2]);
        assert_eq!(vertices[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(vertices[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(vertices[2].position, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn parse_glb_gives_missing_normals_flat_face_normals() {
        // The fixture omits NORMAL: glTF requires flat normals, so the
        // CCW triangle in the z = 0 plane faces +z (web3d-M7; it used
        // to get [0, 1, 0]).
        let bytes = make_minimal_glb();
        let vertices = parse_glb_bytes(&bytes).expect("decode").0.vertices;
        for v in &vertices {
            assert_eq!(v.normal, [0.0, 0.0, 1.0]);
        }
    }

    #[test]
    fn parse_glb_rejects_garbage() {
        // Random bytes — gltf::import_slice should refuse the magic.
        assert!(parse_glb_bytes(b"not a glb").is_err());
    }

    /// One-shot fixture generator: writes the minimal triangle
    /// `.glb` to `examples/assets/triangle.glb`. Marked `#[ignore]`
    /// so it only runs when explicitly requested:
    ///
    ///   cargo test --release write_triangle_glb_fixture -- --ignored
    ///
    /// Re-run after changing `make_minimal_glb` to refresh the
    /// committed file. The committed binary is what
    /// `examples/hello_glb.twe` loads at run time.
    #[test]
    #[ignore]
    fn write_triangle_glb_fixture() {
        let path = std::path::Path::new("examples/assets/triangle.glb");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dir");
        }
        let bytes = make_minimal_glb();
        std::fs::write(path, &bytes).expect("write fixture");
        // Round-trip check: the file we just wrote must decode.
        let written = std::fs::read(path).expect("read fixture back");
        let GlbData {
            vertices, indices, ..
        } = *parse_glb_bytes(&written).expect("decode").0;
        assert_eq!(vertices.len(), 3);
        assert_eq!(indices.len(), 3);
    }

    /// Phase 27: every WGSL shader the play3d pipeline ships
    /// must parse + validate cleanly under naga (wgpu's WGSL
    /// frontend). A failure here would also fail at GPU init.
    /// Catches typos, bind-group / location mismatches, and
    /// invalid type usage without needing a window or adapter.
    /// Passing is necessary, not sufficient: in the browser the
    /// WGSL goes to Tint, which is stricter (see
    /// `shaders_avoid_implicit_derivatives_in_non_uniform_flow`).
    fn validate_wgsl(label: &str, src: &str) {
        let module = match naga::front::wgsl::parse_str(src) {
            Ok(m) => m,
            Err(e) => panic!("naga parse failed for {label}:\n{}\n--- WGSL ---\n{src}", e),
        };
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        );
        if let Err(e) = validator.validate(&module) {
            panic!(
                "naga validate failed for {label}:\n{:?}\n--- WGSL ---\n{src}",
                e
            );
        }
    }

    #[test]
    fn main_shader_parses_and_validates() {
        validate_wgsl("SHADER_SRC", SHADER_SRC);
    }

    /// web3d-M2: Chrome's Tint rejected the main shader (black
    /// canvas) because `sample_shadow` called `textureSampleCompare`
    /// after a data-dependent early return; naga accepted it. Shadow
    /// lookups must use the explicit-level form, which is valid in
    /// non-uniform control flow.
    #[test]
    fn shaders_avoid_implicit_derivatives_in_non_uniform_flow() {
        for (label, src) in [
            ("SHADER_SRC", SHADER_SRC),
            ("SHADOW_SHADER_SRC", SHADOW_SHADER_SRC),
            ("TONEMAP_SHADER_SRC", TONEMAP_SHADER_SRC),
            ("PREPASS_MASK_SRC", PREPASS_MASK_SRC),
        ] {
            assert!(
                !src.contains("textureSampleCompare("),
                "{label}: use textureSampleCompareLevel (Tint rejects \
                 textureSampleCompare in non-uniform control flow)"
            );
        }
    }

    /// web3d-M3: every `visual` block in the examples, compiled as a
    /// mesh material, gives a valid shader (and one Tint won't reject
    /// for implicit-derivative sampling).
    #[test]
    fn example_visuals_compile_to_valid_material_shaders() {
        let mut checked = 0;
        for entry in std::fs::read_dir("examples").expect("examples dir") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|x| x != "twe") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read");
            let Ok(tokens) = crate::lexer::lex(&src) else {
                continue;
            };
            let Ok(program) = crate::parser::parse(&tokens) else {
                continue;
            };
            for stmt in &program.stmts {
                if let crate::ast::Stmt::Decl {
                    kind: crate::ast::DeclKind::Visual,
                    name,
                    members,
                    ..
                } = stmt
                {
                    let pixel = crate::visual_wgsl::compile_material(name, members)
                        .unwrap_or_else(|e| panic!("{}: {name}: {}", path.display(), e.message));
                    let full = material_shader_source(&pixel);
                    validate_wgsl(name, &full);
                    if material_displaces(&pixel) {
                        validate_wgsl(name, &material_depth_source(&pixel));
                    }
                    assert!(!full.contains("textureSampleCompare("));
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "no visual blocks found in examples/");
    }

    #[test]
    fn hud_shader_parses_and_validates() {
        validate_wgsl("HUD_SHADER", crate::kernel::hud::HUD_SHADER);
    }

    #[test]
    fn shadow_shader_parses_and_validates() {
        validate_wgsl("SHADOW_SHADER_SRC", SHADOW_SHADER_SRC);
    }

    fn test_camera(eye: [f32; 3], target: [f32; 3]) -> CascadeCamera {
        let forward = normalize(sub(target, eye));
        let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
        CascadeCamera {
            eye,
            forward,
            right,
            up: cross(right, forward),
            tan_y: (30_f32).to_radians().tan(),
            tan_x: (30_f32).to_radians().tan() * 1.5,
            near: 0.1,
            far: 100.0,
        }
    }

    fn sun_lights() -> LightsUniform {
        let mut l: LightsUniform = bytemuck::Zeroable::zeroed();
        let d = normalize([0.4, 1.0, 0.3]);
        l.sun_dir = [d[0], d[1], d[2], 1.0];
        l
    }

    fn clip(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
        let mut c = [0.0f32; 4];
        for (col, v) in m.iter().zip([p[0], p[1], p[2], 1.0]) {
            for r in 0..4 {
                c[r] += col[r] * v;
            }
        }
        [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
    }

    /// web3d-M7: cascades cover their slice of the camera frustum.
    #[test]
    fn cascades_are_fitted_to_the_view() {
        let splits = cascade_splits(0.1, 40.0);
        assert_eq!(splits[0], 0.1);
        assert!((splits[CASCADE_COUNT] - 40.0).abs() < 1e-3);
        assert!(splits.windows(2).all(|w| w[0] < w[1]), "{splits:?}");

        let cam = test_camera([3.0, 6.0, 9.0], [0.0, 0.0, 0.0]);
        let shadow = ShadowSettings {
            enabled: true,
            extent: 10.0,
        };
        let u = compute_shadow_uniform(&sun_lights(), &cam, shadow);
        let splits = cascade_splits(cam.near, 40.0);
        for i in 0..CASCADE_COUNT {
            assert!((u.split_distances[i] - splits[i + 1]).abs() < 1e-4);
            for d in [splits[i], splits[i + 1]] {
                for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let p: [f32; 3] = std::array::from_fn(|k| {
                        cam.eye[k]
                            + cam.forward[k] * d
                            + cam.right[k] * sx * cam.tan_x * d
                            + cam.up[k] * sy * cam.tan_y * d
                    });
                    let q = clip(u.light_space_matrices[i], p);
                    assert!(
                        q[0].abs() <= 1.0 && q[1].abs() <= 1.0 && (0.0..=1.0).contains(&q[2]),
                        "cascade {i}: corner at depth {d} maps to {q:?}"
                    );
                }
            }
        }
        // Nearer cascades are denser.
        assert!(u.cascade_params[0][0] < u.cascade_params[1][0]);
        assert!(u.cascade_params[1][0] < u.cascade_params[2][0]);
    }

    /// web3d-M7: moving the camera slides each cascade by whole texels,
    /// so static shadow edges land on the same texels (no shimmer).
    #[test]
    fn cascades_move_in_whole_texels() {
        let shadow = ShadowSettings {
            enabled: true,
            extent: 10.0,
        };
        let a = compute_shadow_uniform(&sun_lights(), &test_camera([3.0, 6.0, 9.0], [0.0; 3]), shadow);
        let b = compute_shadow_uniform(
            &sun_lights(),
            &test_camera([3.013, 6.0, 9.021], [0.013, 0.0, 0.021]),
            shadow,
        );
        for i in 0..CASCADE_COUNT {
            assert_eq!(a.cascade_params[i][0], b.cascade_params[i][0], "same size");
            // A world point's shadow-map texel coordinate shifts by an
            // integer between the two frames.
            let p = [1.0, 0.0, -2.0];
            let (qa, qb) = (clip(a.light_space_matrices[i], p), clip(b.light_space_matrices[i], p));
            for k in 0..2 {
                let shift = (qa[k] - qb[k]) * 0.5 * SHADOW_MAP_SIZE as f32;
                assert!((shift - shift.round()).abs() < 0.02, "cascade {i}: {shift} texels");
            }
        }
    }

    /// web3d-M7: each point-shadow face camera draws the direction a
    /// cube sampler reads at that texel (the standard face table, as in
    /// kernel/environment.rs).
    #[test]
    fn point_shadow_faces_match_cube_sampling() {
        let face_dir = |face: usize, u: f32, v: f32| -> [f32; 3] {
            let (s, t) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
            match face {
                0 => [1.0, -t, -s],
                1 => [-1.0, -t, s],
                2 => [s, 1.0, t],
                3 => [s, -1.0, -t],
                4 => [s, -t, 1.0],
                _ => [-s, -t, -1.0],
            }
        };
        let light = [1.0, 2.0, 3.0];
        for face in 0..6 {
            let m = point_face_view_proj(light, face, 20.0);
            for (u, v) in [(0.25, 0.25), (0.8, 0.3), (0.5, 0.9)] {
                let d = face_dir(face, u, v);
                let p = [light[0] + d[0] * 2.0, light[1] + d[1] * 2.0, light[2] + d[2] * 2.0];
                let q = clip(m, p);
                assert!((q[0] - (u * 2.0 - 1.0)).abs() < 1e-4, "face {face} u: {q:?}");
                assert!((q[1] - (1.0 - v * 2.0)).abs() < 1e-4, "face {face} v: {q:?}");
                assert!((0.0..1.0).contains(&q[2]));
            }
        }
    }

    /// web3d-M7: the backdrop and the environment precompute shaders.
    #[test]
    fn environment_shaders_parse_and_validate() {
        validate_wgsl("SKY_SHADER_SRC", SKY_SHADER_SRC);
        validate_wgsl("TAA", crate::kernel::taa::shader_source());
        validate_wgsl(
            "PRECOMPUTE_SHADER",
            crate::kernel::environment::PRECOMPUTE_SHADER,
        );
    }

    #[test]
    fn tonemap_shader_parses_and_validates() {
        validate_wgsl("TONEMAP_SHADER_SRC", TONEMAP_SHADER_SRC);
    }

    /// web3d-M7 session 7: the prepass, AO, bloom and exposure shaders.
    #[test]
    fn post_shaders_parse_and_validate() {
        validate_wgsl("PREPASS_MASK_SRC", PREPASS_MASK_SRC);
        validate_wgsl("AO", crate::kernel::ao::shader_source());
        validate_wgsl("BLOOM", crate::kernel::post::bloom_shader_source());
        validate_wgsl("EXPOSURE", crate::kernel::post::exposure_shader_source());
        validate_wgsl("DOF", crate::kernel::post::dof_shader_source());
        validate_wgsl("MOTION", crate::kernel::post::motion_shader_source());
        validate_wgsl("TRANSMISSION", crate::kernel::post::transmission_shader_source());
        let [cull, hiz] = crate::kernel::gpu_cull::shader_sources();
        validate_wgsl("CULL", cull);
        validate_wgsl("HIZ", hiz);
        validate_wgsl("CLUSTERS", crate::kernel::clusters::shader_source());
    }

    /// Phase 27: the Vertex layout's stride must match what the
    /// shader expects: position + normal + uv (32 B), joints +
    /// weights (24 B), and web3d-M7's uv1 + tangent + colour (40 B)
    /// = 96 B. A mismatch would crash on pipeline creation or
    /// silently scramble vertex data.
    #[test]
    fn vertex_layout_stride_matches() {
        assert_eq!(std::mem::size_of::<Vertex>(), 96);
    }

    /// Phase 27: the joint UBO size must fit in the default wgpu
    /// max_uniform_buffer_binding_size (64 KB). 128 mat4 = 8 KB,
    /// well within budget. If MAX_JOINTS or the matrix size ever
    /// changes, this catches the regression at test time.
    #[test]
    fn joints_uniform_size_within_ubo_budget() {
        assert_eq!(std::mem::size_of::<JointsUniform>(), 128 * 64);
        assert!(std::mem::size_of::<JointsUniform>() <= 65_536);
    }
}
