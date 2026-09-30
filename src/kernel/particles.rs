//! web3d-M7: GPU particles — up to a million, simulated by compute,
//! bounced off the depth buffer, drawn through weighted blended
//! order-independent transparency (McGuire & Bavoil 2013).
//!
//! The kernel knows nothing of Twe syntax: a host hands it programs
//! (WGSL bodies for spawning and updating one particle; the language
//! side compiles them from `particles` blocks in `particles_wgsl.rs`)
//! and emissions (spawn `count` particles of program `k` at a point).
//! Per frame:
//!
//! 1. **Spawn** (compute): each new particle finds its emission, is
//!    initialised, runs its program's spawn body, and is written into
//!    the pool. The pool is a ring sized to the particles alive.
//! 2. **Update** (compute, over the pool): each live particle runs its
//!    program's update body, ages, and (programs with `collide`)
//!    bounces off the depth buffer.
//! 3. **Draw**: a camera-facing soft disc per particle into an
//!    accumulation and a revealage target, depth-tested against the
//!    scene; no sorting.
//! 4. **Composite**: the weighted average over the HDR frame.
//!
//! Particles are visual only: nothing reads them back, so the
//! simulation stays deterministic whatever the GPU does.

use bytemuck::{Pod, Zeroable};

/// A particle program: WGSL statements run with `pt` (the particle,
/// a `var` of type `Particle`), `dt` (update only), `twe_rand()` and
/// the noise / `twe_mod` helpers in scope. `return pt;` ends early.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParticleProgram {
    pub spawn: String,
    pub update: String,
    /// Bounce off the depth buffer after each update.
    pub collide: bool,
}

/// Spawn `count` particles of `program` at `at`, living `lifetime`
/// seconds. `seed` varies their random numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleEmission {
    pub program: u32,
    pub at: [f32; 3],
    pub count: u32,
    pub lifetime: f32,
    pub seed: u32,
}

/// One particle as the GPU stores it (64 bytes). Hosts also use it to
/// hand over particles simulated on the CPU, which are only drawn.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GpuParticle {
    pub pos: [f32; 3],
    pub age: f32,
    pub velocity: [f32; 3],
    pub lifetime: f32,
    /// sRGB colour and linear alpha, as scripts write colours.
    pub color: [f32; 4],
    /// The disc's radius in world units.
    pub size: f32,
    pub age_ratio: f32,
    pub program: u32,
    pub seed: u32,
}

/// A frame's particle input.
#[derive(Clone, Copy, Default)]
pub struct ParticleFrame<'a> {
    /// Program `k` for `ParticleEmission::program == k`. Stable across
    /// frames (a host appends).
    pub programs: &'a [ParticleProgram],
    /// Emitted since the last frame.
    pub emissions: &'a [ParticleEmission],
    /// Particles simulated elsewhere, drawn this frame.
    pub cpu: &'a [GpuParticle],
}

/// The pool never grows past this many particles (64 MB); past it,
/// the oldest are recycled.
pub const MAX_PARTICLES: u32 = 1 << 20;
const MIN_CAPACITY: u32 = 1024;
const WORKGROUP: u32 = 64;
/// Bounciness of `collide` particles (the normal velocity kept).
const RESTITUTION: f32 = 0.5;

pub(crate) const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(crate) const REVEAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// The camera the particles are simulated and drawn with.
pub(crate) struct ParticleCamera {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    right: [f32; 4],
    up: [f32; 4],
    eye: [f32; 4],
    size: [f32; 2],
    dt: f32,
    frame: u32,
    capacity: u32,
    emissions: u32,
    total: u32,
    cursor: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EmissionU {
    pos: [f32; 3],
    program: u32,
    first: u32,
    count: u32,
    seed: u32,
    lifetime: f32,
}

const COMMON: &str = r#"
struct Particle {
    pos: vec3<f32>,
    age: f32,
    velocity: vec3<f32>,
    lifetime: f32,
    color: vec4<f32>,
    size: f32,
    age_ratio: f32,
    program: u32,
    seed: u32,
}

struct Params {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    eye: vec4<f32>,
    size: vec2<f32>,
    dt: f32,
    frame: u32,
    capacity: u32,
    emissions: u32,
    total: u32,
    cursor: u32,
}
"#;

const SIM_HEAD: &str = r#"
struct Emission {
    pos: vec3<f32>,
    program: u32,
    first: u32,
    count: u32,
    seed: u32,
    lifetime: f32,
}

@group(0) @binding(0) var<storage, read_write> pool: array<Particle>;
@group(0) @binding(1) var<storage, read> emissions: array<Emission>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var t_depth: texture_depth_multisampled_2d;

var<private> twe_rng: u32;

fn twe_hash(x: u32) -> u32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

// A float in [0, 1).
fn twe_rand() -> f32 {
    twe_rng = twe_hash(twe_rng);
    return f32(twe_rng >> 8u) / 16777216.0;
}

// Floored modulo, as `math.mod` on the CPU.
fn twe_mod(a: f32, b: f32) -> f32 {
    return a - b * floor(a / b);
}
"#;

const SIM_MAIN: &str = r#"
@compute @workgroup_size(64)
fn cs_spawn(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= params.total) {
        return;
    }
    // The emission this particle belongs to: the last one starting at
    // or before it.
    var lo = 0u;
    var hi = params.emissions - 1u;
    while (lo < hi) {
        let mid = (lo + hi + 1u) / 2u;
        if (emissions[mid].first <= i) {
            lo = mid;
        } else {
            hi = mid - 1u;
        }
    }
    let e = emissions[lo];
    var p: Particle;
    p.pos = e.pos;
    p.velocity = vec3<f32>(0.0);
    p.color = vec4<f32>(1.0);
    p.size = 0.1;
    p.age = 0.0;
    p.age_ratio = 0.0;
    p.lifetime = e.lifetime;
    p.program = e.program;
    p.seed = twe_hash(e.seed ^ twe_hash(i - e.first));
    twe_rng = p.seed;
    p = twe_run_spawn(p);
    p.age = 0.0;
    p.age_ratio = 0.0;
    p.lifetime = e.lifetime;
    p.program = e.program;
    pool[(params.cursor + i) & (params.capacity - 1u)] = p;
}

// The world position the depth buffer holds at pixel `px`.
fn world_at(px: vec2<i32>) -> vec3<f32> {
    let dims = vec2<i32>(params.size);
    let q = clamp(px, vec2<i32>(0), dims - 1);
    let d = textureLoad(t_depth, q, 0);
    let ndc = vec2<f32>((f32(q.x) + 0.5) / params.size.x * 2.0 - 1.0, 1.0 - (f32(q.y) + 0.5) / params.size.y * 2.0);
    let w = params.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return w.xyz / w.w;
}

fn collide(p_in: Particle, prev: vec3<f32>) -> Particle {
    var p = p_in;
    let clip = params.view_proj * vec4<f32>(p.pos, 1.0);
    if (clip.w <= 0.0) {
        return p;
    }
    let ndc = clip.xyz / clip.w;
    if (abs(ndc.x) >= 1.0 || abs(ndc.y) >= 1.0 || ndc.z >= 1.0) {
        return p;
    }
    let px = vec2<i32>(vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * params.size);
    let d = textureLoad(t_depth, px, 0);
    if (ndc.z <= d || d >= 1.0) {
        return p;
    }
    // Behind the visible surface: only a hit if it's just behind (a
    // particle far behind a wall is simply hidden).
    let surface = world_at(px);
    let step = length(p.pos - prev);
    if (distance(p.pos, surface) > max(0.25, 2.0 * step)) {
        return p;
    }
    var n = normalize(cross(world_at(px + vec2<i32>(1, 0)) - surface, world_at(px + vec2<i32>(0, 1)) - surface));
    if (dot(n, params.eye.xyz - surface) < 0.0) {
        n = -n;
    }
    p.pos = prev;
    let vn = dot(p.velocity, n);
    if (vn < 0.0) {
        p.velocity = p.velocity - (1.0 + RESTITUTION) * vn * n;
    }
    return p;
}

@compute @workgroup_size(64)
fn cs_update(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= params.capacity) {
        return;
    }
    var p = pool[i];
    if (!(p.age < p.lifetime)) {
        return;
    }
    twe_rng = twe_hash(p.seed ^ (params.frame * 2654435769u));
    let prev = p.pos;
    let program = p.program;
    let lifetime = p.lifetime;
    p = twe_run_update(p, params.dt);
    // The runtime ages particles after their update, as on the CPU.
    p.program = program;
    p.lifetime = lifetime;
    p.age = p.age + params.dt;
    p.age_ratio = select(1.0, clamp(p.age / p.lifetime, 0.0, 1.0), p.lifetime > 0.0);
    if (twe_collides(program)) {
        p = collide(p, prev);
    }
    pool[i] = p;
}
"#;

const DRAW_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> particles: array<Particle>;
@group(0) @binding(1) var<uniform> params: Params;
@group(1) @binding(0) var t_depth: texture_depth_multisampled_2d;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) depth: f32,
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_particle(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> VOut {
    let p = particles[i];
    var out: VOut;
    if (!(p.age < p.lifetime) || p.size <= 0.0 || p.color.a <= 0.0) {
        // Dead or invisible: a degenerate triangle outside the clip box.
        out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    // A four-vertex strip: (-1, -1), (1, -1), (-1, 1), (1, 1).
    let c = vec2<f32>(f32(v & 1u), f32(v >> 1u)) * 2.0 - 1.0;
    let world = p.pos + (params.right.xyz * c.x + params.up.xyz * c.y) * p.size;
    out.pos = params.view_proj * vec4<f32>(world, 1.0);
    out.corner = c;
    out.color = vec4<f32>(srgb_to_linear(p.color.rgb), clamp(p.color.a, 0.0, 1.0));
    out.depth = out.pos.w;
    return out;
}

struct WboitOut {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: vec4<f32>,
}

@fragment
fn fs_particle(in: VOut) -> WboitOut {
    let r2 = dot(in.corner, in.corner);
    // Outside the disc, or behind the scene (tested here, against the
    // scene's multisampled depth, so the targets can be single-sampled).
    if (r2 >= 1.0 || in.pos.z >= textureLoad(t_depth, vec2<i32>(in.pos.xy), 0)) {
        discard;
    }
    // A soft disc.
    let a = in.color.a * (1.0 - r2) * (1.0 - r2);
    // McGuire & Bavoil's weight (their equation 10), by view depth.
    let z = in.depth / 200.0;
    let w = a * clamp(0.03 / (1e-5 + z * z * z * z), 1e-2, 3e3);
    var out: WboitOut;
    out.accum = vec4<f32>(in.color.rgb * a * w, a * w);
    out.reveal = vec4<f32>(a);
    return out;
}
"#;

const COMPOSITE_SHADER: &str = r#"
@group(0) @binding(0) var t_accum: texture_2d<f32>;
@group(0) @binding(1) var t_reveal: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let reveal = textureLoad(t_reveal, px, 0).r;
    if (reveal >= 0.9999) {
        discard;
    }
    let accum = textureLoad(t_accum, px, 0);
    let average = accum.rgb / max(accum.a, 1e-5);
    return vec4<f32>(average, 1.0 - reveal);
}
"#;

/// The simulation module for `programs`: the shared head, each
/// program's spawn / update function, the dispatchers, the entry
/// points.
pub fn sim_source(programs: &[ParticleProgram]) -> String {
    let mut src = String::from(COMMON);
    src.push_str(SIM_HEAD);
    src.push_str(&format!("const RESTITUTION: f32 = {RESTITUTION:?};\n"));
    src.push_str(crate::visual_wgsl::WGSL_NOISE);
    for (k, p) in programs.iter().enumerate() {
        src.push_str(&format!(
            "fn twe_spawn_{k}(pt_in: Particle) -> Particle {{\n    var pt = pt_in;\n{}    return pt;\n}}\n\n",
            p.spawn
        ));
        src.push_str(&format!(
            "fn twe_update_{k}(pt_in: Particle, dt: f32) -> Particle {{\n    var pt = pt_in;\n{}    return pt;\n}}\n\n",
            p.update
        ));
    }
    let mut spawn = String::from("fn twe_run_spawn(p: Particle) -> Particle {\n    switch p.program {\n");
    let mut update = String::from("fn twe_run_update(p: Particle, dt: f32) -> Particle {\n    switch p.program {\n");
    let mut collides = Vec::new();
    for (k, p) in programs.iter().enumerate() {
        spawn.push_str(&format!("        case {k}u: {{ return twe_spawn_{k}(p); }}\n"));
        update.push_str(&format!("        case {k}u: {{ return twe_update_{k}(p, dt); }}\n"));
        if p.collide {
            collides.push(format!("program == {k}u"));
        }
    }
    spawn.push_str("        default: { return p; }\n    }\n}\n\n");
    update.push_str("        default: { return p; }\n    }\n}\n\n");
    src.push_str(&spawn);
    src.push_str(&update);
    let collides = if collides.is_empty() {
        "false".to_string()
    } else {
        collides.join(" || ")
    };
    src.push_str(&format!(
        "fn twe_collides(program: u32) -> bool {{\n    return {collides};\n}}\n"
    ));
    src.push_str(SIM_MAIN);
    src
}

/// Live particles by when they die, oldest emission first: an estimate
/// of how many the pool must hold.
struct Batch {
    dies: f32,
    count: u32,
}

pub(crate) struct Particles {
    sim_layout: wgpu::BindGroupLayout,
    sim_pipeline_layout: wgpu::PipelineLayout,
    /// The programs the pipelines were built for, and the pipelines.
    programs: Vec<ParticleProgram>,
    spawn: Option<wgpu::ComputePipeline>,
    update: Option<wgpu::ComputePipeline>,
    draw_layout: wgpu::BindGroupLayout,
    depth_layout: wgpu::BindGroupLayout,
    draw_pipeline: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    pool: wgpu::Buffer,
    capacity: u32,
    cursor: u32,
    emissions: wgpu::Buffer,
    emission_capacity: u32,
    cpu: wgpu::Buffer,
    cpu_capacity: u32,
    cpu_count: u32,
    pool_bg: wgpu::BindGroup,
    cpu_bg: wgpu::BindGroup,
    batches: std::collections::VecDeque<Batch>,
    last_time: Option<f32>,
    frame: u32,
    /// This frame's new particles.
    spawned: u32,
}

impl Particles {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let storage = |binding, read_only, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let compute = wgpu::ShaderStages::COMPUTE;
        let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel particle sim bgl"),
            entries: &[
                storage(0, false, compute),
                storage(1, true, compute),
                uniform(2, compute),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: true,
                    },
                    count: None,
                },
            ],
        });
        let sim_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel particle sim layout"),
            bind_group_layouts: &[Some(&sim_layout)],
            immediate_size: 0,
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel particle draw bgl"),
            entries: &[
                storage(0, true, wgpu::ShaderStages::VERTEX),
                uniform(1, wgpu::ShaderStages::VERTEX),
            ],
        });
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel particle depth bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: true,
                },
                count: None,
            }],
        });
        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel particle draw"),
            source: wgpu::ShaderSource::Wgsl(format!("{COMMON}{DRAW_SHADER}").into()),
        });
        let draw_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel particle draw layout"),
            bind_group_layouts: &[Some(&draw_layout), Some(&depth_layout)],
            immediate_size: 0,
        });
        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let reveal = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrc,
            operation: wgpu::BlendOperation::Add,
        };
        let draw_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel particle draw"),
            layout: Some(&draw_pl),
            vertex: wgpu::VertexState {
                module: &draw_shader,
                entry_point: Some("vs_particle"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &draw_shader,
                entry_point: Some("fs_particle"),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: ACCUM_FORMAT,
                        blend: Some(wgpu::BlendState {
                            color: additive,
                            alpha: additive,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: REVEAL_FORMAT,
                        blend: Some(wgpu::BlendState {
                            color: reveal,
                            alpha: reveal,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                ],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel particle composite bgl"),
            entries: &[texture(0), texture(1)],
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel particle composite"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
        });
        let composite_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel particle composite layout"),
            bind_group_layouts: &[Some(&composite_layout)],
            immediate_size: 0,
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel particle composite"),
            layout: Some(&composite_pl),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_full"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_composite"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    // Over the frame; the frame's alpha (transmission's
                    // coverage) is kept.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twe-kernel particle params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pool = particle_buffer(device, "twe-kernel particle pool", MIN_CAPACITY);
        let cpu = particle_buffer(device, "twe-kernel cpu particles", 1);
        let pool_bg = draw_bind_group(device, &draw_layout, &pool, &params);
        let cpu_bg = draw_bind_group(device, &draw_layout, &cpu, &params);
        Particles {
            sim_layout,
            sim_pipeline_layout,
            programs: Vec::new(),
            spawn: None,
            update: None,
            draw_layout,
            depth_layout,
            draw_pipeline,
            composite_layout,
            composite_pipeline,
            params,
            pool,
            capacity: MIN_CAPACITY,
            cursor: 0,
            emissions: emission_buffer(device, 16),
            emission_capacity: 16,
            cpu,
            cpu_capacity: 1,
            cpu_count: 0,
            pool_bg,
            cpu_bg,
            batches: std::collections::VecDeque::new(),
            last_time: None,
            frame: 0,
            spawned: 0,
        }
    }

    /// Take in a frame's particles. Returns whether there is anything
    /// to simulate or draw.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &ParticleFrame,
        camera: &ParticleCamera,
        size: (u32, u32),
        time: f32,
    ) -> bool {
        let dt = self.last_time.map_or(0.0, |t| (time - t).clamp(0.0, 0.1));
        self.last_time = Some(time);
        if frame.programs != self.programs.as_slice() {
            self.build(device, frame.programs);
        }
        // Emissions of programs the pipelines don't have are dropped.
        let emissions: Vec<&ParticleEmission> = frame
            .emissions
            .iter()
            .filter(|e| e.count > 0 && (e.program as usize) < self.programs.len())
            .collect();
        while self.batches.front().is_some_and(|b| b.dies <= time) {
            self.batches.pop_front();
        }
        let live: u64 = self.batches.iter().map(|b| u64::from(b.count)).sum();
        let new: u64 = emissions.iter().map(|e| u64::from(e.count)).sum();
        let total = new.min(u64::from(MAX_PARTICLES)) as u32;
        // Room for twice what's alive: the ring then rarely reaches a
        // particle that outlives its neighbours.
        let want = ((live + new) * 2).min(u64::from(MAX_PARTICLES)) as u32;
        if want > self.capacity {
            self.grow(device, queue, want.next_power_of_two().min(MAX_PARTICLES));
        }
        let mut first = 0u32;
        let mut uploads = Vec::with_capacity(emissions.len());
        for e in &emissions {
            if first >= total {
                break;
            }
            uploads.push(EmissionU {
                pos: e.at,
                program: e.program,
                first,
                count: e.count,
                seed: e.seed,
                lifetime: e.lifetime,
            });
            self.batches.push_back(Batch {
                dies: time + e.lifetime,
                count: e.count,
            });
            first = first.saturating_add(e.count);
        }
        if uploads.len() as u32 > self.emission_capacity {
            self.emission_capacity = (uploads.len() as u32).next_power_of_two();
            self.emissions = emission_buffer(device, self.emission_capacity);
        }
        if !uploads.is_empty() {
            queue.write_buffer(&self.emissions, 0, bytemuck::cast_slice(&uploads));
        }
        // Keep the batches in dying order (emissions differ in lifetime).
        self.batches.make_contiguous().sort_by(|a, b| a.dies.total_cmp(&b.dies));
        self.cpu_count = frame.cpu.len().min(MAX_PARTICLES as usize) as u32;
        if self.cpu_count > self.cpu_capacity {
            self.cpu_capacity = self.cpu_count.next_power_of_two();
            self.cpu = particle_buffer(device, "twe-kernel cpu particles", self.cpu_capacity);
            self.cpu_bg = draw_bind_group(device, &self.draw_layout, &self.cpu, &self.params);
        }
        if self.cpu_count > 0 {
            queue.write_buffer(&self.cpu, 0, bytemuck::cast_slice(&frame.cpu[..self.cpu_count as usize]));
        }
        let params = Params {
            view_proj: camera.view_proj,
            inv_view_proj: camera.inv_view_proj,
            right: [camera.right[0], camera.right[1], camera.right[2], 0.0],
            up: [camera.up[0], camera.up[1], camera.up[2], 0.0],
            eye: [camera.eye[0], camera.eye[1], camera.eye[2], 1.0],
            size: [size.0 as f32, size.1 as f32],
            dt,
            frame: self.frame,
            capacity: self.capacity,
            emissions: uploads.len() as u32,
            total,
            cursor: self.cursor,
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        self.frame = self.frame.wrapping_add(1);
        self.spawned = total;
        self.cursor = (self.cursor + total) & (self.capacity - 1);
        !self.batches.is_empty() || self.cpu_count > 0
    }

    fn build(&mut self, device: &wgpu::Device, programs: &[ParticleProgram]) {
        self.programs = programs.to_vec();
        if programs.is_empty() {
            self.spawn = None;
            self.update = None;
            return;
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel particle sim"),
            source: wgpu::ShaderSource::Wgsl(sim_source(programs).into()),
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&self.sim_pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        self.spawn = Some(pipeline("cs_spawn"));
        self.update = Some(pipeline("cs_update"));
    }

    /// A bigger pool holding the old one's particles; new ones go after
    /// them.
    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, capacity: u32) {
        let pool = particle_buffer(device, "twe-kernel particle pool", capacity);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("twe-kernel particle pool grow"),
        });
        encoder.copy_buffer_to_buffer(&self.pool, 0, &pool, 0, Some(particle_bytes(self.capacity)));
        queue.submit(Some(encoder.finish()));
        self.cursor = self.capacity;
        self.capacity = capacity;
        self.pool = pool;
        self.pool_bg = draw_bind_group(device, &self.draw_layout, &self.pool, &self.params);
    }

    /// Spawn and update, after the opaque scene (its depth is what
    /// `collide` programs bounce off).
    pub(crate) fn record_sim(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView) {
        let (Some(spawn), Some(update)) = (&self.spawn, &self.update) else {
            return;
        };
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel particle sim bg"),
            layout: &self.sim_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.pool.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.emissions.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("twe-kernel particles"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &bg, &[]);
        // Update first: particles born this frame start at age 0.
        pass.set_pipeline(update);
        pass.dispatch_workgroups(self.capacity.div_ceil(WORKGROUP), 1, 1);
        if self.spawned > 0 {
            pass.set_pipeline(spawn);
            pass.dispatch_workgroups(self.spawned.div_ceil(WORKGROUP), 1, 1);
        }
    }

    /// Draw every particle into the accumulation and revealage
    /// targets, depth-tested against the scene's (multisampled) depth.
    pub(crate) fn record_draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        [accum, reveal]: [&wgpu::TextureView; 2],
        depth: &wgpu::TextureView,
    ) {
        let depth_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel particle depth bg"),
            layout: &self.depth_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(depth),
            }],
        });
        let target = |view, clear| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("twe-kernel particle draw"),
            color_attachments: &[
                target(accum, wgpu::Color::TRANSPARENT),
                target(reveal, wgpu::Color::WHITE),
            ],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.draw_pipeline);
        pass.set_bind_group(1, &depth_bg, &[]);
        if self.spawn.is_some() {
            pass.set_bind_group(0, &self.pool_bg, &[]);
            pass.draw(0..4, 0..self.capacity);
        }
        if self.cpu_count > 0 {
            pass.set_bind_group(0, &self.cpu_bg, &[]);
            pass.draw(0..4, 0..self.cpu_count);
        }
    }

    /// Blend the particles over the frame.
    pub(crate) fn record_composite(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        accum: &wgpu::TextureView,
        reveal: &wgpu::TextureView,
        hdr: &wgpu::TextureView,
    ) {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel particle composite bg"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(accum),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(reveal),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("twe-kernel particle composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: hdr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The pool's size (for tests and diagnostics).
    pub(crate) fn capacity(&self) -> u32 {
        self.capacity
    }
}

fn particle_bytes(count: u32) -> u64 {
    u64::from(count) * std::mem::size_of::<GpuParticle>() as u64
}

fn particle_buffer(device: &wgpu::Device, label: &str, count: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: particle_bytes(count.max(1)),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn emission_buffer(device: &wgpu::Device, count: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twe-kernel particle emissions"),
        size: u64::from(count.max(1)) * std::mem::size_of::<EmissionU>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn draw_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    particles: &wgpu::Buffer,
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twe-kernel particle draw bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: particles.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: params.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Whether `program` compiles, checked by naga (wgpu's WGSL front
    /// end).
    pub(crate) fn validate(program: &ParticleProgram) -> Result<(), String> {
        let src = sim_source(std::slice::from_ref(program));
        let module = naga::front::wgsl::parse_str(&src).map_err(|e| e.emit_to_string(&src))?;
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
            .validate(&module)
            .map_err(|e| format!("{e:?}"))?;
        Ok(())
    }

    fn validate_src(label: &str, src: &str) {
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(src)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{label}: {e:?}"));
    }

    #[test]
    fn particle_shaders_validate() {
        validate_src("sim (no programs)", &sim_source(&[]));
        let programs = [
            ParticleProgram::default(),
            ParticleProgram {
                spawn: "    pt.velocity = vec3<f32>(twe_rand(), 1.0, 0.0);\n".into(),
                update: "    pt.pos += pt.velocity * dt;\n".into(),
                collide: true,
            },
        ];
        validate_src("sim", &sim_source(&programs));
        validate_src("draw", &format!("{COMMON}{DRAW_SHADER}"));
        validate_src("composite", COMPOSITE_SHADER);
    }

    #[test]
    fn a_bad_program_is_refused() {
        let bad = ParticleProgram {
            spawn: "    pt.size = vec3<f32>(1.0);\n".into(),
            ..Default::default()
        };
        assert!(validate(&bad).is_err());
    }

    #[test]
    fn layouts_match_the_shader() {
        assert_eq!(std::mem::size_of::<GpuParticle>(), 64);
        assert_eq!(std::mem::size_of::<EmissionU>(), 32);
        assert_eq!(std::mem::size_of::<Params>(), 208);
    }
}
