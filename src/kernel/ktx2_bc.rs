//! web3d (2026-10-08): load a KTX2 holding **BCn** (block-compressed)
//! texture data and upload it to the GPU *as a compressed texture*, with no
//! CPU decode — the "BCn KTX2 upload path" deferred at web3d-M9.
//!
//! M9 shipped build-time transcoding (`twec asset transcode`, behind
//! `asset-tools`) that turns Basis/KTX2 into plain RGBA8 the runtime loads.
//! That stays the path for targets without block compression. This module is
//! the complement: on a GPU that advertises `TEXTURE_COMPRESSION_BC`
//! (desktop wgpu — WebGPU in the browser usually does not), a `.ktx2` whose
//! payload is already BC1–BC7 uploads straight to VRAM compressed, saving
//! both the decode and ~4–8× the VRAM.
//!
//! The `ktx2` crate is a pure-Rust *parser* (no C), so this compiles on every
//! target; the actual upload is gated on the device feature at runtime, and
//! falls back to an explanatory error (pointing at `twec asset transcode`)
//! when BC is unavailable or the KTX2 is supercompressed / a universal format.

use wgpu::TextureFormat;

/// The 12-byte KTX2 file identifier.
pub(crate) const KTX2_IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];

/// True if `bytes` begins with the KTX2 identifier.
pub(crate) fn is_ktx2(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes[..12] == KTX2_IDENTIFIER
}

/// Map a Vulkan `VkFormat` to the wgpu BC texture format, or `None` if it is
/// not a block-compressed format this path uploads directly. The values are
/// the Vulkan enum numbers (`VK_FORMAT_BC1_RGB_UNORM_BLOCK` = 131 … `BC7` =
/// 146); `ktx2::Format::value()` returns them.
pub(crate) fn vk_bc_to_wgpu(vk: u32) -> Option<TextureFormat> {
    Some(match vk {
        131 | 133 => TextureFormat::Bc1RgbaUnorm, // BC1_RGB / BC1_RGBA UNORM
        132 | 134 => TextureFormat::Bc1RgbaUnormSrgb, // BC1_RGB / BC1_RGBA SRGB
        135 => TextureFormat::Bc2RgbaUnorm,
        136 => TextureFormat::Bc2RgbaUnormSrgb,
        137 => TextureFormat::Bc3RgbaUnorm,
        138 => TextureFormat::Bc3RgbaUnormSrgb,
        139 => TextureFormat::Bc4RUnorm,
        140 => TextureFormat::Bc4RSnorm,
        141 => TextureFormat::Bc5RgUnorm,
        142 => TextureFormat::Bc5RgSnorm,
        143 => TextureFormat::Bc6hRgbUfloat,
        144 => TextureFormat::Bc6hRgbFloat,
        145 => TextureFormat::Bc7RgbaUnorm,
        146 => TextureFormat::Bc7RgbaUnormSrgb,
        _ => return None,
    })
}

/// The BC compression feature, if the adapter supports it (empty otherwise).
/// Folded into the device's `required_features` so a BC `.ktx2` can be
/// uploaded; absent the feature, this module errors with guidance instead.
pub(crate) fn bc_device_feature(adapter: &wgpu::Adapter) -> wgpu::Features {
    let bc = wgpu::Features::TEXTURE_COMPRESSION_BC;
    if adapter.features().contains(bc) {
        bc
    } else {
        wgpu::Features::empty()
    }
}

/// A parsed BC KTX2: the wgpu format and each mip level's raw block bytes
/// (level 0 first), ready to upload unchanged.
#[derive(Debug)]
pub(crate) struct Ktx2Bc {
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
}

/// Parse a KTX2 whose payload is an uncompressed BCn format. Errors (with a
/// `twec asset transcode` pointer) on supercompressed / universal / non-BC
/// files, which this direct-upload path cannot take.
pub(crate) fn parse_ktx2_bc(bytes: &[u8]) -> Result<Ktx2Bc, String> {
    let reader = ktx2::Reader::new(bytes).map_err(|e| format!("KTX2 parse failed: {e:?}"))?;
    let header = reader.header();
    if header.supercompression_scheme.is_some() {
        return Err(
            "KTX2 is supercompressed (Basis/zstd); run `twec asset transcode` to produce an \
             uploadable texture"
                .to_string(),
        );
    }
    let vk = header
        .format
        .ok_or("KTX2 has VK_FORMAT_UNDEFINED (a universal/Basis texture); transcode it first")?
        .value();
    let format = vk_bc_to_wgpu(vk).ok_or_else(|| {
        format!("KTX2 VkFormat {vk} is not a BCn format this runtime uploads directly; run `twec asset transcode`")
    })?;
    let levels: Vec<Vec<u8>> = reader.levels().map(|l| l.data.to_vec()).collect();
    if levels.is_empty() {
        return Err("KTX2 has no level data".to_string());
    }
    Ok(Ktx2Bc {
        format,
        width: header.pixel_width.max(1),
        height: header.pixel_height.max(1),
        levels,
    })
}

/// Upload a parsed BC KTX2 to a new compressed GPU texture (one mip per
/// level). Requires the device to have `TEXTURE_COMPRESSION_BC`.
pub(crate) fn upload_ktx2_bc(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    bc: &Ktx2Bc,
) -> Result<wgpu::Texture, String> {
    if !device
        .features()
        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    {
        return Err(
            "this GPU has no BC texture support; run `twec asset transcode` to ship RGBA8 instead"
                .to_string(),
        );
    }
    let block = bc
        .format
        .block_copy_size(None)
        .ok_or("unsupported block format")?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: bc.width,
            height: bc.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: bc.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: bc.format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (mip, data) in bc.levels.iter().enumerate() {
        let mw = (bc.width >> mip).max(1);
        let mh = (bc.height >> mip).max(1);
        // BC formats are 4×4 blocks; a row of the texture is `blocks_w`
        // blocks, each `block` bytes.
        let blocks_w = mw.div_ceil(4);
        let blocks_h = mh.div_ceil(4);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: mip as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(blocks_w * block),
                rows_per_image: Some(blocks_h),
            },
            wgpu::Extent3d {
                width: mw,
                height: mh,
                depth_or_array_layers: 1,
            },
        );
    }
    Ok(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal, spec-valid single-level BC1 KTX2 wrapping one 4×4
    /// block (`VK_FORMAT_BC1_RGBA_UNORM_BLOCK` = 133). The DFD is the bare
    /// 4-byte total-length field with no descriptor blocks, which the `ktx2`
    /// crate accepts.
    fn minimal_bc1_ktx2(block: [u8; 8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&KTX2_IDENTIFIER); // 0..12
        let push32 = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
        let push64 = |b: &mut Vec<u8>, v: u64| b.extend_from_slice(&v.to_le_bytes());
        // header: vkFormat, typeSize, w, h, depth, layers, faces, levels, supercompression
        push32(&mut b, 133); // BC1_RGBA_UNORM_BLOCK
        push32(&mut b, 1); // typeSize (1 for block-compressed)
        push32(&mut b, 4); // width
        push32(&mut b, 4); // height
        push32(&mut b, 0); // depth
        push32(&mut b, 0); // layers
        push32(&mut b, 1); // faces
        push32(&mut b, 1); // levels
        push32(&mut b, 0); // supercompression = none
                           // index: dfdOffset, dfdLen, kvdOffset, kvdLen, sgdOffset(u64), sgdLen(u64)
                           // layout: id(12)+header(36)+index(32)=80, levelIndex(24)=104, dfd(4)=108, level(8)=116
        push32(&mut b, 104); // dfdByteOffset
        push32(&mut b, 4); // dfdByteLength (just the total-size field)
        push32(&mut b, 0); // kvdByteOffset
        push32(&mut b, 0); // kvdByteLength
        push64(&mut b, 0); // sgdByteOffset
        push64(&mut b, 0); // sgdByteLength
                           // level index (one level): byteOffset, byteLength, uncompressedByteLength
        push64(&mut b, 108);
        push64(&mut b, 8);
        push64(&mut b, 8);
        assert_eq!(b.len(), 104);
        push32(&mut b, 4); // DFD: total length = 4, no blocks
        assert_eq!(b.len(), 108);
        b.extend_from_slice(&block); // the 4×4 BC1 block
        assert_eq!(b.len(), 116);
        b
    }

    /// A BC1 block that decodes to a solid colour: color0 = `rgb565`,
    /// color1 = 0, all 2-bit indices = 0 (→ every texel = color0).
    fn solid_bc1_block(rgb565: u16) -> [u8; 8] {
        let mut blk = [0u8; 8];
        blk[0..2].copy_from_slice(&rgb565.to_le_bytes());
        blk // color1 = 0, indices = 0
    }

    #[test]
    fn detects_the_ktx2_identifier() {
        assert!(is_ktx2(&KTX2_IDENTIFIER));
        assert!(is_ktx2(&minimal_bc1_ktx2([0; 8])));
        assert!(!is_ktx2(b"\x89PNG\r\n\x1a\n")); // a PNG header
        assert!(!is_ktx2(&[]));
    }

    #[test]
    fn maps_the_bc_vk_formats() {
        assert_eq!(vk_bc_to_wgpu(131), Some(TextureFormat::Bc1RgbaUnorm));
        assert_eq!(vk_bc_to_wgpu(134), Some(TextureFormat::Bc1RgbaUnormSrgb));
        assert_eq!(vk_bc_to_wgpu(137), Some(TextureFormat::Bc3RgbaUnorm));
        assert_eq!(vk_bc_to_wgpu(141), Some(TextureFormat::Bc5RgUnorm));
        assert_eq!(vk_bc_to_wgpu(145), Some(TextureFormat::Bc7RgbaUnorm));
        assert_eq!(vk_bc_to_wgpu(146), Some(TextureFormat::Bc7RgbaUnormSrgb));
        assert_eq!(vk_bc_to_wgpu(37), None); // R8G8B8A8_UNORM — not BC
        assert_eq!(vk_bc_to_wgpu(0), None);
    }

    #[test]
    fn parses_a_minimal_bc1_ktx2() {
        let red565: u16 = 0xF800; // 5-bit red = 31, green/blue 0
        let bytes = minimal_bc1_ktx2(solid_bc1_block(red565));
        let bc = parse_ktx2_bc(&bytes).expect("parse");
        assert_eq!(bc.format, TextureFormat::Bc1RgbaUnorm);
        assert_eq!((bc.width, bc.height), (4, 4));
        assert_eq!(bc.levels.len(), 1);
        assert_eq!(bc.levels[0].len(), 8);
        assert_eq!(bc.levels[0][0..2], red565.to_le_bytes());
    }

    #[test]
    fn rejects_a_non_bc_ktx2_with_guidance() {
        // Same container, but VK_FORMAT_R8G8B8A8_UNORM (37): not a BC payload.
        let mut bytes = minimal_bc1_ktx2([0; 8]);
        bytes[12..16].copy_from_slice(&37u32.to_le_bytes());
        let err = parse_ktx2_bc(&bytes).unwrap_err();
        assert!(err.contains("transcode"), "guidance in error: {err}");
    }

    /// Real GPU upload, on a device that advertises BC. Skips cleanly where
    /// block compression is unavailable (software adapters, most CI).
    #[test]
    fn uploads_a_bc1_texture_on_a_real_gpu() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            }))
        else {
            eprintln!("skipping BC upload test: no wgpu adapter");
            return;
        };
        if !adapter
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        {
            eprintln!("skipping BC upload test: adapter has no TEXTURE_COMPRESSION_BC");
            return;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("bc-upload-test"),
            required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .expect("device with BC");

        let bytes = minimal_bc1_ktx2(solid_bc1_block(0xF800));
        let bc = parse_ktx2_bc(&bytes).expect("parse");
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let texture = upload_ktx2_bc(&device, &queue, "test-bc1", &bc).expect("upload");
        let _view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let err = pollster::block_on(scope.pop());
        assert!(
            err.is_none(),
            "BC upload produced a validation error: {err:?}"
        );
        assert_eq!(texture.format(), TextureFormat::Bc1RgbaUnorm);
        assert_eq!(texture.width(), 4);
    }
}
