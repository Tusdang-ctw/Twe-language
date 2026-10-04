//! web3d-M9 session 2: generates `examples/assets/morph_quad.glb` if
//! missing, then checks the committed file matches its recipe and that
//! the engine loads it with one morph target + a morph-weight
//! animation. A flat quad in the XZ plane with a single morph target
//! that lifts the two +X corners in +Y, driven by a 0→1→0 weight clip
//! named "pulse". The smallest asset that exercises the morph path.

use std::path::Path;

#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl Builder {
    fn view(&mut self, bytes: &[u8]) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        self.views.push(format!(
            r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{}}}"#,
            bytes.len()
        ));
        self.views.len() - 1
    }

    fn accessor(&mut self, bytes: &[u8], ctype: u32, count: usize, ty: &str, extra: &str) -> usize {
        let view = self.view(bytes);
        self.accessors.push(format!(
            r#"{{"bufferView":{view},"componentType":{ctype},"count":{count},"type":"{ty}"{extra}}}"#
        ));
        self.accessors.len() - 1
    }

    fn floats(&mut self, data: &[f32], per: usize, ty: &str, extra: &str) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.accessor(&bytes, 5126, data.len() / per, ty, extra)
    }
}

fn nums(xs: &[f32]) -> String {
    xs.iter().map(|x| format!("{x}")).collect::<Vec<_>>().join(",")
}

fn build() -> Vec<u8> {
    // A unit quad in the XZ plane, four corners.
    let pos: [f32; 12] = [
        -0.5, 0.0, -0.5, // 0: -X -Z
        0.5, 0.0, -0.5, // 1: +X -Z
        0.5, 0.0, 0.5, // 2: +X +Z
        -0.5, 0.0, 0.5, // 3: -X +Z
    ];
    let nrm: [f32; 12] = [0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0];
    let uv: [f32; 8] = [0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
    let idx: [u32; 6] = [0, 1, 2, 0, 2, 3];
    // Morph target 0: lift the two +X corners (1 and 2) by +1 in Y.
    let morph_pos: [f32; 12] = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];

    let min = [-0.5, 0.0, -0.5];
    let max = [0.5, 0.0, 0.5];
    let mmin = [0.0, 0.0, 0.0];
    let mmax = [0.0, 1.0, 0.0];

    let mut b = Builder::default();
    let a_pos = b.floats(&pos, 3, "VEC3", &format!(r#","min":[{}],"max":[{}]"#, nums(&min), nums(&max)));
    let a_nrm = b.floats(&nrm, 3, "VEC3", "");
    let a_uv = b.floats(&uv, 2, "VEC2", "");
    let idx_bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
    let a_idx = b.accessor(&idx_bytes, 5125, idx.len(), "SCALAR", "");
    let a_morph =
        b.floats(&morph_pos, 3, "VEC3", &format!(r#","min":[{}],"max":[{}]"#, nums(&mmin), nums(&mmax)));

    // Morph-weight animation "pulse": weight 0 → 1 → 0 over 2 s.
    let times: [f32; 3] = [0.0, 1.0, 2.0];
    let a_time = b.floats(&times, 1, "SCALAR", r#","min":[0],"max":[2]"#);
    let weights: [f32; 3] = [0.0, 1.0, 0.0];
    let a_weights = b.floats(&weights, 1, "SCALAR", "");

    let views = b.views.join(",");
    let accessors = b.accessors.join(",");
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"twe tests/gen_morph_quad.rs"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"meshes":[{{"weights":[0.0],"primitives":[{{"attributes":{{"POSITION":{a_pos},"NORMAL":{a_nrm},"TEXCOORD_0":{a_uv}}},"indices":{a_idx},"targets":[{{"POSITION":{a_morph}}}]}}]}}],"animations":[{{"name":"pulse","samplers":[{{"input":{a_time},"output":{a_weights},"interpolation":"LINEAR"}}],"channels":[{{"sampler":0,"target":{{"node":0,"path":"weights"}}}}]}}],"bufferViews":[{views}],"accessors":[{accessors}],"buffers":[{{"byteLength":{}}}]}}"#,
        b.bin.len()
    );
    glb(&json, &b.bin)
}

fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut j = json.as_bytes().to_vec();
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

#[test]
fn morph_quad_exists_and_loads() {
    let path = Path::new("examples/assets/morph_quad.glb");
    if !path.exists() {
        std::fs::write(path, build()).expect("write morph_quad.glb");
    }
    let bytes = std::fs::read(path).expect("read morph_quad.glb");
    assert_eq!(bytes, build(), "morph_quad.glb differs from its recipe");

    // The gltf crate sees one morph target + a weights animation.
    let g = gltf::Gltf::from_slice(&bytes).expect("valid glTF");
    let prim = g
        .document
        .meshes()
        .next()
        .unwrap()
        .primitives()
        .next()
        .unwrap();
    assert_eq!(prim.morph_targets().len(), 1);
    assert_eq!(g.document.animations().count(), 1);

    // The engine loader parses it (no GPU needed for parse_glb_bytes).
    twec::kernel::render::parse_glb_bytes(&bytes).expect("engine loads morph_quad.glb");

    // And `twec asset info` flags the morph target as an M9 gap-closer.
    let report = twec::asset_info::inspect(path).expect("asset info");
    let info = report.gltf.unwrap();
    assert_eq!(info.morph_target_prims, 1);
    assert_eq!(info.morph_target_max, 1);
}
