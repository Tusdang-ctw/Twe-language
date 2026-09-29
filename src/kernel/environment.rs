//! web3d-M7: image-based lighting.
//!
//! An HDR environment map (an equirectangular Radiance `.hdr`) lights
//! the scene the way the glTF references and Three.js do. Built once per
//! environment:
//!
//! - **Specular:** the equirect is resampled into a cubemap with a mip
//!   chain, then GGX-prefiltered into a second cubemap whose mips hold
//!   increasing roughness (Karis 2013's split sum, with filtered
//!   importance sampling, Křivánek & Colbert 2008, so few samples stay
//!   smooth). Shaders read it at `roughness * max_lod`.
//! - **Diffuse:** irradiance as 9 spherical-harmonic coefficients
//!   (Ramamoorthi & Hanrahan 2001), projected on the CPU from the full-
//!   resolution map, so it is exact and deterministic.
//! - **The DFG term** of the split sum (and the multiscatter energy
//!   compensation built on it) comes from a lookup table computed once
//!   at startup, with the same visibility function the direct lights use.
//!
//! The equirect itself stays resident for drawing the environment as
//! the backdrop.
//!
//! Direction convention (shared with the shaders and Three.js's
//! `equirectUv`): u = atan2(z, x) / 2π + 0.5, and v runs from the top
//! row (straight up) to the bottom (straight down).

use std::f32::consts::PI;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Faces of the resampled source cubemap (its mips feed the prefilter).
const SOURCE_SIZE: u32 = 512;
/// Faces of the prefiltered specular cubemap, and its mip count: mip i
/// holds roughness i / (SPECULAR_MIPS - 1).
pub(crate) const SPECULAR_SIZE: u32 = 256;
pub(crate) const SPECULAR_MIPS: u32 = 6;
/// GGX samples per prefiltered texel (mips above 0).
const PREFILTER_SAMPLES: u32 = 1024;
/// The DFG lookup table: `DFG_SIZE`² texels over (n·v, roughness).
pub(crate) const DFG_SIZE: u32 = 64;
const DFG_SAMPLES: u32 = 512;

pub(crate) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The environment as the shaders see it (`Env` in `SHADER_SRC`).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct EnvUniform {
    /// Irradiance SH9, already convolved with the cosine lobe:
    /// E(n) = Σ sh[i] · Yᵢ(n). xyz used.
    pub sh: [[f32; 4]; 9],
    /// intensity, max specular LOD, has environment (0/1), backdrop (0/1).
    pub params: [f32; 4],
}

impl EnvUniform {
    pub fn none() -> Self {
        EnvUniform::zeroed()
    }
}

/// A decoded HDR image: linear RGB, rows top (up) to bottom (down).
pub(crate) struct HdrImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<[f32; 3]>,
}

/// Decode a Radiance `.hdr` (or any float image `image` reads).
pub(crate) fn decode_hdr(bytes: &[u8]) -> Result<HdrImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("environment: {e}"))?;
    let rgb = img.to_rgb32f();
    let (width, height) = rgb.dimensions();
    Ok(HdrImage {
        width,
        height,
        rgb: rgb.pixels().map(|p| p.0).collect(),
    })
}

/// Real SH basis, bands 0–2, at unit direction d.
fn sh_basis(d: [f32; 3]) -> [f32; 9] {
    let [x, y, z] = d;
    [
        0.282095,
        0.488603 * y,
        0.488603 * z,
        0.488603 * x,
        1.092548 * x * y,
        1.092548 * y * z,
        0.315392 * (3.0 * z * z - 1.0),
        1.092548 * x * z,
        0.546274 * (x * x - y * y),
    ]
}

/// Irradiance SH9 of an equirect map: project radiance (weighted by
/// each texel's solid angle), then convolve with the clamped cosine
/// (Â₀ = π, Â₁ = 2π/3, Â₂ = π/4).
pub(crate) fn irradiance_sh9(img: &HdrImage) -> [[f32; 4]; 9] {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut acc = [[0.0f64; 3]; 9];
    let d_phi = 2.0 * std::f64::consts::PI / w as f64;
    let d_theta = std::f64::consts::PI / h as f64;
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        let lat = (0.5 - v) * PI;
        let weight = d_phi * d_theta * f64::from(lat.cos());
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let phi = (u - 0.5) * 2.0 * PI;
            let d = [lat.cos() * phi.cos(), lat.sin(), lat.cos() * phi.sin()];
            let basis = sh_basis(d);
            let c = img.rgb[y * w + x];
            for (i, b) in basis.iter().enumerate() {
                for k in 0..3 {
                    acc[i][k] += f64::from(c[k] * b) * weight;
                }
            }
        }
    }
    let band = [PI, 2.0 * PI / 3.0, 2.0 * PI / 3.0, 2.0 * PI / 3.0, PI / 4.0, PI / 4.0, PI / 4.0, PI / 4.0, PI / 4.0];
    let mut out = [[0.0f32; 4]; 9];
    for i in 0..9 {
        for k in 0..3 {
            out[i][k] = acc[i][k] as f32 * band[i];
        }
    }
    out
}

fn hammersley(i: u32, n: u32) -> [f32; 2] {
    [i as f32 / n as f32, i.reverse_bits() as f32 * 2.328_306_4e-10]
}

/// A GGX-distributed half vector around +z.
fn importance_sample_ggx(xi: [f32; 2], a: f32) -> [f32; 3] {
    let phi = 2.0 * PI * xi[0];
    let cos_t = ((1.0 - xi[1]) / (1.0 + (a * a - 1.0) * xi[1])).sqrt();
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    [sin_t * phi.cos(), sin_t * phi.sin(), cos_t]
}

fn v_smith_ggx_correlated(nov: f32, nol: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = nol * (nov * nov * (1.0 - a2) + a2).sqrt();
    let gl = nov * (nol * nol * (1.0 - a2) + a2).sqrt();
    0.5 / (gv + gl).max(1e-7)
}

/// The split sum's DFG table: texel (i, j) at n·v = (i + ½)/N and
/// perceptual roughness (j + ½)/N holds (A, B) with the specular
/// lobe's directional albedo = f0·A + B.
pub(crate) fn dfg_lut() -> Vec<[f32; 3]> {
    let n = DFG_SIZE;
    let mut out = Vec::with_capacity((n * n) as usize);
    for j in 0..n {
        let roughness = (j as f32 + 0.5) / n as f32;
        let a = roughness * roughness;
        for i in 0..n {
            let nov = (i as f32 + 0.5) / n as f32;
            let v = [(1.0 - nov * nov).sqrt(), 0.0, nov];
            let (mut sa, mut sb) = (0.0f32, 0.0f32);
            for s in 0..DFG_SAMPLES {
                let h = importance_sample_ggx(hammersley(s, DFG_SAMPLES), a);
                let voh = v[0] * h[0] + v[1] * h[1] + v[2] * h[2];
                let l_z = 2.0 * voh * h[2] - v[2];
                let nol = l_z.clamp(0.0, 1.0);
                if nol > 0.0 {
                    let noh = h[2].max(1e-7);
                    let g_vis = v_smith_ggx_correlated(nov, nol, a) * 4.0 * nol * voh.max(0.0) / noh;
                    let fc = (1.0 - voh.max(0.0)).powi(5);
                    sa += (1.0 - fc) * g_vis;
                    sb += fc * g_vis;
                }
            }
            out.push([sa / DFG_SAMPLES as f32, sb / DFG_SAMPLES as f32, sheen_albedo(nov, roughness)]);
        }
    }
    out
}

/// web3d-M7: the directional albedo of the glTF sheen lobe (Charlie
/// distribution, Neubelt visibility) at `nov` for sheen roughness
/// `roughness`: the DFG table's third channel, which scales the layer
/// under the sheen by the energy the sheen takes. Uniform hemisphere
/// sampling (the lobe is broad, so this converges quickly).
pub(crate) fn sheen_albedo(nov: f32, roughness: f32) -> f32 {
    let r = roughness.max(0.07);
    let inv_a = 1.0 / (r * r);
    let v = [(1.0 - nov * nov).sqrt(), 0.0, nov];
    let mut sum = 0.0f32;
    for s in 0..DFG_SAMPLES {
        let [u1, u2] = hammersley(s, DFG_SAMPLES);
        let cos_t = u1;
        let sin_t = (1.0 - cos_t * cos_t).sqrt();
        let phi = 2.0 * std::f32::consts::PI * u2;
        let l = [sin_t * phi.cos(), sin_t * phi.sin(), cos_t];
        let hv = [v[0] + l[0], v[1] + l[1], v[2] + l[2]];
        let len = (hv[0] * hv[0] + hv[1] * hv[1] + hv[2] * hv[2]).sqrt().max(1e-7);
        let noh = hv[2] / len;
        let sin2 = (1.0 - noh * noh).max(0.0078125);
        let d = (2.0 + inv_a) * sin2.powf(inv_a * 0.5) / (2.0 * std::f32::consts::PI);
        let nol = cos_t;
        let vis = (1.0 / (4.0 * (nol + nov - nol * nov))).clamp(0.0, 1.0);
        // pdf of uniform hemisphere sampling is 1 / (2 pi).
        sum += d * vis * nol * 2.0 * std::f32::consts::PI;
    }
    (sum / DFG_SAMPLES as f32).min(1.0)
}

/// f32 → IEEE half (round to nearest; out-of-range clamps to the
/// largest finite half, so bright HDR texels stay bright, not infinite).
pub(crate) fn f16_bits(v: f32) -> u16 {
    if v.is_nan() {
        return 0;
    }
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32 - 127 + 15;
    let mant = x & 0x007f_ffff;
    if exp >= 31 {
        return sign | 0x7bff;
    }
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (mant | 0x0080_0000) >> (14 - exp);
        return sign | ((m + 1) >> 1) as u16;
    }
    let half = ((exp as u32) << 10) | (mant >> 13);
    let round = (mant >> 12) & 1;
    sign | (half + round).min(0x7bff) as u16
}

/// Upload linear RGBA floats as a single-mip Rgba16Float 2D texture.
pub(crate) fn upload_rgba16f(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    texels: &[[f32; 4]],
) -> wgpu::Texture {
    let data: Vec<u16> = texels.iter().flat_map(|t| t.map(f16_bits)).collect();
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::default(),
        bytemuck::cast_slice(&data),
    )
}

/// A built environment, ready to bind.
pub(crate) struct GpuEnvironment {
    pub equirect: wgpu::TextureView,
    pub specular: wgpu::TextureView,
    pub sh: [[f32; 4]; 9],
}

/// Per-pass parameters of the precompute shader.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct FaceParams {
    face: u32,
    roughness: f32,
    source_size: f32,
    samples: u32,
}

pub(crate) const PRECOMPUTE_SHADER: &str = r#"
struct FaceParams {
    face: u32,
    roughness: f32,
    source_size: f32,
    samples: u32,
};
@group(0) @binding(0) var<uniform> p: FaceParams;
@group(0) @binding(1) var t_equirect: texture_2d<f32>;
@group(0) @binding(2) var t_cube: texture_cube<f32>;
@group(0) @binding(3) var s_linear: sampler;

const PI: f32 = 3.14159265;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_face(@builtin(vertex_index) i: u32) -> VOut {
    var pos = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: VOut;
    out.pos = vec4<f32>(pos[i], 0.0, 1.0);
    out.uv = vec2<f32>(pos[i].x * 0.5 + 0.5, 0.5 - pos[i].y * 0.5);
    return out;
}

// The world direction through texel `uv` (v down) of cube face `face`,
// in the standard (D3D / Vulkan / WebGPU) face orientation.
fn face_dir(face: u32, uv: vec2<f32>) -> vec3<f32> {
    let s = uv.x * 2.0 - 1.0;
    let t = uv.y * 2.0 - 1.0;
    var d: vec3<f32>;
    switch face {
        case 0u: { d = vec3<f32>(1.0, -t, -s); }
        case 1u: { d = vec3<f32>(-1.0, -t, s); }
        case 2u: { d = vec3<f32>(s, 1.0, t); }
        case 3u: { d = vec3<f32>(s, -1.0, -t); }
        case 4u: { d = vec3<f32>(s, -t, 1.0); }
        default: { d = vec3<f32>(-s, -t, -1.0); }
    }
    return normalize(d);
}

fn equirect_uv(d: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(atan2(d.z, d.x) / (2.0 * PI) + 0.5, 0.5 - asin(clamp(d.y, -1.0, 1.0)) / PI);
}

@fragment
fn fs_from_equirect(in: VOut) -> @location(0) vec4<f32> {
    let d = face_dir(p.face, in.uv);
    return vec4<f32>(textureSampleLevel(t_equirect, s_linear, equirect_uv(d), 0.0).rgb, 1.0);
}

// One mip down: a bilinear tap between four texels of the level above
// (bound as a single-mip view) is their box average.
@fragment
fn fs_downsample(in: VOut) -> @location(0) vec4<f32> {
    let d = face_dir(p.face, in.uv);
    return vec4<f32>(textureSampleLevel(t_cube, s_linear, d, 0.0).rgb, 1.0);
}

fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), f32(reverseBits(i)) * 2.3283064e-10);
}

fn d_ggx(noh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let f = noh * noh * (a2 - 1.0) + 1.0;
    return a2 / (PI * f * f);
}

// GGX prefilter of the source cube at this mip's roughness, with n = v
// = r (the split-sum assumption) and filtered importance sampling.
@fragment
fn fs_prefilter(in: VOut) -> @location(0) vec4<f32> {
    let n = face_dir(p.face, in.uv);
    let a = max(p.roughness * p.roughness, 1e-4);
    var up = vec3<f32>(0.0, 1.0, 0.0);
    if (abs(n.y) > 0.999) {
        up = vec3<f32>(1.0, 0.0, 0.0);
    }
    let tx = normalize(cross(up, n));
    let ty = cross(n, tx);
    let texel_solid_angle = 4.0 * PI / (6.0 * p.source_size * p.source_size);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i: u32 = 0u; i < p.samples; i = i + 1u) {
        let xi = hammersley(i, p.samples);
        let phi = 2.0 * PI * xi.x;
        let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
        let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
        let h = tx * (sin_t * cos(phi)) + ty * (sin_t * sin(phi)) + n * cos_t;
        let l = 2.0 * dot(n, h) * h - n;
        let nol = dot(n, l);
        if (nol > 0.0) {
            let noh = max(dot(n, h), 0.0);
            let pdf = d_ggx(noh, a) / 4.0;
            let sample_solid_angle = 1.0 / (f32(p.samples) * pdf + 1e-6);
            let lod = max(0.5 * log2(sample_solid_angle / texel_solid_angle) + 1.0, 0.0);
            sum = sum + textureSampleLevel(t_cube, s_linear, l, lod).rgb * nol;
            weight = weight + nol;
        }
    }
    return vec4<f32>(sum / max(weight, 1e-6), 1.0);
}
"#;

/// Build the GPU side of an environment from a decoded HDR map.
pub(crate) fn build(device: &wgpu::Device, queue: &wgpu::Queue, img: &HdrImage) -> GpuEnvironment {
    let texels: Vec<[f32; 4]> = img.rgb.iter().map(|c| [c[0], c[1], c[2], 1.0]).collect();
    let equirect = upload_rgba16f(device, queue, "twe-kernel environment", img.width, img.height, &texels)
        .create_view(&wgpu::TextureViewDescriptor::default());

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twe-kernel env precompute bgl"),
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
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::Cube,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("twe-kernel env precompute"),
        source: wgpu::ShaderSource::Wgsl(PRECOMPUTE_SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("twe-kernel env precompute layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = |entry: &str| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_face"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
    };
    let from_equirect = pipeline("fs_from_equirect");
    let downsample = pipeline("fs_downsample");
    let prefilter = pipeline("fs_prefilter");
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("twe-kernel env precompute sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });

    let cube = |label, size: u32, mips: u32| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 6,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    };
    let source_mips = SOURCE_SIZE.ilog2() + 1;
    let source = cube("twe-kernel env source cube", SOURCE_SIZE, source_mips);
    let specular = cube("twe-kernel env specular cube", SPECULAR_SIZE, SPECULAR_MIPS);
    let cube_view = |t: &wgpu::Texture, base_mip: u32, mips: Option<u32>| {
        t.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            base_mip_level: base_mip,
            mip_level_count: mips,
            ..Default::default()
        })
    };
    let face_view = |t: &wgpu::Texture, mip: u32, face: u32| {
        t.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_mip_level: mip,
            mip_level_count: Some(1),
            base_array_layer: face,
            array_layer_count: Some(1),
            ..Default::default()
        })
    };
    // Placeholders for the input a pass doesn't read.
    let dummy_cube_tex = cube("twe-kernel env dummy cube", 1, 1);
    let dummy_cube = cube_view(&dummy_cube_tex, 0, None);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("twe-kernel env precompute"),
    });
    let mut pass = |pipeline: &wgpu::RenderPipeline,
                    target: &wgpu::TextureView,
                    params: FaceParams,
                    cube_input: &wgpu::TextureView| {
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("twe-kernel env pass params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel env pass bg"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&equirect),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(cube_input),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("twe-kernel env pass"),
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
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, &bg, &[]);
        rp.draw(0..3, 0..1);
    };
    let params = |face: u32, roughness: f32, source_size: u32, samples: u32| FaceParams {
        face,
        roughness,
        source_size: source_size as f32,
        samples,
    };
    // 1. Equirect → source cube, mip 0.
    for face in 0..6 {
        pass(&from_equirect, &face_view(&source, 0, face), params(face, 0.0, 0, 0), &dummy_cube);
    }
    // 2. The source cube's mip chain.
    for mip in 1..source_mips {
        let above = cube_view(&source, mip - 1, Some(1));
        for face in 0..6 {
            pass(&downsample, &face_view(&source, mip, face), params(face, 0.0, 0, 0), &above);
        }
    }
    // 3. The prefiltered specular cube: mip 0 is the mirror reflection
    //    (a plain resample), mip i roughness i / (mips - 1).
    let source_all = cube_view(&source, 0, None);
    for mip in 0..SPECULAR_MIPS {
        let roughness = mip as f32 / (SPECULAR_MIPS - 1) as f32;
        for face in 0..6 {
            let target = face_view(&specular, mip, face);
            if mip == 0 {
                let top = cube_view(&source, 1, Some(1));
                pass(&downsample, &target, params(face, 0.0, 0, 0), &top);
            } else {
                pass(
                    &prefilter,
                    &target,
                    params(face, roughness, SOURCE_SIZE, PREFILTER_SAMPLES),
                    &source_all,
                );
            }
        }
    }
    queue.submit(Some(encoder.finish()));

    GpuEnvironment {
        equirect,
        specular: cube_view(&specular, 0, None),
        sh: irradiance_sh9(img),
    }
}

/// The DFG table as an Rgba16Float texture (A, B, 0, 1).
pub(crate) fn dfg_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let texels: Vec<[f32; 4]> = dfg_lut().iter().map(|[a, b, c]| [*a, *b, *c, 1.0]).collect();
    upload_rgba16f(device, queue, "twe-kernel dfg lut", DFG_SIZE, DFG_SIZE, &texels)
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// A black 1×1 environment for frames without one.
pub(crate) fn placeholder(device: &wgpu::Device, queue: &wgpu::Queue) -> GpuEnvironment {
    let equirect = upload_rgba16f(device, queue, "twe-kernel no environment", 1, 1, &[[0.0, 0.0, 0.0, 1.0]])
        .create_view(&wgpu::TextureViewDescriptor::default());
    let cube = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("twe-kernel no specular"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::default(),
        bytemuck::cast_slice(&[f16_bits(0.0); 4 * 6]),
    );
    GpuEnvironment {
        equirect,
        specular: cube.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        }),
        sh: [[0.0; 4]; 9],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_round_trip_the_values_that_matter() {
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1.0e9), 0x7bff, "too bright clamps, never infinity");
        assert_eq!(f16_bits(f32::NAN), 0);
    }

    #[test]
    fn a_uniform_environment_gives_uniform_irradiance() {
        // Radiance 1 everywhere: irradiance π on every normal.
        let img = HdrImage {
            width: 64,
            height: 32,
            rgb: vec![[1.0; 3]; 64 * 32],
        };
        let sh = irradiance_sh9(&img);
        for n in [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.577, 0.577, 0.577]] {
            let b = sh_basis(n);
            let e: f32 = (0..9).map(|i| sh[i][0] * b[i]).sum();
            assert!((e - PI).abs() < 0.02, "E({n:?}) = {e}");
        }
    }

    #[test]
    fn a_sky_lights_upward_faces_more_than_downward_ones() {
        // Bright top half, dark bottom half.
        let (w, h) = (64usize, 32usize);
        let rgb = (0..w * h).map(|i| if i / w < h / 2 { [2.0; 3] } else { [0.0; 3] }).collect();
        let sh = irradiance_sh9(&HdrImage { width: w as u32, height: h as u32, rgb });
        let e = |n| (0..9).map(|i| sh[i][0] * sh_basis(n)[i]).sum::<f32>();
        assert!(e([0.0, 1.0, 0.0]) > 5.0 * e([0.0, -1.0, 0.0]).max(0.01));
    }

    #[test]
    fn dfg_terms_are_bounded_and_fall_with_roughness_at_grazing_angles() {
        let lut = dfg_lut();
        for [a, b, _] in &lut {
            assert!((0.0..=1.01).contains(&(a + b)), "{a} + {b}");
        }
        // Smooth, facing the viewer: almost all energy is reflected.
        let smooth_head_on = lut[(DFG_SIZE - 1) as usize];
        assert!(smooth_head_on[0] + smooth_head_on[1] > 0.95, "{smooth_head_on:?}");
        // Rough loses energy to single scattering (what the multiscatter
        // compensation restores).
        let rough = lut[((DFG_SIZE - 1) * DFG_SIZE + DFG_SIZE / 2) as usize];
        assert!(rough[0] + rough[1] < smooth_head_on[0] + smooth_head_on[1]);
    }
}
