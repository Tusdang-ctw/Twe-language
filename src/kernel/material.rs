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
//!
//! glTF meshes get one material per glTF material; the game's cubes,
//! spheres and script-textured draws use [`MaterialData::plain`].
//! The shading itself is in `render.rs` (`SHADER_SRC`, `shade`).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub(crate) const SLOTS: usize = 5;
pub(crate) const BASE: usize = 0;
pub(crate) const METAL_ROUGH: usize = 1;
pub(crate) const NORMAL: usize = 2;
pub(crate) const OCCLUSION: usize = 3;
pub(crate) const EMISSIVE: usize = 4;

/// Which slots hold colour (sRGB-encoded) data; the rest are linear.
pub(crate) const SRGB_SLOT: [bool; SLOTS] = [true, false, false, false, true];

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
        }
    }

    /// Read a glTF material (the default material when a primitive has
    /// none: `gltf` reports the spec defaults, metallic 1, rough 1).
    pub fn from_gltf(m: &gltf::Material<'_>) -> Self {
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
        slots[BASE] = info(pbr.base_color_texture());
        slots[METAL_ROUGH] = info(pbr.metallic_roughness_texture());
        slots[EMISSIVE] = info(m.emissive_texture());
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
        MaterialUniform {
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
    }
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
        assert_eq!(std::mem::size_of::<MaterialUniform>(), 14 * 16);
    }
}
