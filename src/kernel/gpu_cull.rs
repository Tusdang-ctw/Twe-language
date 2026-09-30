//! web3d-M7 session 11: GPU-driven culling and indirect draws.
//!
//! The opaque scene is drawn in two phases (Wihlidal 2015, "Optimizing
//! the Graphics Pipeline with Compute"; as in Niagara and Nanite):
//!
//! 1. **Early.** A compute pass keeps the instances that are inside the
//!    view frustum *and* were visible last frame, compacting each draw
//!    group's survivors into that group's region of an output instance
//!    buffer and counting them. A second pass writes one indirect draw
//!    per draw entry (a primitive, or a glTF primitive of a mesh) with
//!    its group's count. The main pass draws these.
//! 2. **Hierarchical Z.** The early depth becomes a max-depth pyramid:
//!    level 0 is the farthest of each pixel's MSAA samples, each level
//!    above the farthest of 2×2 below.
//! 3. **Late.** Every frustum-visible instance's bounding sphere is
//!    projected and tested against the pyramid (the level where its
//!    screen box spans at most 2×2 texels, four texels read). Visible
//!    ones not drawn early are compacted and drawn in a second pass;
//!    the result becomes next frame's "visible last frame".
//!
//! The early set only decides what draws first; correctness doesn't
//! depend on it (an instance that becomes visible is drawn late the
//! same frame), so an immediate-mode draw list whose order shifts
//! costs efficiency, never pixels.
//!
//! WebGPU has no indirect `first_instance` without an optional feature,
//! so each group's compacted instances start at index 0 of their own
//! vertex-buffer slice.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Bytes per instance (`render.rs` `Instance`: position + size, colour,
/// rotation with the draw group in `rot.z`).
pub(crate) const INSTANCE_BYTES: u64 = 48;

/// One draw group's culling data: every instance of the group shares
/// its geometry's bounding radius (scaled by the instance's size) and
/// its region of the output buffers.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(crate) struct CullGroup {
    pub radius: f32,
    /// First slot of the group's region in the output buffers.
    pub base: u32,
    pub _pad: [u32; 2],
}

/// One indirect draw: geometry range and the group whose survivors it
/// draws.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(crate) struct CullDraw {
    pub index_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub group: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CullUniform {
    view_proj: [[f32; 4]; 4],
    planes: [[f32; 4]; 6],
    /// width, height, instance count, draw count.
    params: [f32; 4],
}

/// This frame's culling inputs: target size, the (unjittered)
/// view-projection and its frustum planes, and how many opaque
/// instances (the first ones in the instance buffer) take part.
pub(crate) struct CullFrame {
    pub size: (u32, u32),
    pub view_proj: [[f32; 4]; 4],
    pub planes: [[f32; 4]; 6],
    pub instances: u32,
}

/// Bytes per indirect draw (`DrawIndexedIndirectArgs`).
pub(crate) const ARGS_BYTES: u64 = 20;

const CULL_SHADER: &str = r#"
struct Cull {
    view_proj: mat4x4<f32>,
    planes: array<vec4<f32>, 6>,
    params: vec4<f32>,
};
struct Instance {
    pos_size: vec4<f32>,
    color: vec4<f32>,
    rot: vec4<f32>,
};
struct Group {
    radius: f32,
    base: u32,
    pad0: u32,
    pad1: u32,
};
struct Draw {
    index_count: u32,
    first_index: u32,
    base_vertex: i32,
    group: u32,
};
struct Args {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};

@group(0) @binding(0) var<uniform> cull: Cull;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
@group(0) @binding(2) var<storage, read> groups: array<Group>;
@group(0) @binding(3) var<storage, read> draws: array<Draw>;
// 1 = visible last frame (read by the early pass, rewritten by the late).
@group(0) @binding(4) var<storage, read_write> visible: array<u32>;
@group(0) @binding(5) var hiz: texture_2d<f32>;

@group(1) @binding(0) var<storage, read_write> counts: array<atomic<u32>>;
@group(1) @binding(1) var<storage, read_write> out_instances: array<Instance>;
@group(1) @binding(2) var<storage, read_write> args: array<Args>;

fn sphere(i: u32) -> vec4<f32> {
    let inst = instances[i];
    let g = u32(inst.rot.z);
    return vec4<f32>(inst.pos_size.xyz, groups[g].radius * inst.pos_size.w);
}

fn in_frustum(s: vec4<f32>) -> bool {
    for (var p: i32 = 0; p < 6; p = p + 1) {
        let plane = cull.planes[p];
        if (dot(plane.xyz, s.xyz) + plane.w < -s.w) {
            return false;
        }
    }
    return true;
}

fn append(i: u32) {
    let inst = instances[i];
    let g = u32(inst.rot.z);
    let slot = atomicAdd(&counts[g], 1u);
    out_instances[groups[g].base + slot] = inst;
}

@compute @workgroup_size(64)
fn cs_cull_early(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= u32(cull.params.z)) {
        return;
    }
    if (visible[i] != 0u && in_frustum(sphere(i))) {
        append(i);
    }
}

// Hidden if the sphere's nearest depth is behind the farthest depth the
// pyramid holds over its screen box.
fn occluded(s: vec4<f32>) -> bool {
    var lo = vec3<f32>(1e9);
    var hi = vec3<f32>(-1e9);
    for (var c: i32 = 0; c < 8; c = c + 1) {
        let corner = s.xyz + vec3<f32>(
            select(-s.w, s.w, (c & 1) != 0),
            select(-s.w, s.w, (c & 2) != 0),
            select(-s.w, s.w, (c & 4) != 0),
        );
        let clip = cull.view_proj * vec4<f32>(corner, 1.0);
        if (clip.w <= 1e-4) {
            return false; // crosses the eye plane: keep it
        }
        let ndc = clip.xyz / clip.w;
        lo = min(lo, ndc);
        hi = max(hi, ndc);
    }
    if (lo.z <= 0.0) {
        return false; // reaches the near plane
    }
    let size = cull.params.xy;
    // Screen box in pixels (y down), clamped to the screen.
    let p0 = clamp(vec2<f32>(lo.x * 0.5 + 0.5, 0.5 - hi.y * 0.5) * size, vec2<f32>(0.0), size);
    let p1 = clamp(vec2<f32>(hi.x * 0.5 + 0.5, 0.5 - lo.y * 0.5) * size, vec2<f32>(0.0), size);
    let extent = max(p1.x - p0.x, p1.y - p0.y);
    let levels = i32(textureNumLevels(hiz));
    let level = clamp(i32(ceil(log2(max(extent, 1.0)))), 0, levels - 1);
    let dim = vec2<i32>(textureDimensions(hiz, level));
    let scale = f32(1 << u32(level));
    let t0 = clamp(vec2<i32>(p0 / scale), vec2<i32>(0), dim - vec2<i32>(1));
    let t1 = clamp(vec2<i32>(p1 / scale), vec2<i32>(0), dim - vec2<i32>(1));
    let far = max(
        max(textureLoad(hiz, t0, level).r, textureLoad(hiz, vec2<i32>(t1.x, t0.y), level).r),
        max(textureLoad(hiz, vec2<i32>(t0.x, t1.y), level).r, textureLoad(hiz, t1, level).r),
    );
    return lo.z > far;
}

@compute @workgroup_size(64)
fn cs_cull_late(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= u32(cull.params.z)) {
        return;
    }
    let s = sphere(i);
    let seen = in_frustum(s) && !occluded(s);
    let drawn_early = visible[i] != 0u && in_frustum(s);
    if (seen && !drawn_early) {
        append(i);
    }
    visible[i] = select(0u, 1u, seen);
}

@compute @workgroup_size(64)
fn cs_args(@builtin(global_invocation_id) id: vec3<u32>) {
    let d = id.x;
    if (d >= u32(cull.params.w)) {
        return;
    }
    let draw = draws[d];
    args[d] = Args(draw.index_count, atomicLoad(&counts[draw.group]), draw.first_index, draw.base_vertex, 0u);
}
"#;

const HIZ_SHADER: &str = r#"
@group(0) @binding(0) var t_depth: texture_depth_multisampled_2d;
@group(0) @binding(1) var t_src: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}

// Level 0: the farthest of the pixel's MSAA samples.
@fragment
fn fs_hiz_init(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var d = 0.0;
    for (var s: i32 = 0; s < i32(textureNumSamples(t_depth)); s = s + 1) {
        d = max(d, textureLoad(t_depth, p, s));
    }
    return vec4<f32>(d);
}

// Each level: the farthest of the 2x2 texels below (and the extra row
// or column an odd size leaves at the edge).
@fragment
fn fs_hiz_down(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let src = vec2<i32>(textureDimensions(t_src));
    let dst = vec2<i32>(pos.xy);
    let base = dst * 2;
    var reach = vec2<i32>(1);
    if ((src.x & 1) == 1 && base.x + 2 == src.x - 1) {
        reach.x = 2;
    }
    if ((src.y & 1) == 1 && base.y + 2 == src.y - 1) {
        reach.y = 2;
    }
    var d = 0.0;
    for (var y: i32 = 0; y <= reach.y; y = y + 1) {
        for (var x: i32 = 0; x <= reach.x; x = x + 1) {
            let q = min(base + vec2<i32>(x, y), src - vec2<i32>(1));
            d = max(d, textureLoad(t_src, q, 0).r);
        }
    }
    return vec4<f32>(d);
}
"#;

/// The shaders, for validation tests.
#[cfg(test)]
pub(crate) fn shader_sources() -> [&'static str; 2] {
    [CULL_SHADER, HIZ_SHADER]
}

/// Per-phase output: counts per group, compacted instances, and the
/// indirect draws.
struct Phase {
    counts: wgpu::Buffer,
    out: wgpu::Buffer,
    args: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

pub(crate) struct GpuCull {
    early: wgpu::ComputePipeline,
    late: wgpu::ComputePipeline,
    write_args: wgpu::ComputePipeline,
    frame_layout: wgpu::BindGroupLayout,
    phase_layout: wgpu::BindGroupLayout,
    hiz_init: wgpu::RenderPipeline,
    hiz_down: wgpu::RenderPipeline,
    hiz_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    groups: wgpu::Buffer,
    draws: wgpu::Buffer,
    visible: wgpu::Buffer,
    phases: Option<[Phase; 2]>,
    /// Capacities: instances (visible flags, output regions), groups, draws.
    caps: (u64, u64, u64),
    /// The pyramid (all levels, then one view per level) and its size.
    hiz: Option<(wgpu::TextureView, Vec<wgpu::TextureView>)>,
    hiz_size: (u32, u32),
    /// A 1×1 depth "pyramid" for the early pass (which doesn't read it).
    hiz_dummy: wgpu::TextureView,
    group_count: usize,
    draw_count: u32,
    instance_count: u32,
    /// web3d-M7 follow-up: whether culling pays (see [`CullProbe`]).
    probe: std::cell::RefCell<CullProbe>,
}

/// web3d-M7 follow-up: culling on the GPU pays only in some scenes.
/// Where much is off screen or hidden it saves most of the frame; where
/// every instance is on screen and nothing hides anything (the M7
/// stress scene) its two-phase split only costs: ~3.5 ms natively on an
/// integrated GPU, ~18 ms in Chrome, where storing and reloading the
/// multisampled targets between the early and late passes is
/// expensive. So the renderer times it: every [`CYCLE`] frames it runs
/// [`WINDOW`] frames with culling and [`WINDOW`] without, and keeps
/// culling only if its frames were at least 5% shorter. Under a vsync
/// cap both come out alike and culling stays off (less GPU work for the
/// same frame rate). Timing needs no GPU readback: an earlier version
/// read the late pass's counts back, and in Chrome a GPU-saturated page
/// never saw those `mapAsync` calls resolve.
pub(crate) struct CullProbe {
    /// Frames since culling became possible.
    frame: u32,
    /// The mode between probes.
    cull: bool,
    /// The previous frame's start and whether it culled (its duration
    /// is known at the next frame's start).
    last: Option<(f64, Window)>,
    /// Summed frame times and counts of this probe's two windows.
    on: (f64, u32),
    off: (f64, u32),
}

#[derive(Clone, Copy, PartialEq)]
enum Window {
    On,
    Off,
    /// Between probes, or settling after a switch: not measured.
    None,
}

/// Frames per probe window (the first few settle and aren't timed).
const WINDOW: u32 = 60;
const SETTLE: u32 = 4;
/// Frames from one probe to the next.
const CYCLE: u32 = 720;

impl CullProbe {
    fn new() -> Self {
        CullProbe {
            frame: 0,
            cull: false,
            last: None,
            on: (0.0, 0),
            off: (0.0, 0),
        }
    }

    /// This frame's mode, given that culling is possible (`eligible`)
    /// and the time now (seconds; `None` without a clock: cull always).
    fn decide(&mut self, eligible: bool, now: Option<f64>) -> bool {
        let Some(now) = now else { return eligible };
        // Credit the previous frame's duration to its window.
        if let Some((start, window)) = self.last.take() {
            let dt = now - start;
            match window {
                Window::On => self.on = (self.on.0 + dt, self.on.1 + 1),
                Window::Off => self.off = (self.off.0 + dt, self.off.1 + 1),
                Window::None => {}
            }
        }
        if !eligible {
            self.frame = 0;
            return false;
        }
        let f = self.frame % CYCLE;
        self.frame = self.frame.wrapping_add(1);
        let (cull, window) = if f < WINDOW {
            (true, if f >= SETTLE { Window::On } else { Window::None })
        } else if f < 2 * WINDOW {
            (false, if f >= WINDOW + SETTLE { Window::Off } else { Window::None })
        } else {
            if f == 2 * WINDOW {
                let mean = |(t, n): (f64, u32)| if n > 0 { t / f64::from(n) } else { f64::INFINITY };
                self.cull = mean(self.on) < 0.95 * mean(self.off);
                self.on = (0.0, 0);
                self.off = (0.0, 0);
            }
            (self.cull, Window::None)
        };
        self.last = Some((now, window));
        cull
    }
}

impl GpuCull {
    pub fn new(device: &wgpu::Device) -> Self {
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel cull frame bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(1, true),
                storage(2, true),
                storage(3, true),
                storage(4, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let phase_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel cull phase bgl"),
            entries: &[storage(0, false), storage(1, false), storage(2, false)],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel cull"),
            source: wgpu::ShaderSource::Wgsl(CULL_SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel cull layout"),
            bind_group_layouts: &[Some(&frame_layout), Some(&phase_layout)],
            immediate_size: 0,
        });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let hiz_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel hiz bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: true,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let hiz_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel hiz"),
            source: wgpu::ShaderSource::Wgsl(HIZ_SHADER.into()),
        });
        let hiz_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel hiz layout"),
            bind_group_layouts: &[Some(&hiz_layout)],
            immediate_size: 0,
        });
        let hiz_pipeline = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&hiz_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &hiz_shader,
                    entry_point: Some("vs_full"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &hiz_shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::R32Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage_usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let hiz_dummy = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("twe-kernel hiz dummy"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        GpuCull {
            early: compute("cs_cull_early"),
            late: compute("cs_cull_late"),
            write_args: compute("cs_args"),
            frame_layout,
            phase_layout,
            hiz_init: hiz_pipeline("fs_hiz_init"),
            hiz_down: hiz_pipeline("fs_hiz_down"),
            hiz_layout,
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel cull uniform"),
                contents: bytemuck::bytes_of(&CullUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            groups: buffer("twe-kernel cull groups", 16, storage_usage),
            draws: buffer("twe-kernel cull draws", 16, storage_usage),
            visible: buffer("twe-kernel cull visible", 4, storage_usage),
            phases: None,
            caps: (0, 0, 0),
            hiz: None,
            hiz_size: (0, 0),
            hiz_dummy,
            group_count: 0,
            draw_count: 0,
            instance_count: 0,
            probe: std::cell::RefCell::new(CullProbe::new()),
        }
    }

    /// web3d-M7 follow-up: whether this frame should cull on the GPU,
    /// given that it could (`eligible`): see [`CullProbe`]. Call once
    /// per frame, before `prepare`.
    pub fn should_cull(&self, eligible: bool) -> bool {
        self.probe.borrow_mut().decide(eligible, crate::clock::now_secs())
    }

    /// Size the buffers and pyramid, and upload this frame's groups,
    /// draws and camera. `groups[i].base` must already lay the groups
    /// out one after another.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        f: &CullFrame,
        groups: &[CullGroup],
        draws: &[CullDraw],
    ) {
        let CullFrame {
            size,
            view_proj,
            planes,
            instances,
        } = *f;
        let grow = |cap: u64, need: u64| if need > cap { need.next_power_of_two().max(64) } else { cap };
        let (ci, cg, cd) = self.caps;
        let need = (grow(ci, u64::from(instances)), grow(cg, groups.len() as u64), grow(cd, draws.len() as u64));
        let storage_usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        if need.0 != ci {
            // New flags start at 0: the first frame draws everything late.
            self.visible = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("twe-kernel cull visible"),
                size: need.0 * 4,
                usage: storage_usage,
                mapped_at_creation: false,
            });
        }
        if need.1 != cg {
            self.groups = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("twe-kernel cull groups"),
                size: need.1 * 16,
                usage: storage_usage,
                mapped_at_creation: false,
            });
        }
        if need.2 != cd {
            self.draws = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("twe-kernel cull draws"),
                size: need.2 * 16,
                usage: storage_usage,
                mapped_at_creation: false,
            });
        }
        if need != self.caps || self.phases.is_none() {
            let phase = |label| {
                let counts = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: need.1 * 4,
                    usage: storage_usage | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let out = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: need.0 * INSTANCE_BYTES,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
                    mapped_at_creation: false,
                });
                let args = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: need.2 * ARGS_BYTES,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
                    mapped_at_creation: false,
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: &self.phase_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: counts.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: out.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: args.as_entire_binding(),
                        },
                    ],
                });
                Phase {
                    counts,
                    out,
                    args,
                    bind,
                }
            };
            self.phases = Some([phase("twe-kernel cull early"), phase("twe-kernel cull late")]);
            self.caps = need;
        }
        if self.hiz_size != size || self.hiz.is_none() {
            let levels = 32 - size.0.max(size.1).max(1).leading_zeros();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("twe-kernel hiz"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let mips = (0..levels)
                .map(|mip| {
                    texture.create_view(&wgpu::TextureViewDescriptor {
                        base_mip_level: mip,
                        mip_level_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect();
            self.hiz = Some((texture.create_view(&wgpu::TextureViewDescriptor::default()), mips));
            self.hiz_size = size;
        }
        queue.write_buffer(&self.groups, 0, bytemuck::cast_slice(groups));
        queue.write_buffer(&self.draws, 0, bytemuck::cast_slice(draws));
        let uniform = CullUniform {
            view_proj,
            planes,
            params: [size.0 as f32, size.1 as f32, instances as f32, draws.len() as f32],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        self.group_count = groups.len();
        self.draw_count = draws.len() as u32;
        self.instance_count = instances;
    }

    fn frame_bind(&self, device: &wgpu::Device, instances: &wgpu::Buffer, hiz: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel cull frame bg"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.groups.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.draws.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.visible.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(hiz),
                },
            ],
        })
    }

    fn cull(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, instances: &wgpu::Buffer, late: bool) {
        let Some(phases) = &self.phases else { return };
        let phase = &phases[usize::from(late)];
        encoder.clear_buffer(&phase.counts, 0, Some(self.group_count.max(1) as u64 * 4));
        let hiz = match (&self.hiz, late) {
            (Some((all, _)), true) => all,
            _ => &self.hiz_dummy,
        };
        let frame = self.frame_bind(device, instances, hiz);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(if late { "twe-kernel cull late" } else { "twe-kernel cull early" }),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &frame, &[]);
        pass.set_bind_group(1, &phase.bind, &[]);
        pass.set_pipeline(if late { &self.late } else { &self.early });
        pass.dispatch_workgroups(self.instance_count.div_ceil(64).max(1), 1, 1);
        pass.set_pipeline(&self.write_args);
        pass.dispatch_workgroups(self.draw_count.div_ceil(64).max(1), 1, 1);
    }

    /// Record the early cull over `instances` (the instance buffer).
    pub fn record_early(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, instances: &wgpu::Buffer) {
        self.cull(device, encoder, instances, false);
    }

    /// Build the pyramid from the early pass's multisampled `depth`,
    /// then record the late cull.
    pub fn record_late(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        instances: &wgpu::Buffer,
        depth: &wgpu::TextureView,
    ) {
        let Some((_, mips)) = &self.hiz else { return };
        for (level, target) in mips.iter().enumerate() {
            let src = if level == 0 { &self.hiz_dummy } else { &mips[level - 1] };
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("twe-kernel hiz bg"),
                layout: &self.hiz_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("twe-kernel hiz"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(if level == 0 { &self.hiz_init } else { &self.hiz_down });
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.cull(device, encoder, instances, true);
    }

    /// Diagnostics (native only; blocks on the GPU): how many instances
    /// the last frame drew early and late, out of how many opaque
    /// instances took part. `None` before the first culled frame.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn last_counts(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<(u32, u32, u32)> {
        let phases = self.phases.as_ref()?;
        let bytes = self.group_count as u64 * 4;
        if bytes == 0 {
            return None;
        }
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twe-kernel cull readback"),
            size: bytes * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(&phases[0].counts, 0, &staging, 0, bytes);
        encoder.copy_buffer_to_buffer(&phases[1].counts, 0, &staging, bytes, bytes);
        queue.submit(Some(encoder.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let counts: &[u32] = bytemuck::cast_slice(&data);
        let n = self.group_count;
        let early = counts[..n].iter().sum();
        let late = counts[n..2 * n].iter().sum();
        Some((early, late, self.instance_count))
    }

    /// Draw `d` of phase `late` (false = early): its group's compacted
    /// instances as the instance vertex buffer, and its indirect args.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, late: bool, d: u32, group: &CullGroup, group_len: u32) {
        let Some(phases) = &self.phases else { return };
        let phase = &phases[usize::from(late)];
        let start = u64::from(group.base) * INSTANCE_BYTES;
        let end = start + u64::from(group_len.max(1)) * INSTANCE_BYTES;
        pass.set_vertex_buffer(1, phase.out.slice(start..end));
        pass.draw_indexed_indirect(&phase.args, u64::from(d) * ARGS_BYTES);
    }
}

#[cfg(test)]
mod probe_tests {
    use super::{CullProbe, CYCLE, WINDOW};

    /// Run the probe through one cycle where culled frames take `on` and
    /// unculled frames `off` seconds; return the mode it settles on.
    fn settle(on: f64, off: f64) -> bool {
        let mut p = CullProbe::new();
        let mut t = 0.0;
        let mut culled = false;
        for _ in 0..(2 * WINDOW + 5) {
            culled = p.decide(true, Some(t));
            t += if culled { on } else { off };
        }
        culled
    }

    #[test]
    fn keeps_culling_only_when_it_is_faster() {
        assert!(settle(0.008, 0.020), "culling much faster: keep it");
        assert!(!settle(0.020, 0.012), "culling slower: drop it");
        assert!(!settle(0.0167, 0.0167), "no difference (a vsync cap): drop it");
    }

    #[test]
    fn probes_again_each_cycle_and_culls_without_a_clock() {
        let mut p = CullProbe::new();
        let mut t = 0.0;
        // Settle on "off", then the next cycle's first window culls again.
        for _ in 0..CYCLE {
            let c = p.decide(true, Some(t));
            t += if c { 0.020 } else { 0.010 };
        }
        assert!(p.decide(true, Some(t)), "a new probe starts with culling on");
        assert!(!p.decide(false, Some(t)), "never culls when it can't");
        assert!(CullProbe::new().decide(true, None), "no clock: cull whenever it can");
    }
}
