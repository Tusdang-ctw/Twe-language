//! web3d-M7: temporal anti-aliasing.
//!
//! Each frame the projection is offset by a sub-pixel jitter (Halton
//! 2,3, eight positions), so consecutive frames sample different points
//! of every pixel. A resolve pass after the main pass blends the new
//! frame into a history of earlier ones (Karis 2014, "High Quality
//! Temporal Supersampling"; Playdead's INSIDE talk):
//!
//! - **Reprojection:** each pixel's depth gives its world position,
//!   which the previous frame's (unjittered) view-projection maps to
//!   where it was on screen last frame.
//! - **Clamping:** the history sample is clamped to the 3×3 neighbourhood
//!   of the new frame in YCoCg, which rejects stale history (ghosting).
//! - **Blending:** 10% new frame, weighted by 1 / (1 + luma) so a single
//!   bright sample can't flicker through.
//!
//! Motion comes from the camera only. Objects that move on their own
//! rely on the clamp, which keeps them sharp but can soften them in
//! motion. Per-object motion vectors need instance identity across
//! frames, which the draw queue doesn't carry yet.
//!
//! The history is two persistent HDR textures used alternately (one
//! read, one written), invalidated on resize or when TAA is switched on.

use std::cell::Cell;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// How much of each new frame enters the history.
const BLEND: f32 = 0.1;

/// Halton (2, 3), first eight points, centred on the pixel.
const JITTER: [[f32; 2]; 8] = [
    [0.0, -0.166_666_67],
    [-0.25, 0.166_666_67],
    [0.25, -0.388_888_9],
    [-0.375, -0.055_555_556],
    [0.125, 0.277_777_8],
    [-0.125, -0.277_777_8],
    [0.375, 0.055_555_556],
    [-0.4375, 0.388_888_9],
];

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct TaaUniform {
    inv_view_proj: [[f32; 4]; 4],
    prev_view_proj: [[f32; 4]; 4],
    /// blend, has history (0/1), 1/width, 1/height.
    params: [f32; 4],
}

const TAA_SHADER: &str = r#"
struct Taa {
    inv_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> taa: Taa;
@group(0) @binding(1) var t_current: texture_2d<f32>;
@group(0) @binding(2) var t_depth: texture_depth_multisampled_2d;
@group(0) @binding(3) var t_history: texture_2d<f32>;
@group(0) @binding(4) var s_linear: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_taa(@builtin(vertex_index) i: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: VOut;
    out.pos = vec4<f32>(p[i], 0.0, 1.0);
    out.uv = vec2<f32>(p[i].x * 0.5 + 0.5, 0.5 - p[i].y * 0.5);
    return out;
}

fn to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn from_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@fragment
fn fs_taa(in: VOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(t_current));
    let px = vec2<i32>(in.pos.xy);
    let current = textureLoad(t_current, px, 0).rgb;
    // The new frame's 3x3 neighbourhood bounds, in YCoCg.
    var lo = vec3<f32>(1e9);
    var hi = vec3<f32>(-1e9);
    for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
        for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
            let q = clamp(px + vec2<i32>(dx, dy), vec2<i32>(0), size - vec2<i32>(1));
            let c = to_ycocg(textureLoad(t_current, q, 0).rgb);
            lo = min(lo, c);
            hi = max(hi, c);
        }
    }
    // Where this pixel's surface was last frame.
    let depth = textureLoad(t_depth, px, 0);
    let ndc = vec2<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let world = taa.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    let prev = taa.prev_view_proj * vec4<f32>(world.xyz / world.w, 1.0);
    let prev_uv = vec2<f32>(prev.x / prev.w * 0.5 + 0.5, 0.5 - prev.y / prev.w * 0.5);
    let history_raw = textureSampleLevel(t_history, s_linear, prev_uv, 0.0).rgb;
    if (taa.params.y < 0.5 || any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0))) {
        return vec4<f32>(current, 1.0);
    }
    let history = from_ycocg(clamp(to_ycocg(history_raw), lo, hi));
    let wc = taa.params.x / (1.0 + luma(current));
    let wh = (1.0 - taa.params.x) / (1.0 + luma(history));
    return vec4<f32>((current * wc + history * wh) / (wc + wh), 1.0);
}
"#;

/// The shader, for validation tests.
#[cfg(test)]
pub(crate) fn shader_source() -> &'static str {
    TAA_SHADER
}

pub(crate) struct Taa {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    /// The two history textures and the size they were made for.
    history: Vec<(wgpu::Texture, wgpu::TextureView)>,
    size: (u32, u32),
    /// Which history holds the latest frame, whether it's usable, and
    /// the jitter index (cells: advanced after the frame is recorded,
    /// through a shared reference).
    latest: Cell<usize>,
    valid: Cell<bool>,
    frame: Cell<u32>,
    prev_view_proj: [[f32; 4]; 4],
}

impl Taa {
    pub fn new(device: &wgpu::Device) -> Self {
        let tex = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel taa bgl"),
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
                tex(1, float, false),
                tex(2, wgpu::TextureSampleType::Depth, true),
                tex(3, float, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel taa"),
            source: wgpu::ShaderSource::Wgsl(TAA_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel taa layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel taa"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_taa"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_taa"),
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
        Taa {
            pipeline,
            layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("twe-kernel taa sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel taa uniform"),
                contents: bytemuck::bytes_of(&TaaUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            history: Vec::new(),
            size: (0, 0),
            latest: Cell::new(0),
            valid: Cell::new(false),
            frame: Cell::new(0),
            prev_view_proj: [[0.0; 4]; 4],
        }
    }

    /// The projection offset for this frame, in NDC units, for a
    /// `width × height` target.
    pub fn jitter(&self, width: u32, height: u32) -> [f32; 2] {
        let j = JITTER[(self.frame.get() % JITTER.len() as u32) as usize];
        [2.0 * j[0] / width as f32, 2.0 * j[1] / height as f32]
    }

    /// Forget the history (TAA switched off, a camera cut).
    pub fn reset(&self) {
        self.valid.set(false);
    }

    /// Make the history match the target size, and write this frame's
    /// uniform from the unjittered view-projection.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        view_proj: [[f32; 4]; 4],
        inv_view_proj: [[f32; 4]; 4],
    ) {
        if self.size != (width, height) || self.history.len() != 2 {
            self.history = (0..2)
                .map(|_| {
                    let t = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("twe-kernel taa history"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: HDR_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    });
                    let v = t.create_view(&wgpu::TextureViewDescriptor::default());
                    (t, v)
                })
                .collect();
            self.size = (width, height);
            self.valid.set(false);
        }
        let uniform = TaaUniform {
            inv_view_proj,
            prev_view_proj: self.prev_view_proj,
            params: [
                BLEND,
                if self.valid.get() { 1.0 } else { 0.0 },
                1.0 / width as f32,
                1.0 / height as f32,
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        self.prev_view_proj = view_proj;
    }

    /// The history written this frame (the tonemap's input).
    pub fn output(&self) -> &wgpu::TextureView {
        &self.history[1 - self.latest.get()].1
    }

    /// Record the resolve: `current` (resolved HDR) and `depth` (the
    /// main pass's multisampled depth) into this frame's history.
    pub fn record(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        current: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel taa bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(current),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.history[self.latest.get()].1),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("twe-kernel taa resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.output(),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// After the frame is submitted: the written history becomes the
    /// latest, and the jitter moves on.
    pub fn advance(&self) {
        self.latest.set(1 - self.latest.get());
        self.valid.set(true);
        self.frame.set(self.frame.get().wrapping_add(1));
    }
}

/// Offset a projection so the image shifts by `ndc` (x, y) in NDC.
pub(crate) fn jitter_projection(mut proj: [[f32; 4]; 4], ndc: [f32; 2]) -> [[f32; 4]; 4] {
    for col in &mut proj {
        col[0] += ndc[0] * col[3];
        col[1] += ndc[1] * col[3];
    }
    proj
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_points_stay_inside_the_pixel_and_average_near_its_centre() {
        let (mut sx, mut sy) = (0.0f32, 0.0f32);
        for [x, y] in JITTER {
            assert!(x.abs() < 0.5 && y.abs() < 0.5);
            sx += x;
            sy += y;
        }
        assert!((sx / 8.0).abs() < 0.07 && (sy / 8.0).abs() < 0.07);
    }

    #[test]
    fn a_jittered_projection_shifts_clip_space_by_the_offset() {
        let proj = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, -1.0],
            [0.0, 0.0, -0.2, 0.0],
        ];
        let j = jitter_projection(proj, [0.01, -0.02]);
        // A point at view z = -5: clip w = 5, so ndc shifts by the offset.
        let p = [0.3f32, 0.4, -5.0, 1.0];
        let clip = |m: [[f32; 4]; 4]| {
            let mut c = [0.0f32; 4];
            for (col, v) in m.iter().zip(p) {
                for r in 0..4 {
                    c[r] += col[r] * v;
                }
            }
            c
        };
        let (a, b) = (clip(proj), clip(j));
        assert!((b[0] / b[3] - a[0] / a[3] - 0.01).abs() < 1e-6);
        assert!((b[1] / b[3] - a[1] / a[3] + 0.02).abs() < 1e-6);
    }
}
