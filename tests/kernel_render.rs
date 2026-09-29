//! web3d-M2: headless renderer tests.
//!
//! Drives the real 3D path — a Twe script's `on update` / `on render`,
//! the snapshot `play3d` hands the kernel, and `kernel::render::Renderer`
//! — into an offscreen texture, then checks the pixels. Before this,
//! nothing rendered in tests: `twec play3d` crashed on its first frame
//! for four months (2026-06-01 → web3d-M2) without a test noticing.
//!
//! Needs a GPU adapter (hardware, or a software one such as WARP /
//! lavapipe). Without one the tests print a notice and pass, so CI
//! machines without a GPU don't fail spuriously.
//!
//! Each run writes `target/kernel-render/<name>.png` for eyeballing.

use twec::kernel::render::Renderer;

/// The asset root is process-global: tests that set it hold this lock
/// for the rest of the test, so parallel tests don't swap it underneath.
static ASSET_ROOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

const W: u32 = 320;
const H: u32 = 240;

fn headless() -> Option<Renderer> {
    match pollster::block_on(Renderer::new_headless(W, H)) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping headless render test: {e}");
            None
        }
    }
}

/// Load `path`, run its top level, tick `frames` frames, then render
/// one frame headless and return the RGBA8 pixels.
fn render_script(renderer: &mut Renderer, path: &str, frames: u32) -> Vec<u8> {
    render_script_with(renderer, path, frames, |_| {})
}

/// `render_script`, with `setup` run on the env after the top level.
fn render_script_with(
    renderer: &mut Renderer,
    path: &str,
    frames: u32,
    setup: impl FnOnce(&mut twec::value::Env),
) -> Vec<u8> {
    let src = std::fs::read_to_string(path).expect("read script");
    let tokens = twec::lexer::lex(&src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    setup(&mut env);
    for _ in 0..frames {
        twec::eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
    }
    let mut assets = twec::play3d::NativeAssets::default();
    twec::host3d::render_frame(renderer, &mut env, &mut assets).expect("render");
    // Meshes load on worker threads; draw until they have arrived.
    for _ in 0..200 {
        if assets.pending() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        twec::host3d::render_frame(renderer, &mut env, &mut assets).expect("render");
    }
    twec::host3d::render_frame(renderer, &mut env, &mut assets).expect("render");
    renderer.read_pixels().expect("read pixels")
}

/// Render a script given as source (written to a scratch file).
fn render_source(renderer: &mut Renderer, name: &str, src: &str) -> Vec<u8> {
    let dir = std::path::Path::new("target/kernel-render");
    std::fs::create_dir_all(dir).expect("create output dir");
    let path = dir.join(format!("{name}.twe"));
    std::fs::write(&path, src).expect("write script");
    render_script(renderer, path.to_str().unwrap(), 1)
}

fn save_png(name: &str, rgba: &[u8]) {
    let dir = std::path::Path::new("target/kernel-render");
    std::fs::create_dir_all(dir).expect("create output dir");
    image::save_buffer(
        dir.join(format!("{name}.png")),
        rgba,
        W,
        H,
        image::ColorType::Rgba8,
    )
    .expect("write png");
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * W + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

#[test]
fn hello_3d_renders_its_scene() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let rgba = render_script(&mut renderer, "examples/hello_3d.twe", 30);
    save_png("hello_3d", &rgba);
    assert_eq!(rgba.len(), (W * H * 4) as usize);

    // A corner is empty sky: the clear colour after tonemapping.
    let sky = pixel(&rgba, 2, 2);
    // The white centre sphere sits in the middle of the frame.
    let centre = pixel(&rgba, W / 2, H / 2);
    let brightness = |p: [u8; 3]| p.iter().map(|&c| u32::from(c)).sum::<u32>();
    assert!(
        // (ACES tonemapping compresses the white sphere to a light
        // grey, so the margin is modest but unambiguous.)
        brightness(centre) > brightness(sky) + 100,
        "expected a lit sphere at the centre: centre={centre:?} sky={sky:?}"
    );
    // The scene draws something other than the background in a good
    // share of the frame (the sphere plus the cube ring).
    let differing = (0..H)
        .flat_map(|y| (0..W).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&rgba, x, y) != sky)
        .count();
    assert!(
        differing > (W * H / 50) as usize,
        "only {differing} pixels differ from the sky"
    );
}

/// web3d-M3: a look's `facing` turns the mesh about +Y. A square cube
/// seen from above changes its screen footprint when turned 45 degrees,
/// and a quarter turn maps it back onto itself.
#[test]
fn look_facing_rotates_the_mesh() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let scene = |yaw: &str| {
        format!(
            "camera.eye = vec3(0, 6, 0.001)\ncamera.target = vec3(0, 0, 0)\n\
             entity Box:\n    var pos = vec3(0, 0, 0)\n    look:\n        mesh: \"cube\"\n\
             \x20       scale: 2.0\n        facing: {yaw}\nspawn Box\n"
        )
    };
    let flat = render_source(&mut renderer, "facing_0", &scene("0.0"));
    let turned = render_source(&mut renderer, "facing_45", &scene("0.7853982"));
    let quarter = render_source(&mut renderer, "facing_90", &scene("1.5707964"));
    save_png("facing_45", &turned);
    let differing = |a: &[u8], b: &[u8]| {
        a.chunks(4)
            .zip(b.chunks(4))
            .filter(|(p, q)| p[..3].iter().zip(&q[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
            .count()
    };
    let total = (W * H) as usize;
    assert!(
        differing(&flat, &turned) > total / 100,
        "45 degrees changed too little"
    );
    assert!(
        differing(&flat, &quarter) < total / 200,
        "a quarter turn of a cube should look the same"
    );
}

/// web3d-M3: a look's `material:` paints the mesh with a visual block,
/// and the visual's alpha cuts the mesh out below 0.5.
#[test]
fn look_material_paints_and_cuts_out() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let scene = |pixel: &str| {
        format!(
            "camera.eye = vec3(0, 6, 0.001)\ncamera.target = vec3(0, 0, 0)\n\
             visual Paint:\n    pixel(uv, time) -> color:\n        return {pixel}\n\
             entity Box:\n    var pos = vec3(0, 0, 0)\n    look:\n        scale: 2.0\n\
             \x20       material: Paint\nspawn Box\n"
        )
    };
    let green = render_source(&mut renderer, "material_green", &scene("color.green"));
    save_png("material_green", &green);
    let [r, g, b] = pixel(&green, W / 2, H / 2);
    assert!(
        g > r + 60 && g > b + 60,
        "centre should be green, got {:?}",
        [r, g, b]
    );

    let cut = render_source(
        &mut renderer,
        "material_cutout",
        &scene("color.transparent"),
    );
    let sky = pixel(&cut, 2, 2);
    assert_eq!(
        pixel(&cut, W / 2, H / 2),
        sky,
        "a fully transparent material cuts the mesh out"
    );
}

/// web3d-M3: `text()` / `rect()` in a 3D render draw a HUD over the
/// scene, in the 2D runtime's 640×480 canvas coordinates.
#[test]
fn hud_draws_text_and_rects_over_the_scene() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let src = concat!(
        "on render():\n",
        "    rect((0, 0), (320, 240), color.red)\n",
        "    text(\"TWE\", (340, 400), 120, color.green)\n",
    );
    let rgba = render_source(&mut renderer, "hud", src);
    save_png("hud", &rgba);
    // The rect covers the top-left quarter of the target (160×120 of
    // the 320×240 test target) in the colour it was given.
    let [r, g, b] = pixel(&rgba, 40, 30);
    assert!(
        r > 240 && g < 20 && b < 20,
        "rect should be red, got {:?}",
        [r, g, b]
    );
    // Somewhere in the text's box there is green ink.
    let green = (170..310)
        .flat_map(|x| (140..205).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            let [r, g, b] = pixel(&rgba, x, y);
            g > 200 && r < 60 && b < 60
        })
        .count();
    assert!(green > 100, "only {green} green text pixels");
}

/// web3d-M4: a frame of the v1.0 slice, 20 s into a run, through the
/// real renderer: the arena, the swarm and the HUD all draw.
#[test]
fn survive3d_frame_renders() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let _root = ASSET_ROOT.lock().unwrap_or_else(|e| e.into_inner());
    twec::bundle::set_asset_root(Some("examples/survive3d".into()));
    let rgba = render_script(&mut renderer, "examples/survive3d/main.twe", 1200);
    save_png("survive3d", &rgba);
    // Not a blank frame: many distinct colours on screen.
    let mut colours = std::collections::HashSet::new();
    for px in rgba.chunks(4) {
        colours.insert([px[0] / 16, px[1] / 16, px[2] / 16]);
    }
    assert!(colours.len() > 20, "only {} colours", colours.len());
}

/// web3d-M4: the level-up picker (cards, highlight, dimmed world).
#[test]
fn survive3d_level_up_renders() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let _root = ASSET_ROOT.lock().unwrap_or_else(|e| e.into_inner());
    twec::bundle::set_asset_root(Some("examples/survive3d".into()));
    let rgba = render_script_with(&mut renderer, "examples/survive3d/main.twe", 2, |env| {
        env.set("xp".to_string(), twec::value::Value::from_int(50));
    });
    save_png("survive3d_level_up", &rgba);
    // The highlighted card's yellow frame is on screen.
    let yellow = rgba
        .chunks(4)
        .filter(|p| p[0] > 200 && p[1] > 200 && p[2] < 80)
        .count();
    assert!(yellow > 150, "only {yellow} yellow pixels");
}

/// web3d-M4: the survive3d hero close up, mid-stride: skinning, the
/// `walk` clip (by mesh path) and the palette texture all reach the
/// frame.
#[test]
fn hero_renders_mid_stride() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let _root = ASSET_ROOT.lock().unwrap_or_else(|e| e.into_inner());
    twec::bundle::set_asset_root(Some("examples/survive3d".into()));
    let src = r#"
camera.eye = vec3(1.2, 1.3, 3.0)
camera.target = vec3(0, 0.8, 0)
mesh_anim.play("assets/hero.glb", "walk", true)
mesh_anim.advance(0.2)
on render():
    mesh("assets/hero.glb", at: vec3(0, 0, 0), color: (1, 1, 1, 1), size: 1.0)
"#;
    let rgba = render_source(&mut renderer, "hero_walk", src);
    save_png("hero_walk", &rgba);
    // The shirt's blue and the skin tone are both on screen.
    let count = |f: &dyn Fn(i32, i32, i32) -> bool| {
        rgba.chunks(4)
            .filter(|p| f(i32::from(p[0]), i32::from(p[1]), i32::from(p[2])))
            .count()
    };
    // (The clear colour is a duller blue: b ≈ 140.)
    let blue = count(&|r, _, b| b > 170 && r < 90);
    let skin = count(&|r, g, b| r > 150 && g > 100 && r > b + 40);
    assert!(blue > 300, "shirt pixels: {blue}");
    assert!(skin > 100, "skin pixels: {skin}");
}

/// Pack a glTF JSON (with `{bin_len}` for the buffer length) and a
/// binary chunk into a .glb.
fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut j = json.replace("{bin_len}", &bin.len().to_string()).into_bytes();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let total = (12 + 8 + j.len() + 8 + b.len()) as u32;
    let mut out = b"glTF".to_vec();
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&j);
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&b);
    out
}

/// web3d-M7: a glTF model's primitives draw with their own materials:
/// the left quad is a red dielectric, the right one black but emissive
/// blue (strength 2), so it glows whatever the lighting.
#[test]
fn gltf_primitives_draw_with_their_own_materials() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let quad = |x0: f32, x1: f32| -> Vec<f32> {
        vec![x0, -0.6, 0.0, x1, -0.6, 0.0, x1, 0.6, 0.0, x0, 0.6, 0.0]
    };
    let mut positions = quad(-1.3, -0.1);
    positions.extend(quad(0.1, 1.3));
    let normals: Vec<f32> = [0.0f32, 0.0, 1.0].repeat(8);
    let indices: [u16; 12] = [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
    let mut bin: Vec<u8> = positions.iter().flat_map(|f| f.to_le_bytes()).collect();
    bin.extend(normals.iter().flat_map(|f| f.to_le_bytes()));
    bin.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
    let json = r#"{"asset":{"version":"2.0"},"extensionsUsed":["KHR_materials_emissive_strength"],
        "scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],
        "meshes":[{"primitives":[
            {"attributes":{"POSITION":0,"NORMAL":1},"indices":2,"material":0},
            {"attributes":{"POSITION":0,"NORMAL":1},"indices":3,"material":1}]}],
        "materials":[
            {"pbrMetallicRoughness":{"baseColorFactor":[0.8,0.05,0.05,1],"metallicFactor":0,"roughnessFactor":0.9}},
            {"pbrMetallicRoughness":{"baseColorFactor":[0,0,0,1],"metallicFactor":0},
             "emissiveFactor":[0.1,0.2,1],"extensions":{"KHR_materials_emissive_strength":{"emissiveStrength":2}}}],
        "accessors":[
            {"bufferView":0,"componentType":5126,"count":8,"type":"VEC3","min":[-1.3,-0.6,0],"max":[1.3,0.6,0]},
            {"bufferView":1,"componentType":5126,"count":8,"type":"VEC3"},
            {"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"},
            {"bufferView":2,"byteOffset":12,"componentType":5123,"count":6,"type":"SCALAR"}],
        "bufferViews":[
            {"buffer":0,"byteOffset":0,"byteLength":96},
            {"buffer":0,"byteOffset":96,"byteLength":96},
            {"buffer":0,"byteOffset":192,"byteLength":24}],
        "buffers":[{"byteLength":{bin_len}}]}"#;
    let dir = std::path::Path::new("target/kernel-render");
    std::fs::create_dir_all(dir).expect("create output dir");
    std::fs::write(dir.join("two_materials.glb"), glb(json, &bin)).expect("write glb");
    let _root = ASSET_ROOT.lock().unwrap_or_else(|e| e.into_inner());
    twec::bundle::set_asset_root(Some(dir.into()));
    let src = r#"
camera.eye = vec3(0, 0, 3)
camera.target = vec3(0, 0, 0)
on render():
    mesh("two_materials.glb", at: vec3(0, 0, 0), color: (1, 1, 1, 1), size: 1.0)
"#;
    let rgba = render_source(&mut renderer, "two_materials", src);
    save_png("two_materials", &rgba);
    let [lr, lg, lb] = pixel(&rgba, W / 4, H / 2);
    let [rr, rg, rb] = pixel(&rgba, 3 * W / 4, H / 2);
    assert!(lr > lg + 40 && lr > lb + 40, "left quad red: {:?}", [lr, lg, lb]);
    assert!(rb > rr + 60 && rb > 150, "right quad glows blue: {:?}", [rr, rg, rb]);
}

/// A Radiance .hdr (flat RGBE scanlines) of `w × h`, colour per row.
fn hdr_file(w: usize, h: usize, row_color: impl Fn(usize) -> [f32; 3]) -> Vec<u8> {
    let mut out = format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {h} +X {w}\n").into_bytes();
    for y in 0..h {
        let c = row_color(y);
        let max = c[0].max(c[1]).max(c[2]);
        let texel = if max < 1e-32 {
            [0, 0, 0, 0]
        } else {
            let e = max.log2().floor() as i32 + 1;
            let scale = 256.0 / 2f32.powi(e);
            [
                (c[0] * scale) as u8,
                (c[1] * scale) as u8,
                (c[2] * scale) as u8,
                (e + 128) as u8,
            ]
        };
        for _ in 0..w {
            out.extend_from_slice(&texel);
        }
    }
    out
}

/// web3d-M7: image-based lighting. A sphere lit only by an environment
/// map (bright blue sky above, dark ground below) is bright and blue
/// on top and dark underneath, and the environment is the backdrop.
#[test]
fn environment_lights_and_backs_the_scene() {
    use twec::kernel::render::{
        Camera3d, EnvironmentSettings, PostFx, RenderSnapshot, ShadowSettings,
    };
    use twec::render3d_types::{DrawCall3d, Primitive};
    let Some(mut renderer) = headless() else {
        return;
    };
    let dir = std::path::Path::new("target/kernel-render");
    std::fs::create_dir_all(dir).expect("create output dir");
    std::fs::write(
        dir.join("sky.hdr"),
        hdr_file(64, 32, |y| if y < 16 { [0.4, 0.7, 2.0] } else { [0.02, 0.02, 0.02] }),
    )
    .expect("write hdr");
    let _root = ASSET_ROOT.lock().unwrap_or_else(|e| e.into_inner());
    twec::bundle::set_asset_root(Some(dir.into()));

    let draws = [DrawCall3d {
        primitive: Primitive::Sphere,
        at: [0.0, 0.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
        size: 1.6,
        texture: 0,
        yaw: 0.0,
        material: 0,
    }];
    let anim = |_: u32| Default::default();
    let mut assets = twec::play3d::NativeAssets::default();
    for _ in 0..3 {
        let snap = RenderSnapshot {
            camera: Camera3d::new([0.0, 0.0, 3.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            environment: Some(EnvironmentSettings {
                path: "sky.hdr",
                intensity: 1.0,
                backdrop: true,
            }),
            background: [0.0; 3],
            lights: bytemuck::Zeroable::zeroed(),
            shadow: ShadowSettings {
                enabled: false,
                extent: 10.0,
            },
            post: PostFx {
                tonemap_aces: true,
                taa: false,
                vignette: 0.0,
                vignette_color: [0.0; 3],
                bloom_intensity: 0.0,
                bloom_threshold: 1.0,
                frustum_cull: false,
            },
            draws: &draws,
            mesh_paths: &[],
            texture_paths: &[],
            time: 0.0,
            materials: &[],
            hud: &[],
            anim: &anim,
        };
        renderer.render(&snap, &mut assets).expect("render");
    }
    let rgba = renderer.read_pixels().expect("read pixels");
    save_png("environment", &rgba);
    let luma = |[r, g, b]: [u8; 3]| u32::from(r) + u32::from(g) + u32::from(b);
    let top = pixel(&rgba, W / 2, H / 2 - 50);
    let bottom = pixel(&rgba, W / 2, H / 2 + 50);
    assert!(luma(top) > luma(bottom) + 150, "sky-lit top {top:?} vs ground-lit bottom {bottom:?}");
    assert!(top[2] > top[0], "the sky is blue: {top:?}");
    let sky = pixel(&rgba, W / 2, 5);
    assert!(sky[2] > 150, "the backdrop shows the sky: {sky:?}");
}

/// web3d-M7: temporal anti-aliasing. With `postfx.taa(true)` a still
/// scene converges: the picture is the scene (close to the MSAA-only
/// image, not black or smeared) and consecutive frames barely differ.
#[test]
fn taa_converges_on_a_still_scene() {
    let src = r#"
camera.eye = vec3(3, 2.5, 4)
camera.target = vec3(0, 0, 0)
on render():
    cube(at: vec3(0, 0, 0), color: (0.9, 0.3, 0.2, 1), size: 1.2)
    sphere(at: vec3(1.4, 0.2, -0.6), color: (0.2, 0.5, 0.95, 1), size: 0.8)
"#;
    let frames = |taa: bool, count: usize| -> Vec<Vec<u8>> {
        let mut renderer = headless().expect("gpu");
        let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
        let mut env = twec::value::Env::new();
        twec::stdlib::install(&mut env);
        twec::eval::run_top_level(&mut env, &program).expect("top level");
        let toggle = format!("postfx.taa({taa})\n");
        let toggle = twec::parser::parse(&twec::lexer::lex(&toggle).expect("lex")).expect("parse");
        twec::eval::run_top_level(&mut env, &toggle).expect("toggle");
        let mut assets = twec::play3d::NativeAssets::default();
        (0..count)
            .map(|_| {
                twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).expect("render");
                renderer.read_pixels().expect("read pixels")
            })
            .collect()
    };
    if headless().is_none() {
        return;
    }
    let mean_diff = |a: &[u8], b: &[u8]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from(x.abs_diff(*y)))
            .sum::<f64>()
            / a.len() as f64
    };
    let taa = frames(true, 24);
    let msaa = frames(false, 1).pop().unwrap();
    let last = &taa[23];
    save_png("taa", last);
    let settle = mean_diff(&taa[22], last);
    assert!(settle < 1.0, "converged frames still differ by {settle:.2}");
    let versus_msaa = mean_diff(last, &msaa);
    assert!(
        versus_msaa < 4.0,
        "TAA drifted from the scene: mean difference {versus_msaa:.2} from MSAA"
    );
    // Not a blank frame.
    let lit = last.chunks(4).filter(|p| p[0] > 150).count();
    assert!(lit > 500, "only {lit} bright pixels");
}

/// web3d-M7: TAA under camera motion. The camera orbits for 24 frames;
/// reprojection keeps the history attached to the scene, so the final
/// TAA frame stays close to a plain frame from the same camera (a
/// wrong reprojection smears ghosts across the image).
#[test]
fn taa_follows_a_moving_camera() {
    if headless().is_none() {
        return;
    }
    let scene = |taa: bool| {
        format!(
            r#"
postfx.taa({taa})
var t = 0.0
on render():
    t += 0.05
    camera.eye = vec3(4 * math.sin(t), 2.5, 4 * math.cos(t))
    camera.target = vec3(0, 0, 0)
    cube(at: vec3(0, 0, 0), color: (0.9, 0.3, 0.2, 1), size: 1.2)
    sphere(at: vec3(1.4, 0.2, -0.6), color: (0.2, 0.5, 0.95, 1), size: 0.8)
"#
        )
    };
    let last_frame = |src: &str, frames: usize| {
        let mut renderer = headless().expect("gpu");
        let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
        let mut env = twec::value::Env::new();
        twec::stdlib::install(&mut env);
        twec::eval::run_top_level(&mut env, &program).expect("top level");
        let mut assets = twec::play3d::NativeAssets::default();
        for _ in 0..frames {
            twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).expect("render");
        }
        renderer.read_pixels().expect("read pixels")
    };
    let taa = last_frame(&scene(true), 24);
    let plain = last_frame(&scene(false), 24);
    save_png("taa_moving", &taa);
    let diff = taa
        .iter()
        .zip(&plain)
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / taa.len() as f64;
    assert!(diff < 4.0, "TAA under camera motion is {diff:.2} away from the plain frame");
}

/// Render `src` once headless (after its top level), for the shadow
/// tests' on / off comparisons.
fn render_once(src: &str) -> Vec<u8> {
    let mut renderer = headless().expect("gpu");
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    let mut assets = twec::play3d::NativeAssets::default();
    twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).expect("render");
    renderer.read_pixels().expect("read pixels")
}

/// Pixels (of W × H) whose brightness drops by more than `by` from
/// `lit` to `shadowed`, and those that brighten by more than `by`.
fn darkened(lit: &[u8], shadowed: &[u8], by: i32) -> (usize, usize) {
    let luma = |p: &[u8]| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]);
    let (mut darker, mut brighter) = (0, 0);
    for (a, b) in lit.chunks(4).zip(shadowed.chunks(4)) {
        let d = luma(a) - luma(b);
        if d > by {
            darker += 1;
        } else if d < -by {
            brighter += 1;
        }
    }
    (darker, brighter)
}

/// web3d-M7: `light.shadow(h, true)` makes a point light cast shadows
/// (cube shadow maps): a block beside the light darkens the floor
/// behind it, and nothing gets brighter.
#[test]
fn point_lights_cast_shadows() {
    if headless().is_none() {
        return;
    }
    let scene = |shadow: bool| {
        format!(
            r#"
light.clear()
sun.intensity(0.0)
light.ambient((0.02, 0.02, 0.02, 1.0))
let lamp = light.add((1.5, 1.2, 0.0), (1.0, 0.9, 0.7, 1.0), 12.0)
light.shadow(lamp, {shadow})
camera.eye = vec3(0, 6, 7)
camera.target = vec3(0, -0.5, 0)
on render():
    cube(at: vec3(0, -5.5, 0), color: (0.8, 0.8, 0.8, 1), size: 10.0)
    cube(at: vec3(0, 0.0, 0), color: (0.9, 0.3, 0.2, 1), size: 1.0)
"#
        )
    };
    let lit = render_once(&scene(false));
    let shadowed = render_once(&scene(true));
    save_png("point_shadow", &shadowed);
    let (darker, brighter) = darkened(&lit, &shadowed, 30);
    assert!(darker > 1500, "only {darker} pixels fell into shadow");
    assert!(brighter < 50, "{brighter} pixels got brighter");
}

/// web3d-M7: the sun's cascaded shadows, fitted to the view and soft
/// (PCSS): a block 4 m up shadows the ground, and the shadow has a
/// penumbra (pixels partly darkened) rather than a hard edge.
#[test]
fn sun_shadows_are_soft() {
    if headless().is_none() {
        return;
    }
    let scene = |shadow: bool| {
        format!(
            r#"
light.clear()
sun.direction(vec3(0.5, 1.0, 0.2))
sun.intensity(1.0)
light.ambient((0.05, 0.05, 0.05, 1.0))
sun.shadow({shadow})
sun.shadow_extent(10.0)
camera.eye = vec3(0, 6, 7)
camera.target = vec3(0, -0.5, 0)
on render():
    cube(at: vec3(0, -5.5, 0), color: (0.8, 0.8, 0.8, 1), size: 10.0)
    cube(at: vec3(0, 4.0, 0), color: (0.9, 0.3, 0.2, 1), size: 1.0)
"#
        )
    };
    let lit = render_once(&scene(false));
    let shadowed = render_once(&scene(true));
    save_png("sun_shadow", &shadowed);
    let (hard, brighter) = darkened(&lit, &shadowed, 60);
    let (any, _) = darkened(&lit, &shadowed, 8);
    assert!(hard > 300, "only {hard} pixels in shadow");
    assert!(brighter < 50, "{brighter} pixels got brighter");
    // Soft edge: a band of partly darkened pixels around the umbra.
    assert!(any - hard > 150, "penumbra of only {} pixels", any - hard);
}
