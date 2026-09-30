//! web3d-M7 session 12: clustered forward lighting.
//!
//! The view frustum is divided into a grid of clusters: 16 × 9 screen
//! tiles, each split into 24 depth slices spaced exponentially (Olsson,
//! Billeter & Assarsson 2012, "Clustered Deferred and Forward Shading";
//! the slicing of Doom 2016 and Filament). A compute pass lists, for
//! every cluster, the lights whose range reaches its view-space box; a
//! surface then shades only the lights of the cluster it falls in, so
//! a scene can hold hundreds of lights while each pixel pays for the
//! few near it.
//!
//! Spot lights are listed by the sphere of their range (conservative;
//! the cone test happens when shading).

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::render3d_types::{PointLightU, MAX_LIGHTS};

pub(crate) const GRID_X: u32 = 32;
pub(crate) const GRID_Y: u32 = 18;
pub(crate) const GRID_Z: u32 = 64;
pub(crate) const CLUSTERS: u32 = GRID_X * GRID_Y * GRID_Z;
/// Most lights one cluster lists (a count, then the indices). Beyond
/// this a cluster drops the rest; at 128 overlapping lights per cluster
/// a scene has other problems.
pub(crate) const MAX_PER_CLUSTER: u32 = 128;

/// The clustering parameters, shared by the build pass and every
/// surface shader.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct ClusterUniform {
    /// World → view.
    pub view: [[f32; 4]; 4],
    /// Clip → view (unjittered).
    pub inv_proj: [[f32; 4]; 4],
    /// Grid x, y, z, light count.
    pub grid: [f32; 4],
    /// Near, far, slice scale, slice bias: slice = log(z)·scale − bias.
    pub depth: [f32; 4],
    /// Target width, height, _, _.
    pub screen: [f32; 4],
}

impl ClusterUniform {
    pub fn new(view: [[f32; 4]; 4], inv_proj: [[f32; 4]; 4], near: f32, far: f32, size: (u32, u32), lights: u32) -> Self {
        let scale = GRID_Z as f32 / (far / near).ln();
        ClusterUniform {
            view,
            inv_proj,
            grid: [GRID_X as f32, GRID_Y as f32, GRID_Z as f32, lights as f32],
            depth: [near, far, scale, scale * near.ln()],
            screen: [size.0 as f32, size.1 as f32, 0.0, 0.0],
        }
    }
}

/// The depth slice holding view depth `z` (the shader's formula).
#[cfg(test)]
pub(crate) fn slice_of(u: &ClusterUniform, z: f32) -> u32 {
    ((z.ln() * u.depth[2] - u.depth[3]).floor()).clamp(0.0, GRID_Z as f32 - 1.0) as u32
}

const BUILD_SHADER: &str = r#"
struct Clusters {
    view: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    grid: vec4<f32>,
    depth: vec4<f32>,
    screen: vec4<f32>,
};
struct Light {
    pos: vec4<f32>,
    color_radius: vec4<f32>,
    cone: vec4<f32>,
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> clusters: Clusters;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var<storage, read_write> grid: array<u32>;

const MAX_PER_CLUSTER: u32 = 128u;

// A view-space point on the ray through `ndc` at view depth `z` (> 0).
fn at_depth(ndc: vec2<f32>, z: f32) -> vec3<f32> {
    let p = clusters.inv_proj * vec4<f32>(ndc, 1.0, 1.0);
    let dir = p.xyz / p.w;
    return dir * (z / -dir.z);
}

// web3d-M7 follow-up: lights go through workgroup memory in batches of
// 64 — loaded and moved to view space once per workgroup, not once per
// cluster — so the build reads each light 1/64th as often (500 lights
// over 36,864 clusters cost 2.4 ms a frame on an integrated GPU).
var<workgroup> batch_lights: array<vec4<f32>, 64>;

@compute @workgroup_size(64)
fn cs_build(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let gx = u32(clusters.grid.x);
    let gy = u32(clusters.grid.y);
    let gz = u32(clusters.grid.z);
    let c = id.x;
    // Every invocation reaches the barriers; out-of-range ones only
    // help load.
    let valid = c < gx * gy * gz;
    let tx = c % gx;
    let ty = (c / gx) % gy;
    let tz = min(c / (gx * gy), gz - 1u);
    // The cluster's view-space box: its tile's corners at the slice's
    // near and far depths.
    let near = clusters.depth.x;
    let far = clusters.depth.y;
    let z0 = near * pow(far / near, f32(tz) / f32(gz));
    let z1 = near * pow(far / near, f32(tz + 1u) / f32(gz));
    let x0 = f32(tx) / f32(gx) * 2.0 - 1.0;
    let x1 = f32(tx + 1u) / f32(gx) * 2.0 - 1.0;
    // Tiles count down from the top of the screen.
    let y0 = 1.0 - f32(ty + 1u) / f32(gy) * 2.0;
    let y1 = 1.0 - f32(ty) / f32(gy) * 2.0;
    var lo = vec3<f32>(1e30);
    var hi = vec3<f32>(-1e30);
    for (var k: i32 = 0; k < 8; k = k + 1) {
        let ndc = vec2<f32>(select(x0, x1, (k & 1) != 0), select(y0, y1, (k & 2) != 0));
        let p = at_depth(ndc, select(z0, z1, (k & 4) != 0));
        lo = min(lo, p);
        hi = max(hi, p);
    }
    let count_max = u32(clusters.grid.w);
    let base = c * (MAX_PER_CLUSTER + 1u);
    var n = 0u;
    for (var start = 0u; start < count_max; start = start + 64u) {
        let i = start + li;
        var entry = vec4<f32>(0.0, 0.0, 0.0, -1.0);
        if (i < count_max) {
            let l = lights[i];
            if (l.color_radius.w > 0.0) {
                entry = vec4<f32>((clusters.view * vec4<f32>(l.pos.xyz, 1.0)).xyz, l.color_radius.w);
            }
        }
        batch_lights[li] = entry;
        workgroupBarrier();
        if (valid) {
            let m = min(64u, count_max - start);
            for (var j = 0u; j < m; j = j + 1u) {
                let e = batch_lights[j];
                if (e.w > 0.0) {
                    let d = e.xyz - clamp(e.xyz, lo, hi);
                    if (dot(d, d) <= e.w * e.w && n < MAX_PER_CLUSTER) {
                        grid[base + 1u + n] = start + j;
                        n = n + 1u;
                    }
                }
            }
        }
        workgroupBarrier();
    }
    if (valid) {
        grid[base] = n;
    }
}
"#;

/// The build shader, for validation tests.
#[cfg(test)]
pub(crate) fn shader_source() -> &'static str {
    BUILD_SHADER
}

/// The light list, the cluster grid and the build pass.
pub(crate) struct Clusters {
    pub uniform: wgpu::Buffer,
    pub lights: wgpu::Buffer,
    pub grid: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
}

impl Clusters {
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel clusters bgl"),
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
                storage(2, false),
            ],
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("twe-kernel clusters uniform"),
            contents: bytemuck::bytes_of(&ClusterUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let lights = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twe-kernel lights"),
            size: (MAX_LIGHTS * std::mem::size_of::<PointLightU>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let grid = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twe-kernel light clusters"),
            size: u64::from(CLUSTERS * (MAX_PER_CLUSTER + 1)) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel clusters"),
            source: wgpu::ShaderSource::Wgsl(BUILD_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel clusters layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("twe-kernel clusters"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cs_build"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel clusters bg"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: grid.as_entire_binding(),
                },
            ],
        });
        Clusters {
            uniform,
            lights,
            grid,
            pipeline,
            bind,
        }
    }

    /// Upload the frame's lights and parameters.
    pub fn prepare(&self, queue: &wgpu::Queue, lights: &[PointLightU], u: &ClusterUniform) {
        if !lights.is_empty() {
            queue.write_buffer(&self.lights, 0, bytemuck::cast_slice(lights));
        }
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(u));
    }

    /// Record the build pass.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("twe-kernel light clusters"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.dispatch_workgroups(CLUSTERS.div_ceil(64), 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slices cover near to far, exponentially: each spans the same
    /// ratio of depths.
    #[test]
    fn slices_are_exponential() {
        let id = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let u = ClusterUniform::new(id, id, 0.1, 100.0, (1280, 720), 0);
        assert_eq!(slice_of(&u, 0.1001), 0);
        assert_eq!(slice_of(&u, 99.9), GRID_Z - 1);
        // Each slice spans the same depth ratio: (far / near)^(1 / Z).
        let ratio = (100.0f32 / 0.1).powf(1.0 / GRID_Z as f32);
        for k in 1..GRID_Z {
            let start = 0.1 * ratio.powi(k as i32);
            assert_eq!(slice_of(&u, start * 1.001), k, "slice {k} starts at {start}");
        }
    }
}
