//! web3d-M4: the hero of `examples/survive3d`, built in code.
//!
//! Writes `examples/survive3d/assets/hero.glb` if it is missing, then
//! checks it is the file this recipe builds and that it loads with its
//! skin and clips. The file is committed, so players never run this;
//! it is the procedural spec of the model (edit the recipe, delete the
//! file, rerun to regenerate), like `gen_survive3d_sounds.rs`.
//!
//! The model: a blocky figure of box parts (a palette texture colours
//! them), skinned to five joints (hips, two legs, two arms), with a
//! looping `walk` and `idle` clip. It faces +Z, which `look: facing: 0`
//! faces.

use std::fmt::Write as _;
use std::path::Path;

/// Palette cells, left to right in the texture.
const SKIN: usize = 0;
const SHIRT: usize = 1;
const PANTS: usize = 2;
const DARK: usize = 3;
const PALETTE: [[u8; 3]; 4] = [[236, 190, 150], [60, 120, 220], [55, 55, 80], [40, 30, 25]];
/// Each cell is CELL × CELL pixels, so mip levels keep the colours
/// apart down to 4 × 1.
const CELL: usize = 16;

/// Joints: (name, parent joint, translation relative to the parent).
const JOINTS: [(&str, Option<usize>, [f32; 3]); 5] = [
    ("hips", None, [0.0, 0.0, 0.0]),
    ("leg_l", Some(0), [-0.15, 0.56, 0.0]),
    ("leg_r", Some(0), [0.15, 0.56, 0.0]),
    ("arm_l", Some(0), [-0.36, 1.15, 0.0]),
    ("arm_r", Some(0), [0.36, 1.15, 0.0]),
];

/// Box parts: (min, max, joint, palette cell). Bind-pose model space,
/// metres, feet at y = 0.
const PARTS: &[([f32; 3], [f32; 3], u16, usize)] = &[
    ([-0.28, 0.55, -0.17], [0.28, 1.2, 0.17], 0, SHIRT), // torso
    ([-0.2, 1.22, -0.2], [0.2, 1.56, 0.2], 0, SKIN),     // head
    ([-0.21, 1.56, -0.21], [0.21, 1.66, 0.21], 0, DARK), // hair
    ([-0.14, 1.37, 0.2], [0.14, 1.44, 0.23], 0, DARK),   // eyes (front, +Z)
    ([-0.25, 0.1, -0.1], [-0.05, 0.56, 0.1], 1, PANTS),  // left leg
    ([-0.26, 0.0, -0.11], [-0.04, 0.12, 0.14], 1, DARK), // left boot
    ([0.05, 0.1, -0.1], [0.25, 0.56, 0.1], 2, PANTS),    // right leg
    ([0.04, 0.0, -0.11], [0.26, 0.12, 0.14], 2, DARK),   // right boot
    ([-0.42, 0.66, -0.07], [-0.3, 1.18, 0.07], 3, SHIRT), // left arm
    ([-0.42, 0.55, -0.07], [-0.3, 0.66, 0.07], 3, SKIN), // left hand
    ([0.3, 0.66, -0.07], [0.42, 1.18, 0.07], 4, SHIRT),  // right arm
    ([0.3, 0.55, -0.07], [0.42, 0.66, 0.07], 4, SKIN),   // right hand
];

/// A clip: (name, key times, per-key hips lift, per-joint X-rotation
/// degrees for legs l/r and arms l/r).
struct Clip {
    name: &'static str,
    times: &'static [f32],
    lift: &'static [f32],
    swing: [&'static [f32]; 4],
}

const CLIPS: [Clip; 2] = [
    // Legs swing ±35°, arms counter-swing ±30°; the hips rise as the
    // legs pass under the body.
    Clip {
        name: "walk",
        times: &[0.0, 0.2, 0.4, 0.6, 0.8],
        lift: &[0.04, 0.0, 0.04, 0.0, 0.04],
        swing: [
            &[0.0, 35.0, 0.0, -35.0, 0.0],
            &[0.0, -35.0, 0.0, 35.0, 0.0],
            &[0.0, -30.0, 0.0, 30.0, 0.0],
            &[0.0, 30.0, 0.0, -30.0, 0.0],
        ],
    },
    // Breathing: a slight rise and arm sway.
    Clip {
        name: "idle",
        times: &[0.0, 1.0, 2.0],
        lift: &[0.0, 0.015, 0.0],
        swing: [
            &[0.0, 0.0, 0.0],
            &[0.0, 0.0, 0.0],
            &[0.0, 4.0, 0.0],
            &[0.0, -4.0, 0.0],
        ],
    },
];

/// The binary buffer, and the JSON's bufferViews / accessors as it
/// grows.
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

    /// An accessor over `bytes`; `extra` adds min/max etc.
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

fn quat_x(deg: f32) -> [f32; 4] {
    let h = deg.to_radians() / 2.0;
    [h.sin(), 0.0, 0.0, h.cos()]
}

fn build() -> Vec<u8> {
    // Geometry: 24 vertices per box (flat-shaded faces).
    let (mut pos, mut nrm, mut uv) = (Vec::new(), Vec::new(), Vec::new());
    let (mut joints, mut weights, mut idx) = (Vec::<u16>::new(), Vec::new(), Vec::<u32>::new());
    for &(lo, hi, joint, cell) in PARTS {
        let u = (cell as f32 + 0.5) / PALETTE.len() as f32;
        // (normal, four corners counter-clockwise seen from outside)
        let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
            ([1.0, 0.0, 0.0], [[hi[0], lo[1], hi[2]], [hi[0], lo[1], lo[2]], [hi[0], hi[1], lo[2]], [hi[0], hi[1], hi[2]]]),
            ([-1.0, 0.0, 0.0], [[lo[0], lo[1], lo[2]], [lo[0], lo[1], hi[2]], [lo[0], hi[1], hi[2]], [lo[0], hi[1], lo[2]]]),
            ([0.0, 1.0, 0.0], [[lo[0], hi[1], hi[2]], [hi[0], hi[1], hi[2]], [hi[0], hi[1], lo[2]], [lo[0], hi[1], lo[2]]]),
            ([0.0, -1.0, 0.0], [[lo[0], lo[1], lo[2]], [hi[0], lo[1], lo[2]], [hi[0], lo[1], hi[2]], [lo[0], lo[1], hi[2]]]),
            ([0.0, 0.0, 1.0], [[lo[0], lo[1], hi[2]], [hi[0], lo[1], hi[2]], [hi[0], hi[1], hi[2]], [lo[0], hi[1], hi[2]]]),
            ([0.0, 0.0, -1.0], [[hi[0], lo[1], lo[2]], [lo[0], lo[1], lo[2]], [lo[0], hi[1], lo[2]], [hi[0], hi[1], lo[2]]]),
        ];
        for (n, corners) in faces {
            let base = (pos.len() / 3) as u32;
            for c in corners {
                pos.extend_from_slice(&c);
                nrm.extend_from_slice(&n);
                uv.extend_from_slice(&[u, 0.5]);
                joints.extend_from_slice(&[joint, 0, 0, 0]);
                weights.extend_from_slice(&[1.0, 0.0, 0.0, 0.0]);
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in pos.chunks(3) {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }

    let mut b = Builder::default();
    let a_pos = b.floats(&pos, 3, "VEC3", &format!(r#","min":[{}],"max":[{}]"#, nums(&min), nums(&max)));
    let a_nrm = b.floats(&nrm, 3, "VEC3", "");
    let a_uv = b.floats(&uv, 2, "VEC2", "");
    let joint_bytes: Vec<u8> = joints.iter().flat_map(|j| j.to_le_bytes()).collect();
    let a_joints = b.accessor(&joint_bytes, 5123, joints.len() / 4, "VEC4", "");
    let a_weights = b.floats(&weights, 4, "VEC4", "");
    let idx_bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
    let a_idx = b.accessor(&idx_bytes, 5125, idx.len(), "SCALAR", "");

    // Inverse bind matrices: each joint's bind pose is a translation
    // by its world position, so the inverse translates back.
    let world = |j: usize| {
        let (_, parent, t) = JOINTS[j];
        let p = parent.map_or([0.0; 3], |p| JOINTS[p].2);
        [t[0] + p[0], t[1] + p[1], t[2] + p[2]]
    };
    let mut ibm = Vec::new();
    for j in 0..JOINTS.len() {
        let w = world(j);
        ibm.extend_from_slice(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        ibm.extend_from_slice(&[-w[0], -w[1], -w[2], 1.0]);
    }
    let a_ibm = b.floats(&ibm, 16, "MAT4", "");

    // Animations. Joint j is node j + 1 (node 0 is the mesh).
    let mut anims = Vec::new();
    for clip in &CLIPS {
        let t_last = *clip.times.last().unwrap();
        let a_t = b.floats(clip.times, 1, "SCALAR", &format!(r#","min":[0],"max":[{t_last}]"#));
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        let lift: Vec<f32> = clip.lift.iter().flat_map(|y| [0.0, *y, 0.0]).collect();
        let a_lift = b.floats(&lift, 3, "VEC3", "");
        samplers.push(format!(r#"{{"input":{a_t},"output":{a_lift},"interpolation":"LINEAR"}}"#));
        channels.push(r#"{"sampler":0,"target":{"node":1,"path":"translation"}}"#.to_string());
        for (k, swing) in clip.swing.iter().enumerate() {
            let rot: Vec<f32> = swing.iter().flat_map(|d| quat_x(*d)).collect();
            let a_rot = b.floats(&rot, 4, "VEC4", "");
            samplers.push(format!(r#"{{"input":{a_t},"output":{a_rot},"interpolation":"LINEAR"}}"#));
            channels.push(format!(
                r#"{{"sampler":{},"target":{{"node":{},"path":"rotation"}}}}"#,
                k + 1,
                k + 2
            ));
        }
        anims.push(format!(
            r#"{{"name":"{}","samplers":[{}],"channels":[{}]}}"#,
            clip.name,
            samplers.join(","),
            channels.join(",")
        ));
    }

    let png = palette_png();
    let img_view = b.view(&png);

    let mut nodes = vec![r#"{"name":"hero","mesh":0,"skin":0}"#.to_string()];
    for (j, (name, _, t)) in JOINTS.iter().enumerate() {
        let children: Vec<String> = JOINTS
            .iter()
            .enumerate()
            .filter(|(_, (_, p, _))| *p == Some(j))
            .map(|(c, _)| (c + 1).to_string())
            .collect();
        let mut node = format!(r#"{{"name":"{name}","translation":[{}]"#, nums(t));
        if !children.is_empty() {
            let _ = write!(node, r#","children":[{}]"#, children.join(","));
        }
        node.push('}');
        nodes.push(node);
    }
    let joint_nodes: Vec<String> = (1..=JOINTS.len()).map(|n| n.to_string()).collect();

    let json = format!(
        concat!(
            r#"{{"asset":{{"version":"2.0","generator":"twe tests/gen_survive3d_hero.rs"}},"#,
            r#""scene":0,"scenes":[{{"nodes":[0,1]}}],"#,
            r#""nodes":[{nodes}],"#,
            r#""meshes":[{{"name":"hero","primitives":[{{"attributes":{{"POSITION":{p},"NORMAL":{n},"TEXCOORD_0":{uv},"JOINTS_0":{j},"WEIGHTS_0":{w}}},"indices":{i},"material":0}}]}}],"#,
            r#""skins":[{{"joints":[{jn}],"inverseBindMatrices":{ibm},"skeleton":1}}],"#,
            r#""animations":[{anims}],"#,
            r#""materials":[{{"name":"palette","pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}},"metallicFactor":0,"roughnessFactor":1}}}}],"#,
            r#""textures":[{{"source":0,"sampler":0}}],"#,
            r#""samplers":[{{"magFilter":9728,"minFilter":9984}}],"#,
            r#""images":[{{"bufferView":{img},"mimeType":"image/png"}}],"#,
            r#""buffers":[{{"byteLength":{len}}}],"#,
            r#""bufferViews":[{views}],"#,
            r#""accessors":[{accessors}]}}"#
        ),
        nodes = nodes.join(","),
        p = a_pos,
        n = a_nrm,
        uv = a_uv,
        j = a_joints,
        w = a_weights,
        i = a_idx,
        jn = joint_nodes.join(","),
        ibm = a_ibm,
        anims = anims.join(","),
        img = img_view,
        len = b.bin.len().next_multiple_of(4),
        views = b.views.join(","),
        accessors = b.accessors.join(","),
    );
    glb(&json, &b.bin)
}

/// Pack JSON + binary chunks into a GLB container.
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

/// The palette as an RGB PNG, CELL px per colour, deflate "stored"
/// blocks (no compressor needed; it is 3 KB).
fn palette_png() -> Vec<u8> {
    let (w, h) = (CELL * PALETTE.len(), CELL);
    // Every row is the same: filter byte "none", then the pixels.
    let mut row = vec![0u8];
    for x in 0..w {
        row.extend_from_slice(&PALETTE[x / CELL]);
    }
    let raw = row.repeat(h);
    // zlib: header, stored blocks of up to 65535 bytes, Adler-32.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(&b"IHDR"[..], &ihdr[..]), (b"IDAT", &z), (b"IEND", &[])] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(data);
        png.extend_from_slice(&crc_input);
        png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
    png
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn survive3d_hero_exists_and_loads() {
    let path = Path::new("examples/survive3d/assets/hero.glb");
    if !path.exists() {
        std::fs::write(path, build()).expect("write hero.glb");
    }
    let bytes = std::fs::read(path).expect("read hero.glb");
    assert_eq!(bytes, build(), "hero.glb differs from its recipe");

    // It parses as glTF, with the skin, both clips and the texture.
    let (doc, _, images) = gltf::import_slice(&bytes).expect("valid glTF");
    assert_eq!(doc.skins().count(), 1);
    let names: Vec<_> = doc.animations().filter_map(|a| a.name().map(str::to_string)).collect();
    assert_eq!(names, ["walk", "idle"]);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].width as usize, CELL * PALETTE.len());
    // And the engine's loader takes it.
    twec::kernel::render::parse_glb_bytes(&bytes).expect("engine loads hero.glb");
}
