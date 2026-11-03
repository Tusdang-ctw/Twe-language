//! web3d-M6: the kernel's 2D canvas — draws a frame's [`Draw2d`] list.
//!
//! One renderer for every 2D picture: a 2D game's whole frame, and the
//! HUD over a 3D game (it replaces the M3 `hud.rs`, which drew only
//! rectangles and built-in-font text).
//!
//! Coordinates are the script's 640×480 virtual canvas, scaled to the
//! target; under a `camera2d` transform they are world space first.
//! The shapes are tessellated exactly as the macroquad 2D runtime drew
//! them, so the two produce the same picture (`tests/references/m6/`):
//! - filled circles are 20-sided fans; circle outlines are 30 quads
//!   from the radius outward;
//! - rectangle outlines are half the requested thickness, inside the
//!   rectangle;
//! - lines are quads centred on the segment;
//! - text is rasterised by fontdue at the requested size (truncated to
//!   whole pixels) and placed with its baseline at `y`.
//!
//! Glyphs live in one RGBA atlas (white, coverage in alpha) beside a
//! solid white block for untextured shapes, so shapes and text share a
//! texture and batch together; images break the batch. Images and
//! fonts load through the host's [`AssetSource`] as raw bytes.
//!
//! Blending happens in the target's space. On a non-sRGB target that is
//! gamma space, as macroquad blended; on an sRGB target (the 3D HUD)
//! colours are decoded to linear first, as the M3 HUD did.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use crate::draw2d::Draw2d;
use crate::kernel::render::{AssetKind, AssetReady, AssetSource};

/// The virtual canvas 2D coordinates live in.
pub const CANVAS_W: f32 = 640.0;
pub const CANVAS_H: f32 = 480.0;

const ATLAS_SIZE: u32 = 1024;
/// The solid white block, top-left of the atlas.
const SOLID: u32 = 4;

pub(crate) const CANVAS_SHADER: &str = r#"
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
@group(0) @binding(0) var t_image: texture_2d<f32>;
@group(0) @binding(1) var s_image: sampler;

// 1 when the target encodes sRGB on write: script colours are sRGB, so
// decode them to linear and they land as written.
override srgb_target: f32 = 0.0;

@vertex
fn vs_canvas(v: VIn) -> VOut {
    var out: VOut;
    out.clip = vec4<f32>(v.pos, 0.0, 1.0);
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

@fragment
fn fs_canvas(in: VOut) -> @location(0) vec4<f32> {
    let c = in.color * textureSample(t_image, s_image, in.uv);
    if (srgb_target > 0.5) {
        return vec4<f32>(pow(c.rgb, vec3<f32>(2.2)), c.a);
    }
    return c;
}
"#;

/// One draw list and the camera it is drawn under.
pub type Layer<'a> = (&'a [Draw2d], Option<Camera2d>);

/// A `camera2d` view: the world point at the canvas centre, and zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera2d {
    pub center: [f32; 2],
    pub zoom: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Copy)]
struct Glyph {
    uv: [f32; 4],
    width: f32,
    height: f32,
    xmin: f32,
    ymin: f32,
    advance: f32,
}

enum Slot<T> {
    Pending,
    Ready(T),
    Failed,
}

struct Image {
    bind_group: wgpu::BindGroup,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bind {
    Atlas,
    Image(usize),
}

/// What a raw-bytes request was for.
enum Request {
    Image(String),
    Font(String),
}

struct Atlas {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    glyphs: HashMap<(usize, u16, char), Glyph>,
    x: u32,
    y: u32,
    row: u32,
    /// Set when a glyph didn't fit; the frame is laid out again.
    full: bool,
}

pub struct Canvas {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    atlas: Atlas,
    /// Font 0 is the built-in one.
    fonts: Vec<fontdue::Font>,
    font_paths: HashMap<String, Slot<usize>>,
    images: Vec<Image>,
    image_paths: HashMap<String, Slot<usize>>,
    requests: HashMap<u32, Request>,
    next_request: u32,
    vertices: Vec<Vertex>,
    batches: Vec<(Bind, std::ops::Range<u32>)>,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    /// Lanternfall E1: the canvas's size in script units — 640×480 for
    /// 2D games, the view's aspect at 480 tall for the 3D HUD
    /// ([`Canvas::set_units`]).
    units: [f32; 2],
}

impl Canvas {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel canvas bgl"),
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
        // Linear, as macroquad sampled sprites and its glyph atlas.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("twe-kernel canvas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twe-kernel canvas shader"),
            source: wgpu::ShaderSource::Wgsl(CANVAS_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twe-kernel canvas layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        let srgb = [("srgb_target", if format.is_srgb() { 1.0 } else { 0.0 })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twe-kernel canvas pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_canvas"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &ATTRS,
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_canvas"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &srgb,
                    ..Default::default()
                },
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("twe-kernel canvas atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_rgba(
            queue,
            &texture,
            0,
            0,
            SOLID,
            SOLID,
            &[255; (SOLID * SOLID * 4) as usize],
        );
        let bind_group = bind(
            device,
            &layout,
            &sampler,
            &texture,
            "twe-kernel canvas atlas bg",
        );
        let atlas = Atlas {
            texture,
            bind_group,
            glyphs: HashMap::new(),
            x: SOLID + 1,
            y: 0,
            row: SOLID,
            full: false,
        };
        let vertex_capacity = 6 * 1024;
        let vertex_buffer = vertex_buffer(device, vertex_capacity);
        let font = crate::draw2d::default_font().clone();
        Canvas {
            pipeline,
            layout,
            sampler,
            atlas,
            fonts: vec![font],
            font_paths: HashMap::new(),
            images: Vec::new(),
            image_paths: HashMap::new(),
            requests: HashMap::new(),
            next_request: 0,
            vertices: Vec::new(),
            batches: Vec::new(),
            vertex_buffer,
            vertex_capacity,
            units: [CANVAS_W, CANVAS_H],
        }
    }

    /// Lanternfall E1: lay out the next frames in a `width × height`
    /// unit canvas, stretched over whatever target they are drawn to.
    /// The 3D HUD uses `camera3d::hud_size` so it keeps its proportions;
    /// 2D games keep the 640×480 default.
    pub fn set_units(&mut self, width: f32, height: f32) {
        self.units = [width.max(1.0), height.max(1.0)];
    }

    /// Drop every loaded image and font (hot reload).
    pub fn clear_asset_caches(&mut self) {
        self.images.clear();
        self.image_paths.clear();
        self.fonts.truncate(1);
        self.font_paths.clear();
        self.requests.clear();
        self.atlas.glyphs.clear();
        self.atlas.x = SOLID + 1;
        self.atlas.y = 0;
        self.atlas.row = SOLID;
    }

    /// Whether an image or font the frame needs is still loading.
    pub fn pending(&self) -> bool {
        !self.requests.is_empty()
    }

    /// Hand a finished load to the canvas. Returns loads that belong to
    /// someone else (a host shares one [`AssetSource`] with the 3D
    /// renderer).
    pub fn accept(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ready: AssetReady,
    ) -> Option<AssetReady> {
        let AssetReady::Raw(id, bytes) = ready else {
            return Some(ready);
        };
        let Some(request) = self.requests.remove(&id) else {
            return Some(AssetReady::Raw(id, bytes));
        };
        match request {
            Request::Image(path) => {
                let slot = match bytes.and_then(|b| self.upload_image(device, queue, &path, &b)) {
                    Ok(index) => Slot::Ready(index),
                    Err(e) => {
                        log_error(&format!("image `{path}`: {e}"));
                        Slot::Failed
                    }
                };
                self.image_paths.insert(path, slot);
            }
            Request::Font(path) => {
                let parsed = bytes.and_then(|b| {
                    fontdue::Font::from_bytes(b, fontdue::FontSettings::default())
                        .map_err(|e| e.to_string())
                });
                let slot = match parsed {
                    Ok(font) => {
                        self.fonts.push(font);
                        Slot::Ready(self.fonts.len() - 1)
                    }
                    Err(e) => {
                        log_error(&format!("font `{path}`: {e}"));
                        Slot::Failed
                    }
                };
                self.font_paths.insert(path, slot);
            }
        }
        None
    }

    /// Lay out `items` for a `width × height` target and upload them.
    /// Images and fonts not yet loaded are requested from `assets` and
    /// skipped this frame. Call before the pass that [`Canvas::draw`]s.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[Draw2d],
        camera: Option<Camera2d>,
        width: u32,
        height: u32,
        assets: &mut dyn AssetSource,
    ) {
        self.prepare_layers(device, queue, &[(items, camera)], width, height, assets);
    }

    /// [`Canvas::prepare`] for several lists, drawn in order, each under
    /// its own camera (a 2D game's frame, then the shell's overlay).
    pub fn prepare_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layers: &[Layer<'_>],
        width: u32,
        height: u32,
        assets: &mut dyn AssetSource,
    ) {
        self.layout_frame(queue, layers, width as f32, height as f32, assets);
        if self.atlas.full {
            // A glyph didn't fit: start the atlas over and lay out again.
            self.atlas.glyphs.clear();
            self.atlas.x = SOLID + 1;
            self.atlas.y = 0;
            self.atlas.row = SOLID;
            self.layout_frame(queue, layers, width as f32, height as f32, assets);
            if self.atlas.full {
                log_error("2D canvas: this frame's text doesn't fit the glyph atlas");
            }
        }
        if self.vertices.is_empty() {
            return;
        }
        if self.vertices.len() as u64 > self.vertex_capacity {
            self.vertex_capacity = (self.vertices.len() as u64).next_power_of_two();
            self.vertex_buffer = vertex_buffer(device, self.vertex_capacity);
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
    }

    /// [`Canvas::prepare`] for a host that owns `assets` alone: loads
    /// what the frame needs before laying it out, so an image appears
    /// on the frame that first draws it (as it did under macroquad).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_loaded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[Draw2d],
        camera: Option<Camera2d>,
        width: u32,
        height: u32,
        assets: &mut dyn AssetSource,
    ) {
        self.prepare_layers_loaded(device, queue, &[(items, camera)], width, height, assets);
    }

    /// [`Canvas::prepare_layers`], loading what the frame needs first.
    pub fn prepare_layers_loaded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layers: &[Layer<'_>],
        width: u32,
        height: u32,
        assets: &mut dyn AssetSource,
    ) {
        for _ in 0..4 {
            self.prepare_layers(device, queue, layers, width, height, assets);
            if !self.pending() {
                return;
            }
            for ready in assets.poll() {
                self.accept(device, queue, ready);
            }
        }
    }

    /// Draw the prepared frame into the current pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.vertices.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for (bind, range) in &self.batches {
            let group = match bind {
                Bind::Atlas => &self.atlas.bind_group,
                Bind::Image(i) => &self.images[*i].bind_group,
            };
            pass.set_bind_group(0, group, &[]);
            pass.draw(range.clone(), 0..1);
        }
    }

    fn upload_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
        bytes: &[u8],
    ) -> Result<usize, String> {
        let rgba = image::load_from_memory(bytes)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let (w, h) = rgba.dimensions();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(path),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_rgba(queue, &texture, 0, 0, w, h, rgba.as_raw());
        let bind_group = bind(device, &self.layout, &self.sampler, &texture, path);
        self.images.push(Image {
            bind_group,
            width: w as f32,
            height: h as f32,
        });
        Ok(self.images.len() - 1)
    }

    fn request(&mut self, assets: &mut dyn AssetSource, request: Request) {
        let id = self.next_request;
        self.next_request = self.next_request.wrapping_add(1);
        let path = match &request {
            Request::Image(p) | Request::Font(p) => p.clone(),
        };
        self.requests.insert(id, request);
        assets.request(AssetKind::Raw, id, &path);
    }

    fn image(&mut self, path: &str, assets: &mut dyn AssetSource) -> Option<usize> {
        match self.image_paths.get(path) {
            Some(Slot::Ready(i)) => Some(*i),
            Some(Slot::Pending) | Some(Slot::Failed) => None,
            None => {
                self.image_paths.insert(path.to_string(), Slot::Pending);
                self.request(assets, Request::Image(path.to_string()));
                None
            }
        }
    }

    fn font(&mut self, path: Option<&str>, assets: &mut dyn AssetSource) -> Option<usize> {
        let Some(path) = path else { return Some(0) };
        match self.font_paths.get(path) {
            Some(Slot::Ready(i)) => Some(*i),
            Some(Slot::Pending) | Some(Slot::Failed) => None,
            None => {
                self.font_paths.insert(path.to_string(), Slot::Pending);
                self.request(assets, Request::Font(path.to_string()));
                None
            }
        }
    }

    /// The atlas glyph for `ch` of font `font` rasterised at `px`.
    fn glyph(&mut self, queue: &wgpu::Queue, font: usize, px: u16, ch: char) -> Option<Glyph> {
        if let Some(g) = self.atlas.glyphs.get(&(font, px, ch)) {
            return Some(*g);
        }
        let (m, coverage) = self.fonts[font].rasterize(ch, f32::from(px));
        let (w, h) = (m.width as u32, m.height as u32);
        let a = &mut self.atlas;
        if a.x + w + 1 > ATLAS_SIZE {
            a.x = 0;
            a.y += a.row + 1;
            a.row = 0;
        }
        if a.y + h > ATLAS_SIZE {
            a.full = true;
            return None;
        }
        let (gx, gy) = (a.x, a.y);
        a.x += w + 1;
        a.row = a.row.max(h);
        if w > 0 && h > 0 {
            let rgba: Vec<u8> = coverage.iter().flat_map(|c| [255, 255, 255, *c]).collect();
            write_rgba(queue, &a.texture, gx, gy, w, h, &rgba);
        }
        let s = ATLAS_SIZE as f32;
        let g = Glyph {
            uv: [
                gx as f32 / s,
                gy as f32 / s,
                (gx + w) as f32 / s,
                (gy + h) as f32 / s,
            ],
            width: w as f32,
            height: h as f32,
            xmin: m.xmin as f32,
            ymin: m.ymin as f32,
            advance: m.advance_width,
        };
        a.glyphs.insert((font, px, ch), g);
        Some(g)
    }

    fn layout_frame(
        &mut self,
        queue: &wgpu::Queue,
        layers: &[Layer<'_>],
        width: f32,
        height: f32,
        assets: &mut dyn AssetSource,
    ) {
        self.atlas.full = false;
        let mut out = Out {
            vertices: std::mem::take(&mut self.vertices),
            batches: std::mem::take(&mut self.batches),
            bind: Bind::Atlas,
            batch_start: 0,
            map: Map::new(None, width, height, self.units),
        };
        out.vertices.clear();
        out.batches.clear();
        let solid_uv = {
            let s = ATLAS_SIZE as f32;
            [
                1.0 / s,
                1.0 / s,
                (SOLID - 1) as f32 / s,
                (SOLID - 1) as f32 / s,
            ]
        };
        let white = [solid_uv[0], solid_uv[1]];
        for (items, camera) in layers {
            out.map.camera = *camera;
            for item in items.iter() {
                match item {
                    Draw2d::Rect { x, y, w, h, color } => {
                        out.set_bind(Bind::Atlas);
                        out.quad([*x, *y], [x + w, y + h], solid_uv, *color);
                    }
                    Draw2d::RectLines {
                        x,
                        y,
                        w,
                        h,
                        thickness,
                        color,
                    } => {
                        // macroquad's outline: an inner rectangle inset by
                        // half the thickness, joined to the outer one.
                        out.set_bind(Bind::Atlas);
                        let t = thickness / 2.0;
                        let v = [
                            [*x, *y],
                            [x + w, *y],
                            [x + w, y + h],
                            [*x, y + h],
                            [x + t, y + t],
                            [x + w - t, y + t],
                            [x + w - t, y + h - t],
                            [x + t, y + h - t],
                        ];
                        const IDX: [usize; 24] = [
                            0, 1, 4, 1, 4, 5, 1, 5, 6, 1, 2, 6, 3, 7, 2, 2, 7, 6, 0, 4, 3, 3, 4, 7,
                        ];
                        for tri in IDX.chunks(3) {
                            out.tri([v[tri[0]], v[tri[1]], v[tri[2]]], white, *color);
                        }
                    }
                    Draw2d::Circle { x, y, r, color } => {
                        out.set_bind(Bind::Atlas);
                        const SIDES: usize = 20;
                        let at = |i: usize| {
                            let a = i as f32 / SIDES as f32 * std::f32::consts::TAU;
                            [x + r * a.cos(), y + r * a.sin()]
                        };
                        for i in 0..SIDES {
                            out.tri([[*x, *y], at(i), at(i + 1)], white, *color);
                        }
                    }
                    Draw2d::CircleLines {
                        x,
                        y,
                        r,
                        thickness,
                        color,
                    } => {
                        out.set_bind(Bind::Atlas);
                        const SIDES: usize = 30;
                        let span = std::f32::consts::TAU / SIDES as f32;
                        let at = |angle: f32, radius: f32| {
                            [x + radius * angle.cos(), y + radius * angle.sin()]
                        };
                        for i in 0..SIDES {
                            let (a0, a1) = (i as f32 * span, i as f32 * span + span);
                            let p = [
                                at(a0, *r),
                                at(a0, r + thickness),
                                at(a1, *r),
                                at(a1, r + thickness),
                            ];
                            out.tri([p[0], p[1], p[2]], white, *color);
                            out.tri([p[2], p[1], p[3]], white, *color);
                        }
                    }
                    Draw2d::Line {
                        x1,
                        y1,
                        x2,
                        y2,
                        thickness,
                        color,
                    } => {
                        let (nx, ny) = (-(y2 - y1), x2 - x1);
                        let tlen = (nx * nx + ny * ny).sqrt() / (thickness * 0.5);
                        if tlen < f32::EPSILON {
                            continue;
                        }
                        out.set_bind(Bind::Atlas);
                        let (tx, ty) = (nx / tlen, ny / tlen);
                        let p = [
                            [x1 + tx, y1 + ty],
                            [x1 - tx, y1 - ty],
                            [x2 + tx, y2 + ty],
                            [x2 - tx, y2 - ty],
                        ];
                        out.tri([p[0], p[1], p[2]], white, *color);
                        out.tri([p[2], p[1], p[3]], white, *color);
                    }
                    Draw2d::Text {
                        text,
                        x,
                        y,
                        size,
                        color,
                        font,
                    } => {
                        let Some(font) = self.font(font.as_deref(), assets) else {
                            continue;
                        };
                        // macroquad truncates the size to whole pixels; the
                        // glyphs are rasterised at the target's scale so
                        // they stay sharp when the canvas is scaled up.
                        let px = (*size as u16).max(1);
                        let raster = ((f32::from(px) * out.map.scale[1]).round() as u16).max(1);
                        let k = f32::from(px) / f32::from(raster);
                        out.set_bind(Bind::Atlas);
                        let mut pen = *x;
                        for ch in text.chars() {
                            let Some(g) = self.glyph(queue, font, raster, ch) else {
                                break;
                            };
                            if g.width > 0.0 && g.height > 0.0 {
                                let x0 = pen + g.xmin * k;
                                let y0 = y - (g.height + g.ymin) * k;
                                out.quad(
                                    [x0, y0],
                                    [x0 + g.width * k, y0 + g.height * k],
                                    g.uv,
                                    *color,
                                );
                            }
                            pen += g.advance * k;
                        }
                    }
                    Draw2d::Texture {
                        path,
                        x,
                        y,
                        dest,
                        source,
                        tint,
                    } => {
                        let Some(index) = self.image(path, assets) else {
                            continue;
                        };
                        let (iw, ih) = (self.images[index].width, self.images[index].height);
                        let src = source.unwrap_or([0.0, 0.0, iw, ih]);
                        let [dw, dh] = dest.unwrap_or([src[2], src[3]]);
                        let uv = [
                            src[0] / iw,
                            src[1] / ih,
                            (src[0] + src[2]) / iw,
                            (src[1] + src[3]) / ih,
                        ];
                        out.set_bind(Bind::Image(index));
                        out.quad([*x, *y], [x + dw, y + dh], uv, *tint);
                    }
                }
            }
        }
        out.finish();
        self.vertices = out.vertices;
        self.batches = out.batches;
    }
}

/// Script coordinates → clip space for one frame.
struct Map {
    camera: Option<Camera2d>,
    /// Target pixels per canvas unit.
    scale: [f32; 2],
    size: [f32; 2],
    /// The canvas size in script units.
    units: [f32; 2],
}

impl Map {
    fn new(camera: Option<Camera2d>, width: f32, height: f32, units: [f32; 2]) -> Self {
        Map {
            camera,
            scale: [width / units[0], height / units[1]],
            size: [width, height],
            units,
        }
    }

    fn clip(&self, p: [f32; 2]) -> [f32; 2] {
        let c = match self.camera {
            Some(cam) => [
                (p[0] - cam.center[0]) * cam.zoom + self.units[0] * 0.5,
                (p[1] - cam.center[1]) * cam.zoom + self.units[1] * 0.5,
            ],
            None => p,
        };
        let (tx, ty) = (c[0] * self.scale[0], c[1] * self.scale[1]);
        [tx / self.size[0] * 2.0 - 1.0, 1.0 - ty / self.size[1] * 2.0]
    }
}

/// The vertices and texture batches of a frame being laid out.
struct Out {
    vertices: Vec<Vertex>,
    batches: Vec<(Bind, std::ops::Range<u32>)>,
    bind: Bind,
    batch_start: u32,
    map: Map,
}

impl Out {
    fn set_bind(&mut self, bind: Bind) {
        if bind != self.bind {
            self.finish();
            self.bind = bind;
        }
    }

    fn finish(&mut self) {
        let end = self.vertices.len() as u32;
        if end > self.batch_start {
            self.batches.push((self.bind, self.batch_start..end));
        }
        self.batch_start = end;
    }

    fn tri(&mut self, p: [[f32; 2]; 3], uv: [f32; 2], color: [f32; 4]) {
        let color = bytes(color);
        for q in p {
            self.vertices.push(Vertex {
                pos: self.map.clip(q),
                uv,
                color,
            });
        }
    }

    fn quad(&mut self, min: [f32; 2], max: [f32; 2], uv: [f32; 4], color: [f32; 4]) {
        let color = bytes(color);
        let v = |x: f32, y: f32, u: f32, t: f32| Vertex {
            pos: self.map.clip([x, y]),
            uv: [u, t],
            color,
        };
        let (a, b, c, d) = (
            v(min[0], min[1], uv[0], uv[1]),
            v(max[0], min[1], uv[2], uv[1]),
            v(max[0], max[1], uv[2], uv[3]),
            v(min[0], max[1], uv[0], uv[3]),
        );
        self.vertices.extend_from_slice(&[a, b, c, a, c, d]);
    }
}

/// A colour as macroquad stored it per vertex: each channel truncated
/// to a byte (`(c * 255) as u8`), so `0.05` is 12, not 13.
fn bytes(color: [f32; 4]) -> [f32; 4] {
    color.map(|c| (c.clamp(0.0, 1.0) * 255.0).floor() / 255.0)
}

fn vertex_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twe-kernel canvas vertices"),
        size: capacity * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    texture: &wgpu::Texture,
    label: &str,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn write_rgba(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    rgba: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * w),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
}

fn log_error(msg: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("error: {msg}");
    #[cfg(target_arch = "wasm32")]
    let _ = msg;
}

/// Loads asset bytes synchronously from the bundle or the filesystem:
/// the headless canvas's source (and any host that doesn't stream).
#[derive(Default)]
pub struct SyncAssets {
    ready: Vec<AssetReady>,
}

impl AssetSource for SyncAssets {
    fn request(&mut self, kind: AssetKind, id: u32, path: &str) {
        let bytes = crate::bundle::read_asset_bytes(path).map_err(|e| format!("`{path}`: {e}"));
        self.ready.push(match kind {
            AssetKind::Raw => AssetReady::Raw(id, bytes),
            AssetKind::Texture => AssetReady::Texture(id, bytes),
            AssetKind::Environment => AssetReady::Environment(id, bytes),
            AssetKind::Lut => AssetReady::Lut(id, bytes),
            AssetKind::Mesh => AssetReady::Mesh(id, Err("meshes aren't 2D".to_string())),
        });
    }

    fn poll(&mut self) -> Vec<AssetReady> {
        std::mem::take(&mut self.ready)
    }
}

/// web3d-M6: the canvas drawing into an offscreen texture, for tests and
/// the reference comparison. The target is non-sRGB `Rgba8Unorm`, like
/// macroquad's framebuffer, so blending happens in gamma space.
pub struct Offscreen {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: wgpu::Texture,
    width: u32,
    height: u32,
    canvas: Canvas,
    assets: SyncAssets,
}

impl Offscreen {
    pub async fn new(width: u32, height: u32) -> Result<Offscreen, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: crate::kernel::render::power_preference(),
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| format!("no compatible wgpu adapter found: {e}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("twe-kernel canvas device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| e.to_string())?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("twe-kernel canvas target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let canvas = Canvas::new(&device, &queue, format);
        Ok(Offscreen {
            device,
            queue,
            target,
            width,
            height,
            canvas,
            assets: SyncAssets::default(),
        })
    }

    /// Clear to black, draw `items`, and return the RGBA8 pixels.
    pub fn render(
        &mut self,
        items: &[Draw2d],
        camera: Option<Camera2d>,
    ) -> Result<Vec<u8>, String> {
        self.canvas.prepare_loaded(
            &self.device,
            &self.queue,
            items,
            camera,
            self.width,
            self.height,
            &mut self.assets,
        );
        let view = self
            .target
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("twe-kernel canvas pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            self.canvas.draw(&mut pass);
        }
        self.queue.submit(Some(encoder.finish()));
        read_texture(
            &self.device,
            &self.queue,
            &self.target,
            self.width,
            self.height,
        )
    }
}

/// Copy an RGBA8 texture back to the CPU, rows unpadded.
pub(crate) fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    w: u32,
    h: u32,
) -> Result<Vec<u8>, String> {
    let row = 4 * w;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twe-kernel canvas readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity((row * h) as usize);
    for y in 0..h as usize {
        let start = y * padded as usize;
        out.extend_from_slice(&data[start..start + row as usize]);
    }
    Ok(out)
}
