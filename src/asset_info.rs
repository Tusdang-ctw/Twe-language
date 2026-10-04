// web3d-M9 session 1: `twec asset info <file>` — a GPU-free inspector
// for glTF / glb files and standalone images. It reports structure
// (meshes, primitives, attributes, morph targets, materials + which
// KHR extensions each uses, textures, skins, animations), flags the
// compression schemes the current runtime does *not* yet load
// (EXT_meshopt_compression, KHR_draco_mesh_compression, KTX2/Basis),
// and estimates VRAM against the 512 MiB budget.
//
// This is deliberately independent of `src/kernel/*` (no wgpu, no
// device): it reads metadata only, via `gltf::Gltf::from_slice` (which
// parses the JSON + binary chunk without decoding any image), so it
// runs anywhere and never allocates a GPU. It is the foundation the
// rest of M9 builds on — until sessions 5-7 land, this tool is how we
// see the gap ("Draco: present (not loaded)").

use crate::json::{self, Value};
use std::path::Path;

/// The GPU VRAM budget M9 measures assets against (see the M9 plan).
pub const VRAM_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Gltf,
    Image,
    Unknown,
}

impl AssetKind {
    fn as_str(self) -> &'static str {
        match self {
            AssetKind::Gltf => "gltf",
            AssetKind::Image => "image",
            AssetKind::Unknown => "unknown",
        }
    }
}

/// How an image is encoded on disk, detected by magic bytes (not by
/// file extension — a `.bin` buffer-view image is still a PNG).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Hdr,
    /// KTX2 container. `supercompression` distinguishes Basis ETC1S
    /// (scheme 1) from Zstd (2) / none (0) / etc.
    Ktx2 { supercompression: u32, vk_format: u32 },
    Unknown,
}

impl ImageFormat {
    fn label(self) -> String {
        match self {
            ImageFormat::Png => "PNG".into(),
            ImageFormat::Jpeg => "JPEG".into(),
            ImageFormat::Hdr => "Radiance HDR".into(),
            ImageFormat::Ktx2 { supercompression, vk_format } => {
                let sc = match supercompression {
                    0 if vk_format == 0 => "Basis UASTC",
                    0 => "uncompressed/BCn",
                    1 => "Basis ETC1S",
                    2 => "Zstd",
                    3 => "ZLIB",
                    _ => "unknown supercompression",
                };
                format!("KTX2 ({sc})")
            }
            ImageFormat::Unknown => "unknown".into(),
        }
    }

    /// True when the current runtime cannot decode this on load (M9
    /// closes KTX2/Basis in session 7).
    fn runtime_loads(self) -> bool {
        matches!(self, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Hdr)
    }
}

#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub name: Option<String>,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// On-disk encoded size when known (buffer-view length or file size).
    pub encoded_bytes: u64,
    /// Decoded RGBA8 footprint incl. a full mip chain — what the
    /// runtime actually uploads today.
    pub rgba8_vram_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct AttrFlags {
    pub normals: bool,
    pub tangents: bool,
    pub uv0: bool,
    pub uv1: bool,
    pub colors: bool,
    pub joints: bool,
    pub weights: bool,
}

#[derive(Debug, Clone)]
pub struct MaterialInfo {
    pub name: Option<String>,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CompressionFlags {
    pub meshopt: bool,
    pub draco: bool,
    pub basisu: bool,
}

#[derive(Debug, Clone)]
pub struct GltfInfo {
    pub generator: Option<String>,
    pub extensions_used: Vec<String>,
    pub extensions_required: Vec<String>,
    pub meshes: usize,
    pub primitives: usize,
    pub non_triangle_primitives: usize,
    pub total_vertices: u64,
    pub total_indices: u64,
    pub attrs: AttrFlags,
    pub morph_target_max: usize,
    pub morph_target_prims: usize,
    pub materials: Vec<MaterialInfo>,
    pub images: Vec<ImageInfo>,
    pub skins: usize,
    pub skin_joints: Vec<usize>,
    pub animations: usize,
    pub compression: CompressionFlags,
}

#[derive(Debug, Clone)]
pub struct VramEstimate {
    pub geometry_bytes: u64,
    pub texture_bytes: u64,
    pub total_bytes: u64,
    pub budget_bytes: u64,
}

impl VramEstimate {
    pub fn over_budget(&self) -> bool {
        self.total_bytes > self.budget_bytes
    }
}

#[derive(Debug, Clone)]
pub struct AssetReport {
    pub path: String,
    pub kind: AssetKind,
    pub file_bytes: u64,
    pub gltf: Option<GltfInfo>,
    pub image: Option<ImageInfo>,
    pub vram: VramEstimate,
    /// Feature-gap notes: what Three.js would load here that Twe does
    /// not yet (closed over the course of M9).
    pub gaps: Vec<String>,
    pub warnings: Vec<String>,
}

/// The glTF compression extensions this file declares, read from raw
/// `extensionsUsed` WITHOUT the strict `gltf` crate (which rejects a
/// Draco document outright). Returns the subset the runtime can't load:
/// `KHR_draco_mesh_compression`, `EXT_meshopt_compression`,
/// `KHR_texture_basisu`. Used by the `twec build` asset audit so it can
/// warn even about files `inspect` cannot fully parse.
pub fn compression_extensions(path: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let json_text: Option<String> = if bytes.len() >= 20 && &bytes[0..4] == b"glTF" {
        // GLB: the first chunk is JSON; its length is the u32 at offset 12.
        let len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
        bytes
            .get(20..20 + len)
            .map(|s| String::from_utf8_lossy(s).into_owned())
    } else {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    };
    let Some(text) = json_text else {
        return Vec::new();
    };
    let Ok(doc) = json::parse(&text) else {
        return Vec::new();
    };
    let used = doc.get("extensionsUsed").and_then(|v| v.as_array());
    let known = [
        "KHR_draco_mesh_compression",
        "EXT_meshopt_compression",
        "KHR_texture_basisu",
    ];
    used.map(|a| {
        a.iter()
            .filter_map(|e| e.as_str())
            .filter(|e| known.contains(e))
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

/// Inspect a file. Never touches a GPU; never decodes a full image.
pub fn inspect(path: &Path) -> Result<AssetReport, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let file_bytes = bytes.len() as u64;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "glb" || ext == "gltf" {
        inspect_gltf(path, &bytes, file_bytes)
    } else if is_image_magic(&bytes) || matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "hdr" | "ktx2") {
        inspect_image_file(path, &bytes, file_bytes)
    } else {
        Err(format!(
            "unrecognised asset '{}': expected .glb/.gltf or an image (.png/.jpg/.hdr/.ktx2)",
            path.display()
        ))
    }
}

fn inspect_image_file(path: &Path, bytes: &[u8], file_bytes: u64) -> Result<AssetReport, String> {
    let info = image_info_from_bytes(None, bytes, file_bytes);
    let mut gaps = Vec::new();
    if !info.format.runtime_loads() {
        gaps.push(format!(
            "{} not loaded by the runtime yet (M9 session 7)",
            info.format.label()
        ));
    }
    let vram = VramEstimate {
        geometry_bytes: 0,
        texture_bytes: info.rgba8_vram_bytes,
        total_bytes: info.rgba8_vram_bytes,
        budget_bytes: VRAM_BUDGET_BYTES,
    };
    Ok(AssetReport {
        path: path.display().to_string(),
        kind: AssetKind::Image,
        file_bytes,
        gltf: None,
        image: Some(info),
        vram,
        gaps,
        warnings: Vec::new(),
    })
}

fn inspect_gltf(path: &Path, bytes: &[u8], file_bytes: u64) -> Result<AssetReport, String> {
    let g = gltf::Gltf::from_slice(bytes).map_err(|e| format!("not a valid glTF: {e}"))?;
    let doc = &g.document;
    let blob = g.blob.as_deref();
    let mut warnings = Vec::new();

    let generator = gltf_generator(doc);

    let extensions_used: Vec<String> = doc.extensions_used().map(|s| s.to_string()).collect();
    let extensions_required: Vec<String> = doc.extensions_required().map(|s| s.to_string()).collect();

    let compression = CompressionFlags {
        meshopt: extensions_used.iter().any(|e| e == "EXT_meshopt_compression"),
        draco: extensions_used.iter().any(|e| e == "KHR_draco_mesh_compression"),
        basisu: extensions_used.iter().any(|e| e == "KHR_texture_basisu"),
    };

    // Geometry walk.
    let mut primitives = 0usize;
    let mut non_triangle_primitives = 0usize;
    let mut total_vertices = 0u64;
    let mut total_indices = 0u64;
    let mut geometry_bytes = 0u64;
    let mut attrs = AttrFlags::default();
    let mut morph_target_max = 0usize;
    let mut morph_target_prims = 0usize;

    for mesh in doc.meshes() {
        for prim in mesh.primitives() {
            primitives += 1;
            if prim.mode() != gltf::mesh::Mode::Triangles {
                non_triangle_primitives += 1;
            }
            let morphs = prim.morph_targets().len();
            if morphs > 0 {
                morph_target_prims += 1;
                morph_target_max = morph_target_max.max(morphs);
            }
            for (sem, acc) in prim.attributes() {
                geometry_bytes += accessor_bytes(&acc);
                use gltf::Semantic::*;
                match sem {
                    Positions => total_vertices += acc.count() as u64,
                    Normals => attrs.normals = true,
                    Tangents => attrs.tangents = true,
                    TexCoords(0) => attrs.uv0 = true,
                    TexCoords(_) => attrs.uv1 = true,
                    Colors(_) => attrs.colors = true,
                    Joints(_) => attrs.joints = true,
                    Weights(_) => attrs.weights = true,
                }
            }
            if let Some(idx) = prim.indices() {
                total_indices += idx.count() as u64;
                geometry_bytes += accessor_bytes(&idx);
            }
        }
    }

    // Materials + their extensions.
    let materials: Vec<MaterialInfo> = doc
        .materials()
        .map(|m| {
            let extensions = m
                .extensions()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            MaterialInfo {
                name: m.name().map(|s| s.to_string()),
                extensions,
            }
        })
        .collect();

    // Images (deduped at the image level, like the uploader).
    let mut images = Vec::new();
    let mut texture_bytes = 0u64;
    for img in doc.images() {
        let (name, bytes_for_img) = image_source_bytes(&img, blob, path);
        if let Some(b) = bytes_for_img {
            let info = image_info_from_bytes(name, &b, b.len() as u64);
            texture_bytes += info.rgba8_vram_bytes;
            images.push(info);
        } else {
            warnings.push(format!(
                "image #{} source could not be read (external or data URI)",
                img.index()
            ));
        }
    }

    let skins = doc.skins().count();
    let skin_joints: Vec<usize> = doc.skins().map(|s| s.joints().count()).collect();
    let animations = doc.animations().count();

    // Feature-gap notes for the M9 report.
    let mut gaps = Vec::new();
    if compression.meshopt {
        gaps.push("EXT_meshopt_compression present — geometry not loaded yet (M9 session 5)".into());
    }
    if compression.draco {
        gaps.push("KHR_draco_mesh_compression present — geometry not loaded yet (M9 session 6)".into());
    }
    if compression.basisu || images.iter().any(|i| matches!(i.format, ImageFormat::Ktx2 { .. })) {
        gaps.push("KTX2/Basis textures present — not decoded yet (M9 session 7)".into());
    }
    // Morph targets (M9 s2), multi-skin (M9 s3) and KHR_materials_anisotropy
    // (M9 s4) are supported now — no longer gaps.

    let total = geometry_bytes + texture_bytes;
    let vram = VramEstimate {
        geometry_bytes,
        texture_bytes,
        total_bytes: total,
        budget_bytes: VRAM_BUDGET_BYTES,
    };

    let info = GltfInfo {
        generator,
        extensions_used,
        extensions_required,
        meshes: doc.meshes().count(),
        primitives,
        non_triangle_primitives,
        total_vertices,
        total_indices,
        attrs,
        morph_target_max,
        morph_target_prims,
        materials,
        images,
        skins,
        skin_joints,
        animations,
        compression,
    };

    Ok(AssetReport {
        path: path.display().to_string(),
        kind: AssetKind::Gltf,
        file_bytes,
        gltf: Some(info),
        image: None,
        vram,
        gaps,
        warnings,
    })
}

fn gltf_generator(doc: &gltf::Document) -> Option<String> {
    // The gltf crate doesn't surface asset.generator on Document in a
    // stable typed accessor across versions; pull it from the raw JSON.
    doc.as_json().asset.generator.clone()
}

fn accessor_bytes(acc: &gltf::Accessor) -> u64 {
    acc.count() as u64 * acc.data_type().size() as u64 * acc.dimensions().multiplicity() as u64
}

/// Resolve an image's raw encoded bytes without decoding: from the glb
/// binary blob (buffer view), or an external relative file. Data URIs
/// are not base64-decoded here (reported as unreadable).
fn image_source_bytes(
    img: &gltf::Image,
    blob: Option<&[u8]>,
    gltf_path: &Path,
) -> (Option<String>, Option<Vec<u8>>) {
    let name = img.name().map(|s| s.to_string());
    match img.source() {
        gltf::image::Source::View { view, .. } => {
            let start = view.offset();
            let end = start + view.length();
            let slice = blob.and_then(|b| b.get(start..end)).map(|s| s.to_vec());
            (name, slice)
        }
        gltf::image::Source::Uri { uri, .. } => {
            // Security: only read a relative path that stays under the
            // glTF's own directory. Rejecting absolute / UNC / scheme /
            // `..` URIs stops a crafted glTF from reading an arbitrary
            // local file — and, on Windows, from triggering an SMB/UNC
            // fetch that would leak NTLM credentials.
            if uri.starts_with("data:") || !is_safe_asset_uri(uri) {
                (name, None)
            } else {
                let decoded = percent_decode(uri);
                let p = gltf_path
                    .parent()
                    .map(|d| d.join(&decoded))
                    .unwrap_or_else(|| Path::new(&decoded).to_path_buf());
                (name, std::fs::read(p).ok())
            }
        }
    }
}

/// True when `uri` is a relative file reference that stays within the
/// referencing glTF's directory: no URL scheme, no UNC path, no drive
/// letter, no absolute path, and no `..` component. A `data:` URI is
/// not a file and returns false here (callers handle it separately).
/// Shared by the inspector and the transcoder.
pub fn is_safe_asset_uri(uri: &str) -> bool {
    if uri.is_empty() || uri.starts_with("data:") {
        return false;
    }
    if uri.contains("://") || uri.starts_with("//") || uri.starts_with('\\') {
        return false; // scheme (http/file/…) or UNC
    }
    let decoded = percent_decode(uri);
    if decoded.contains(':') || decoded.starts_with('\\') || decoded.starts_with('/') {
        return false; // drive letter / UNC / absolute after decoding
    }
    let p = Path::new(&decoded);
    if p.is_absolute() {
        return false;
    }
    !p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Minimal percent-decode for glTF URIs (spaces as %20 etc.). Avoids a
/// url crate dependency for the common case.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn is_image_magic(bytes: &[u8]) -> bool {
    detect_format(bytes) != ImageFormat::Unknown
}

const KTX2_ID: [u8; 12] = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];

fn detect_format(bytes: &[u8]) -> ImageFormat {
    if bytes.len() >= 4 && bytes[..4] == [0x89, 0x50, 0x4E, 0x47] {
        ImageFormat::Png
    } else if bytes.len() >= 3 && bytes[..3] == [0xFF, 0xD8, 0xFF] {
        ImageFormat::Jpeg
    } else if bytes.len() >= 12 && bytes[..12] == KTX2_ID {
        let vk_format = le_u32(bytes, 12).unwrap_or(0);
        let supercompression = le_u32(bytes, 44).unwrap_or(0);
        ImageFormat::Ktx2 { supercompression, vk_format }
    } else if bytes.len() >= 2 && (&bytes[..2] == b"#?") {
        ImageFormat::Hdr
    } else {
        ImageFormat::Unknown
    }
}

fn le_u32(bytes: &[u8], off: usize) -> Option<u32> {
    bytes
        .get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Read dimensions cheaply (header only for png/jpeg/hdr via the image
/// crate's reader; a manual parse for KTX2) and compute the decoded
/// RGBA8 + mip footprint.
fn image_info_from_bytes(name: Option<String>, bytes: &[u8], encoded_bytes: u64) -> ImageInfo {
    let format = detect_format(bytes);
    let (width, height) = match format {
        ImageFormat::Ktx2 { .. } => (le_u32(bytes, 20).unwrap_or(0), le_u32(bytes, 24).unwrap_or(0)),
        ImageFormat::Unknown => (0, 0),
        _ => {
            use std::io::Cursor;
            image::ImageReader::new(Cursor::new(bytes))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok())
                .unwrap_or((0, 0))
        }
    };
    let base = width as u64 * height as u64 * 4;
    // Full mip chain multiplies the base level by ~4/3.
    let rgba8_vram_bytes = base + base / 3;
    ImageInfo {
        name,
        format,
        width,
        height,
        encoded_bytes,
        rgba8_vram_bytes,
    }
}

// ---------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

impl AssetReport {
    /// Human-readable report.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("{}\n", self.path));
        s.push_str(&format!(
            "  kind: {}   file: {:.2} MiB\n",
            self.kind.as_str(),
            mib(self.file_bytes)
        ));
        if let Some(g) = &self.gltf {
            if let Some(gen) = &g.generator {
                s.push_str(&format!("  generator: {gen}\n"));
            }
            s.push_str(&format!(
                "  geometry: {} meshes, {} primitives, {} vertices, {} indices\n",
                g.meshes, g.primitives, g.total_vertices, g.total_indices
            ));
            if g.non_triangle_primitives > 0 {
                s.push_str(&format!(
                    "    note: {} non-triangle primitive(s) (not rendered)\n",
                    g.non_triangle_primitives
                ));
            }
            let a = &g.attrs;
            let mut present = Vec::new();
            for (flag, label) in [
                (a.normals, "normals"),
                (a.tangents, "tangents"),
                (a.uv0, "uv0"),
                (a.uv1, "uv1"),
                (a.colors, "colors"),
                (a.joints, "joints"),
                (a.weights, "weights"),
            ] {
                if flag {
                    present.push(label);
                }
            }
            s.push_str(&format!("    attributes: {}\n", present.join(", ")));
            if g.morph_target_prims > 0 {
                s.push_str(&format!(
                    "  morph targets: up to {} per primitive on {} primitive(s)\n",
                    g.morph_target_max, g.morph_target_prims
                ));
            }
            s.push_str(&format!("  skins: {}", g.skins));
            if !g.skin_joints.is_empty() {
                s.push_str(&format!(" (joints: {:?})", g.skin_joints));
            }
            s.push_str(&format!("   animations: {}\n", g.animations));
            s.push_str(&format!("  materials: {}\n", g.materials.len()));
            for m in &g.materials {
                let nm = m.name.as_deref().unwrap_or("(unnamed)");
                if m.extensions.is_empty() {
                    s.push_str(&format!("    - {nm}\n"));
                } else {
                    s.push_str(&format!("    - {nm}  [{}]\n", m.extensions.join(", ")));
                }
            }
            s.push_str(&format!("  textures: {}\n", g.images.len()));
            for img in &g.images {
                s.push_str(&format!("    - {}\n", describe_image(img)));
            }
            if !g.extensions_used.is_empty() {
                s.push_str(&format!("  extensions used: {}\n", g.extensions_used.join(", ")));
            }
            if !g.extensions_required.is_empty() {
                s.push_str(&format!(
                    "  extensions required: {}\n",
                    g.extensions_required.join(", ")
                ));
            }
        }
        if let Some(img) = &self.image {
            s.push_str(&format!("  image: {}\n", describe_image(img)));
        }
        s.push_str(&format!(
            "  VRAM estimate: {:.1} MiB (geometry {:.1} + textures {:.1}) / {} MiB budget{}\n",
            mib(self.vram.total_bytes),
            mib(self.vram.geometry_bytes),
            mib(self.vram.texture_bytes),
            self.vram.budget_bytes / (1024 * 1024),
            if self.vram.over_budget() { "  ** OVER BUDGET **" } else { "" }
        ));
        if !self.gaps.is_empty() {
            s.push_str("  M9 gaps:\n");
            for gap in &self.gaps {
                s.push_str(&format!("    ! {gap}\n"));
            }
        }
        for w in &self.warnings {
            s.push_str(&format!("  warning: {w}\n"));
        }
        s
    }

    /// Machine-readable report (Studio AssetBrowser + LLM grounding).
    pub fn to_json(&self) -> String {
        let mut root = json::obj([
            ("path", Value::Str(self.path.clone())),
            ("kind", Value::Str(self.kind.as_str().to_string())),
            ("file_bytes", Value::Int(self.file_bytes as i64)),
        ]);
        let map = match &mut root {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        if let Some(g) = &self.gltf {
            map.insert("gltf".into(), gltf_json(g));
        }
        if let Some(img) = &self.image {
            map.insert("image".into(), image_json(img));
        }
        map.insert(
            "vram".into(),
            json::obj([
                ("geometry_bytes", Value::Int(self.vram.geometry_bytes as i64)),
                ("texture_bytes", Value::Int(self.vram.texture_bytes as i64)),
                ("total_bytes", Value::Int(self.vram.total_bytes as i64)),
                ("budget_bytes", Value::Int(self.vram.budget_bytes as i64)),
                ("over_budget", Value::Bool(self.vram.over_budget())),
            ]),
        );
        map.insert(
            "gaps".into(),
            Value::Array(self.gaps.iter().cloned().map(Value::Str).collect()),
        );
        map.insert(
            "warnings".into(),
            Value::Array(self.warnings.iter().cloned().map(Value::Str).collect()),
        );
        json::to_string(&root)
    }
}

fn describe_image(img: &ImageInfo) -> String {
    let nm = img.name.as_deref().map(|n| format!("{n} ")).unwrap_or_default();
    let gap = if img.format.runtime_loads() { "" } else { "  (not loaded yet)" };
    format!(
        "{nm}{} {}x{}  {:.2} MiB on disk, ~{:.1} MiB VRAM{gap}",
        img.format.label(),
        img.width,
        img.height,
        mib(img.encoded_bytes),
        mib(img.rgba8_vram_bytes),
    )
}

fn image_json(img: &ImageInfo) -> Value {
    json::obj([
        (
            "name",
            img.name.clone().map_or(Value::Null, Value::Str),
        ),
        ("format", Value::Str(img.format.label())),
        ("width", Value::Int(img.width as i64)),
        ("height", Value::Int(img.height as i64)),
        ("encoded_bytes", Value::Int(img.encoded_bytes as i64)),
        ("rgba8_vram_bytes", Value::Int(img.rgba8_vram_bytes as i64)),
        ("runtime_loads", Value::Bool(img.format.runtime_loads())),
    ])
}

fn gltf_json(g: &GltfInfo) -> Value {
    let attrs = json::obj([
        ("normals", Value::Bool(g.attrs.normals)),
        ("tangents", Value::Bool(g.attrs.tangents)),
        ("uv0", Value::Bool(g.attrs.uv0)),
        ("uv1", Value::Bool(g.attrs.uv1)),
        ("colors", Value::Bool(g.attrs.colors)),
        ("joints", Value::Bool(g.attrs.joints)),
        ("weights", Value::Bool(g.attrs.weights)),
    ]);
    let materials = Value::Array(
        g.materials
            .iter()
            .map(|m| {
                json::obj([
                    ("name", m.name.clone().map_or(Value::Null, Value::Str)),
                    (
                        "extensions",
                        Value::Array(m.extensions.iter().cloned().map(Value::Str).collect()),
                    ),
                ])
            })
            .collect(),
    );
    let images = Value::Array(g.images.iter().map(image_json).collect());
    let compression = json::obj([
        ("meshopt", Value::Bool(g.compression.meshopt)),
        ("draco", Value::Bool(g.compression.draco)),
        ("basisu", Value::Bool(g.compression.basisu)),
    ]);
    json::obj([
        ("generator", g.generator.clone().map_or(Value::Null, Value::Str)),
        (
            "extensions_used",
            Value::Array(g.extensions_used.iter().cloned().map(Value::Str).collect()),
        ),
        (
            "extensions_required",
            Value::Array(g.extensions_required.iter().cloned().map(Value::Str).collect()),
        ),
        ("meshes", Value::Int(g.meshes as i64)),
        ("primitives", Value::Int(g.primitives as i64)),
        ("non_triangle_primitives", Value::Int(g.non_triangle_primitives as i64)),
        ("total_vertices", Value::Int(g.total_vertices as i64)),
        ("total_indices", Value::Int(g.total_indices as i64)),
        ("attributes", attrs),
        ("morph_target_max", Value::Int(g.morph_target_max as i64)),
        ("morph_target_prims", Value::Int(g.morph_target_prims as i64)),
        ("materials", materials),
        ("images", images),
        ("skins", Value::Int(g.skins as i64)),
        (
            "skin_joints",
            Value::Array(g.skin_joints.iter().map(|j| Value::Int(*j as i64)).collect()),
        ),
        ("animations", Value::Int(g.animations as i64)),
        ("compression", compression),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ktx2_magic() {
        let mut bytes = KTX2_ID.to_vec();
        bytes.extend_from_slice(&[0u8; 64]);
        // pixelWidth = 256 at offset 20, pixelHeight = 128 at offset 24.
        bytes[20..24].copy_from_slice(&256u32.to_le_bytes());
        bytes[24..28].copy_from_slice(&128u32.to_le_bytes());
        bytes[44..48].copy_from_slice(&1u32.to_le_bytes()); // BasisLZ
        let info = image_info_from_bytes(None, &bytes, bytes.len() as u64);
        assert_eq!((info.width, info.height), (256, 128));
        assert!(matches!(info.format, ImageFormat::Ktx2 { supercompression: 1, .. }));
        assert!(!info.format.runtime_loads());
    }

    #[test]
    fn percent_decode_spaces() {
        assert_eq!(percent_decode("my%20tex.png"), "my tex.png");
        assert_eq!(percent_decode("plain.png"), "plain.png");
    }

    #[test]
    fn safe_uri_rejects_traversal_and_unc() {
        // Allowed: ordinary relative references.
        assert!(is_safe_asset_uri("tex.png"));
        assert!(is_safe_asset_uri("textures/base.png"));
        assert!(is_safe_asset_uri("my%20tex.png"));
        // Rejected: escapes, absolutes, UNC, schemes, drive letters.
        assert!(!is_safe_asset_uri("../secret.png"));
        assert!(!is_safe_asset_uri("a/../../secret"));
        assert!(!is_safe_asset_uri("/etc/passwd"));
        assert!(!is_safe_asset_uri("\\\\attacker\\share\\x")); // UNC → SMB cred leak
        assert!(!is_safe_asset_uri("//attacker/share/x"));
        assert!(!is_safe_asset_uri("C:\\Windows\\x"));
        assert!(!is_safe_asset_uri("http://evil/x.png"));
        assert!(!is_safe_asset_uri("file:///etc/passwd"));
        assert!(!is_safe_asset_uri("data:image/png;base64,AAAA"));
    }

    #[test]
    fn vram_over_budget_flag() {
        let v = VramEstimate {
            geometry_bytes: 0,
            texture_bytes: VRAM_BUDGET_BYTES + 1,
            total_bytes: VRAM_BUDGET_BYTES + 1,
            budget_bytes: VRAM_BUDGET_BYTES,
        };
        assert!(v.over_budget());
    }
}
