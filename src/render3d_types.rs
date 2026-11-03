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

/// web3d-M4 / M11: one animation's playback — a mesh's shared one
/// (`mesh_anim.play("hero.glb", ...)`) or, Lanternfall E4, one entity's
/// own (`mesh_anim.play(self, ...)`, stored on the entity).
#[derive(Clone, Default, Debug)]
pub struct AnimEntry {
    /// Active clip name; empty string = stopped.
    pub clip: String,
    /// Time elapsed since clip started, in seconds.
    pub time: f32,
    /// Whether the clip loops at the end (vs holding the last frame).
    pub looping: bool,
    /// Optional blend target — when present, the renderer interpolates
    /// joints between `clip` and `blend_clip` by `blend_t` ∈ [0, 1].
    pub blend_clip: Option<String>,
    pub blend_t: f32,
    /// web3d-M11 session 1: the blend target's own playback time, so a
    /// crossfade's outgoing clip (or an explicit `blend` partner)
    /// animates independently of the primary clip.
    pub blend_time: f32,
    /// Seconds left in an auto-crossfade started by `play()` (0 = none).
    pub crossfade_remaining: f32,
    /// The crossfade's total duration, for computing `blend_t`.
    pub crossfade_duration: f32,
    /// Crossfade length (`None` → the default), set by
    /// `mesh_anim.set_crossfade`. 0 means hard clip switches.
    pub crossfade_config: Option<f32>,
}

impl AnimEntry {
    /// Advance playback by `dt` seconds, ramping an auto-crossfade out.
    pub fn advance(&mut self, dt: f32) {
        self.time += dt;
        self.blend_time += dt;
        if self.crossfade_remaining > 0.0 {
            self.crossfade_remaining = (self.crossfade_remaining - dt).max(0.0);
            self.blend_t = if self.crossfade_duration > 0.0 {
                (self.crossfade_remaining / self.crossfade_duration).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if self.crossfade_remaining <= 0.0 {
                self.blend_clip = None;
                self.blend_t = 0.0;
            }
        }
    }

    /// What the renderer needs to pose a skin.
    pub fn snapshot(&self) -> AnimSnapshot {
        AnimSnapshot {
            clip: self.clip.clone(),
            time: self.time,
            blend_clip: self.blend_clip.clone(),
            blend_t: self.blend_t,
            blend_time: self.blend_time,
        }
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
    /// web3d-M11 session 1: the blend target's own playback time (for a
    /// crossfade's outgoing clip), independent of `time`.
    pub blend_time: f32,
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
    /// Lanternfall E3: orientation after `yaw`, an (x, y, z, w)
    /// quaternion from a look's `rotation:` ([`NO_ROTATION`] otherwise).
    pub rotation: [f32; 4],
    /// Lanternfall E3: per-axis scale, multiplied by `size` (a look's
    /// vec3 `scale:`; `[1.0; 3]` otherwise).
    pub scale: [f32; 3],
    /// Lanternfall E3: emission added to the lit colour, in the
    /// `emission:` convention (sRGB up to 1, HDR above) — a look's
    /// `glow:`; `[0.0; 3]` otherwise.
    pub glow: [f32; 3],
    /// Lanternfall E4: 0 = a mesh draw posed by the mesh's shared
    /// animation; n > 0 = posed by `RenderSnapshot::instance_anims[n - 1]`
    /// (an entity's own, from `mesh_anim.play(self, ...)`).
    pub anim: u32,
}

/// The identity quaternion: a draw that isn't tilted.
pub const NO_ROTATION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

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

/// web3d-M3: a HUD element drawn over the 3D scene. web3d-M6: the same
/// command every 2D draw pushes (`crate::draw2d`).
pub use crate::draw2d::Draw2d as HudItem;
