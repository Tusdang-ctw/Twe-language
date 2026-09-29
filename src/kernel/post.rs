//! web3d-M7: post-processing before the tonemap: bloom and automatic
//! exposure.
//!
//! **Bloom** is the downsample / upsample chain of Jimenez 2014 ("Next
//! Generation Post Processing in Call of Duty: Advanced Warfare"), the
//! one Unreal, Unity HDRP, Blender Eevee and Bevy use:
//!
//! - the HDR frame is halved repeatedly (up to seven levels) with a
//!   13-tap filter; the first level applies the threshold and weights
//!   its taps by 1 / (1 + luma) (Karis) so single bright pixels can't
//!   flicker into large blobs;
//! - walking back up, each level adds a 3×3 tent upsample of the level
//!   below, so the result sums every level: a tight core with a wide,
//!   soft falloff, as a real lens's glare has;
//! - the tonemap adds the top level, averaged over the levels, times
//!   the intensity.
//!
//! **Auto exposure** measures the frame's luminance with a 256-bin
//! histogram of log2 luminance (a compute pass), averages the bins
//! between the 40th and 95th percentile (so neither the sky nor deep
//! shadow dominates), and eases the exposure toward the one that maps
//! that average to middle grey (0.18). The state stays on the GPU; the
//! tonemap reads it from a storage buffer.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Most bloom levels (the first at half resolution).
const MAX_LEVELS: u32 = 7;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BloomUniform {
    /// threshold, knee, first level (1/0), _.
    params: [f32; 4],
}

const BLOOM_SHADER: &str = r#"
struct Bloom {
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> bloom: Bloom;
@group(0) @binding(1) var t_src: texture_2d<f32>;
@group(0) @binding(2) var s_linear: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: VOut;
    out.pos = vec4<f32>(p[i], 0.0, 1.0);
    out.uv = vec2<f32>(p[i].x * 0.5 + 0.5, 0.5 - p[i].y * 0.5);
    return out;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn tap(uv: vec2<f32>, texel: vec2<f32>, x: f32, y: f32) -> vec3<f32> {
    return textureSampleLevel(t_src, s_linear, uv + vec2<f32>(x, y) * texel, 0.0).rgb;
}

fn karis(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + luma(c));
}

// Soft threshold with a quadratic knee (as Unity / Eevee): nothing
// below threshold - knee, the full excess above threshold + knee.
fn threshold(c: vec3<f32>) -> vec3<f32> {
    let t = bloom.params.x;
    if (t <= 0.0) {
        return c;
    }
    let knee = bloom.params.y;
    let br = max(c.r, max(c.g, c.b));
    var soft = clamp(br - t + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-5);
    let w = max(soft, br - t) / max(br, 1e-5);
    return c * w;
}

@fragment
fn fs_down(in: VOut) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(t_src));
    let a = tap(in.uv, texel, -2.0, -2.0);
    let b = tap(in.uv, texel, 0.0, -2.0);
    let c = tap(in.uv, texel, 2.0, -2.0);
    let d = tap(in.uv, texel, -2.0, 0.0);
    let e = tap(in.uv, texel, 0.0, 0.0);
    let f = tap(in.uv, texel, 2.0, 0.0);
    let g = tap(in.uv, texel, -2.0, 2.0);
    let h = tap(in.uv, texel, 0.0, 2.0);
    let i = tap(in.uv, texel, 2.0, 2.0);
    let j = tap(in.uv, texel, -1.0, -1.0);
    let k = tap(in.uv, texel, 1.0, -1.0);
    let l = tap(in.uv, texel, -1.0, 1.0);
    let m = tap(in.uv, texel, 1.0, 1.0);
    // Five overlapping 2x2 boxes: the centre one weighs 0.5, the
    // corner ones 0.125 each.
    let g0 = (a + b + d + e) * 0.25;
    let g1 = (b + c + e + f) * 0.25;
    let g2 = (d + e + g + h) * 0.25;
    let g3 = (e + f + h + i) * 0.25;
    let g4 = (j + k + l + m) * 0.25;
    var out: vec3<f32>;
    if (bloom.params.z > 0.5) {
        // First level: Karis-weighted boxes, then the threshold.
        let w0 = 0.125 * karis(g0);
        let w1 = 0.125 * karis(g1);
        let w2 = 0.125 * karis(g2);
        let w3 = 0.125 * karis(g3);
        let w4 = 0.5 * karis(g4);
        out = (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / (w0 + w1 + w2 + w3 + w4);
        out = threshold(out);
    } else {
        out = (g0 + g1 + g2 + g3) * 0.125 + g4 * 0.5;
    }
    return vec4<f32>(max(out, vec3<f32>(0.0)), 1.0);
}

// 3x3 tent over the smaller level, added onto this one (additive blend).
@fragment
fn fs_up(in: VOut) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(t_src));
    var s = tap(in.uv, texel, 0.0, 0.0) * 4.0;
    s = s + (tap(in.uv, texel, -1.0, 0.0) + tap(in.uv, texel, 1.0, 0.0) + tap(in.uv, texel, 0.0, -1.0) + tap(in.uv, texel, 0.0, 1.0)) * 2.0;
    s = s + tap(in.uv, texel, -1.0, -1.0) + tap(in.uv, texel, 1.0, -1.0) + tap(in.uv, texel, -1.0, 1.0) + tap(in.uv, texel, 1.0, 1.0);
    return vec4<f32>(s / 16.0, 1.0);
}
"#;

/// The bloom shader, for validation tests.
#[cfg(test)]
pub(crate) fn bloom_shader_source() -> &'static str {
    BLOOM_SHADER
}

/// The bloom chain: one texture with a mip per level (the first at
/// half resolution), rebuilt when the target size changes.
pub(crate) struct Bloom {
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Uniforms for the first level (threshold on) and the others.
    first: wgpu::Buffer,
    rest: wgpu::Buffer,
    levels: Vec<wgpu::TextureView>,
    size: (u32, u32),
    /// Black, bound when bloom is off.
    black: wgpu::TextureView,
    generation: u64,
}

impl Bloom {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel bloom bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel bloom"),
            source: wgpu::ShaderSource::Wgsl(BLOOM_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel bloom layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_full"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend,
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
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let uniform = |label, first: bool| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&BloomUniform {
                    params: [0.0, 0.0, if first { 1.0 } else { 0.0 }, 0.0],
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        };
        let black = crate::kernel::material::upload_rgba(
            device,
            queue,
            "twe-kernel bloom black",
            &[0, 0, 0, 255],
            1,
            1,
            false,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        Bloom {
            down: pipeline("fs_down", None),
            up: pipeline(
                "fs_up",
                Some(wgpu::BlendState {
                    color: add,
                    alpha: add,
                }),
            ),
            layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("twe-kernel bloom sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            first: uniform("twe-kernel bloom first", true),
            rest: uniform("twe-kernel bloom rest", false),
            levels: Vec::new(),
            size: (0, 0),
            black,
            generation: 0,
        }
    }

    /// Size the chain for a `width × height` target and set the
    /// threshold.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, threshold: f32) {
        if self.size != (width, height) || self.levels.is_empty() {
            let (w, h) = ((width / 2).max(1), (height / 2).max(1));
            // Down to about 8 pixels on the short side.
            let levels = (32 - w.min(h).max(1).leading_zeros()).saturating_sub(3).clamp(1, MAX_LEVELS);
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("twe-kernel bloom chain"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.levels = (0..levels)
                .map(|mip| {
                    texture.create_view(&wgpu::TextureViewDescriptor {
                        base_mip_level: mip,
                        mip_level_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect();
            self.size = (width, height);
            self.generation += 1;
        }
        let uniform = BloomUniform {
            params: [threshold, threshold * 0.5, 1.0, 0.0],
        };
        queue.write_buffer(&self.first, 0, bytemuck::bytes_of(&uniform));
    }

    /// Levels in the chain (the tonemap divides the sum by it).
    pub fn level_count(&self) -> usize {
        self.levels.len().max(1)
    }

    /// The finished bloom (the top level), or black when off.
    pub fn view(&self, on: bool) -> &wgpu::TextureView {
        match self.levels.first() {
            Some(v) if on => v,
            _ => &self.black,
        }
    }

    /// Changes whenever [`view`](Self::view) would return a different
    /// texture.
    pub fn key(&self, on: bool) -> u64 {
        if on && !self.levels.is_empty() {
            self.generation
        } else {
            0
        }
    }

    /// Record the chain from the HDR frame `source`.
    pub fn record(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, source: &wgpu::TextureView) {
        let bind = |uniform: &wgpu::Buffer, input: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("twe-kernel bloom bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            })
        };
        let draw = |encoder: &mut wgpu::CommandEncoder,
                    pipeline: &wgpu::RenderPipeline,
                    bg: &wgpu::BindGroup,
                    target: &wgpu::TextureView,
                    load: wgpu::LoadOp<wgpu::Color>| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("twe-kernel bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        };
        for (i, target) in self.levels.iter().enumerate() {
            let (uniform, input) = if i == 0 {
                (&self.first, source)
            } else {
                (&self.rest, &self.levels[i - 1])
            };
            draw(
                encoder,
                &self.down,
                &bind(uniform, input),
                target,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            );
        }
        for i in (0..self.levels.len().saturating_sub(1)).rev() {
            draw(
                encoder,
                &self.up,
                &bind(&self.rest, &self.levels[i + 1]),
                &self.levels[i],
                wgpu::LoadOp::Load,
            );
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct ExposureUniform {
    /// Adaptation rate this frame (1 - e^(-dt·speed); 1 = snap), log2
    /// luminance range min and width, _.
    params: [f32; 4],
}

/// Histogram range: log2 luminance from 2^-10 to 2^8.
const LOG_MIN: f32 = -10.0;
const LOG_RANGE: f32 = 18.0;
/// Adaptation speed (1/s): about a second to settle.
const ADAPT_SPEED: f32 = 2.0;

const EXPOSURE_SHADER: &str = r#"
struct Params {
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Params;
@group(0) @binding(1) var t_hdr: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> histogram: array<atomic<u32>, 256>;
// x = current log2 exposure, y = has value (0/1).
@group(0) @binding(3) var<storage, read_write> state: vec4<f32>;

var<workgroup> bins: array<atomic<u32>, 256>;

fn bin_of(c: vec3<f32>) -> u32 {
    let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    if (l < 1e-5) {
        return 0u;
    }
    let t = clamp((log2(l) - u.params.y) / u.params.z, 0.0, 1.0);
    return u32(t * 254.0 + 1.0);
}

// Every other pixel on each axis, 16x16 threads per group.
@compute @workgroup_size(16, 16, 1)
fn cs_histogram(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    atomicStore(&bins[li], 0u);
    workgroupBarrier();
    let dim = textureDimensions(t_hdr);
    let px = id.xy * 2u;
    if (px.x < dim.x && px.y < dim.y) {
        atomicAdd(&bins[bin_of(textureLoad(t_hdr, px, 0).rgb)], 1u);
    }
    workgroupBarrier();
    atomicAdd(&histogram[li], atomicLoad(&bins[li]));
}

var<workgroup> counts: array<u32, 256>;

@compute @workgroup_size(256, 1, 1)
fn cs_average(@builtin(local_invocation_index) li: u32) {
    counts[li] = atomicLoad(&histogram[li]);
    atomicStore(&histogram[li], 0u);
    workgroupBarrier();
    if (li != 0u) {
        return;
    }
    // Mean log2 luminance of the bins between the 40th and 95th
    // percentile of the lit pixels.
    var total = 0u;
    for (var i = 1u; i < 256u; i = i + 1u) {
        total = total + counts[i];
    }
    if (total == 0u) {
        return;
    }
    let lo = f32(total) * 0.40;
    let hi = f32(total) * 0.95;
    var seen = 0.0;
    var sum = 0.0;
    var weight = 0.0;
    for (var i = 1u; i < 256u; i = i + 1u) {
        let c = f32(counts[i]);
        let take = max(min(seen + c, hi) - max(seen, lo), 0.0);
        seen = seen + c;
        let log_l = (f32(i) - 0.5) / 254.0 * u.params.z + u.params.y;
        sum = sum + log_l * take;
        weight = weight + take;
    }
    let average = sum / max(weight, 1.0);
    // Expose so the average lands on middle grey.
    let target_ev = clamp(log2(0.18) - average, -8.0, 8.0);
    var rate = u.params.x;
    if (state.y < 0.5) {
        rate = 1.0;
    }
    state = vec4<f32>(mix(state.x, target_ev, rate), 1.0, 0.0, 0.0);
}
"#;

/// The exposure shader, for validation tests.
#[cfg(test)]
pub(crate) fn exposure_shader_source() -> &'static str {
    EXPOSURE_SHADER
}

/// Automatic exposure: a histogram, and the adapted exposure kept in a
/// storage buffer the tonemap reads.
pub(crate) struct AutoExposure {
    histogram_pipeline: wgpu::ComputePipeline,
    average_pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    histogram: wgpu::Buffer,
    state: wgpu::Buffer,
    last_time: Option<f32>,
}

impl AutoExposure {
    pub fn new(device: &wgpu::Device) -> Self {
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel exposure bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(2),
                storage(3),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel exposure"),
            source: wgpu::ShaderSource::Wgsl(EXPOSURE_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel exposure layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        AutoExposure {
            histogram_pipeline: pipeline("cs_histogram"),
            average_pipeline: pipeline("cs_average"),
            layout,
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel exposure uniform"),
                contents: bytemuck::bytes_of(&ExposureUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            histogram: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel exposure histogram"),
                contents: &[0u8; 256 * 4],
                usage: wgpu::BufferUsages::STORAGE,
            }),
            state: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel exposure state"),
                contents: &[0u8; 16],
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            }),
            last_time: None,
        }
    }

    /// The adapted exposure (log2), read by the tonemap.
    pub fn state(&self) -> &wgpu::Buffer {
        &self.state
    }

    /// Forget the adapted value (auto exposure switched off): the next
    /// measurement snaps instead of easing.
    pub fn reset(&mut self, queue: &wgpu::Queue) {
        if self.last_time.take().is_some() {
            queue.write_buffer(&self.state, 0, &[0u8; 16]);
        }
    }

    /// Write this frame's adaptation rate from the time since the last.
    pub fn prepare(&mut self, queue: &wgpu::Queue, time: f32) {
        let dt = self.last_time.map_or(0.0, |t| (time - t).clamp(0.0, 0.25));
        self.last_time = Some(time);
        let uniform = ExposureUniform {
            params: [1.0 - (-dt * ADAPT_SPEED).exp(), LOG_MIN, LOG_RANGE, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
    }

    /// Record the measurement of `hdr` (single-sample, `width × height`).
    pub fn record(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        hdr: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel exposure bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(hdr),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.histogram.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.state.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("twe-kernel exposure"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &bg, &[]);
        pass.set_pipeline(&self.histogram_pipeline);
        pass.dispatch_workgroups(width.div_ceil(32), height.div_ceil(32), 1);
        pass.set_pipeline(&self.average_pipeline);
        pass.dispatch_workgroups(1, 1, 1);
    }
}
