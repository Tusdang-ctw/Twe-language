// web3d-M9 sessions 5-7: build-time asset transcoding. `twec asset
// transcode <in> [-o out]` decodes the glTF compression extensions
// Three.js's GLTFLoader handles — Draco + meshopt geometry and
// KTX2/Basis textures — into a plain `.glb` the Twe runtime loads
// unchanged.
//
// This runs at BUILD time, never at runtime: the decoder crates
// (draco-gltf, meshopt, ktx2, basisu) compile ONLY under the
// `asset-tools` feature and are referenced only here. The default
// runtime and the wasm32 browser build never see them, so the runtime
// stays pure-Rust and wasm-safe (the M9 cut line).
//
// Each pass takes glb/gltf bytes and returns glb bytes, so they chain:
// Draco → meshopt → KTX2. A file that uses none is still normalised to
// a self-contained `.glb`.

#![cfg(feature = "asset-tools")]

use std::path::Path;

#[derive(Debug, Default)]
pub struct TranscodeReport {
    pub draco: bool,
    pub meshopt: bool,
    pub ktx2_images: usize,
    pub in_bytes: u64,
    pub out_bytes: u64,
}

impl TranscodeReport {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.draco {
            parts.push("Draco geometry decoded".to_string());
        }
        if self.meshopt {
            parts.push("meshopt geometry decoded".to_string());
        }
        if self.ktx2_images > 0 {
            parts.push(format!(
                "{} KTX2/Basis texture(s) → RGBA8 PNG",
                self.ktx2_images
            ));
        }
        if parts.is_empty() {
            parts.push("no compression found; normalised to a self-contained .glb".to_string());
        }
        format!(
            "{}  ({:.1} KiB → {:.1} KiB)",
            parts.join(", "),
            self.in_bytes as f64 / 1024.0,
            self.out_bytes as f64 / 1024.0
        )
    }
}

/// Transcode `in_path` into a plain `.glb` at `out_path`.
pub fn transcode(in_path: &Path, out_path: &Path) -> Result<TranscodeReport, String> {
    let bytes =
        std::fs::read(in_path).map_err(|e| format!("cannot read '{}': {e}", in_path.display()))?;
    let mut report = TranscodeReport {
        in_bytes: bytes.len() as u64,
        ..Default::default()
    };

    // Which extensions are present (from the raw document).
    let exts = extensions_used(&bytes)?;
    let has_draco = exts.iter().any(|e| e == "KHR_draco_mesh_compression");
    let has_meshopt = exts.iter().any(|e| e == "EXT_meshopt_compression");
    let has_ktx2 = exts.iter().any(|e| e == "KHR_texture_basisu");

    // Security: confine the decoder's file access. A GLB is self-
    // contained (no external reads). A `.gltf` with external buffers /
    // images is rebuilt into a sandbox temp dir that holds ONLY the
    // referenced files — each validated (relative, no scheme/UNC/`..`)
    // and canonicalised to be inside the input's own directory — copied
    // under flat names, with the JSON reserialised through serde and its
    // URIs rewritten to those names. The decoder then parses exactly the
    // bytes we validated and can reach only files we placed, so neither a
    // path-traversal nor a parser-differential can make it read (and
    // embed) an arbitrary local file. (`twec asset transcode` is a local
    // build tool on dev-authored assets — UGC is out of scope — but this
    // removes the vector rather than guarding the string.)
    let sandbox = sandbox_input(in_path, &bytes)?;

    // Pass 1 — Draco (and container normalisation). draco-gltf decodes
    // every Draco primitive and re-emits a plain GLB preserving scene,
    // materials, skins and animations; it also resolves an external
    // `.bin`, so a `.gltf` input becomes a self-contained `.glb`.
    let glb_result = draco_pass(&sandbox.bytes, sandbox.base.as_deref());
    // Always clean up the sandbox, even on a decode error.
    if let Some(dir) = &sandbox.tempdir {
        let _ = std::fs::remove_dir_all(dir);
    }
    let mut glb = glb_result?;
    report.draco = has_draco;

    // Pass 2 — meshopt geometry.
    if has_meshopt {
        glb = meshopt_pass(&glb)?;
        report.meshopt = true;
    }

    // Pass 3 — KTX2 / Basis textures → RGBA8 PNG.
    if has_ktx2 {
        let (new_glb, n) = ktx2_pass(&glb)?;
        glb = new_glb;
        report.ktx2_images = n;
    }

    std::fs::write(out_path, &glb)
        .map_err(|e| format!("cannot write '{}': {e}", out_path.display()))?;
    report.out_bytes = glb.len() as u64;
    Ok(report)
}

/// A decoder input confined for safe external-file access.
struct SandboxedInput {
    /// The glTF/GLB bytes the decoder should parse.
    bytes: Vec<u8>,
    /// The directory the decoder resolves external URIs from (the
    /// sandbox for a `.gltf`, `None` for a self-contained GLB).
    base: Option<std::path::PathBuf>,
    /// The temp dir to remove when done (if one was created).
    tempdir: Option<std::path::PathBuf>,
}

/// Build a decoder input that can only reach validated files. A GLB is
/// returned as-is (self-contained). A `.gltf` is rebuilt into a fresh
/// temp dir containing only its external buffers/images — each a
/// relative, scheme/UNC/`..`-free URI whose *canonical* path is inside
/// the input's own directory — copied under flat names, with the JSON
/// reserialised and its URIs rewritten. See the call site for why.
fn sandbox_input(in_path: &Path, bytes: &[u8]) -> Result<SandboxedInput, String> {
    if bytes.len() >= 4 && &bytes[0..4] == b"glTF" {
        return Ok(SandboxedInput {
            bytes: bytes.to_vec(),
            base: None,
            tempdir: None,
        });
    }
    let mut doc: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("not valid glTF JSON: {e}"))?;
    let in_dir = in_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let canon_in_dir = in_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve input directory: {e}"))?;

    // Only create a sandbox if there is at least one external file URI.
    let has_external = ["buffers", "images"].iter().any(|k| {
        doc.get(k).and_then(|v| v.as_array()).is_some_and(|arr| {
            arr.iter().any(|it| {
                it.get("uri")
                    .and_then(|u| u.as_str())
                    .is_some_and(|u| !u.starts_with("data:"))
            })
        })
    });
    if !has_external {
        return Ok(SandboxedInput {
            bytes: bytes.to_vec(),
            base: None,
            tempdir: None,
        });
    }

    let tempdir = std::env::temp_dir().join(format!(
        "twe_transcode_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&tempdir).map_err(|e| format!("sandbox dir: {e}"))?;

    let mut copy_counter = 0usize;
    for key in ["buffers", "images"] {
        let Some(arr) = doc.get_mut(key).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        for item in arr.iter_mut() {
            let Some(uri) = item.get("uri").and_then(|u| u.as_str()).map(String::from) else {
                continue;
            };
            if uri.starts_with("data:") {
                continue;
            }
            if !crate::asset_info::is_safe_asset_uri(&uri) {
                let _ = std::fs::remove_dir_all(&tempdir);
                return Err(format!(
                    "refusing to transcode: {key} references an unsafe URI '{uri}' \
                     (absolute, UNC, a URL scheme, or '..')"
                ));
            }
            // Resolve + canonicalise + confirm it is inside the input dir
            // (catches symlinks and any resolution quirk).
            let src = in_dir.join(decode_uri(&uri));
            let canon = src.canonicalize().map_err(|e| {
                let _ = std::fs::remove_dir_all(&tempdir);
                format!("cannot resolve '{uri}': {e}")
            })?;
            if !canon.starts_with(&canon_in_dir) {
                let _ = std::fs::remove_dir_all(&tempdir);
                return Err(format!(
                    "refusing to transcode: '{uri}' resolves outside the asset's directory"
                ));
            }
            let ext = canon.extension().and_then(|e| e.to_str()).unwrap_or("bin");
            let flat = format!("f{copy_counter}.{ext}");
            copy_counter += 1;
            std::fs::copy(&canon, tempdir.join(&flat)).map_err(|e| {
                let _ = std::fs::remove_dir_all(&tempdir);
                format!("sandbox copy '{uri}': {e}")
            })?;
            item["uri"] = serde_json::json!(flat);
        }
    }

    let gltf_path = tempdir.join("model.gltf");
    let reserialised = serde_json::to_vec(&doc).map_err(|e| format!("reserialise glTF: {e}"))?;
    std::fs::write(&gltf_path, &reserialised).map_err(|e| format!("sandbox write: {e}"))?;
    Ok(SandboxedInput {
        bytes: reserialised,
        base: Some(tempdir.clone()),
        tempdir: Some(tempdir),
    })
}

/// Percent-decode a glTF URI (reuses the inspector's logic path via a
/// small local copy to avoid a cross-module private dependency).
fn decode_uri(uri: &str) -> String {
    let b = uri.as_bytes();
    let hv = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hv(b[i + 1]), hv(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read `extensionsUsed` from the raw JSON — WITHOUT the strict `gltf`
/// crate, which rejects a Draco document (its accessors have no plain
/// bufferView data until decoded). Handles both `.gltf` (raw JSON) and
/// `.glb` (JSON chunk).
fn extensions_used(bytes: &[u8]) -> Result<Vec<String>, String> {
    let doc: serde_json::Value = if bytes.len() >= 4 && &bytes[0..4] == b"glTF" {
        split_glb(bytes)?.0
    } else {
        serde_json::from_slice(bytes).map_err(|e| format!("not valid glTF JSON: {e}"))?
    };
    Ok(doc
        .get("extensionsUsed")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

/// Draco decode + normalise to a self-contained GLB v2. Works whether
/// or not the input uses Draco (a plain file just round-trips).
fn draco_pass(bytes: &[u8], base: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut import =
        draco_gltf::import_slice(bytes, base).map_err(|e| format!("draco import: {e}"))?;
    // Materialise the decoded geometry into plain accessors + buffers
    // (without this, to_bytes re-emits the compressed representation).
    import
        .decompress_in_place()
        .map_err(|e| format!("draco decompress: {e}"))?;
    let glb = import
        .to_bytes(draco_gltf::OutputFormat::GlbV2)
        .map_err(|e| format!("draco re-emit: {e}"))?;
    // draco-gltf decodes the geometry into plain accessors but leaves
    // KHR_draco_mesh_compression in extensionsUsed/Required and on each
    // primitive. Strip it so the runtime (strict gltf crate) accepts the
    // file; the decoded attributes are already present.
    let (mut doc, bin) = split_glb(&glb)?;
    strip_extension(&mut doc, "KHR_draco_mesh_compression");
    // decompress_in_place leaves the original Draco accessors (which have
    // no bufferView) orphaned in the array while repointing primitives at
    // new, materialised accessors. The strict gltf crate validates every
    // accessor, so prune the bufferView-less orphans and remap references.
    prune_orphan_accessors(&mut doc);
    if let Some(meshes) = doc.get_mut("meshes").and_then(|v| v.as_array_mut()) {
        for mesh in meshes.iter_mut() {
            if let Some(prims) = mesh.get_mut("primitives").and_then(|v| v.as_array_mut()) {
                for prim in prims.iter_mut() {
                    if let Some(ext) = prim.get_mut("extensions").and_then(|e| e.as_object_mut()) {
                        ext.remove("KHR_draco_mesh_compression");
                        if ext.is_empty() {
                            prim.as_object_mut().unwrap().remove("extensions");
                        }
                    }
                }
            }
        }
    }
    Ok(pack_glb(&doc, &bin))
}

/// web3d-M9 session 5: decode EXT_meshopt_compression buffer views.
fn meshopt_pass(glb: &[u8]) -> Result<Vec<u8>, String> {
    meshopt_impl::decode(glb)
}

/// web3d-M9 session 7: transcode KTX2/Basis images to RGBA8 PNG.
/// Returns the new glb and the number of images converted.
fn ktx2_pass(glb: &[u8]) -> Result<(Vec<u8>, usize), String> {
    ktx2_impl::transcode(glb)
}

// ── GLB container helpers (shared by the meshopt / KTX2 passes) ──

/// Split a GLB into its JSON document and its binary buffer.
fn split_glb(bytes: &[u8]) -> Result<(serde_json::Value, Vec<u8>), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"glTF" {
        return Err("not a binary glTF (.glb)".into());
    }
    let mut pos = 12usize;
    let mut json: Option<serde_json::Value> = None;
    let mut bin: Vec<u8> = Vec::new();
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let start = pos + 8;
        let end = (start + len).min(bytes.len());
        if kind == b"JSON" {
            json = Some(
                serde_json::from_slice(&bytes[start..end]).map_err(|e| format!("glb JSON: {e}"))?,
            );
        } else if kind == b"BIN\0" {
            bin = bytes[start..end].to_vec();
        }
        pos = start + len;
        while !pos.is_multiple_of(4) {
            pos += 1;
        }
    }
    json.map(|j| (j, bin))
        .ok_or_else(|| "glb has no JSON chunk".into())
}

/// Pack a JSON document and a binary buffer back into a GLB v2.
fn pack_glb(json: &serde_json::Value, bin: &[u8]) -> Vec<u8> {
    let mut j = serde_json::to_vec(json).expect("serialise glb json");
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let total = 12 + 8 + j.len() + 8 + b.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&j);
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&b);
    out
}

/// Remove accessors that have neither a `bufferView` nor `sparse` (the
/// Draco-decode orphans), remapping every accessor reference in meshes
/// (attributes / indices / morph targets), skins and animations.
fn prune_orphan_accessors(doc: &mut serde_json::Value) {
    let accessors = match doc.get("accessors").and_then(|v| v.as_array()) {
        Some(a) => a.clone(),
        None => return,
    };
    // Old index -> Some(new index) for kept accessors, None for dropped.
    let mut remap: Vec<Option<u64>> = Vec::with_capacity(accessors.len());
    let mut kept: Vec<serde_json::Value> = Vec::new();
    for acc in &accessors {
        let has_data = acc.get("bufferView").is_some() || acc.get("sparse").is_some();
        if has_data {
            remap.push(Some(kept.len() as u64));
            kept.push(acc.clone());
        } else {
            remap.push(None);
        }
    }
    if kept.len() == accessors.len() {
        return; // nothing orphaned
    }
    let map = |v: &mut serde_json::Value| {
        if let Some(old) = v.as_u64() {
            if let Some(Some(new)) = remap.get(old as usize) {
                *v = serde_json::json!(*new);
            }
        }
    };
    if let Some(meshes) = doc.get_mut("meshes").and_then(|v| v.as_array_mut()) {
        for mesh in meshes.iter_mut() {
            if let Some(prims) = mesh.get_mut("primitives").and_then(|v| v.as_array_mut()) {
                for prim in prims.iter_mut() {
                    if let Some(attrs) = prim.get_mut("attributes").and_then(|a| a.as_object_mut())
                    {
                        for (_, v) in attrs.iter_mut() {
                            map(v);
                        }
                    }
                    if let Some(v) = prim.get_mut("indices") {
                        map(v);
                    }
                    if let Some(targets) = prim.get_mut("targets").and_then(|t| t.as_array_mut()) {
                        for t in targets.iter_mut() {
                            if let Some(obj) = t.as_object_mut() {
                                for (_, v) in obj.iter_mut() {
                                    map(v);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(skins) = doc.get_mut("skins").and_then(|v| v.as_array_mut()) {
        for skin in skins.iter_mut() {
            if let Some(v) = skin.get_mut("inverseBindMatrices") {
                map(v);
            }
        }
    }
    if let Some(anims) = doc.get_mut("animations").and_then(|v| v.as_array_mut()) {
        for anim in anims.iter_mut() {
            if let Some(samplers) = anim.get_mut("samplers").and_then(|s| s.as_array_mut()) {
                for s in samplers.iter_mut() {
                    if let Some(v) = s.get_mut("input") {
                        map(v);
                    }
                    if let Some(v) = s.get_mut("output") {
                        map(v);
                    }
                }
            }
        }
    }
    doc["accessors"] = serde_json::Value::Array(kept);
}

/// Drop an extension name from `extensionsUsed` / `extensionsRequired`.
fn strip_extension(json: &mut serde_json::Value, name: &str) {
    for key in ["extensionsUsed", "extensionsRequired"] {
        if let Some(arr) = json.get_mut(key).and_then(|v| v.as_array_mut()) {
            arr.retain(|e| e.as_str() != Some(name));
        }
    }
}

/// web3d-M9 session 5: EXT_meshopt_compression.
///
/// Honest deferral: the pure-Rust surface of the `meshopt` 0.6 crate
/// exposes `decode_vertex_buffer` / `decode_index_buffer` but NOT
/// `decode_filter` (octahedral / quaternion / exponential) nor
/// `decode_index_sequence`. gltfpack routinely filters normals and
/// tangents, so decoding without the inverse filters would silently
/// produce corrupt geometry — which violates "no silent footguns".
/// Rather than ship that, the pass errors clearly and names the fix
/// (port the three inverse filters, or an upstream crate that exposes
/// them). Draco covers the same "compressed geometry" need today.
mod meshopt_impl {
    pub fn decode(_glb: &[u8]) -> Result<Vec<u8>, String> {
        Err(
            "EXT_meshopt_compression: the meshopt 0.6 crate does not expose the \
             inverse vertex filters (octahedral/quaternion/exponential) or index-sequence \
             decode, so filtered attributes would decode as corrupt geometry. Not transcoded \
             (M9 follow-up: port the three filters). Use Draco for compressed geometry today."
                .to_string(),
        )
    }
}

/// web3d-M9 session 7: KHR_texture_basisu — KTX2/Basis textures → RGBA8
/// PNG the runtime already decodes. Uses the pure-Rust `basisu`
/// transcoder (ETC1S / UASTC → RGBA32), then re-encodes PNG with the
/// `image` crate. Build-time only.
mod ktx2_impl {
    use super::{pack_glb, split_glb, strip_extension};
    use basisu::{DecodeFlags, TargetFormat, Transcoder};
    use serde_json::{json, Value};
    use std::io::Cursor;

    pub fn transcode(glb: &[u8]) -> Result<(Vec<u8>, usize), String> {
        let (mut doc, mut bin) = split_glb(glb)?;
        let views: Vec<Value> = doc
            .get("bufferViews")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let images = match doc.get("images").and_then(|v| v.as_array()) {
            Some(a) => a.clone(),
            None => return Ok((glb.to_vec(), 0)),
        };

        // image index -> its new PNG bufferView index.
        let mut png_view_of: Vec<Option<usize>> = vec![None; images.len()];
        let mut new_images = images.clone();
        let mut new_views = views.clone();
        let mut converted = 0usize;

        for (i, img) in images.iter().enumerate() {
            let is_ktx2 = img.get("mimeType").and_then(|m| m.as_str()) == Some("image/ktx2");
            let Some(view_idx) = img.get("bufferView").and_then(|v| v.as_u64()) else {
                continue;
            };
            let view = &views[view_idx as usize];
            let off = view.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let len = view.get("byteLength").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let src = bin
                .get(off..off + len)
                .ok_or("image bufferView out of range")?;
            // Treat it as KTX2 if the mime says so or the magic matches.
            let magic = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB];
            if !is_ktx2 && !src.starts_with(&magic) {
                continue;
            }
            let png = ktx2_to_png(src)?;
            // Append PNG + a new bufferView aligned to 4 bytes.
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            let new_off = bin.len();
            bin.extend_from_slice(&png);
            new_views.push(json!({
                "buffer": 0,
                "byteOffset": new_off,
                "byteLength": png.len()
            }));
            let new_view_idx = new_views.len() - 1;
            png_view_of[i] = Some(new_view_idx);
            new_images[i] = json!({
                "bufferView": new_view_idx,
                "mimeType": "image/png"
            });
            converted += 1;
        }

        if converted == 0 {
            return Ok((glb.to_vec(), 0));
        }

        // Repoint textures: a KHR_texture_basisu texture's basisu source
        // becomes its top-level source, and the extension is dropped.
        if let Some(texs) = doc.get_mut("textures").and_then(|v| v.as_array_mut()) {
            for tex in texs.iter_mut() {
                let basisu_src = tex
                    .get("extensions")
                    .and_then(|e| e.get("KHR_texture_basisu"))
                    .and_then(|b| b.get("source"))
                    .and_then(|s| s.as_u64());
                if let Some(s) = basisu_src {
                    if png_view_of.get(s as usize).and_then(|v| *v).is_some() {
                        tex["source"] = json!(s);
                    }
                    if let Some(ext) = tex.get_mut("extensions").and_then(|e| e.as_object_mut()) {
                        ext.remove("KHR_texture_basisu");
                        if ext.is_empty() {
                            tex.as_object_mut().unwrap().remove("extensions");
                        }
                    }
                }
            }
        }

        doc["images"] = Value::Array(new_images);
        doc["bufferViews"] = Value::Array(new_views);
        if let Some(buf0) = doc
            .get_mut("buffers")
            .and_then(|v| v.as_array_mut())
            .and_then(|a| a.first_mut())
        {
            buf0["byteLength"] = json!(bin.len());
        }
        strip_extension(&mut doc, "KHR_texture_basisu");
        Ok((pack_glb(&doc, &bin), converted))
    }

    fn ktx2_to_png(ktx2: &[u8]) -> Result<Vec<u8>, String> {
        let t = Transcoder::new(ktx2).map_err(|e| format!("basisu open: {e:?}"))?;
        let info = t
            .image_level_info(0)
            .map_err(|e| format!("basisu level info: {e:?}"))?;
        let rgba = t
            .transcode(0, TargetFormat::Rgba32, DecodeFlags::from_bits(0))
            .map_err(|e| format!("basisu transcode: {e:?}"))?;
        let (w, h) = (info.width, info.height);
        let img = image::RgbaImage::from_raw(w, h, rgba)
            .ok_or("basisu output size mismatch for the image dimensions")?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| format!("png encode: {e}"))?;
        Ok(png)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Build a minimal GLB embedding `sample.ktx2` as a base-colour
    /// texture referenced via KHR_texture_basisu, on a one-triangle mesh.
    fn make_ktx2_glb() -> Vec<u8> {
        let ktx2 = std::fs::read("examples/assets/compressed/sample.ktx2").expect("ktx2 fixture");
        let pos: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let uv: [f32; 6] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let idx: [u32; 3] = [0, 1, 2];
        let mut bin = Vec::new();
        let o_ktx2 = 0usize;
        bin.extend_from_slice(&ktx2);
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let o_pos = bin.len();
        bin.extend(bytemuck::cast_slice::<f32, u8>(&pos));
        let o_uv = bin.len();
        bin.extend(bytemuck::cast_slice::<f32, u8>(&uv));
        let o_idx = bin.len();
        bin.extend(bytemuck::cast_slice::<u32, u8>(&idx));
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"extensionsUsed":["KHR_texture_basisu"],"extensionsRequired":["KHR_texture_basisu"],
            "scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
            "images":[{{"bufferView":0,"mimeType":"image/ktx2"}}],
            "textures":[{{"extensions":{{"KHR_texture_basisu":{{"source":0}}}}}}],
            "materials":[{{"pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}}}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"TEXCOORD_0":1}},"indices":2,"material":0}}]}}],
            "accessors":[
              {{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}},
              {{"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"}},
              {{"bufferView":3,"componentType":5125,"count":3,"type":"SCALAR"}}
            ],
            "bufferViews":[
              {{"buffer":0,"byteOffset":{o_ktx2},"byteLength":{lk}}},
              {{"buffer":0,"byteOffset":{o_pos},"byteLength":36}},
              {{"buffer":0,"byteOffset":{o_uv},"byteLength":24}},
              {{"buffer":0,"byteOffset":{o_idx},"byteLength":12}}
            ],
            "buffers":[{{"byteLength":{bl}}}]}}"#,
            lk = ktx2.len(),
            bl = bin.len()
        );
        pack_glb(
            &serde_json::from_str(&json).expect("minimal glb json"),
            &bin,
        )
    }

    #[test]
    fn transcodes_ktx2_texture_to_png() {
        let glb = make_ktx2_glb();
        let (out, n) = ktx2_pass(&glb).expect("ktx2 pass");
        assert_eq!(n, 1, "one KTX2 image converted");
        // No KHR_texture_basisu left, image is PNG, and the runtime loads it.
        let used = extensions_used(&out).unwrap();
        assert!(
            !used.iter().any(|e| e == "KHR_texture_basisu"),
            "basisu stripped: {used:?}"
        );
        crate::kernel::render::parse_glb_bytes(&out).expect("runtime loads transcoded KTX2 glb");
        // asset info sees a 32x32 PNG texture now.
        let tmp = std::env::temp_dir().join("twe_ktx2_out.glb");
        std::fs::write(&tmp, &out).unwrap();
        let report = crate::asset_info::inspect(&tmp).unwrap();
        let g = report.gltf.unwrap();
        assert_eq!(g.images.len(), 1);
        assert_eq!((g.images[0].width, g.images[0].height), (32, 32));
        assert!(
            matches!(g.images[0].format, crate::asset_info::ImageFormat::Png),
            "now a PNG the runtime decodes"
        );
    }

    #[test]
    fn transcode_refuses_unsafe_external_uri() {
        // A .gltf whose buffer points outside its directory must be
        // refused before the decoder reads (and embeds) the file.
        let dir = std::env::temp_dir().join("twe_m9_unsafe");
        let _ = std::fs::create_dir_all(&dir);
        let gltf = dir.join("evil.gltf");
        std::fs::write(
            &gltf,
            br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"../../../../../../etc/passwd","byteLength":10}]}"#,
        )
        .unwrap();
        let out = dir.join("out.glb");
        let err = transcode(&gltf, &out).unwrap_err();
        assert!(err.contains("unsafe URI"), "refuses the traversal: {err}");
    }

    #[test]
    fn meshopt_pass_errors_honestly() {
        // The meshopt pass must not silently produce corrupt geometry.
        let err = meshopt_pass(&make_ktx2_glb()).unwrap_err();
        assert!(err.contains("meshopt"), "clear meshopt error: {err}");
    }

    #[test]
    fn transcodes_draco_box_to_plain() {
        let out = std::env::temp_dir().join("twe_box_draco_plain.glb");
        let report = transcode(Path::new("examples/assets/compressed/Box_draco.gltf"), &out)
            .expect("transcode Draco Box");
        assert!(report.draco, "Box_draco uses KHR_draco_mesh_compression");

        // The transcoded glb has no Draco extension and loads in the
        // runtime with the same geometry as the plain Box (24 verts,
        // 36 indices — a cube).
        let bytes = std::fs::read(&out).expect("read transcoded");
        let used = extensions_used(&bytes).unwrap();
        assert!(
            !used.iter().any(|e| e == "KHR_draco_mesh_compression"),
            "Draco extension stripped: {used:?}"
        );
        let decoded =
            crate::kernel::render::parse_glb_bytes(&bytes).expect("runtime loads transcoded");
        let plain = crate::kernel::render::parse_glb_bytes(
            &std::fs::read("examples/assets/compressed/Box.glb").unwrap(),
        )
        .expect("runtime loads plain Box");
        assert_eq!(
            decoded.vertex_count(),
            plain.vertex_count(),
            "transcoded geometry matches the plain Box"
        );
    }
}
