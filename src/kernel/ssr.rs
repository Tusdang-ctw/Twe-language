//! web3d-M7: screen-space reflections.
//!
//! The main passes write a surface record per pixel (octahedral world
//! normal, roughness, and the weight of the environment's specular
//! reflection in the pixel; see `g_surface` in the main shader). This
//! pass marches each reflective pixel's reflected ray through the depth
//! buffer; where it hits something on screen, what the pixel reflects is
//! that, not the environment. The correction `weight · (hit − env)` goes
//! into a target that is then added to the frame — so a reflection
//! replaces the environment's reflection instead of adding to it.
//!
//! Rays that leave the screen, pass behind geometry or point back at the
//! camera fade to the environment; rough surfaces (roughness ≥ 0.7) keep
//! it (the frame has no mips to blur a hit by).

use bytemuck::{Pod, Zeroable};

/// Reflections keep the environment above this roughness.
const MAX_ROUGHNESS: f32 = 0.7;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SsrUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    /// width, height, strength, frame.
    screen: [f32; 4],
    /// Environment intensity, max specular LOD, has environment (0/1),
    /// the roughness cut-off.
    env: [f32; 4],
    /// The ambient colour (what's reflected without an environment).
    ambient: [f32; 4],
}

/// One frame's inputs.
pub(crate) struct SsrFrame {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub strength: f32,
    pub frame: u32,
    /// Environment intensity, max specular LOD, has environment.
    pub env: [f32; 3],
    pub ambient: [f32; 3],
}

const SHADER: &str = r#"
struct U {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    screen: vec4<f32>,
    env: vec4<f32>,
    ambient: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var t_hdr: texture_2d<f32>;
@group(0) @binding(2) var t_depth: texture_depth_multisampled_2d;
@group(0) @binding(3) var t_surface: texture_2d<f32>;
@group(0) @binding(4) var t_env: texture_cube<f32>;
@group(0) @binding(5) var s_linear: sampler;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}

fn world_at(uv: vec2<f32>, d: f32) -> vec3<f32> {
    let w = u.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    return w.xyz / w.w;
}

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    let f = e * 2.0 - 1.0;
    var n = vec3<f32>(f, 1.0 - abs(f.x) - abs(f.y));
    let t = clamp(-n.z, 0.0, 1.0);
    n = vec3<f32>(n.xy + select(vec2<f32>(t), vec2<f32>(-t), n.xy >= vec2<f32>(0.0)), n.z);
    return normalize(n);
}

fn depth_at(uv: vec2<f32>) -> f32 {
    let px = clamp(vec2<i32>(uv * u.screen.xy), vec2<i32>(0), vec2<i32>(u.screen.xy) - 1);
    return textureLoad(t_depth, px, 0);
}

const STEPS: i32 = 48;
const MIN_WEIGHT: f32 = 0.05;
const MAX_DISTANCE: f32 = 40.0;

@fragment
fn fs_trace(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let surf = textureLoad(t_surface, px, 0);
    let weight = surf.a * u.screen.z;
    let roughness = surf.b;
    let d = textureLoad(t_depth, px, 0);
    // Too faint to see (a dielectric head-on reflects ~4%), too rough,
    // or the backdrop: keep the environment.
    if (weight < MIN_WEIGHT || roughness >= u.env.w || d >= 1.0) {
        return vec4<f32>(0.0);
    }
    let uv = frag.xy / u.screen.xy;
    let p = world_at(uv, d);
    let n = oct_decode(surf.xy);
    let v = normalize(u.eye.xyz - p);
    let r = reflect(-v, n);
    // Interleaved gradient noise (Jimenez 2014) staggers the steps per
    // pixel and frame (TAA averages the pattern away).
    let jitter = fract(52.9829189 * fract(dot(frag.xy + u.screen.w * 5.588238, vec2<f32>(0.06711056, 0.00583715))));
    var hit = false;
    var hit_uv = vec2<f32>(0.0);
    var hit_t = 0.0;
    var prev = 0.0;
    for (var i: i32 = 0; i < STEPS; i = i + 1) {
        // Steps grow with distance (finer close to the surface).
        let s = (f32(i) + jitter) / f32(STEPS);
        let t = MAX_DISTANCE * s * s + 0.02;
        let q = p + r * t;
        let clip = u.view_proj * vec4<f32>(q, 1.0);
        if (clip.w <= 0.0) {
            break;
        }
        let ndc = clip.xyz / clip.w;
        if (abs(ndc.x) >= 1.0 || abs(ndc.y) >= 1.0 || ndc.z >= 1.0) {
            break;
        }
        let quv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let sd = depth_at(quv);
        if (ndc.z > sd) {
            // Behind the depth buffer: a hit if only just (the surface's
            // assumed thickness grows with the step).
            let behind = distance(u.eye.xyz, q) - distance(u.eye.xyz, world_at(quv, sd));
            if (behind < 0.25 + 0.1 * (t - prev)) {
                // Refine between the last step in front and this one.
                var lo = prev;
                var hi = t;
                for (var k: i32 = 0; k < 5; k = k + 1) {
                    let mid = 0.5 * (lo + hi);
                    let mc = u.view_proj * vec4<f32>(p + r * mid, 1.0);
                    let mn = mc.xyz / mc.w;
                    let muv = vec2<f32>(mn.x * 0.5 + 0.5, 0.5 - mn.y * 0.5);
                    if (mn.z > depth_at(muv)) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                let hc = u.view_proj * vec4<f32>(p + r * hi, 1.0);
                let hn = hc.xy / hc.w;
                hit_uv = vec2<f32>(hn.x * 0.5 + 0.5, 0.5 - hn.y * 0.5);
                hit_t = hi;
                hit = true;
            }
            break;
        }
        prev = t;
    }
    if (!hit) {
        return vec4<f32>(0.0);
    }
    // Fade toward the screen's edges, with distance, with roughness, and
    // for rays heading back toward the camera.
    let edge = hit_uv * 2.0 - 1.0;
    let edge_fade = 1.0 - smoothstep(0.8, 1.0, max(abs(edge.x), abs(edge.y)));
    let distance_fade = 1.0 - smoothstep(0.6, 1.0, hit_t / MAX_DISTANCE);
    let rough_fade = 1.0 - smoothstep(u.env.w * 0.5, u.env.w, roughness);
    let facing_fade = 1.0 - smoothstep(0.4, 0.9, dot(r, v));
    let confidence = edge_fade * distance_fade * rough_fade * facing_fade;
    let reflected = textureSampleLevel(t_hdr, s_linear, hit_uv, 0.0).rgb;
    var env = u.ambient.rgb;
    if (u.env.z > 0.5) {
        env = textureSampleLevel(t_env, s_linear, r, roughness * u.env.y).rgb * u.env.x;
    }
    return vec4<f32>(weight * confidence * (reflected - env), 0.0);
}

@group(0) @binding(0) var t_reflection: texture_2d<f32>;

@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(textureLoad(t_reflection, vec2<i32>(frag.xy), 0).rgb, 0.0);
}
"#;

/// The trace shader alone (the composite's binding 0 differs).
fn trace_source() -> String {
    SHADER[..SHADER.find("@group(0) @binding(0) var t_reflection").expect("composite")].to_string()
}

/// The composite shader alone.
fn composite_source() -> String {
    let start = SHADER.find("@group(0) @binding(0) var t_reflection").expect("composite");
    let vs_start = SHADER.find("@vertex").expect("vs");
    let vs_end = SHADER.find("fn world_at").expect("world_at");
    format!("{}{}", &SHADER[vs_start..vs_end], &SHADER[start..])
}

pub(crate) struct Ssr {
    layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    trace: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
}

pub(crate) const REFLECTION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

impl Ssr {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let texture = |binding, sample_type, dim, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: dim,
                multisampled,
            },
            count: None,
        };
        use wgpu::TextureViewDimension as Dim;
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel ssr bgl"),
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
                texture(1, float, Dim::D2, false),
                texture(2, wgpu::TextureSampleType::Depth, Dim::D2, true),
                texture(3, wgpu::TextureSampleType::Float { filterable: false }, Dim::D2, false),
                texture(4, float, Dim::Cube, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel ssr composite bgl"),
            entries: &[texture(0, wgpu::TextureSampleType::Float { filterable: false }, Dim::D2, false)],
        });
        let pipeline = |label, source: String, layout: &wgpu::BindGroupLayout, entry, blend| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: REFLECTION_FORMAT,
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
        // The correction adds onto the frame; the frame's alpha (the
        // transmission coverage) is kept.
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        Ssr {
            trace: pipeline("twe-kernel ssr trace", trace_source(), &layout, "fs_trace", None),
            composite: pipeline("twe-kernel ssr composite", composite_source(), &composite_layout, "fs_composite", Some(add)),
            layout,
            composite_layout,
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("twe-kernel ssr uniform"),
                size: std::mem::size_of::<SsrUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("twe-kernel ssr sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    pub(crate) fn prepare(&self, queue: &wgpu::Queue, size: (u32, u32), f: &SsrFrame) {
        let u = SsrUniform {
            view_proj: f.view_proj,
            inv_view_proj: f.inv_view_proj,
            eye: [f.eye[0], f.eye[1], f.eye[2], 1.0],
            screen: [size.0 as f32, size.1 as f32, f.strength, (f.frame % 64) as f32],
            env: [f.env[0], f.env[1], f.env[2], MAX_ROUGHNESS],
            ambient: [f.ambient[0], f.ambient[1], f.ambient[2], 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
    }

    /// Trace into `reflection`: the correction each pixel's reflection
    /// makes to the frame.
    pub(crate) fn record_trace(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        inputs: [&wgpu::TextureView; 4],
        reflection: &wgpu::TextureView,
    ) {
        let [hdr, depth, surface, env] = inputs;
        let view = wgpu::BindingResource::TextureView;
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel ssr bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry { binding: 1, resource: view(hdr) },
                wgpu::BindGroupEntry { binding: 2, resource: view(depth) },
                wgpu::BindGroupEntry { binding: 3, resource: view(surface) },
                wgpu::BindGroupEntry { binding: 4, resource: view(env) },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        fullscreen(encoder, "twe-kernel ssr trace", &self.trace, &bg, reflection, true);
    }

    /// Add the correction onto the frame.
    pub(crate) fn record_composite(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        reflection: &wgpu::TextureView,
        hdr: &wgpu::TextureView,
    ) {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel ssr composite bg"),
            layout: &self.composite_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(reflection),
            }],
        });
        fullscreen(encoder, "twe-kernel ssr composite", &self.composite, &bg, hdr, false);
    }
}

fn fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::RenderPipeline,
    bg: &wgpu::BindGroup,
    target: &wgpu::TextureView,
    clear: bool,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                } else {
                    wgpu::LoadOp::Load
                },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(label: &str, src: &str) {
        let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(src)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{label}: {e:?}"));
    }

    #[test]
    fn ssr_shaders_validate() {
        validate("trace", &trace_source());
        validate("composite", &composite_source());
    }

    #[test]
    fn uniform_layout() {
        assert_eq!(std::mem::size_of::<SsrUniform>(), 192);
    }
}
