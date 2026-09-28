//! web3d-M3: the 3D renderer's HUD — screen-space text and rectangles
//! drawn over the tonemapped scene (`text()` / `rect()` inside a 3D
//! `on render():`, `docs/06` §7.5).
//!
//! Coordinates are the 2D runtime's 640×480 canvas, scaled to the
//! target, so HUD code reads the same in 2D and 3D. Text is ProggyClean
//! (MIT, see `ProggyClean.LICENSE.txt`), rasterised once by `fontdue`
//! into an R8 atlas; a solid block in the atlas serves rectangles. One
//! alpha-blended, instanced-free triangle list per frame.

use bytemuck::{Pod, Zeroable};

use crate::render3d_types::HudItem;

const FONT: &[u8] = include_bytes!("ProggyClean.ttf");
/// Pixel size glyphs are rasterised at; drawn sizes scale from it.
/// ProggyClean is a 13 px bitmap-style font; 26 keeps it crisp at 2×.
const BASE_PX: f32 = 26.0;
/// The virtual canvas HUD coordinates live in.
pub const CANVAS_W: f32 = 640.0;
pub const CANVAS_H: f32 = 480.0;
const FIRST: u32 = 32;
const LAST: u32 = 126;
const ATLAS_W: u32 = 512;

pub(crate) const HUD_SHADER: &str = r#"
struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};
@group(0) @binding(0) var t_atlas: texture_2d<f32>;
@group(0) @binding(1) var s_atlas: sampler;

@vertex
fn vs_hud(v: VIn) -> VOut {
    var out: VOut;
    out.clip = vec4<f32>(v.pos, 0.0, 1.0);
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

@fragment
fn fs_hud(in: VOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(t_atlas, s_atlas, in.uv).r;
    // Script colours are sRGB; the target is an sRGB view that encodes
    // on write, so decode to linear here and they land as written.
    let linear = pow(in.color.rgb, vec3<f32>(2.2));
    return vec4<f32>(linear, in.color.a * coverage);
}
"#;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct HudVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Copy, Default)]
struct Glyph {
    uv: [f32; 4],
    width: f32,
    height: f32,
    xmin: f32,
    ymin: f32,
    advance: f32,
}

pub struct Hud {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    glyphs: Vec<Glyph>,
    /// UV rectangle of a solid block, for rectangles.
    solid: [f32; 4],
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    vertex_count: u32,
}

impl Hud {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let font = fontdue::Font::from_bytes(FONT, fontdue::FontSettings::default())
            .expect("the bundled HUD font parses");
        // Rasterise the printable ASCII range into rows of the atlas.
        let mut rasters = Vec::new();
        for code in FIRST..=LAST {
            let ch = char::from_u32(code).unwrap_or(' ');
            rasters.push(font.rasterize(ch, BASE_PX));
        }
        let pad = 2u32;
        let (mut x, mut y, mut row_h) = (pad, pad, 0u32);
        let mut places = Vec::with_capacity(rasters.len());
        for (m, _) in &rasters {
            let (w, h) = (m.width as u32, m.height as u32);
            if x + w + pad > ATLAS_W {
                x = pad;
                y += row_h + pad;
                row_h = 0;
            }
            places.push((x, y));
            x += w + pad;
            row_h = row_h.max(h);
        }
        // The solid block for rectangles, on its own row.
        let solid_at = (pad, y + row_h + pad);
        let atlas_h = (solid_at.1 + 4 + pad).next_power_of_two();
        let mut pixels = vec![0u8; (ATLAS_W * atlas_h) as usize];
        for ((m, bitmap), (px, py)) in rasters.iter().zip(&places) {
            for row in 0..m.height {
                let dst = ((py + row as u32) * ATLAS_W + px) as usize;
                pixels[dst..dst + m.width]
                    .copy_from_slice(&bitmap[row * m.width..(row + 1) * m.width]);
            }
        }
        for row in 0..4 {
            let dst = ((solid_at.1 + row) * ATLAS_W + solid_at.0) as usize;
            pixels[dst..dst + 4].fill(255);
        }
        let (aw, ah) = (ATLAS_W as f32, atlas_h as f32);
        let glyphs = rasters
            .iter()
            .zip(&places)
            .map(|((m, _), (px, py))| Glyph {
                uv: [
                    *px as f32 / aw,
                    *py as f32 / ah,
                    (*px as f32 + m.width as f32) / aw,
                    (*py as f32 + m.height as f32) / ah,
                ],
                width: m.width as f32,
                height: m.height as f32,
                xmin: m.xmin as f32,
                ymin: m.ymin as f32,
                advance: m.advance_width,
            })
            .collect();
        // Sample the middle of the solid block so filtering stays inside it.
        let solid = [
            (solid_at.0 as f32 + 1.0) / aw,
            (solid_at.1 as f32 + 1.0) / ah,
            (solid_at.0 as f32 + 3.0) / aw,
            (solid_at.1 as f32 + 3.0) / ah,
        ];

        let size = wgpu::Extent3d {
            width: ATLAS_W,
            height: atlas_h,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("twe-kernel hud atlas"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_W),
                rows_per_image: Some(atlas_h),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("twe-kernel hud sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel hud bgl"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel hud bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel hud shader"),
            source: wgpu::ShaderSource::Wgsl(HUD_SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel hud layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel hud pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_hud"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HudVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &ATTRS,
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_hud"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
        let vertex_capacity = 6 * 256;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twe-kernel hud vertices"),
            size: vertex_capacity * std::mem::size_of::<HudVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Hud {
            pipeline,
            bind_group,
            glyphs,
            solid,
            vertex_buffer,
            vertex_capacity,
            vertex_count: 0,
        }
    }

    /// Lay out this frame's items for a `width × height` target and
    /// upload them. Call before the pass that [`Hud::draw`]s.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[HudItem],
        width: u32,
        height: u32,
    ) {
        let verts = self.layout(items, width as f32, height as f32);
        self.vertex_count = verts.len() as u32;
        if verts.is_empty() {
            return;
        }
        if verts.len() as u64 > self.vertex_capacity {
            self.vertex_capacity = (verts.len() as u64).next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("twe-kernel hud vertices"),
                size: self.vertex_capacity * std::mem::size_of::<HudVertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&verts));
    }

    /// Draw the prepared HUD into the current pass (over the scene).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }

    fn layout(&self, items: &[HudItem], w: f32, h: f32) -> Vec<HudVertex> {
        let (sx, sy) = (w / CANVAS_W, h / CANVAS_H);
        let clip = |x: f32, y: f32| [x / w * 2.0 - 1.0, 1.0 - y / h * 2.0];
        let mut out = Vec::new();
        let mut quad = |x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4], color: [f32; 4]| {
            let v = |x: f32, y: f32, u: f32, t: f32| HudVertex {
                pos: clip(x, y),
                uv: [u, t],
                color,
            };
            let (a, b, c, d) = (
                v(x0, y0, uv[0], uv[1]),
                v(x1, y0, uv[2], uv[1]),
                v(x1, y1, uv[2], uv[3]),
                v(x0, y1, uv[0], uv[3]),
            );
            out.extend_from_slice(&[a, b, c, a, c, d]);
        };
        for item in items {
            match item {
                HudItem::Rect {
                    x,
                    y,
                    w: rw,
                    h: rh,
                    color,
                } => quad(
                    x * sx,
                    y * sy,
                    (x + rw) * sx,
                    (y + rh) * sy,
                    self.solid,
                    *color,
                ),
                HudItem::Text {
                    text,
                    x,
                    y,
                    size,
                    color,
                } => {
                    // `(x, y)` is the baseline start, as in the 2D `text()`.
                    let k = size * sy / BASE_PX;
                    let mut pen = x * sx;
                    let baseline = y * sy;
                    for ch in text.chars() {
                        let code = ch as u32;
                        let g = if (FIRST..=LAST).contains(&code) {
                            self.glyphs[(code - FIRST) as usize]
                        } else {
                            // Outside the atlas: draw a `?`.
                            self.glyphs[(u32::from('?') - FIRST) as usize]
                        };
                        if g.width > 0.0 && g.height > 0.0 {
                            let x0 = pen + g.xmin * k;
                            let y1 = baseline - g.ymin * k;
                            let y0 = y1 - g.height * k;
                            quad(x0, y0, x0 + g.width * k, y1, g.uv, *color);
                        }
                        pen += g.advance * k;
                    }
                }
            }
        }
        out
    }
}
