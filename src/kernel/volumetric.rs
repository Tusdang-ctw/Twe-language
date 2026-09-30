//! web3d-M7: volumetric fog (Wronski 2014; Hillaire 2015, "Physically
//! based and unified volumetric rendering in Frostbite").
//!
//! The view frustum is divided into froxels — 160 × 90 screen tiles ×
//! 64 depth slices, each slice the same ratio deeper than the last. Per
//! frame:
//!
//! 1. **Scatter** (compute, per froxel): the height fog's density at the
//!    froxel's centre, and the light it scatters toward the eye — the
//!    fog colour, three quarters of it lit by the sun only where the sun reaches
//!    (one lookup in the sun's shadow cascades: light shafts), with the
//!    forward-scattering glow toward the sun, plus the point and spot
//!    lights of the froxel's light cluster (halos).
//! 2. **Integrate** (compute, per tile): front to back along the slices,
//!    the light reaching the eye and the transmittance so far, with the
//!    energy-conserving step (a slice of constant light and density
//!    scatters `L · (1 − e^(−σ·Δ))`).
//! 3. **Apply** (fullscreen): each pixel looks the volume up at its
//!    depth and blends `frame · T + S` in place.
//!
//! Unshadowed and without point lights, the result matches the closed-
//! form height fog it replaces (the main shader's `apply_fog` is turned
//! off meanwhile).

use bytemuck::{Pod, Zeroable};

pub(crate) const GRID: [u32; 3] = [160, 90, 64];
/// The first slice ends here (world units); slices grow exponentially
/// from it to the far plane.
const NEAR: f32 = 0.5;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct VolumeUniform {
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    /// Camera forward (xyz), first slice end.
    forward: [f32; 4],
    /// Density at the base height, falloff, base height, far distance.
    params: [f32; 4],
    /// Linear fog colour, forward-scattering strength.
    color: [f32; 4],
    /// Screen width, height.
    screen: [f32; 4],
}

/// One frame's fog.
pub(crate) struct VolumeFrame {
    pub inv_view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub forward: [f32; 3],
    pub density: f32,
    pub falloff: f32,
    pub far: f32,
    /// Linear.
    pub color: [f32; 3],
    pub glow: f32,
    pub screen: (u32, u32),
}

const COMMON: &str = r#"
struct Volume {
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    params: vec4<f32>,
    color: vec4<f32>,
    screen: vec4<f32>,
}

const GRID = vec3<u32>(160u, 90u, 64u);
// The share of the fog's light that comes from the sky, not the sun.
const SKY_SHARE: f32 = 0.25;

// Where slice boundary `k` (0..=64) lies, as view depth.
fn slice_depth(k: f32) -> f32 {
    if (k <= 0.0) {
        return 0.0;
    }
    return vol.forward.w * pow(vol.params.w / vol.forward.w, (k - 1.0) / (f32(GRID.z) - 1.0));
}

// The unit ray through a tile's (fractional) position, and the distance
// along it per unit of view depth.
fn tile_ray(uv: vec2<f32>) -> vec4<f32> {
    let w = vol.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    let d = normalize(w.xyz / w.w - vol.eye.xyz);
    return vec4<f32>(d, 1.0 / max(dot(d, vol.forward.xyz), 1e-3));
}
"#;

const SCATTER: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    time: vec4<f32>,
    eye: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
};
struct Lights {
    ambient: vec4<f32>,
    sun_dir: vec4<f32>,
};
struct PointLight {
    pos: vec4<f32>,
    color_radius: vec4<f32>,
    cone: vec4<f32>,
    params: vec4<f32>,
};
struct Clusters {
    view: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    grid: vec4<f32>,
    depth: vec4<f32>,
    screen: vec4<f32>,
};
struct Shadow {
    light_space_matrices: array<mat4x4<f32>, 3>,
    split_distances: vec4<f32>,
    flags: vec4<f32>,
    cascade_params: array<vec4<f32>, 3>,
};
@group(0) @binding(1) var<uniform> lights: Lights;
@group(0) @binding(11) var<storage, read> point_lights: array<PointLight>;
@group(0) @binding(12) var<storage, read> light_grid: array<u32>;
@group(0) @binding(13) var<uniform> clusters: Clusters;
@group(1) @binding(0) var<uniform> shadow_u: Shadow;
@group(1) @binding(1) var t_shadow: texture_depth_2d_array;
@group(1) @binding(2) var s_shadow: sampler_comparison;
@group(2) @binding(0) var<uniform> vol: Volume;
@group(2) @binding(1) var out_scatter: texture_storage_3d<rgba16float, write>;

fn cluster_of(frag: vec2<f32>, z: f32) -> u32 {
    let g = vec2<u32>(clusters.grid.xy);
    let tile = min(vec2<u32>(frag / (clusters.screen.xy / clusters.grid.xy)), g - vec2<u32>(1u));
    let slice = u32(clamp(floor(log(max(z, 1e-4)) * clusters.depth.z - clusters.depth.w), 0.0, clusters.grid.z - 1.0));
    return tile.x + g.x * (tile.y + g.y * slice);
}

// Whether the sun reaches `p` (one hardware-filtered tap).
fn sun_visibility(p: vec3<f32>, view_z: f32) -> f32 {
    if (shadow_u.flags.w < 0.5) {
        return 1.0;
    }
    var cascade: i32 = 2;
    if (view_z < shadow_u.split_distances.x) {
        cascade = 0;
    } else if (view_z < shadow_u.split_distances.y) {
        cascade = 1;
    } else if (view_z >= shadow_u.split_distances.z) {
        return 1.0;
    }
    let lp = shadow_u.light_space_matrices[cascade] * vec4<f32>(p, 1.0);
    let suv = vec3<f32>(lp.x / lp.w * 0.5 + 0.5, lp.y / lp.w * -0.5 + 0.5, lp.z / lp.w);
    if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0 || suv.z < 0.0 || suv.z > 1.0) {
        return 1.0;
    }
    return textureSampleCompareLevel(t_shadow, s_shadow, suv.xy, cascade, suv.z);
}

@compute @workgroup_size(8, 8, 1)
fn cs_scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= GRID.x || id.y >= GRID.y || id.z >= GRID.z) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(GRID.xy);
    let ray = tile_ray(uv);
    let z = 0.5 * (slice_depth(f32(id.z)) + slice_depth(f32(id.z) + 1.0));
    let p = vol.eye.xyz + ray.xyz * (z * ray.w);
    let density = vol.params.x * exp(-vol.params.y * (p.y - vol.params.z));
    // The fog colour: a quarter from the sky, which no shadow blocks,
    // and the rest from the sun (so unoccluded it is the closed-form
    // fog's brightness), glowing toward the sun.
    var light = vol.color.rgb * SKY_SHARE;
    if (lights.sun_dir.w > 0.0) {
        let lobe = pow(max(dot(ray.xyz, normalize(lights.sun_dir.xyz)), 0.0), 8.0) * vol.color.w;
        light = light + vol.color.rgb * (1.0 - SKY_SHARE + lobe) * sun_visibility(p, z);
    } else {
        light = vol.color.rgb;
    }
    // Point and spot lights of this froxel's cluster, with the surfaces'
    // falloff.
    if (clusters.grid.w > 0.0) {
        let base = cluster_of(uv * clusters.screen.xy, z) * 129u;
        let count = light_grid[base];
        for (var k: u32 = 0u; k < count; k = k + 1u) {
            let pl = point_lights[light_grid[base + 1u + k]];
            let r = pl.color_radius.w;
            let to_light = pl.pos.xyz - p;
            let dist = length(to_light);
            if (r <= 0.0 || dist >= r) {
                continue;
            }
            var spot = 1.0;
            if (pl.cone.w > -1.5) {
                spot = smoothstep(pl.cone.w, pl.params.x, dot(-to_light / dist, pl.cone.xyz));
            }
            let t = 1.0 - dist / r;
            light = light + vol.color.rgb * pl.color_radius.rgb * (t * t * spot);
        }
    }
    textureStore(out_scatter, id, vec4<f32>(light, density));
}
"#;

const INTEGRATE: &str = r#"
@group(0) @binding(0) var<uniform> vol: Volume;
@group(0) @binding(1) var in_scatter: texture_3d<f32>;
@group(0) @binding(2) var out_volume: texture_storage_3d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_integrate(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= GRID.x || id.y >= GRID.y) {
        return;
    }
    let ray = tile_ray((vec2<f32>(id.xy) + 0.5) / vec2<f32>(GRID.xy));
    var scattered = vec3<f32>(0.0);
    var transmittance = 1.0;
    for (var k: u32 = 0u; k < GRID.z; k = k + 1u) {
        let v = textureLoad(in_scatter, vec3<u32>(id.xy, k), 0);
        let seg = (slice_depth(f32(k) + 1.0) - slice_depth(f32(k))) * ray.w;
        let step = exp(-v.a * seg);
        scattered = scattered + transmittance * v.rgb * (1.0 - step);
        transmittance = transmittance * step;
        textureStore(out_volume, vec3<u32>(id.xy, k), vec4<f32>(scattered, transmittance));
    }
}
"#;

const APPLY: &str = r#"
@group(0) @binding(0) var<uniform> vol: Volume;
@group(0) @binding(1) var t_volume: texture_3d<f32>;
@group(0) @binding(2) var s_volume: sampler;
@group(0) @binding(3) var t_depth: texture_depth_multisampled_2d;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}

@fragment
fn fs_apply(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy / vol.screen.xy;
    let d = textureLoad(t_depth, vec2<i32>(frag.xy), 0);
    var z = vol.params.w;
    if (d < 1.0) {
        let w = vol.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
        z = dot(w.xyz / w.w - vol.eye.xyz, vol.forward.xyz);
    }
    // Slice k stores what has accumulated by its far boundary k + 1.
    var k = 0.0;
    if (z > vol.forward.w) {
        k = 1.0 + log(z / vol.forward.w) / log(vol.params.w / vol.forward.w) * (f32(GRID.z) - 1.0);
    } else {
        k = z / vol.forward.w;
    }
    let w = clamp((k - 0.5) / f32(GRID.z), 0.5 / f32(GRID.z), 1.0 - 0.5 / f32(GRID.z));
    var v = textureSampleLevel(t_volume, s_volume, vec3<f32>(uv, w), 0.0);
    // In front of the first slice's end, fade in from nothing.
    if (k < 1.0) {
        v = mix(vec4<f32>(0.0, 0.0, 0.0, 1.0), v, clamp(k, 0.0, 1.0));
    }
    return v;
}
"#;

pub(crate) struct Volumetric {
    uniform: wgpu::Buffer,
    volume_view: wgpu::TextureView,
    scatter_pipeline: wgpu::ComputePipeline,
    scatter_bg: wgpu::BindGroup,
    integrate_pipeline: wgpu::ComputePipeline,
    integrate_bg: wgpu::BindGroup,
    apply_pipeline: wgpu::RenderPipeline,
    apply_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Volumetric {
    /// `frame_layout` / `shadow_layout`: the frame and shadow groups the
    /// scatter pass reads (lights, clusters; the sun's cascades).
    pub(crate) fn new(
        device: &wgpu::Device,
        frame_layout: &wgpu::BindGroupLayout,
        shadow_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let volume = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: GRID[0],
                        height: GRID[1],
                        depth_or_array_layers: GRID[2],
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let scatter_view = volume("twe-kernel fog scatter");
        let volume_view = volume("twe-kernel fog volume");
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twe-kernel fog volume uniform"),
            size: std::mem::size_of::<VolumeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        };
        let texture_entry = |binding, visibility, sample_type, dim, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: dim,
                multisampled,
            },
            count: None,
        };
        let compute = wgpu::ShaderStages::COMPUTE;
        let scatter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel fog scatter bgl"),
            entries: &[uniform_entry(0, compute), storage_entry(1)],
        });
        let integrate_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel fog integrate bgl"),
            entries: &[
                uniform_entry(0, compute),
                texture_entry(
                    1,
                    compute,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::TextureViewDimension::D3,
                    false,
                ),
                storage_entry(2),
            ],
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let apply_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel fog apply bgl"),
            entries: &[
                uniform_entry(0, fragment),
                texture_entry(
                    1,
                    fragment,
                    wgpu::TextureSampleType::Float { filterable: true },
                    wgpu::TextureViewDimension::D3,
                    false,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_entry(3, fragment, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2, true),
            ],
        });
        let module = |label, body: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(format!("{COMMON}{body}").into()),
            })
        };
        let compute_pipeline = |label, layouts: &[Option<&wgpu::BindGroupLayout>], module: &wgpu::ShaderModule, entry| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let scatter_pipeline = compute_pipeline(
            "twe-kernel fog scatter",
            &[Some(frame_layout), Some(shadow_layout), Some(&scatter_layout)],
            &module("twe-kernel fog scatter", SCATTER),
            "cs_scatter",
        );
        let integrate_pipeline = compute_pipeline(
            "twe-kernel fog integrate",
            &[Some(&integrate_layout)],
            &module("twe-kernel fog integrate", INTEGRATE),
            "cs_integrate",
        );
        let apply_module = module("twe-kernel fog apply", APPLY);
        let apply_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel fog apply"),
            bind_group_layouts: &[Some(&apply_layout)],
            immediate_size: 0,
        });
        let apply_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel fog apply"),
            layout: Some(&apply_pl),
            vertex: wgpu::VertexState {
                module: &apply_module,
                entry_point: Some("vs_full"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &apply_module,
                entry_point: Some("fs_apply"),
                // frame · T + S, in place; the frame's alpha is kept.
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::SrcAlpha,
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
        let scatter_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel fog scatter bg"),
            layout: &scatter_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&scatter_view),
                },
            ],
        });
        let integrate_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel fog integrate bg"),
            layout: &integrate_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&scatter_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&volume_view),
                },
            ],
        });
        Volumetric {
            uniform,
            volume_view,
            scatter_pipeline,
            scatter_bg,
            integrate_pipeline,
            integrate_bg,
            apply_pipeline,
            apply_layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("twe-kernel fog volume sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    pub(crate) fn prepare(&self, queue: &wgpu::Queue, f: &VolumeFrame) {
        let u = VolumeUniform {
            inv_view_proj: f.inv_view_proj,
            eye: [f.eye[0], f.eye[1], f.eye[2], 1.0],
            forward: [f.forward[0], f.forward[1], f.forward[2], NEAR.min(f.far * 0.5)],
            params: [f.density, f.falloff, 0.0, f.far],
            color: [f.color[0], f.color[1], f.color[2], f.glow],
            screen: [f.screen.0 as f32, f.screen.1 as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
    }

    /// Scatter and integrate. `frame` / `shadow`: the main pass's frame
    /// and shadow groups.
    pub(crate) fn record_volume(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::BindGroup,
        shadow: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("twe-kernel fog volume"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.scatter_pipeline);
        pass.set_bind_group(0, frame, &[]);
        pass.set_bind_group(1, shadow, &[]);
        pass.set_bind_group(2, &self.scatter_bg, &[]);
        pass.dispatch_workgroups(GRID[0].div_ceil(8), GRID[1].div_ceil(8), GRID[2]);
        pass.set_pipeline(&self.integrate_pipeline);
        pass.set_bind_group(0, &self.integrate_bg, &[]);
        pass.dispatch_workgroups(GRID[0].div_ceil(8), GRID[1].div_ceil(8), 1);
    }

    /// Blend the fog over the frame in `hdr`.
    pub(crate) fn record_apply(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        hdr: &wgpu::TextureView,
    ) {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel fog apply bg"),
            layout: &self.apply_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.volume_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("twe-kernel fog apply"),
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
        pass.set_pipeline(&self.apply_pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }
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
    fn fog_volume_shaders_validate() {
        validate("scatter", &format!("{COMMON}{SCATTER}"));
        validate("integrate", &format!("{COMMON}{INTEGRATE}"));
        validate("apply", &format!("{COMMON}{APPLY}"));
    }

    #[test]
    fn uniform_layout() {
        assert_eq!(std::mem::size_of::<VolumeUniform>(), 144);
    }
}
