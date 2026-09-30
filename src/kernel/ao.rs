//! web3d-M7: screen-space ambient occlusion (GTAO).
//!
//! Ground-truth ambient occlusion (Jimenez, Wu, Pesce, Jarabo 2016,
//! "Practical Realtime Strategies for Accurate Indirect Occlusion"):
//!
//! - A **depth prepass** (drawn by the renderer before the main pass)
//!   gives every pixel's view-space position; normals come from the
//!   depth, picking the smoother neighbour on each axis so edges don't
//!   bend them.
//! - For each pixel, a few **slices** (planes through the eye ray, at
//!   rotated screen directions) are searched in both directions for
//!   the highest **horizon**. Samples fade out past the world radius,
//!   so distant geometry doesn't occlude.
//! - The visibility of each slice is integrated **in closed form**
//!   against the cosine lobe of the normal projected into the slice,
//!   which is what makes the result match ray-traced AO, not merely
//!   resemble it.
//! - Slice rotation and step offsets follow a 4×4 pattern, and a 4×4
//!   depth-aware blur averages it away (TAA also rotates it per frame).
//!
//! - web3d-M7 session 16: occlusion is computed and blurred at **half
//!   resolution** (each texel from the full-resolution depth at its
//!   top-left pixel), a quarter of the pixels: on an integrated GPU in
//!   Chrome, full-resolution GTAO cost `survive3d` about 18 ms a frame.
//!   Occlusion is low-frequency, and the main pass reads the half-size
//!   result.
//!
//! The result multiplies **indirect light only** (the environment or
//! ambient term, with Jimenez's multi-bounce fit and Lagarde's specular
//! occlusion in the main shader): direct light has its own shadows.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct AoUniform {
    /// 1 / proj[0][0], 1 / proj[1][1], near, far.
    inv_proj: [f32; 4],
    /// Width, height, world radius, power (intensity).
    size: [f32; 4],
    /// x = pixels per world unit at distance 1 (proj[1][1] · height / 2),
    /// y = this frame's slice rotation offset (0..1), zw unused.
    params: [f32; 4],
}

const AO_SHADER: &str = r#"
struct Ao {
    inv_proj: vec4<f32>,
    size: vec4<f32>,
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> ao: Ao;
@group(0) @binding(1) var t_depth: texture_depth_2d;
@group(0) @binding(2) var t_raw: texture_2d<f32>;

const PI: f32 = 3.14159265;
const SLICES: i32 = 3;
const STEPS: i32 = 6;

struct VOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: VOut;
    out.pos = vec4<f32>(p[i], 0.0, 1.0);
    return out;
}

fn linear_depth(d: f32) -> f32 {
    let near = ao.inv_proj.z;
    let far = ao.inv_proj.w;
    return near * far / (far - d * (far - near));
}

fn size_i() -> vec2<i32> {
    return vec2<i32>(ao.size.xy);
}

// View-space position of pixel `px` (clamped to the screen).
fn view_pos(px: vec2<i32>) -> vec3<f32> {
    let q = clamp(px, vec2<i32>(0), size_i() - vec2<i32>(1));
    let l = linear_depth(textureLoad(t_depth, q, 0));
    let uv = (vec2<f32>(q) + 0.5) / ao.size.xy;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    return vec3<f32>(ndc.x * l * ao.inv_proj.x, ndc.y * l * ao.inv_proj.y, -l);
}

// 4x4 ordered pattern: any 4x4 window holds each value once, so the
// blur averages every slice rotation.
fn pattern(px: vec2<i32>) -> f32 {
    var bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let i = (px.x & 3) + 4 * (px.y & 3);
    return (bayer[i] + 0.5) / 16.0;
}

@fragment
fn fs_gtao(in: VOut) -> @location(0) vec4<f32> {
    // A half-resolution texel, at its full-resolution pixel.
    let hpx = vec2<i32>(in.pos.xy);
    let px = min(hpx * 2, size_i() - vec2<i32>(1));
    let d = textureLoad(t_depth, px, 0);
    if (d >= 1.0) {
        return vec4<f32>(1.0);
    }
    let p = view_pos(px);
    let v = normalize(-p);

    // Normal from depth: on each axis, the neighbour closer in depth.
    let pr = view_pos(px + vec2<i32>(1, 0)) - p;
    let pl = p - view_pos(px - vec2<i32>(1, 0));
    let pd = view_pos(px + vec2<i32>(0, 1)) - p;
    let pu = p - view_pos(px - vec2<i32>(0, 1));
    var dx = pr;
    if (abs(pl.z) < abs(pr.z)) {
        dx = pl;
    }
    var dy = pd;
    if (abs(pu.z) < abs(pd.z)) {
        dy = pu;
    }
    var n = normalize(cross(dy, dx));
    if (dot(n, v) < 0.0) {
        n = -n;
    }

    let radius = ao.size.z;
    let radius_px = min(radius * ao.params.x / -p.z, 256.0);
    if (radius_px < 1.0) {
        return vec4<f32>(1.0);
    }
    // Samples fade out between 38% and 100% of the radius (XeGTAO's
    // falloff), so far geometry doesn't occlude.
    let falloff_range = 0.615 * radius;
    let falloff_from = radius - falloff_range;
    let falloff_mul = -1.0 / falloff_range;
    let falloff_add = falloff_from / falloff_range + 1.0;

    let noise_slice = fract(pattern(hpx) + ao.params.y);
    let noise_step = fract(pattern(hpx.yx) * 0.618 + 0.25 + ao.params.y * 1.618);

    var visibility = 0.0;
    for (var s: i32 = 0; s < SLICES; s = s + 1) {
        let phi = (f32(s) + noise_slice) * PI / f32(SLICES);
        let omega = vec2<f32>(cos(phi), sin(phi));   // screen, y down
        let dir = vec3<f32>(omega.x, -omega.y, 0.0);  // view space
        let ortho = dir - v * dot(dir, v);
        let axis = normalize(cross(dir, v));
        let proj_n = n - axis * dot(n, axis);
        let proj_len = length(proj_n);
        let cos_n = clamp(dot(proj_n, v) / max(proj_len, 1e-5), -1.0, 1.0);
        let n_angle = sign(dot(ortho, proj_n)) * acos(cos_n);

        // Horizons as cosines from the view vector; start at the
        // hemisphere's edge on each side.
        let low_pos = cos(n_angle + PI * 0.5);
        let low_neg = cos(n_angle - PI * 0.5);
        var hc_pos = low_pos;
        var hc_neg = low_neg;
        for (var j: i32 = 0; j < STEPS; j = j + 1) {
            var t = (f32(j) + noise_step) / f32(STEPS);
            t = t * t;
            let off = vec2<i32>(round(omega * max(t * radius_px, 2.0 * f32(j) + 2.0)));
            let dp = view_pos(px + off) - p;
            let dn = view_pos(px - off) - p;
            let lp = length(dp);
            let ln = length(dn);
            let wp = clamp(lp * falloff_mul + falloff_add, 0.0, 1.0);
            let wn = clamp(ln * falloff_mul + falloff_add, 0.0, 1.0);
            hc_pos = max(hc_pos, mix(low_pos, dot(dp / max(lp, 1e-6), v), wp));
            hc_neg = max(hc_neg, mix(low_neg, dot(dn / max(ln, 1e-6), v), wn));
        }
        // Horizon angles, clamped to the normal's hemisphere.
        let h_pos = n_angle + min(acos(clamp(hc_pos, -1.0, 1.0)) - n_angle, PI * 0.5);
        let h_neg = n_angle + max(-acos(clamp(hc_neg, -1.0, 1.0)) - n_angle, -PI * 0.5);
        // Cosine-weighted visible arc (Jimenez 2016, eq. 10).
        let sin_n = sin(n_angle);
        let arc_pos = (cos_n + 2.0 * h_pos * sin_n - cos(2.0 * h_pos - n_angle)) * 0.25;
        let arc_neg = (cos_n + 2.0 * h_neg * sin_n - cos(2.0 * h_neg - n_angle)) * 0.25;
        visibility = visibility + proj_len * (arc_pos + arc_neg);
    }
    visibility = clamp(visibility / f32(SLICES), 0.0, 1.0);
    return vec4<f32>(pow(visibility, ao.size.w));
}

// 4x4 blur (at half resolution), weighted by depth similarity so
// occlusion doesn't bleed across silhouettes.
@fragment
fn fs_blur(in: VOut) -> @location(0) vec4<f32> {
    let hpx = vec2<i32>(in.pos.xy);
    let half = vec2<i32>(textureDimensions(t_raw));
    let d = textureLoad(t_depth, min(hpx * 2, size_i() - vec2<i32>(1)), 0);
    if (d >= 1.0) {
        return vec4<f32>(1.0);
    }
    let center = linear_depth(d);
    var sum = 0.0;
    var weight = 0.0;
    for (var y: i32 = -2; y < 2; y = y + 1) {
        for (var x: i32 = -2; x < 2; x = x + 1) {
            let q = clamp(hpx + vec2<i32>(x, y), vec2<i32>(0), half - vec2<i32>(1));
            let l = linear_depth(textureLoad(t_depth, min(q * 2, size_i() - vec2<i32>(1)), 0));
            let w = max(1.0 - abs(l - center) / (0.05 * center), 0.0) + 1e-4;
            sum = sum + textureLoad(t_raw, q, 0).r * w;
            weight = weight + w;
        }
    }
    return vec4<f32>(sum / weight);
}
"#;

/// The shader, for validation tests.
#[cfg(test)]
pub(crate) fn shader_source() -> &'static str {
    AO_SHADER
}

/// This frame's AO settings.
pub(crate) struct AoFrame {
    pub proj: [[f32; 4]; 4],
    pub near: f32,
    pub far: f32,
    pub radius: f32,
    pub intensity: f32,
    pub frame: u32,
}

pub(crate) struct Ao {
    gtao: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    /// Raw and blurred occlusion, at half the target's size.
    raw: Option<wgpu::TextureView>,
    blurred: Option<wgpu::TextureView>,
    size: (u32, u32),
    /// A 1×1 white texture, bound when AO is off.
    white: wgpu::TextureView,
    /// Bumped when `blurred` is reallocated.
    generation: u64,
}

impl Ao {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel ao bgl"),
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
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel gtao"),
            source: wgpu::ShaderSource::Wgsl(AO_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel ao layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
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
                        format: AO_FORMAT,
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
        let white = crate::kernel::material::upload_rgba(
            device,
            queue,
            "twe-kernel ao white",
            &[255, 255, 255, 255],
            1,
            1,
            false,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        Ao {
            gtao: pipeline("fs_gtao"),
            blur: pipeline("fs_blur"),
            layout,
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("twe-kernel ao uniform"),
                contents: bytemuck::bytes_of(&AoUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            raw: None,
            blurred: None,
            size: (0, 0),
            white,
            generation: 0,
        }
    }

    /// Size the targets to `width × height` and write this frame's
    /// uniform.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        f: &AoFrame,
    ) {
        if self.size != (width, height) || self.blurred.is_none() {
            let target = |label| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: width.div_ceil(2),
                            height: height.div_ceil(2),
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: AO_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            self.raw = Some(target("twe-kernel ao raw"));
            self.blurred = Some(target("twe-kernel ao"));
            self.size = (width, height);
            self.generation += 1;
        }
        // Rotate the slices a little every frame, so TAA can average
        // more directions than one frame takes.
        const ROTATION: [f32; 4] = [0.0, 0.5, 0.25, 0.75];
        let uniform = AoUniform {
            inv_proj: [1.0 / f.proj[0][0], 1.0 / f.proj[1][1], f.near, f.far],
            size: [width as f32, height as f32, f.radius, f.intensity],
            params: [
                f.proj[1][1] * height as f32 * 0.5,
                ROTATION[(f.frame % 4) as usize] / 16.0,
                0.0,
                0.0,
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
    }

    /// The occlusion the main pass reads: the blurred result, or white
    /// when AO is off (or not prepared yet).
    pub fn view(&self, on: bool) -> &wgpu::TextureView {
        match (&self.blurred, on) {
            (Some(v), true) => v,
            _ => &self.white,
        }
    }

    /// Changes whenever [`view`](Self::view) would return a different
    /// texture.
    pub fn key(&self, on: bool) -> u64 {
        if on && self.blurred.is_some() {
            self.generation
        } else {
            0
        }
    }

    /// Record GTAO and the blur from the prepass `depth`.
    pub fn record(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView) {
        let (Some(raw), Some(blurred)) = (&self.raw, &self.blurred) else {
            return;
        };
        let bind = |input: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("twe-kernel ao bg"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                ],
            })
        };
        // GTAO's own input slot is unused; bind the white texture there.
        for (pipeline, input, output, label) in [
            (&self.gtao, &self.white, raw, "twe-kernel gtao"),
            (&self.blur, raw, blurred, "twe-kernel ao blur"),
        ] {
            let bg = bind(input);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
