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

// ---------------------------------------------------------------------
// web3d-M7 session 8: depth of field, motion blur, colour-grading LUTs.
// ---------------------------------------------------------------------

/// Height of the simulated camera sensor: full frame (36 × 24 mm).
const SENSOR_HEIGHT_M: f32 = 0.024;

/// This frame's depth-of-field settings, in scene units (metres).
pub(crate) struct DofFrame {
    pub focus: f32,
    pub f_stop: f32,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

/// The circle of confusion's radius in pixels per unit of
/// `|z - focus| / z`, for a thin lens: focal length from the vertical
/// field of view on a full-frame sensor, aperture diameter = f / N.
/// `None` when the settings describe no lens (focus inside the focal
/// length, a non-positive f-number).
pub(crate) fn coc_scale(f: &DofFrame, height_px: f32) -> Option<f32> {
    let focal = 0.5 * SENSOR_HEIGHT_M / (0.5 * f.fov_y).tan();
    if f.f_stop <= 0.0 || f.focus <= focal {
        return None;
    }
    let aperture = focal / f.f_stop;
    let coc_diameter = aperture * focal / (f.focus - focal);
    Some(0.5 * coc_diameter / SENSOR_HEIGHT_M * height_px)
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct DofUniform {
    /// focus distance, CoC scale (px), max radius (px), _.
    lens: [f32; 4],
    /// near, far, full-res width, height.
    depth: [f32; 4],
}

const DOF_SHADER: &str = r#"
struct Dof {
    lens: vec4<f32>,
    depth: vec4<f32>,
};
@group(0) @binding(0) var<uniform> dof: Dof;
@group(0) @binding(1) var t_color: texture_2d<f32>;
@group(0) @binding(2) var t_depth: texture_depth_multisampled_2d;
@group(0) @binding(3) var s_linear: sampler;
@group(0) @binding(4) var t_blur: texture_2d<f32>;

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

fn full_size() -> vec2<f32> {
    return dof.depth.zw;
}

fn texel(px: vec2<f32>) -> vec2<i32> {
    return clamp(vec2<i32>(px), vec2<i32>(0), vec2<i32>(full_size()) - vec2<i32>(1));
}

// The nearest of the pixel's MSAA depth samples: an edge pixel's
// resolved colour is part foreground, so it counts as foreground.
fn linear_depth(px: vec2<f32>) -> f32 {
    let q = texel(px);
    var d = 1.0;
    for (var i: i32 = 0; i < i32(textureNumSamples(t_depth)); i = i + 1) {
        d = min(d, textureLoad(t_depth, q, i));
    }
    let near = dof.depth.x;
    let far = dof.depth.y;
    return near * far / (far - d * (far - near));
}

// Circle of confusion radius in full-resolution pixels (thin lens).
fn coc(z: f32) -> f32 {
    return min(dof.lens.y * abs(z - dof.lens.x) / z, dof.lens.z);
}

const GOLDEN_ANGLE: f32 = 2.39996323;
const RAD_SCALE: f32 = 1.0;

// Half resolution: scatter-as-gather bokeh (Gustafsson 2018, "Bokeh
// depth of field in a single pass"). A golden-angle spiral out to the
// largest blur; each sample counts where its own circle of confusion
// reaches this pixel, so blurry foreground spreads over sharp
// background, and background behind a sharp pixel can't bleed onto it.
@fragment
fn fs_gather(in: VOut) -> @location(0) vec4<f32> {
    let px = in.uv * full_size();
    let center_z = linear_depth(px);
    let center_coc = coc(center_z);
    // Colour and depth come from the same texel (no filtering), so an
    // edge's colour can't be counted at the depth behind it.
    var color = textureLoad(t_color, texel(px), 0).rgb;
    var total = 1.0;
    // How far blurry foreground spills onto this pixel.
    var spill = 0.0;
    var radius = RAD_SCALE;
    var angle = 0.0;
    for (var i: i32 = 0; i < 256; i = i + 1) {
        if (radius >= dof.lens.z) {
            break;
        }
        let q = px + vec2<f32>(cos(angle), sin(angle)) * radius;
        let c = textureLoad(t_color, texel(q), 0).rgb;
        let z = linear_depth(q);
        var size = coc(z);
        if (z > center_z) {
            size = clamp(size, 0.0, center_coc * 2.0);
        }
        let m = smoothstep(radius - 0.5, radius + 0.5, size);
        color = color + mix(color / total, c, m);
        total = total + 1.0;
        if (z < center_z) {
            spill = max(spill, m * size);
        }
        radius = radius + RAD_SCALE / radius;
        angle = angle + GOLDEN_ANGLE;
    }
    return vec4<f32>(color / total, spill);
}

// Full resolution: the sharp frame where nothing is out of focus, the
// bokeh where this pixel is blurred or foreground spills over it. The
// half-resolution bokeh is upsampled bilaterally (bilinear weights
// times depth similarity), so a sharp edge doesn't pick up the blur of
// what's behind it.
@fragment
fn fs_composite(in: VOut) -> @location(0) vec4<f32> {
    let px = in.pos.xy;
    let sharp = textureLoad(t_color, vec2<i32>(px), 0).rgb;
    let z = linear_depth(px);
    let half_size = vec2<i32>(textureDimensions(t_blur));
    let h = px * 0.5 - 0.5;
    let base = vec2<i32>(floor(h));
    let f = h - floor(h);
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    for (var dy: i32 = 0; dy < 2; dy = dy + 1) {
        for (var dx: i32 = 0; dx < 2; dx = dx + 1) {
            let q = clamp(base + vec2<i32>(dx, dy), vec2<i32>(0), half_size - vec2<i32>(1));
            let bilinear = select(1.0 - f.x, f.x, dx == 1) * select(1.0 - f.y, f.y, dy == 1);
            let zq = linear_depth(vec2<f32>(q) * 2.0 + 1.0);
            let w = bilinear * max(1.0 - abs(zq - z) / (0.1 * z), 0.001);
            sum = sum + textureLoad(t_blur, q, 0) * w;
            weight = weight + w;
        }
    }
    let blurred = sum / weight;
    let t = smoothstep(0.5, 1.5, max(coc(z), blurred.a));
    return vec4<f32>(mix(sharp, blurred.rgb, t), 1.0);
}
"#;

/// The DoF shader, for validation tests.
#[cfg(test)]
pub(crate) fn dof_shader_source() -> &'static str {
    DOF_SHADER
}

/// Bind group layout shared by DoF and motion blur: uniform, colour,
/// multisampled depth, linear sampler, and a second colour input.
fn screen_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    let texture = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
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
            texture(1, wgpu::TextureSampleType::Float { filterable: true }, false),
            texture(2, wgpu::TextureSampleType::Depth, true),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            texture(4, wgpu::TextureSampleType::Float { filterable: true }, false),
        ],
    })
}

/// A fullscreen pipeline writing HDR colour, from `shader`'s `entry`.
fn screen_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_full"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
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
}

/// Draw one fullscreen pass into `target`.
fn screen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::RenderPipeline,
    bg: &wgpu::BindGroup,
    target: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bg, &[]);
    pass.draw(0..3, 0..1);
}

fn linear_sampler(device: &wgpu::Device, label: &str) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

/// Depth of field: a half-resolution bokeh gather, composited over the
/// sharp frame by circle of confusion.
pub(crate) struct Dof {
    gather: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    half: Option<wgpu::TextureView>,
    size: (u32, u32),
}

impl Dof {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = screen_layout(device, "twe-kernel dof bgl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel dof"),
            source: wgpu::ShaderSource::Wgsl(DOF_SHADER.into()),
        });
        Dof {
            gather: screen_pipeline(device, &layout, &shader, "fs_gather"),
            composite: screen_pipeline(device, &layout, &shader, "fs_composite"),
            sampler: linear_sampler(device, "twe-kernel dof sampler"),
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel dof uniform"),
                contents: bytemuck::bytes_of(&DofUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            layout,
            half: None,
            size: (0, 0),
        }
    }

    /// Size the half-resolution target and write the lens. Returns
    /// false when the settings blur nothing (the pass is skipped).
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, f: &DofFrame) -> bool {
        let Some(scale) = coc_scale(f, height as f32) else {
            return false;
        };
        if self.size != (width, height) || self.half.is_none() {
            self.half = Some(
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("twe-kernel dof half"),
                        size: wgpu::Extent3d {
                            width: (width / 2).max(1),
                            height: (height / 2).max(1),
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: HDR_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default()),
            );
            self.size = (width, height);
        }
        // The largest blur: 1.5% of the frame height (16 px at 1080p).
        let max_radius = (height as f32 * 0.015).max(2.0);
        let uniform = DofUniform {
            lens: [f.focus, scale, max_radius, 0.0],
            depth: [f.near, f.far, width as f32, height as f32],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        true
    }

    /// Record the gather and the composite: `color` (full-resolution
    /// HDR) and the main pass's multisampled `depth` into `output`.
    pub fn record(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) {
        let Some(half) = &self.half else {
            return;
        };
        let bind = |second: &wgpu::TextureView| {
            screen_bind_group(device, &self.layout, &self.uniform, color, depth, &self.sampler, second)
        };
        // The gather's second input is unused; bind the colour again.
        screen_pass(encoder, "twe-kernel dof gather", &self.gather, &bind(color), half);
        screen_pass(encoder, "twe-kernel dof composite", &self.composite, &bind(half), output);
    }
}

fn screen_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    second: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twe-kernel screen pass bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(color),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(depth),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(second),
            },
        ],
    })
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct MotionUniform {
    inv_view_proj: [[f32; 4]; 4],
    prev_view_proj: [[f32; 4]; 4],
    /// shutter (fraction of the frame), max blur radius (px), width,
    /// height.
    params: [f32; 4],
    /// near, far, tile size (px), _.
    depth: [f32; 4],
}

const MOTION_SHADER: &str = r#"
struct Motion {
    inv_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
    params: vec4<f32>,
    depth: vec4<f32>,
};
@group(0) @binding(0) var<uniform> motion: Motion;
@group(0) @binding(1) var t_color: texture_2d<f32>;
@group(0) @binding(2) var t_depth: texture_depth_multisampled_2d;
// Per pixel: xy = blur radius vector (px), z = linear depth.
@group(0) @binding(3) var t_velocity: texture_2d<f32>;
// The tile pass's input (velocity, or tile maxima); the gather's
// neighbourhood maxima.
@group(0) @binding(4) var t_tiles: texture_2d<f32>;

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

// 1. Velocity: each pixel's depth gives its world position; last
// frame's view-projection gives where it was on screen. Stored as the
// blur's radius vector (half the shutter-scaled motion), clamped.
@fragment
fn fs_velocity(in: VOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.pos.xy);
    let d = textureLoad(t_depth, px, 0);
    let ndc = vec2<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let world = motion.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    let prev = motion.prev_view_proj * vec4<f32>(world.xyz / world.w, 1.0);
    var v = vec2<f32>(0.0);
    if (prev.w > 0.0) {
        let prev_uv = vec2<f32>(prev.x / prev.w * 0.5 + 0.5, 0.5 - prev.y / prev.w * 0.5);
        v = (in.uv - prev_uv) * motion.params.zw * motion.params.x * 0.5;
        let len = length(v);
        if (len > motion.params.y) {
            v = v * (motion.params.y / len);
        }
    }
    let near = motion.depth.x;
    let far = motion.depth.y;
    return vec4<f32>(v, near * far / (far - d * (far - near)), 1.0);
}

// 2. The longest velocity in each tile (tile = the largest blur radius).
@fragment
fn fs_tile_max(in: VOut) -> @location(0) vec4<f32> {
    let tile = i32(motion.depth.z);
    let base = vec2<i32>(in.pos.xy) * tile;
    let size = vec2<i32>(textureDimensions(t_tiles));
    var best = vec2<f32>(0.0);
    for (var y: i32 = 0; y < tile; y = y + 1) {
        for (var x: i32 = 0; x < tile; x = x + 1) {
            let q = base + vec2<i32>(x, y);
            if (q.x < size.x && q.y < size.y) {
                let v = textureLoad(t_tiles, q, 0).xy;
                if (dot(v, v) > dot(best, best)) {
                    best = v;
                }
            }
        }
    }
    return vec4<f32>(best, 0.0, 1.0);
}

// 3. The longest over each tile's 3x3 neighbourhood: anything that
// could streak into this tile.
@fragment
fn fs_neighbor_max(in: VOut) -> @location(0) vec4<f32> {
    let t = vec2<i32>(in.pos.xy);
    let size = vec2<i32>(textureDimensions(t_tiles));
    var best = vec2<f32>(0.0);
    for (var y: i32 = -1; y <= 1; y = y + 1) {
        for (var x: i32 = -1; x <= 1; x = x + 1) {
            let v = textureLoad(t_tiles, clamp(t + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1)), 0).xy;
            if (dot(v, v) > dot(best, best)) {
                best = v;
            }
        }
    }
    return vec4<f32>(best, 0.0, 1.0);
}

fn cone(d: f32, radius: f32) -> f32 {
    return clamp(1.0 - d / radius, 0.0, 1.0);
}

fn cylinder(d: f32, radius: f32) -> f32 {
    return 1.0 - smoothstep(0.95 * radius, 1.05 * radius, d);
}

// 1 when depth `a` is in front of depth `b` (soft over 5% of the depth).
fn in_front(a: f32, b: f32) -> f32 {
    return clamp(1.0 - (a - b) / (0.05 * b), 0.0, 1.0);
}

const SAMPLES: i32 = 15;

// 4. Reconstruction (McGuire, Hennessy, Bukowski, Osman 2012): gather
// along the neighbourhood's longest velocity. A sample counts if it is
// in front and its own blur reaches this pixel, or behind and this
// pixel's blur reaches it, or both blur across each other; so a moving
// edge streaks over a still background and not the reverse.
@fragment
fn fs_reconstruct(in: VOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.pos.xy);
    let size = vec2<i32>(textureDimensions(t_color));
    let color = textureLoad(t_color, px, 0).rgb;
    let tile = i32(motion.depth.z);
    let vn = textureLoad(t_tiles, px / vec2<i32>(tile), 0).xy;
    if (length(vn) < 0.5) {
        return vec4<f32>(color, 1.0);
    }
    let x = textureLoad(t_velocity, px, 0);
    let rx = max(length(x.xy), 0.5);
    var weight = 1.0 / rx;
    var sum = color * weight;
    let jitter = fract(52.9829189 * fract(dot(in.pos.xy, vec2<f32>(0.06711056, 0.00583715)))) - 0.5;
    for (var i: i32 = 0; i < SAMPLES; i = i + 1) {
        if (i == SAMPLES / 2) {
            continue;
        }
        let t = mix(-1.0, 1.0, (f32(i) + jitter + 1.0) / f32(SAMPLES + 1));
        let q = clamp(vec2<i32>(round(in.pos.xy + vn * t)), vec2<i32>(0), size - vec2<i32>(1));
        let y = textureLoad(t_velocity, q, 0);
        let ry = max(length(y.xy), 0.5);
        let d = length(vn * t);
        let a = in_front(y.z, x.z) * cone(d, ry)
            + in_front(x.z, y.z) * cone(d, rx)
            + cylinder(d, ry) * cylinder(d, rx) * 2.0;
        sum = sum + textureLoad(t_color, q, 0).rgb * a;
        weight = weight + a;
    }
    return vec4<f32>(sum / weight, 1.0);
}
"#;

/// The motion-blur shader, for validation tests.
#[cfg(test)]
pub(crate) fn motion_shader_source() -> &'static str {
    MOTION_SHADER
}

/// This frame's camera, for motion blur: the unjittered
/// view-projection and its inverse, the clip planes, and the shutter.
pub(crate) struct MotionFrame {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub near: f32,
    pub far: f32,
    pub shutter: f32,
}

/// Camera motion blur: velocity, tile maxima and the reconstruction
/// gather, with last frame's view-projection kept between frames.
pub(crate) struct MotionBlur {
    velocity_pipeline: wgpu::RenderPipeline,
    tile_pipeline: wgpu::RenderPipeline,
    neighbor_pipeline: wgpu::RenderPipeline,
    reconstruct_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    /// Velocity (full size), tile maxima and neighbourhood maxima.
    targets: Option<[wgpu::TextureView; 3]>,
    size: (u32, u32, u32),
    prev_view_proj: Option<[[f32; 4]; 4]>,
}

impl MotionBlur {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let unfiltered = wgpu::TextureSampleType::Float { filterable: false };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel motion blur bgl"),
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
                texture(1, unfiltered, false),
                texture(2, wgpu::TextureSampleType::Depth, true),
                texture(3, unfiltered, false),
                texture(4, unfiltered, false),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel motion blur"),
            source: wgpu::ShaderSource::Wgsl(MOTION_SHADER.into()),
        });
        MotionBlur {
            velocity_pipeline: screen_pipeline(device, &layout, &shader, "fs_velocity"),
            tile_pipeline: screen_pipeline(device, &layout, &shader, "fs_tile_max"),
            neighbor_pipeline: screen_pipeline(device, &layout, &shader, "fs_neighbor_max"),
            reconstruct_pipeline: screen_pipeline(device, &layout, &shader, "fs_reconstruct"),
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel motion blur uniform"),
                contents: bytemuck::bytes_of(&MotionUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            layout,
            targets: None,
            size: (0, 0, 0),
            prev_view_proj: None,
        }
    }

    /// Forget last frame's camera (motion blur off, a camera cut).
    pub fn reset(&mut self) {
        self.prev_view_proj = None;
    }

    /// Size the targets and write this frame's uniform; the first
    /// frame after a reset blurs nothing.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, f: &MotionFrame) {
        let MotionFrame {
            view_proj,
            inv_view_proj,
            near,
            far,
            shutter,
        } = *f;
        // The blur radius is capped at 2.5% of the frame width, and a
        // tile is that wide, so a tile's 3x3 neighbourhood holds every
        // velocity that can reach it.
        let max_radius = (width as f32 * 0.025).max(4.0);
        let tile = max_radius.ceil() as u32;
        if self.size != (width, height, tile) || self.targets.is_none() {
            let target = |label, w: u32, h: u32| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: w.max(1),
                            height: h.max(1),
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: HDR_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            let (tw, th) = (width.div_ceil(tile), height.div_ceil(tile));
            self.targets = Some([
                target("twe-kernel motion velocity", width, height),
                target("twe-kernel motion tiles", tw, th),
                target("twe-kernel motion neighbours", tw, th),
            ]);
            self.size = (width, height, tile);
        }
        let prev = self.prev_view_proj.replace(view_proj).unwrap_or(view_proj);
        let uniform = MotionUniform {
            inv_view_proj,
            prev_view_proj: prev,
            params: [shutter, max_radius, width as f32, height as f32],
            depth: [near, far, tile as f32, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
    }

    pub fn record(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) {
        let Some([velocity, tiles, neighbors]) = &self.targets else {
            return;
        };
        let bind = |vel: &wgpu::TextureView, tiles_in: &wgpu::TextureView| {
            let view = wgpu::BindingResource::TextureView;
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("twe-kernel motion blur bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: view(color),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: view(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: view(vel),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: view(tiles_in),
                    },
                ],
            })
        };
        // Unused inputs are bound to the colour (never the pass's own
        // target).
        screen_pass(encoder, "twe-kernel motion velocity", &self.velocity_pipeline, &bind(color, color), velocity);
        screen_pass(encoder, "twe-kernel motion tiles", &self.tile_pipeline, &bind(color, velocity), tiles);
        screen_pass(encoder, "twe-kernel motion neighbours", &self.neighbor_pipeline, &bind(color, tiles), neighbors);
        screen_pass(
            encoder,
            "twe-kernel motion blur",
            &self.reconstruct_pipeline,
            &bind(velocity, neighbors),
            output,
        );
    }
}

/// A parsed `.cube` 3D LUT (Adobe / Resolve format): `size³` RGB
/// entries, red varying fastest, then green, then blue.
#[derive(Debug)]
pub(crate) struct CubeLut {
    pub size: u32,
    pub data: Vec<[f32; 3]>,
}

/// Parse a `.cube` file. Only 3D LUTs over the default [0, 1] domain
/// are supported (what grading tools export for display-referred
/// looks); anything else is an error naming what's unsupported.
pub(crate) fn parse_cube(text: &str) -> Result<CubeLut, String> {
    let mut size = None;
    let mut data = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or_default();
        match first {
            "TITLE" => {}
            "LUT_3D_SIZE" => {
                let s: u32 = words
                    .next()
                    .and_then(|w| w.parse().ok())
                    .ok_or(format!("line {}: LUT_3D_SIZE needs a number", n + 1))?;
                if !(2..=256).contains(&s) {
                    return Err(format!("line {}: LUT_3D_SIZE {s} is out of range (2..256)", n + 1));
                }
                size = Some(s);
            }
            "LUT_1D_SIZE" => return Err("1D LUTs aren't supported; export a 3D LUT (LUT_3D_SIZE)".to_string()),
            "DOMAIN_MIN" | "DOMAIN_MAX" => {
                let want = if first == "DOMAIN_MIN" { 0.0 } else { 1.0 };
                let values: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                if values.len() != 3 || values.iter().any(|v| (v - want).abs() > 1e-6) {
                    return Err(format!("line {}: only the default domain (0 to 1) is supported", n + 1));
                }
            }
            _ => {
                let rgb: Vec<f32> = line
                    .split_whitespace()
                    .map(|w| w.parse::<f32>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| format!("line {}: expected three numbers, got `{line}`", n + 1))?;
                if rgb.len() != 3 {
                    return Err(format!("line {}: expected three numbers, got `{line}`", n + 1));
                }
                data.push([rgb[0], rgb[1], rgb[2]]);
            }
        }
    }
    let size = size.ok_or("no LUT_3D_SIZE line")?;
    let want = (size * size * size) as usize;
    if data.len() != want {
        return Err(format!("LUT_3D_SIZE {size} needs {want} entries, found {}", data.len()));
    }
    Ok(CubeLut { size, data })
}

/// Upload a LUT as a 3D texture (half floats, filterable everywhere).
pub(crate) fn upload_lut(device: &wgpu::Device, queue: &wgpu::Queue, lut: &CubeLut) -> wgpu::TextureView {
    let bits = crate::kernel::environment::f16_bits;
    let texels: Vec<u16> = lut
        .data
        .iter()
        .flat_map(|c| [bits(c[0]), bits(c[1]), bits(c[2]), bits(1.0)])
        .collect();
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("twe-kernel colour lut"),
            size: wgpu::Extent3d {
                width: lut.size,
                height: lut.size,
                depth_or_array_layers: lut.size,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&texels),
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The identity LUT (2³), bound when grading is off.
pub(crate) fn identity_lut(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let mut data = Vec::with_capacity(8);
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                data.push([r as f32, g as f32, b as f32]);
            }
        }
    }
    upload_lut(device, queue, &CubeLut { size: 2, data })
}

// ---------------------------------------------------------------------
// web3d-M7 session 10: the transmission source.
// ---------------------------------------------------------------------

const TRANSMISSION_SHADER: &str = r#"
@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_linear: sampler;

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

// A copy (same size) or a 2x2 box (half size): one bilinear tap. Alpha
// is coverage (0 where nothing was drawn), kept for the shader.
@fragment
fn fs_down(in: VOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(t_src, s_linear, in.uv, 0.0);
}
"#;

/// The transmission shader, for validation tests.
#[cfg(test)]
pub(crate) fn transmission_shader_source() -> &'static str {
    TRANSMISSION_SHADER
}

/// web3d-M7: what transmissive surfaces see through — the opaque scene,
/// copied after the opaque pass into a full mip chain so rough glass
/// can blur it (a mip per roughness, as Three.js and the Khronos sample
/// viewer do).
pub(crate) struct Transmission {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The whole chain (sampled by surfaces) and one view per mip.
    chain: Option<(wgpu::TextureView, Vec<wgpu::TextureView>)>,
    size: (u32, u32),
    /// Black, bound when nothing transmits.
    black: wgpu::TextureView,
    generation: u64,
}

impl Transmission {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel transmission bgl"),
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
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel transmission"),
            source: wgpu::ShaderSource::Wgsl(TRANSMISSION_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel transmission layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel transmission"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_full"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_down"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
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
        });
        let black = crate::kernel::material::upload_rgba(
            device,
            queue,
            "twe-kernel transmission black",
            &[0, 0, 0, 255],
            1,
            1,
            false,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        Transmission {
            pipeline,
            layout,
            sampler: linear_sampler(device, "twe-kernel transmission sampler"),
            chain: None,
            size: (0, 0),
            black,
            generation: 0,
        }
    }

    /// Size the chain for a `width × height` frame.
    pub fn prepare(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.size == (width, height) && self.chain.is_some() {
            return;
        }
        let mips = 32 - width.max(height).max(1).leading_zeros();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("twe-kernel transmission source"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let levels = (0..mips)
            .map(|mip| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        self.chain = Some((texture.create_view(&wgpu::TextureViewDescriptor::default()), levels));
        self.size = (width, height);
        self.generation += 1;
    }

    /// The chain surfaces sample, or black when nothing transmits.
    pub fn view(&self, on: bool) -> &wgpu::TextureView {
        match &self.chain {
            Some((v, _)) if on => v,
            _ => &self.black,
        }
    }

    /// Changes whenever [`view`](Self::view) would return a different
    /// texture.
    pub fn key(&self, on: bool) -> u64 {
        if on && self.chain.is_some() {
            self.generation
        } else {
            0
        }
    }

    /// Copy the resolved opaque frame `source` into mip 0, then halve
    /// it down the chain.
    pub fn record(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, source: &wgpu::TextureView) {
        let Some((_, levels)) = &self.chain else {
            return;
        };
        for (i, target) in levels.iter().enumerate() {
            let input = if i == 0 { source } else { &levels[i - 1] };
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("twe-kernel transmission bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            screen_pass(encoder, "twe-kernel transmission mip", &self.pipeline, &bg, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_files_parse() {
        let text = "# a comment\nTITLE \"invert\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n\
                    1 1 1\n0 1 1\n1 0 1\n0 0 1\n1 1 0\n0 1 0\n1 0 0\n0 0 0\n";
        let lut = parse_cube(text).expect("parses");
        assert_eq!(lut.size, 2);
        assert_eq!(lut.data[1], [0.0, 1.0, 1.0], "red varies fastest");
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0 0\n").unwrap_err().contains("needs 8 entries"));
        assert!(parse_cube("LUT_1D_SIZE 16\n").unwrap_err().contains("1D"));
        assert!(parse_cube("LUT_3D_SIZE 2\nDOMAIN_MAX 2 2 2\n").unwrap_err().contains("domain"));
    }

    /// A 50 mm lens (27° vertical on full frame) at f/2, focused at
    /// 2 m, against the thin-lens formula worked by hand.
    #[test]
    fn circle_of_confusion_follows_the_thin_lens() {
        let fov_y = 2.0 * (0.012_f32 / 0.05).atan();
        let f = DofFrame {
            focus: 2.0,
            f_stop: 2.0,
            fov_y,
            near: 0.1,
            far: 100.0,
        };
        // Aperture 25 mm, CoC diameter 25 mm * 50 mm / 1.95 m per unit
        // of |z - s| / z = 0.641 mm; radius on a 1080-px, 24 mm sensor.
        let scale = coc_scale(&f, 1080.0).expect("a lens");
        let want = 0.5 * (0.025 * 0.05 / 1.95) / 0.024 * 1080.0;
        assert!((scale - want).abs() < 1e-3, "{scale} vs {want}");
        assert!(coc_scale(&DofFrame { f_stop: 0.0, ..f }, 1080.0).is_none());
    }
}
