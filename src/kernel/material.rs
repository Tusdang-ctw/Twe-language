//! web3d-M7: glTF metallic-roughness materials on the GPU.
//!
//! Every surface the kernel draws binds a *material* at group 1: a
//! uniform (factors, alpha mode, per-texture UV transforms) plus five
//! texture slots, each with its own sampler:
//!
//! | slot | texture              | encoding | default     |
//! |------|----------------------|----------|-------------|
//! | 0    | base colour          | sRGB     | white       |
//! | 1    | metallic-roughness   | linear   | white       |
//! | 2    | normal (tangent)     | linear   | flat (0,0,1)|
//! | 3    | occlusion            | linear   | white       |
//! | 4    | emissive             | sRGB     | white       |
//! | 5–8  | extension textures   | per use  | white       |
//!
//! web3d-M7 session 10: glTF material extensions (clearcoat, sheen,
//! specular, IOR, transmission, volume, iridescence) read their
//! textures through four shared *extension slots*: each extension
//! texture a material uses (a "role") is assigned a slot, identical
//! references share one, and the uniform records which slot each role
//! reads. WebGPU allows 16 sampled textures per shader stage, and the
//! frame, shadow and material groups use 16 with these four; a
//! material referencing more than four distinct extension textures
//! loses the rest (with a warning), which no Khronos sample does.
//!
//! glTF meshes get one material per glTF material; the game's cubes,
//! spheres and script-textured draws use [`MaterialData::plain`].
//! The shading itself is in `render.rs` (`SHADER_SRC`, `shade`).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub(crate) const SLOTS: usize = 9;
pub(crate) const BASE: usize = 0;
pub(crate) const METAL_ROUGH: usize = 1;
pub(crate) const NORMAL: usize = 2;
pub(crate) const OCCLUSION: usize = 3;
pub(crate) const EMISSIVE: usize = 4;
/// The first of the four extension slots.
pub(crate) const EXT: usize = 5;
pub(crate) const EXT_SLOTS: usize = 4;

/// Extension texture roles (the uniform's `roles` table, in order).
pub(crate) const ROLE_SPECULAR: usize = 0; // A
pub(crate) const ROLE_SPECULAR_COLOR: usize = 1; // RGB, sRGB
pub(crate) const ROLE_CLEARCOAT: usize = 2; // R
pub(crate) const ROLE_CLEARCOAT_ROUGHNESS: usize = 3; // G
pub(crate) const ROLE_CLEARCOAT_NORMAL: usize = 4; // RGB
pub(crate) const ROLE_SHEEN_COLOR: usize = 5; // RGB, sRGB
pub(crate) const ROLE_SHEEN_ROUGHNESS: usize = 6; // A
pub(crate) const ROLE_TRANSMISSION: usize = 7; // R
pub(crate) const ROLE_THICKNESS: usize = 8; // G
pub(crate) const ROLE_IRIDESCENCE: usize = 9; // R
pub(crate) const ROLE_IRIDESCENCE_THICKNESS: usize = 10; // G
pub(crate) const ROLES: usize = 12;

/// A glTF sampler, reduced to what wgpu needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SamplerKey {
    /// u, v: 0 repeat, 1 clamp to edge, 2 mirrored repeat.
    pub wrap: [u8; 2],
    pub mag_nearest: bool,
    pub min_nearest: bool,
    pub mip_nearest: bool,
}

impl Default for SamplerKey {
    /// glTF's default: repeat, linear, trilinear.
    fn default() -> Self {
        SamplerKey {
            wrap: [0, 0],
            mag_nearest: false,
            min_nearest: false,
            mip_nearest: false,
        }
    }
}

/// One texture reference of a material.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TexSlot {
    /// Index into the model's images.
    pub image: usize,
    pub sampler: SamplerKey,
    /// Which UV set (0 or 1).
    pub tex_coord: u32,
    /// `KHR_texture_transform`.
    pub offset: [f32; 2],
    pub rotation: f32,
    pub scale: [f32; 2],
    /// Colour data (decoded from sRGB) rather than linear data.
    pub srgb: bool,
}

impl TexSlot {
    /// Two references read the same texels the same way (so an
    /// extension slot can serve both).
    fn same_texels(&self, o: &TexSlot) -> bool {
        self.image == o.image
            && self.sampler == o.sampler
            && self.tex_coord == o.tex_coord
            && self.offset == o.offset
            && self.rotation == o.rotation
            && self.scale == o.scale
            && self.srgb == o.srgb
    }
}

/// web3d-M7: glTF material extension parameters (spec defaults when
/// the material doesn't use the extension).
#[derive(Clone, Debug)]
pub(crate) struct Extensions {
    /// `KHR_materials_ior`.
    pub ior: f32,
    /// `KHR_materials_specular`: weight and colour of the dielectric
    /// specular lobe.
    pub specular: f32,
    pub specular_color: [f32; 3],
    /// `KHR_materials_clearcoat`.
    pub clearcoat: f32,
    pub clearcoat_roughness: f32,
    pub clearcoat_normal_scale: f32,
    /// `KHR_materials_sheen`.
    pub sheen_color: [f32; 3],
    pub sheen_roughness: f32,
    /// `KHR_materials_transmission` and `KHR_materials_volume`
    /// (attenuation distance 0 = none, i.e. infinite).
    pub transmission: f32,
    pub thickness: f32,
    pub attenuation_distance: f32,
    pub attenuation_color: [f32; 3],
    /// `KHR_materials_iridescence`: strength, film IOR, film thickness
    /// range (nm).
    pub iridescence: f32,
    pub iridescence_ior: f32,
    pub iridescence_thickness: [f32; 2],
    /// Which extension slot (0..4) each role samples, or -1.
    pub roles: [i32; ROLES],
}

impl Default for Extensions {
    fn default() -> Self {
        Extensions {
            ior: 1.5,
            specular: 1.0,
            specular_color: [1.0; 3],
            clearcoat: 0.0,
            clearcoat_roughness: 0.0,
            clearcoat_normal_scale: 1.0,
            sheen_color: [0.0; 3],
            sheen_roughness: 0.0,
            transmission: 0.0,
            thickness: 0.0,
            attenuation_distance: 0.0,
            attenuation_color: [1.0; 3],
            iridescence: 0.0,
            iridescence_ior: 1.3,
            iridescence_thickness: [100.0, 400.0],
            roles: [-1; ROLES],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AlphaMode {
    Opaque,
    Mask,
    Blend,
}

/// A material as the loader reads it, before GPU upload.
#[derive(Clone, Debug)]
pub(crate) struct MaterialData {
    pub base_color: [f32; 4],
    /// Linear RGB, already multiplied by `KHR_materials_emissive_strength`.
    pub emissive: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    pub slots: [Option<TexSlot>; SLOTS],
    /// web3d-M7: glTF material extensions.
    pub ext: Extensions,
}

impl MaterialData {
    /// The surface of the game's cubes and spheres and of script-
    /// textured draws: a dielectric of medium roughness, coloured by
    /// the draw's tint (and texture).
    pub fn plain() -> Self {
        MaterialData {
            base_color: [1.0; 4],
            emissive: [0.0; 3],
            metallic: 0.0,
            roughness: 0.5,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            slots: [None; SLOTS],
            ext: Extensions::default(),
        }
    }

    /// Read a glTF material (the default material when a primitive has
    /// none: `gltf` reports the spec defaults, metallic 1, rough 1).
    pub fn from_gltf(m: &gltf::Material<'_>, doc: &gltf::Document) -> Self {
        let pbr = m.pbr_metallic_roughness();
        let info = |i: Option<gltf::texture::Info<'_>>| {
            i.map(|t| {
                let x = t.texture_transform();
                tex_slot(
                    &t.texture(),
                    t.tex_coord(),
                    x.as_ref().map(|x| (x.offset(), x.rotation(), x.scale(), x.tex_coord())),
                )
            })
        };
        let mut slots = [None; SLOTS];
        slots[BASE] = info(pbr.base_color_texture()).map(|s| TexSlot { srgb: true, ..s });
        slots[METAL_ROUGH] = info(pbr.metallic_roughness_texture());
        slots[EMISSIVE] = info(m.emissive_texture()).map(|s| TexSlot { srgb: true, ..s });
        let mut normal_scale = 1.0;
        if let Some(n) = m.normal_texture() {
            normal_scale = n.scale();
            slots[NORMAL] = Some(tex_slot(
                &n.texture(),
                n.tex_coord(),
                raw_transform(n.extensions().and_then(|e| e.get("KHR_texture_transform"))),
            ));
        }
        let mut occlusion_strength = 1.0;
        if let Some(o) = m.occlusion_texture() {
            occlusion_strength = o.strength();
            slots[OCCLUSION] = Some(tex_slot(
                &o.texture(),
                o.tex_coord(),
                raw_transform(o.extensions().and_then(|e| e.get("KHR_texture_transform"))),
            ));
        }
        let strength = m.emissive_strength().unwrap_or(1.0);
        let e = m.emissive_factor();
        let (ext, ext_slots) = read_extensions(m, doc);
        for (i, s) in ext_slots.into_iter().enumerate() {
            slots[EXT + i] = s;
        }
        MaterialData {
            base_color: pbr.base_color_factor(),
            emissive: [e[0] * strength, e[1] * strength, e[2] * strength],
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            normal_scale,
            occlusion_strength,
            alpha_mode: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                gltf::material::AlphaMode::Mask => AlphaMode::Mask,
                gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            },
            alpha_cutoff: m.alpha_cutoff().unwrap_or(0.5),
            double_sided: m.double_sided(),
            slots,
            ext,
        }
    }

    /// The material's uniform block (`Material` in the shader).
    pub fn uniform(&self) -> MaterialUniform {
        let mut xf = [[0.0; 4]; 2 * SLOTS];
        for (i, slot) in self.slots.iter().enumerate() {
            match slot {
                Some(s) => {
                    xf[2 * i] = [s.offset[0], s.offset[1], s.rotation, s.tex_coord as f32];
                    xf[2 * i + 1] = [s.scale[0], s.scale[1], 1.0, 0.0];
                }
                None => xf[2 * i + 1] = [1.0, 1.0, 0.0, 0.0],
            }
        }
        let x = &self.ext;
        let mut roles = [[-1.0; 4]; 3];
        for (i, r) in x.roles.iter().enumerate() {
            roles[i / 4][i % 4] = *r as f32;
        }
        MaterialUniform {
            ext: [
                [x.ior, x.specular, x.clearcoat, x.clearcoat_roughness],
                [x.specular_color[0], x.specular_color[1], x.specular_color[2], x.clearcoat_normal_scale],
                [x.sheen_color[0], x.sheen_color[1], x.sheen_color[2], x.sheen_roughness],
                [x.transmission, x.thickness, x.attenuation_distance, x.iridescence],
                [x.attenuation_color[0], x.attenuation_color[1], x.attenuation_color[2], x.iridescence_ior],
                [x.iridescence_thickness[0], x.iridescence_thickness[1], 0.0, 0.0],
            ],
            roles,
            base_color: self.base_color,
            emissive: [self.emissive[0], self.emissive[1], self.emissive[2], 0.0],
            params: [
                self.metallic,
                self.roughness,
                self.normal_scale,
                self.occlusion_strength,
            ],
            alpha: [
                match self.alpha_mode {
                    AlphaMode::Opaque => 0.0,
                    AlphaMode::Mask => 1.0,
                    AlphaMode::Blend => 2.0,
                },
                self.alpha_cutoff,
                if self.double_sided { 1.0 } else { 0.0 },
                0.0,
            ],
            xf,
        }
    }
}

/// `KHR_texture_transform` values: (offset, rotation, scale, texCoord).
type Transform = ([f32; 2], f32, [f32; 2], Option<u32>);

fn tex_slot(texture: &gltf::Texture<'_>, tex_coord: u32, xf: Option<Transform>) -> TexSlot {
    use gltf::texture::{MagFilter, MinFilter, WrappingMode};
    let s = texture.sampler();
    let wrap = |w: WrappingMode| match w {
        WrappingMode::Repeat => 0,
        WrappingMode::ClampToEdge => 1,
        WrappingMode::MirroredRepeat => 2,
    };
    let (min_nearest, mip_nearest) = match s.min_filter() {
        Some(MinFilter::Nearest) | Some(MinFilter::NearestMipmapNearest) => (true, true),
        Some(MinFilter::NearestMipmapLinear) => (true, false),
        Some(MinFilter::LinearMipmapNearest) => (false, true),
        _ => (false, false),
    };
    let (offset, rotation, scale, override_coord) = xf.unwrap_or(([0.0; 2], 0.0, [1.0; 2], None));
    TexSlot {
        image: texture.source().index(),
        sampler: SamplerKey {
            wrap: [wrap(s.wrap_s()), wrap(s.wrap_t())],
            mag_nearest: s.mag_filter() == Some(MagFilter::Nearest),
            min_nearest,
            mip_nearest,
        },
        tex_coord: override_coord.unwrap_or(tex_coord),
        offset,
        rotation,
        scale,
        srgb: false,
    }
}

/// web3d-M7: read the material extensions from the raw glTF JSON (the
/// `gltf` crate doesn't model most of them), and assign each extension
/// texture an extension slot.
fn read_extensions(m: &gltf::Material<'_>, doc: &gltf::Document) -> (Extensions, [Option<TexSlot>; EXT_SLOTS]) {
    use gltf::json::Value;
    let mut x = Extensions::default();
    let mut slots: [Option<TexSlot>; EXT_SLOTS] = [None; EXT_SLOTS];
    let Some(all) = m.extensions() else {
        return (x, slots);
    };
    let doc_textures: Vec<gltf::Texture<'_>> = doc.textures().collect();
    let num = |v: &Value, k: &str, d: f32| v.get(k).and_then(|x| x.as_f64()).map_or(d, |x| x as f32);
    let rgb = |v: &Value, k: &str, d: [f32; 3]| {
        v.get(k)
            .and_then(|x| x.as_array())
            .filter(|a| a.len() >= 3)
            .map_or(d, |a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(d[i] as f64) as f32))
    };
    // A texture reference -> its slot description.
    let texture = |v: &Value, k: &str, srgb: bool| -> Option<(TexSlot, f32)> {
        let info = v.get(k)?;
        let index = info.get("index")?.as_u64()? as usize;
        let tex = doc_textures.get(index)?;
        let coord = info.get("texCoord").and_then(|c| c.as_u64()).unwrap_or(0) as u32;
        let xf = raw_transform(info.get("extensions").and_then(|e| e.get("KHR_texture_transform")));
        let scale = info.get("scale").and_then(|s| s.as_f64()).unwrap_or(1.0) as f32;
        Some((TexSlot { srgb, ..tex_slot(tex, coord, xf) }, scale))
    };
    let mut refs: Vec<(usize, TexSlot)> = Vec::new();
    if let Some(e) = all.get("KHR_materials_ior") {
        x.ior = num(e, "ior", 1.5);
    }
    if let Some(e) = all.get("KHR_materials_specular") {
        x.specular = num(e, "specularFactor", 1.0);
        x.specular_color = rgb(e, "specularColorFactor", [1.0; 3]);
        refs.extend(texture(e, "specularTexture", false).map(|t| (ROLE_SPECULAR, t.0)));
        refs.extend(texture(e, "specularColorTexture", true).map(|t| (ROLE_SPECULAR_COLOR, t.0)));
    }
    if let Some(e) = all.get("KHR_materials_clearcoat") {
        x.clearcoat = num(e, "clearcoatFactor", 0.0);
        x.clearcoat_roughness = num(e, "clearcoatRoughnessFactor", 0.0);
        refs.extend(texture(e, "clearcoatTexture", false).map(|t| (ROLE_CLEARCOAT, t.0)));
        refs.extend(texture(e, "clearcoatRoughnessTexture", false).map(|t| (ROLE_CLEARCOAT_ROUGHNESS, t.0)));
        if let Some((t, scale)) = texture(e, "clearcoatNormalTexture", false) {
            x.clearcoat_normal_scale = scale;
            refs.push((ROLE_CLEARCOAT_NORMAL, t));
        }
    }
    if let Some(e) = all.get("KHR_materials_sheen") {
        x.sheen_color = rgb(e, "sheenColorFactor", [0.0; 3]);
        x.sheen_roughness = num(e, "sheenRoughnessFactor", 0.0);
        refs.extend(texture(e, "sheenColorTexture", true).map(|t| (ROLE_SHEEN_COLOR, t.0)));
        refs.extend(texture(e, "sheenRoughnessTexture", false).map(|t| (ROLE_SHEEN_ROUGHNESS, t.0)));
    }
    if let Some(e) = all.get("KHR_materials_transmission") {
        x.transmission = num(e, "transmissionFactor", 0.0);
        refs.extend(texture(e, "transmissionTexture", false).map(|t| (ROLE_TRANSMISSION, t.0)));
    }
    if let Some(e) = all.get("KHR_materials_volume") {
        x.thickness = num(e, "thicknessFactor", 0.0);
        x.attenuation_distance = num(e, "attenuationDistance", 0.0);
        x.attenuation_color = rgb(e, "attenuationColor", [1.0; 3]);
        refs.extend(texture(e, "thicknessTexture", false).map(|t| (ROLE_THICKNESS, t.0)));
    }
    if let Some(e) = all.get("KHR_materials_iridescence") {
        x.iridescence = num(e, "iridescenceFactor", 0.0);
        x.iridescence_ior = num(e, "iridescenceIor", 1.3);
        x.iridescence_thickness = [
            num(e, "iridescenceThicknessMinimum", 100.0),
            num(e, "iridescenceThicknessMaximum", 400.0),
        ];
        refs.extend(texture(e, "iridescenceTexture", false).map(|t| (ROLE_IRIDESCENCE, t.0)));
        refs.extend(texture(e, "iridescenceThicknessTexture", false).map(|t| (ROLE_IRIDESCENCE_THICKNESS, t.0)));
    }
    for (role, t) in refs {
        let slot = match slots.iter().position(|s| s.is_some_and(|s| s.same_texels(&t))) {
            Some(i) => Some(i),
            None => slots.iter().position(Option::is_none).inspect(|&i| slots[i] = Some(t)),
        };
        match slot {
            Some(i) => x.roles[role] = i as i32,
            None => eprintln!(
                "warning: glTF material `{}` uses more than {EXT_SLOTS} extension textures; one is ignored",
                m.name().unwrap_or("")
            ),
        }
    }
    (x, slots)
}

/// `KHR_texture_transform` from a texture reference's raw extension
/// JSON (the `gltf` crate only parses it on base-colour-style infos).
fn raw_transform(t: Option<&gltf::json::Value>) -> Option<Transform> {
    let t = t?;
    let pair = |k: &str, d: f32| -> [f32; 2] {
        t.get(k)
            .and_then(|v| v.as_array())
            .map(|a| {
                [
                    a.first().and_then(|x| x.as_f64()).unwrap_or(d as f64) as f32,
                    a.get(1).and_then(|x| x.as_f64()).unwrap_or(d as f64) as f32,
                ]
            })
            .unwrap_or([d, d])
    };
    Some((
        pair("offset", 0.0),
        t.get("rotation").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        pair("scale", 1.0),
        t.get("texCoord").and_then(|v| v.as_u64()).map(|v| v as u32),
    ))
}

/// The shader's `Material` uniform.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct MaterialUniform {
    pub base_color: [f32; 4],
    /// rgb = emissive (linear, strength applied).
    pub emissive: [f32; 4],
    /// metallic, roughness, normal scale, occlusion strength.
    pub params: [f32; 4],
    /// alpha mode (0 opaque, 1 mask, 2 blend), cutoff, double-sided.
    pub alpha: [f32; 4],
    /// Per slot: (offset.xy, rotation, uv set), (scale.xy, has texture, _).
    pub xf: [[f32; 4]; 2 * SLOTS],
    /// web3d-M7 extensions: (ior, specular, clearcoat, clearcoat
    /// roughness), (specular colour, clearcoat normal scale), (sheen
    /// colour, sheen roughness), (transmission, thickness, attenuation
    /// distance, iridescence), (attenuation colour, iridescence IOR),
    /// (iridescence thickness min, max, _, _).
    pub ext: [[f32; 4]; 6],
    /// Extension slot per role (-1 = none), four roles per vec4.
    pub roles: [[f32; 4]; 3],
}

/// A glTF image as RGBA8 (whatever the source channel layout).
pub(crate) struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ImageData {
    pub fn from_gltf(img: &gltf::image::Data) -> Self {
        use gltf::image::Format;
        let px = &img.pixels;
        let n = (img.width * img.height) as usize;
        let mut rgba = Vec::with_capacity(n * 4);
        let high = |i: usize| px[i + 1]; // 16-bit little-endian: keep the high byte
        match img.format {
            Format::R8G8B8A8 => rgba.extend_from_slice(px),
            Format::R8G8B8 => {
                for c in px.chunks_exact(3) {
                    rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
            Format::R8G8 => {
                for c in px.chunks_exact(2) {
                    rgba.extend_from_slice(&[c[0], c[1], 0, 255]);
                }
            }
            Format::R8 => {
                for &v in px {
                    rgba.extend_from_slice(&[v, v, v, 255]);
                }
            }
            Format::R16G16B16A16 => {
                for i in (0..px.len()).step_by(8) {
                    rgba.extend_from_slice(&[high(i), high(i + 2), high(i + 4), high(i + 6)]);
                }
            }
            Format::R16G16B16 => {
                for i in (0..px.len()).step_by(6) {
                    rgba.extend_from_slice(&[high(i), high(i + 2), high(i + 4), 255]);
                }
            }
            _ => rgba.resize(n * 4, 255),
        }
        rgba.resize(n * 4, 255);
        ImageData {
            width: img.width,
            height: img.height,
            rgba,
        }
    }
}

/// The group-1 layout, default textures and a sampler cache: builds a
/// bind group for any material.
pub(crate) struct MaterialKit {
    pub layout: wgpu::BindGroupLayout,
    white_srgb: wgpu::TextureView,
    white_linear: wgpu::TextureView,
    flat_normal: wgpu::TextureView,
    samplers: HashMap<SamplerKey, wgpu::Sampler>,
}

impl MaterialKit {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];
        for i in 0..SLOTS as u32 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 1 + i,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 1 + SLOTS as u32 + i,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twe-kernel material bgl"),
            entries: &entries,
        });
        let pixel = |label, rgba: [u8; 4], srgb| {
            upload_rgba(device, queue, label, &rgba, 1, 1, srgb)
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        MaterialKit {
            layout,
            white_srgb: pixel("twe-kernel white (srgb)", [255; 4], true),
            white_linear: pixel("twe-kernel white (linear)", [255; 4], false),
            flat_normal: pixel("twe-kernel flat normal", [128, 128, 255, 255], false),
            samplers: HashMap::new(),
        }
    }

    fn sampler(&mut self, device: &wgpu::Device, key: SamplerKey) -> wgpu::Sampler {
        self.samplers
            .entry(key)
            .or_insert_with(|| {
                let wrap = |w: u8| match w {
                    1 => wgpu::AddressMode::ClampToEdge,
                    2 => wgpu::AddressMode::MirrorRepeat,
                    _ => wgpu::AddressMode::Repeat,
                };
                let filter = |nearest| {
                    if nearest {
                        wgpu::FilterMode::Nearest
                    } else {
                        wgpu::FilterMode::Linear
                    }
                };
                let all_linear = !(key.mag_nearest || key.min_nearest || key.mip_nearest);
                device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("twe-kernel material sampler"),
                    address_mode_u: wrap(key.wrap[0]),
                    address_mode_v: wrap(key.wrap[1]),
                    address_mode_w: wgpu::AddressMode::Repeat,
                    mag_filter: filter(key.mag_nearest),
                    min_filter: filter(key.min_nearest),
                    mipmap_filter: if key.mip_nearest {
                        wgpu::MipmapFilterMode::Nearest
                    } else {
                        wgpu::MipmapFilterMode::Linear
                    },
                    // Anisotropy needs all-linear filtering in WebGPU.
                    anisotropy_clamp: if all_linear { 16 } else { 1 },
                    ..Default::default()
                })
            })
            .clone()
    }

    /// A bind group for `data`, with `views[slot]` for each texture the
    /// material uses (`None` falls back to the slot's default).
    pub fn bind_group(
        &mut self,
        device: &wgpu::Device,
        data: &MaterialData,
        views: [Option<&wgpu::TextureView>; SLOTS],
    ) -> wgpu::BindGroup {
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("twe-kernel material uniform"),
            contents: bytemuck::bytes_of(&data.uniform()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let samplers: Vec<wgpu::Sampler> = (0..SLOTS)
            .map(|i| {
                let key = data.slots[i].map(|s| s.sampler).unwrap_or_default();
                self.sampler(device, key)
            })
            .collect();
        let defaults = [
            &self.white_srgb,
            &self.white_linear,
            &self.flat_normal,
            &self.white_linear,
            &self.white_srgb,
            &self.white_linear,
            &self.white_linear,
            &self.white_linear,
            &self.white_linear,
        ];
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }];
        for i in 0..SLOTS {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + i as u32,
                resource: wgpu::BindingResource::TextureView(views[i].unwrap_or(defaults[i])),
            });
        }
        for (i, s) in samplers.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + (SLOTS + i) as u32,
                resource: wgpu::BindingResource::Sampler(s),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twe-kernel material bg"),
            layout: &self.layout,
            entries: &entries,
        })
    }
}

/// Upload an RGBA8 image with a full mip chain, as sRGB colour or as
/// linear data. Mips are resampled from the full image (Triangle
/// filter); colour mips are filtered in encoded space, a small bias
/// most engines accept.
pub(crate) fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    rgba: &[u8],
    width: u32,
    height: u32,
    srgb: bool,
) -> wgpu::Texture {
    let mip_level_count = width.max(height).max(1).ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let write = |level: u32, data: &[u8], w: u32, h: u32| {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
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
    };
    write(0, rgba, width, height);
    if mip_level_count > 1 {
        let source: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
            image::ImageBuffer::from_raw(width, height, rgba.to_vec())
                .expect("rgba slice length should be width * height * 4");
        for level in 1..mip_level_count {
            let (mw, mh) = ((width >> level).max(1), (height >> level).max(1));
            let mip = image::imageops::resize(&source, mw, mh, image::imageops::FilterType::Triangle);
            write(level, &mip, mw, mh);
        }
    }
    texture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_material_is_a_dielectric_with_no_textures() {
        let u = MaterialData::plain().uniform();
        assert_eq!(u.params, [0.0, 0.5, 1.0, 1.0]);
        assert_eq!(u.alpha, [0.0, 0.5, 0.0, 0.0]);
        for i in 0..SLOTS {
            assert_eq!(u.xf[2 * i + 1], [1.0, 1.0, 0.0, 0.0], "slot {i} unused");
        }
        // Four vec4 of factors, the transforms, six of extension
        // parameters and three of texture roles.
        assert_eq!(std::mem::size_of::<MaterialUniform>(), (4 + 2 * SLOTS + 6 + 3) * 16);
        assert_eq!(u.ext[0], [1.5, 1.0, 0.0, 0.0], "IOR 1.5, full specular, no clearcoat");
        assert!(u.roles.iter().flatten().all(|r| *r == -1.0), "no extension textures");
    }

    /// web3d-M7: extension parameters and textures. Two roles reading
    /// the same texture share a slot; past four distinct textures the
    /// rest are dropped.
    #[test]
    fn material_extensions_read_from_gltf() {
        let json = r#"{"asset":{"version":"2.0"},
            "images":[{"uri":"a.png"},{"uri":"b.png"},{"uri":"c.png"},{"uri":"d.png"},{"uri":"e.png"}],
            "textures":[{"source":0},{"source":1},{"source":2},{"source":3},{"source":4}],
            "materials":[{"extensions":{
                "KHR_materials_ior":{"ior":1.4},
                "KHR_materials_clearcoat":{"clearcoatFactor":1,"clearcoatRoughnessFactor":0.2,
                    "clearcoatTexture":{"index":0},"clearcoatRoughnessTexture":{"index":0},
                    "clearcoatNormalTexture":{"index":1,"scale":0.5}},
                "KHR_materials_sheen":{"sheenColorFactor":[0.5,0.25,0.1],"sheenRoughnessFactor":0.4},
                "KHR_materials_iridescence":{"iridescenceFactor":1,"iridescenceThicknessTexture":{"index":2}},
                "KHR_materials_specular":{"specularTexture":{"index":3},"specularColorTexture":{"index":4}},
                "KHR_materials_transmission":{"transmissionFactor":0.9},
                "KHR_materials_volume":{"thicknessFactor":0.1,"attenuationDistance":2,"attenuationColor":[1,0.5,0.5]}}}]}"#;
        let gltf = gltf::Gltf::from_slice(json.as_bytes()).expect("parses");
        let m = gltf.document.materials().next().expect("a material");
        let data = MaterialData::from_gltf(&m, &gltf.document);
        let x = &data.ext;
        assert_eq!(x.ior, 1.4);
        assert_eq!((x.clearcoat, x.clearcoat_roughness, x.clearcoat_normal_scale), (1.0, 0.2, 0.5));
        assert_eq!(x.sheen_color, [0.5, 0.25, 0.1]);
        assert_eq!((x.transmission, x.thickness, x.attenuation_distance), (0.9, 0.1, 2.0));
        assert_eq!(x.iridescence_thickness, [100.0, 400.0], "the spec's default range");
        // Clearcoat and its roughness share slot 0; the normal takes 1.
        assert_eq!(x.roles[ROLE_CLEARCOAT], x.roles[ROLE_CLEARCOAT_ROUGHNESS]);
        assert_ne!(x.roles[ROLE_CLEARCOAT], x.roles[ROLE_CLEARCOAT_NORMAL]);
        // Five distinct textures, four slots: exactly one role lost.
        let assigned = x.roles.iter().filter(|r| **r >= 0).count();
        assert_eq!(assigned, 5, "six roles referenced, one dropped: {:?}", x.roles);
        assert!(data.slots[EXT..].iter().all(Option::is_some));
        // Specular colour is colour data; the rest are linear.
        let colour_slot = x.roles[ROLE_SPECULAR_COLOR];
        if colour_slot >= 0 {
            assert!(data.slots[EXT + colour_slot as usize].unwrap().srgb);
        }
        assert!(!data.slots[EXT + x.roles[ROLE_CLEARCOAT] as usize].unwrap().srgb);
    }
}
