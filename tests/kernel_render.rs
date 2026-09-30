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
        Camera3d, EnvironmentSettings, PostFx, RenderSnapshot, ShadowSettings, Tonemapper,
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
            lut: None,
            point_lights: &[],
            fog: None,
            particles: Default::default(),
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
                tonemapper: Tonemapper::Aces,
                taa: false,
                vignette: 0.0,
                vignette_color: [0.0; 3],
                bloom_intensity: 0.0,
                bloom_threshold: 1.0,
                frustum_cull: false,
                ..PostFx::default()
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

/// web3d-M7 session 7: render `frames` frames of an empty scene over a
/// flat `background` (linear HDR) with `post`, time advancing 1/60 s a
/// frame from `t0`, and return the centre pixel of the last one.
fn backdrop_pixel(
    renderer: &mut Renderer,
    background: [f32; 3],
    post: twec::kernel::render::PostFx,
    frames: u32,
    t0: f32,
) -> [u8; 3] {
    use twec::kernel::render::{Camera3d, RenderSnapshot, ShadowSettings};
    let anim = |_: u32| Default::default();
    let mut assets = twec::play3d::NativeAssets::default();
    for i in 0..frames {
        let snap = RenderSnapshot {
            lut: None,
            point_lights: &[],
            fog: None,
            particles: Default::default(),
            camera: Camera3d::new([0.0, 0.0, 3.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            environment: None,
            background,
            lights: bytemuck::Zeroable::zeroed(),
            shadow: ShadowSettings {
                enabled: false,
                extent: 10.0,
            },
            post,
            draws: &[],
            mesh_paths: &[],
            texture_paths: &[],
            time: t0 + i as f32 / 60.0,
            materials: &[],
            hud: &[],
            anim: &anim,
        };
        renderer.render(&snap, &mut assets).expect("render");
    }
    pixel(&renderer.read_pixels().expect("read pixels"), W / 2, H / 2)
}

fn srgb8(linear: f32) -> f32 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    s * 255.0
}

/// Column-major 3x3 (columns as in the WGSL) times a vector.
fn mat3(cols: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| cols[0][i] * v[0] + cols[1][i] * v[1] + cols[2][i] * v[2])
}

/// CPU references for the tonemap curves, from their published forms.
fn aces_ref(x: f32) -> f32 {
    let aces_in = [
        [0.59719, 0.07600, 0.02840],
        [0.35458, 0.90834, 0.13383],
        [0.04823, 0.01566, 0.83777],
    ];
    let aces_out = [
        [1.60475, -0.10208, -0.00327],
        [-0.53108, 1.10813, -0.07276],
        [-0.07367, -0.00605, 1.07602],
    ];
    let v = mat3(aces_in, [x / 0.6; 3]);
    let fit = v.map(|v| (v * (v + 0.024_578_6) - 0.000_090_537) / (v * (0.983_729 * v + 0.432_951) + 0.238_081));
    mat3(aces_out, fit)[1].clamp(0.0, 1.0)
}

fn agx_ref(x: f32) -> f32 {
    let to_2020 = [[0.6274, 0.0691, 0.0164], [0.3293, 0.9195, 0.0880], [0.0433, 0.0113, 0.8956]];
    let from_2020 = [[1.6605, -0.1246, -0.0182], [-0.5876, 1.1329, -0.1006], [-0.0728, -0.0083, 1.1187]];
    let inset = [
        [0.856_627_2, 0.137_319, 0.111_898_2],
        [0.095_121_24, 0.761_242, 0.076_799_42],
        [0.048_251_6, 0.101_439, 0.811_302_4],
    ];
    let outset = [
        [1.127_100_6, -0.141_329_76, -0.141_329_76],
        [-0.110_606_64, 1.157_823_7, -0.110_606_64],
        [-0.016_493_94, -0.016_493_94, 1.251_936_4],
    ];
    let (min_ev, max_ev) = (-12.473_93_f32, 4.026_069_f32);
    let c = mat3(inset, mat3(to_2020, [x; 3])).map(|c| {
        let t = ((c.max(1e-10).log2() - min_ev) / (max_ev - min_ev)).clamp(0.0, 1.0);
        let (t2, t4) = (t * t, t * t * t * t);
        15.5 * t4 * t2 - 40.14 * t4 * t + 31.96 * t4 - 6.868 * t2 * t + 0.4298 * t2 + 0.1191 * t - 0.00232
    });
    let c = mat3(outset, c).map(|c| c.max(0.0).powf(2.2));
    mat3(from_2020, c)[1].clamp(0.0, 1.0)
}

fn neutral_ref(x: f32) -> f32 {
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    let c = x - offset;
    let start = 0.76;
    if c < start {
        return c;
    }
    let d = 1.0 - start;
    // Grey stays grey: the desaturation mix toward white is a no-op.
    1.0 - d * d / (c + d - start)
}

/// web3d-M7: every tonemap curve matches its published form on a grey
/// ramp (checked against CPU references), exposure scales the frame by
/// 2^stops, and PBR Neutral leaves mid-tones alone (0.5 -> 0.46).
#[test]
fn tonemap_curves_and_exposure() {
    use twec::kernel::render::{PostFx, Tonemapper};
    let Some(mut renderer) = headless() else {
        return;
    };
    let clamp: fn(f32) -> f32 = |x| x.clamp(0.0, 1.0);
    for (curve, reference) in [
        (Tonemapper::None, clamp),
        (Tonemapper::Aces, aces_ref as fn(f32) -> f32),
        (Tonemapper::AgX, agx_ref),
        (Tonemapper::Neutral, neutral_ref),
    ] {
        for x in [0.02_f32, 0.18, 0.5, 2.0, 8.0] {
            let post = PostFx {
                tonemapper: curve,
                ..PostFx::default()
            };
            let got = backdrop_pixel(&mut renderer, [x; 3], post, 1, 0.0);
            let want = srgb8(reference(x));
            for c in got {
                assert!(
                    (f32::from(c) - want).abs() <= 2.0,
                    "{curve:?}({x}): got {got:?}, want {want:.1}"
                );
            }
        }
    }
    assert!((srgb8(neutral_ref(0.5)) - srgb8(0.46)).abs() < 0.01);
    // +1 stop doubles the light: 0.25 shows as 0.5.
    let post = PostFx {
        tonemapper: Tonemapper::None,
        exposure: 1.0,
        ..PostFx::default()
    };
    let got = backdrop_pixel(&mut renderer, [0.25; 3], post, 1, 0.0);
    assert!((f32::from(got[1]) - srgb8(0.5)).abs() <= 2.0, "exposure +1: {got:?}");
}

/// web3d-M7: auto exposure brings a dark and a bright scene to middle
/// grey, snapping on the first frame and easing after a change.
#[test]
fn auto_exposure_adapts() {
    use twec::kernel::render::{PostFx, Tonemapper};
    let Some(mut renderer) = headless() else {
        return;
    };
    let post = PostFx {
        tonemapper: Tonemapper::None,
        auto_exposure: true,
        ..PostFx::default()
    };
    let grey = srgb8(0.18);
    let dark = backdrop_pixel(&mut renderer, [0.02; 3], post, 1, 0.0);
    assert!((f32::from(dark[1]) - grey).abs() <= 4.0, "dark scene exposed to {dark:?}, want {grey:.0}");
    // The lights come on: the first frame is still exposed for the
    // dark (blown out); five seconds later it has adapted.
    let first = backdrop_pixel(&mut renderer, [2.0; 3], post, 1, 1.0 / 60.0);
    assert!(first[1] > 250, "adaptation eases, not snaps: {first:?}");
    let adapted = backdrop_pixel(&mut renderer, [2.0; 3], post, 300, 2.0 / 60.0);
    assert!((f32::from(adapted[1]) - grey).abs() <= 4.0, "bright scene exposed to {adapted:?}, want {grey:.0}");
}

/// web3d-M7: multi-level bloom spreads a bright light well beyond the
/// old 12-pixel inline kernel, and leaves the far frame alone.
#[test]
fn bloom_reaches_far() {
    if headless().is_none() {
        return;
    }
    let scene = |bloom: f32| {
        format!(
            r#"
light.clear()
sun.direction(vec3(0, 0, 1))
sun.intensity(40.0)
light.ambient((0.0, 0.0, 0.0, 1.0))
postfx.bloom({bloom})
postfx.bloom_threshold(1.0)
camera.eye = vec3(0, 0, 6)
camera.target = vec3(0, 0, 0)
on render():
    sphere(at: vec3(0, 0, 0), color: (1, 1, 1, 1), size: 0.3)
"#
        )
    };
    let off = render_once(&scene(0.0));
    let on = render_once(&scene(0.8));
    save_png("bloom", &on);
    let luma = |p: [u8; 3]| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]);
    // The sphere faces its light (it once drew inside-out, dark).
    assert!(luma(pixel(&off, W / 2, H / 2)) > 600, "lit sphere: {:?}", pixel(&off, W / 2, H / 2));
    for dx in [20, 40] {
        let (a, b) = (pixel(&off, W / 2 + dx, H / 2), pixel(&on, W / 2 + dx, H / 2));
        assert!(luma(b) > luma(a) + 6, "{dx} px out: off {a:?}, on {b:?}");
    }
    let corner = (pixel(&off, 5, 5), pixel(&on, 5, 5));
    assert!((luma(corner.0) - luma(corner.1)).abs() <= 6, "far corner: {corner:?}");
}

/// web3d-M7: GTAO darkens the floor where it meets a block, and leaves
/// open floor untouched (no self-occlusion on flat ground).
#[test]
fn ambient_occlusion_darkens_contact() {
    if headless().is_none() {
        return;
    }
    let scene = |ao: f32| {
        format!(
            r#"
light.clear()
sun.intensity(0.0)
light.ambient((0.8, 0.8, 0.8, 1.0))
postfx.ao({ao})
postfx.ao_radius(1.0)
camera.eye = vec3(0, 3, 5)
camera.target = vec3(0, 0, 0)
on render():
    cube(at: vec3(0, -5.5, 0), color: (0.8, 0.8, 0.8, 1), size: 10.0)
    cube(at: vec3(0, 0, 0), color: (0.8, 0.8, 0.8, 1), size: 1.0)
"#
        )
    };
    let off = render_once(&scene(0.0));
    let on = render_once(&scene(1.0));
    save_png("ambient_occlusion", &on);
    let (darker, brighter) = darkened(&off, &on, 8);
    assert!(darker > 500, "only {darker} pixels occluded");
    assert!(brighter < 20, "{brighter} pixels got brighter");
    // Open floor, far from the block.
    let luma = |p: [u8; 3]| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]);
    for (x, y) in [(30, 225), (290, 225), (160, 230)] {
        let (a, b) = (pixel(&off, x, y), pixel(&on, x, y));
        assert!((luma(a) - luma(b)).abs() <= 6, "open floor at ({x}, {y}): off {a:?}, on {b:?}");
    }
}

/// Render `frames` frames of `src` (ticking 1/60 s between them) and
/// return the last.
fn render_frames(src: &str, frames: u32) -> Vec<u8> {
    render_frames_env(src, frames).0
}

/// [`render_frames`], also returning the interpreter.
fn render_frames_env(src: &str, frames: u32) -> (Vec<u8>, twec::value::Env) {
    let mut renderer = headless().expect("gpu");
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    // As the 3D hosts do.
    env.gpu_particles = true;
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    let mut assets = twec::play3d::NativeAssets::default();
    for _ in 0..frames {
        twec::eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
        twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).expect("render");
    }
    (renderer.read_pixels().expect("read pixels"), env)
}

fn inverse_srgb8(v: f32) -> f32 {
    let c = v / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// web3d-M7: `postfx.lut` grades through a `.cube` LUT in display space:
/// an inverting LUT turns each channel v into 255 - v, and strength 0.5
/// mixes the two halfway (in linear light).
#[test]
fn color_lut_grades_the_frame() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let dir = std::path::Path::new("target/kernel-render");
    std::fs::create_dir_all(dir).expect("create output dir");
    let mut cube = String::from("TITLE \"invert\"\nLUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                cube.push_str(&format!("{} {} {}\n", 1 - r, 1 - g, 1 - b));
            }
        }
    }
    std::fs::write(dir.join("invert.cube"), cube).expect("write lut");
    let _root = ASSET_ROOT.lock().unwrap_or_else(|e| e.into_inner());
    twec::bundle::set_asset_root(Some(dir.into()));
    let scene = |strength: f32| {
        format!("light.clear()\npostfx.tonemap(\"none\")\npostfx.lut(\"invert.cube\", {strength})\n")
    };
    let plain = pixel(&render_source(&mut renderer, "lut_off", &scene(0.0)), W / 2, H / 2);
    let full = pixel(&render_source(&mut renderer, "lut_full", &scene(1.0)), W / 2, H / 2);
    let half = pixel(&render_source(&mut renderer, "lut_half", &scene(0.5)), W / 2, H / 2);
    twec::bundle::set_asset_root(None);
    for c in 0..3 {
        let v = f32::from(plain[c]);
        assert!((f32::from(full[c]) - (255.0 - v)).abs() <= 3.0, "inverted {full:?} vs plain {plain:?}");
        let mixed = 0.5 * inverse_srgb8(v) + 0.5 * inverse_srgb8(255.0 - v);
        assert!((f32::from(half[c]) - srgb8(mixed)).abs() <= 3.0, "half {half:?} vs plain {plain:?}");
    }
}

/// web3d-M7: depth of field blurs what's out of focus and leaves the
/// focused object sharp.
#[test]
fn depth_of_field_blurs_out_of_focus() {
    if headless().is_none() {
        return;
    }
    let scene = |f_stop: f32| {
        format!(
            r#"
light.clear()
sun.intensity(0.0)
light.ambient((1.0, 1.0, 1.0, 1.0))
postfx.dof(5, {f_stop})
camera.eye = vec3(0, 0, 5)
camera.target = vec3(0, 0, 0)
on render():
    cube(at: vec3(-1, 0, 0), color: (1, 1, 1, 1), size: 1.0)
    cube(at: vec3(3, 0, -20), color: (1, 1, 1, 1), size: 4.0)
"#
        )
    };
    let sharp = render_once(&scene(0.0));
    let blurred = render_once(&scene(0.1));
    save_png("depth_of_field", &blurred);
    save_png("depth_of_field_sharp", &sharp);
    let luma = |p: [u8; 3]| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]);
    let (mut near, mut far) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            if (luma(pixel(&sharp, x, y)) - luma(pixel(&blurred, x, y))).abs() > 30 {
                if x < W / 2 - 5 {
                    near += 1;
                } else {
                    far += 1;
                }
            }
        }
    }
    assert!(far > 60, "the far block's edges barely changed: {far} pixels");
    assert!(near < 10, "the focused block changed: {near} pixels");
}

/// web3d-M7: camera motion blur smears the frame along the camera's
/// motion, and changes nothing while the camera is still.
#[test]
fn motion_blur_follows_the_camera() {
    if headless().is_none() {
        return;
    }
    let scene = |shutter: f32, speed: f32| {
        format!(
            r#"
light.clear()
sun.intensity(0.0)
light.ambient((1.0, 1.0, 1.0, 1.0))
postfx.motion_blur({shutter})
var x = 0.0
on update(dt):
    x += {speed} * dt
on render():
    camera.eye = vec3(x, 0, 5)
    camera.target = vec3(x, 0, 0)
    cube(at: vec3(0, 0, 0), color: (1, 1, 1, 1), size: 1.0)
"#
        )
    };
    let changed = |a: &[u8], b: &[u8]| a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| p != q).count();
    let still_off = render_frames(&scene(0.0, 0.0), 3);
    let still_on = render_frames(&scene(1.0, 0.0), 3);
    assert_eq!(changed(&still_off, &still_on), 0, "a still camera blurs nothing");
    let moving_off = render_frames(&scene(0.0, 20.0), 3);
    let moving_on = render_frames(&scene(1.0, 20.0), 3);
    save_png("motion_blur", &moving_on);
    // The block's left and right edges streak sideways onto the
    // background; its top and bottom edges (the block's centre column,
    // x = 118 with the camera 1 unit right) stay sharp.
    let luma = |p: [u8; 3]| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]);
    let differ = |a: [u8; 3], b: [u8; 3]| (luma(a) - luma(b)).abs() > 30;
    let row = (0..W).filter(|&x| differ(pixel(&moving_off, x, H / 2), pixel(&moving_on, x, H / 2))).count();
    let column = (0..H).filter(|&y| differ(pixel(&moving_off, 118, y), pixel(&moving_on, 118, y))).count();
    assert!(row >= 8, "only {row} pixels of the centre row streaked");
    assert!(column <= 2, "{column} pixels of the block's top and bottom edges blurred");
}

/// web3d-M7: a tint with alpha below 1 is translucent, and translucent
/// draws blend back to front whatever order the script drew them in:
/// green (front) over red (behind) shows more green than red.
#[test]
fn translucent_draws_blend_back_to_front() {
    if headless().is_none() {
        return;
    }
    let scene = |alpha: f32| {
        format!(
            r#"
light.clear()
sun.intensity(0.0)
light.ambient((0.5, 0.5, 0.5, 1.0))
light.fog(0, 0, color.white)
postfx.tonemap("none")
camera.eye = vec3(0, 0, 5)
camera.target = vec3(0, 0, 0)
on render():
    cube(at: vec3(0, 0, 0), color: (0, 1, 0, {alpha}), size: 1.0)
    cube(at: vec3(0, 0, -4), color: (1, 0, 0, {alpha}), size: 3.0)
"#
        )
    };
    let opaque = render_once(&scene(1.0));
    let blended = render_once(&scene(0.5));
    save_png("translucent", &blended);
    let [r, g, _] = pixel(&opaque, W / 2, H / 2);
    assert!(g > r + 100, "the opaque front block hides the back one: {:?}", [r, g]);
    let [r, g, b] = pixel(&blended, W / 2, H / 2);
    assert!(r > 60, "the red block shows through: {:?}", [r, g, b]);
    assert!(g > r + 30, "green is in front (blended last): {:?}", [r, g, b]);
    // Beside the front block only the back one (and the background).
    let [r2, g2, _] = pixel(&blended, W / 2 + 32, H / 2);
    assert!(r2 > g2 + 40, "the back block alone beside it: {:?}", [r2, g2]);
}

/// web3d-M7: a glTF BLEND material is translucent: the background
/// shows through a half-transparent blue quad.
#[test]
fn gltf_blend_materials_are_translucent() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let positions: [f32; 12] = [-1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0];
    let normals: Vec<f32> = [0.0f32, 0.0, 1.0].repeat(4);
    let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
    let mut bin: Vec<u8> = positions.iter().flat_map(|f| f.to_le_bytes()).collect();
    bin.extend(normals.iter().flat_map(|f| f.to_le_bytes()));
    bin.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
    let json = r#"{"asset":{"version":"2.0"},
        "scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1},"indices":2,"material":0}]}],
        "materials":[{"alphaMode":"BLEND","pbrMetallicRoughness":{"baseColorFactor":[0,0,0,0.5],"metallicFactor":0},
            "emissiveFactor":[0,0,1]}],
        "accessors":[
            {"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-1,-1,0],"max":[1,1,0]},
            {"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"},
            {"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"}],
        "bufferViews":[
            {"buffer":0,"byteOffset":0,"byteLength":48},
            {"buffer":0,"byteOffset":48,"byteLength":48},
            {"buffer":0,"byteOffset":96,"byteLength":12}],
        "buffers":[{"byteLength":{bin_len}}]}"#;
    let dir = std::path::Path::new("target/kernel-render");
    std::fs::create_dir_all(dir).expect("create output dir");
    std::fs::write(dir.join("blend_quad.glb"), glb(json, &bin)).expect("write glb");
    let _root = ASSET_ROOT.lock().unwrap_or_else(|e| e.into_inner());
    twec::bundle::set_asset_root(Some(dir.into()));
    let src = r#"
light.clear()
sun.intensity(0.0)
light.ambient((0.0, 0.0, 0.0, 1.0))
light.fog(0, 0, color.white)
camera.eye = vec3(0, 0, 3)
camera.target = vec3(0, 0, 0)
on render():
    mesh("blend_quad.glb", at: vec3(0, 0, 0), color: (1, 1, 1, 1), size: 1.0)
"#;
    let rgba = render_source(&mut renderer, "blend_quad", src);
    twec::bundle::set_asset_root(None);
    save_png("blend_quad", &rgba);
    let [r, g, b] = pixel(&rgba, W / 2, H / 2);
    let [br, bg, _] = pixel(&rgba, 5, 5);
    // Half of an emissive blue over the background: bluish, not the
    // opaque quad's full blue, with the background's red and green.
    assert!((150..240).contains(&b), "half-transparent blue: {:?}", [r, g, b]);
    assert!(r > 10 && g > 20, "the background shows through: {:?} over {:?}", [r, g, b], [br, bg]);
}

/// web3d-M7: height fog thickens with distance and toward the ground,
/// and the background fogs over too.
#[test]
fn height_fog_thickens_with_distance_and_depth() {
    if headless().is_none() {
        return;
    }
    let src = r#"
light.clear()
sun.intensity(0.0)
light.ambient((1.0, 1.0, 1.0, 1.0))
light.fog(0.08, 0.3, (1, 0, 0))
camera.eye = vec3(0, 5, 10)
camera.target = vec3(0, 5, 0)
on render():
    cube(at: vec3(-2.5, 5, 7), color: (1, 1, 1, 1), size: 1.0)
    cube(at: vec3(4, 12, -20), color: (1, 1, 1, 1), size: 6.0)
    cube(at: vec3(4, -2, -20), color: (1, 1, 1, 1), size: 6.0)
"#;
    let rgba = render_once(src);
    save_png("height_fog", &rgba);
    let [nr, ng, _] = pixel(&rgba, 20, 120);
    assert!(ng > 180, "the near block is barely fogged: {:?}", [nr, ng]);
    let high = pixel(&rgba, 200, 60);
    let low = pixel(&rgba, 200, 175);
    assert!(low[0] > low[1] + 40, "the far low block is fogged red: {low:?}");
    assert!(high[1] > low[1] + 30, "fog thins with height: high {high:?} vs low {low:?}");
    // The background fogs too: fully looking down into the fog,
    // partly looking up out of it.
    let (down, up) = (pixel(&rgba, 5, 230), pixel(&rgba, 160, 3));
    assert!(down[0] > 200 && down[1] < 60, "looking down: {down:?}");
    assert!(up[0] < down[0] - 40, "looking up escapes the fog: {up:?} vs {down:?}");
}

/// web3d-M7: a glTF with one quad per material, side by side across
/// x ∈ [-1.8, 1.8] at z = 0, facing +z (the camera).
fn quads_glb(materials: &[&str]) -> Vec<u8> {
    let n = materials.len();
    let width = 3.6 / n as f32;
    let (mut positions, mut normals, mut indices) = (Vec::<f32>::new(), Vec::<f32>::new(), Vec::<u16>::new());
    for i in 0..n {
        let x0 = -1.8 + i as f32 * width + 0.05;
        let x1 = x0 + width - 0.1;
        positions.extend([x0, -0.6, 0.0, x1, -0.6, 0.0, x1, 0.6, 0.0, x0, 0.6, 0.0]);
        normals.extend([0.0f32, 0.0, 1.0].repeat(4));
        let b = (4 * i) as u16;
        indices.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let mut bin: Vec<u8> = positions.iter().flat_map(|f| f.to_le_bytes()).collect();
    bin.extend(normals.iter().flat_map(|f| f.to_le_bytes()));
    bin.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
    let vbytes = positions.len() * 4;
    let primitives: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"attributes":{{"POSITION":0,"NORMAL":1}},"indices":{},"material":{i}}}"#, 2 + i))
        .collect();
    let index_accessors: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"bufferView":2,"byteOffset":{},"componentType":5123,"count":6,"type":"SCALAR"}}"#, i * 12))
        .collect();
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},
        "extensionsUsed":["KHR_materials_sheen","KHR_materials_clearcoat","KHR_materials_iridescence","KHR_materials_transmission","KHR_materials_volume"],
        "scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
        "meshes":[{{"primitives":[{}]}}],
        "materials":[{}],
        "accessors":[
            {{"bufferView":0,"componentType":5126,"count":{v},"type":"VEC3","min":[-1.8,-0.6,0],"max":[1.8,0.6,0]}},
            {{"bufferView":1,"componentType":5126,"count":{v},"type":"VEC3"}},{}],
        "bufferViews":[
            {{"buffer":0,"byteOffset":0,"byteLength":{vb}}},
            {{"buffer":0,"byteOffset":{vb},"byteLength":{vb}}},
            {{"buffer":0,"byteOffset":{ib},"byteLength":{il}}}],
        "buffers":[{{"byteLength":{{bin_len}}}}]}}"#,
        primitives.join(","),
        materials.join(","),
        index_accessors.join(","),
        v = 4 * n,
        vb = vbytes,
        ib = 2 * vbytes,
        il = indices.len() * 2,
    );
    glb(&json, &bin)
}

/// Render `glb` (written as `name`.glb) with the script `src`, which
/// draws it through `mesh("<name>.glb", …)`.
fn render_glb(name: &str, glb_bytes: Vec<u8>, src: &str) -> Vec<u8> {
    let mut renderer = headless().expect("gpu");
    let dir = std::path::Path::new("target/kernel-render");
    std::fs::create_dir_all(dir).expect("create output dir");
    std::fs::write(dir.join(format!("{name}.glb")), glb_bytes).expect("write glb");
    let _root = ASSET_ROOT.lock().unwrap_or_else(|e| e.into_inner());
    twec::bundle::set_asset_root(Some(dir.into()));
    let rgba = render_source(&mut renderer, name, src);
    twec::bundle::set_asset_root(None);
    save_png(name, &rgba);
    rgba
}

/// web3d-M7: glTF material extensions change the surface as their specs
/// say. Under flat ambient light, against a black dielectric: sheen
/// and clearcoat add reflected light, and a thin film makes a grey
/// metal iridescent (coloured, where plain it is grey; on a perfect
/// mirror a film reflects every wavelength, so the base is grey).
#[test]
fn gltf_material_extensions_shade() {
    if headless().is_none() {
        return;
    }
    let glb_bytes = quads_glb(&[
        r#"{"pbrMetallicRoughness":{"baseColorFactor":[0,0,0,1],"metallicFactor":0,"roughnessFactor":0.6}}"#,
        r#"{"pbrMetallicRoughness":{"baseColorFactor":[0,0,0,1],"metallicFactor":0,"roughnessFactor":0.6},
            "extensions":{"KHR_materials_sheen":{"sheenColorFactor":[1,1,1],"sheenRoughnessFactor":0.5}}}"#,
        r#"{"pbrMetallicRoughness":{"baseColorFactor":[0,0,0,1],"metallicFactor":0,"roughnessFactor":0.6},
            "extensions":{"KHR_materials_clearcoat":{"clearcoatFactor":1,"clearcoatRoughnessFactor":0.1}}}"#,
        r#"{"pbrMetallicRoughness":{"baseColorFactor":[0.5,0.5,0.5,1],"metallicFactor":1,"roughnessFactor":0.3},
            "extensions":{"KHR_materials_iridescence":{"iridescenceFactor":1,"iridescenceThicknessMaximum":450}}}"#,
    ]);
    let src = r#"
light.clear()
sun.intensity(0.0)
light.ambient((0.4, 0.4, 0.4, 1.0))
light.fog(0, 0, color.white)
postfx.tonemap("none")
camera.eye = vec3(0, 0, 3)
camera.target = vec3(0, 0, 0)
on render():
    mesh("extensions.glb", at: vec3(0, 0, 0), color: (1, 1, 1, 1), size: 1.0)
"#;
    let rgba = render_glb("extensions", glb_bytes, src);
    let luma = |p: [u8; 3]| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]);
    let at = |i: u32| pixel(&rgba, W / 8 + i * W / 4, H / 2);
    let plain = at(0);
    // Sheen is a grazing lobe: faint head-on, but there.
    assert!(luma(at(1)) > luma(plain) + 30, "sheen adds light: {:?} vs {plain:?}", at(1));
    assert!(luma(at(2)) > luma(plain) + 20, "clearcoat adds a reflection: {:?} vs {plain:?}", at(2));
    let film = at(3);
    let spread = film.iter().max().unwrap() - film.iter().min().unwrap();
    // A plain grey metal is exactly grey (spread 0).
    assert!(spread > 12, "a thin film colours the metal: {film:?}");
}

/// web3d-M7: transmission shows what's behind (a red block through a
/// clear quad), and a volume absorbs it: blue attenuation over a thick
/// slab leaves little red.
#[test]
fn gltf_transmission_and_volume() {
    if headless().is_none() {
        return;
    }
    let glb_bytes = quads_glb(&[
        r#"{"pbrMetallicRoughness":{"baseColorFactor":[1,1,1,1],"metallicFactor":0,"roughnessFactor":0},
            "extensions":{"KHR_materials_transmission":{"transmissionFactor":0}}}"#,
        r#"{"pbrMetallicRoughness":{"baseColorFactor":[1,1,1,1],"metallicFactor":0,"roughnessFactor":0},
            "extensions":{"KHR_materials_transmission":{"transmissionFactor":1}}}"#,
        r#"{"pbrMetallicRoughness":{"baseColorFactor":[1,1,1,1],"metallicFactor":0,"roughnessFactor":0},
            "extensions":{"KHR_materials_transmission":{"transmissionFactor":1},
              "KHR_materials_volume":{"thicknessFactor":0.5,"attenuationDistance":0.1,"attenuationColor":[0.1,0.1,1]}}}"#,
    ]);
    let src = r#"
light.clear()
sun.intensity(0.0)
light.ambient((0.4, 0.4, 0.4, 1.0))
light.fog(0, 0, color.white)
postfx.tonemap("none")
camera.eye = vec3(0, 0, 3)
camera.target = vec3(0, 0, 0)
on render():
    cube(at: vec3(0, 0, -5), color: (1, 0, 0, 1), size: 6.0)
    mesh("transmission.glb", at: vec3(0, 0, 0), color: (1, 1, 1, 1), size: 1.0)
"#;
    let rgba = render_glb("transmission", glb_bytes, src);
    let at = |i: u32| pixel(&rgba, W / 6 + i * W / 3, H / 2);
    let (opaque, clear, absorbed) = (at(0), at(1), at(2));
    assert!(opaque[0].abs_diff(opaque[1]) < 20, "the opaque quad is white-grey: {opaque:?}");
    assert!(clear[0] > clear[1] + 60, "the red block shows through: {clear:?}");
    assert!(absorbed[0] + 40 < clear[0], "the blue volume absorbs the red: {absorbed:?} vs {clear:?}");
}

/// web3d-M7: GPU culling changes no pixels, and it culls: 6400 cubes
/// behind a wall render identically with and without it, and with it
/// most of them are never drawn (the wall hides them from the
/// hierarchical-Z test). Several frames, so the "visible last frame"
/// set settles.
#[test]
fn gpu_culling_is_invisible_and_culls() {
    use twec::kernel::render::{Camera3d, PostFx, RenderSnapshot, ShadowSettings};
    use twec::render3d_types::{DrawCall3d, Primitive};
    if headless().is_none() {
        return;
    }
    let cube = |at: [f32; 3], size: f32, color: [f32; 4]| DrawCall3d {
        primitive: Primitive::Cube,
        at,
        color,
        size,
        texture: 0,
        yaw: 0.0,
        material: 0,
    };
    let mut draws = Vec::new();
    for i in 0..80 {
        for j in 0..80 {
            let (x, z) = ((i as f32 - 40.0) * 2.0, -(j as f32) * 2.0 - 10.0);
            draws.push(cube([x, 0.5, z], 1.0, [0.2 + (i % 5) as f32 * 0.15, 0.6, 0.3, 1.0]));
        }
    }
    // The wall covers the middle of the view; cubes to its sides stay
    // visible, so both sets are exercised.
    draws.push(cube([0.0, 2.0, -4.0], 6.0, [0.8, 0.8, 0.8, 1.0]));
    let render = |cull: bool| -> (Vec<u8>, Option<(u32, u32, u32)>) {
        let mut renderer = headless().expect("gpu");
        let anim = |_: u32| Default::default();
        let mut assets = twec::play3d::NativeAssets::default();
        let mut lights: twec::render3d_types::LightsUniform = bytemuck::Zeroable::zeroed();
        lights.ambient = [0.4, 0.4, 0.4, 0.0];
        lights.sun_dir = [0.3, 0.8, 0.5, 1.0];
        for _ in 0..4 {
            let snap = RenderSnapshot {
                lut: None,
                point_lights: &[],
                fog: None,
                particles: Default::default(),
                camera: Camera3d {
                    far: 400.0,
                    ..Camera3d::new([0.0, 3.0, 4.0], [0.0, 2.0, -20.0], [0.0, 1.0, 0.0])
                },
                environment: None,
                background: [0.05, 0.07, 0.1],
                lights,
                shadow: ShadowSettings {
                    enabled: false,
                    extent: 10.0,
                },
                post: PostFx {
                    frustum_cull: cull,
                    ..PostFx::default()
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
        (renderer.read_pixels().expect("pixels"), renderer.gpu_cull_counts())
    };
    let (plain, _) = render(false);
    let (culled, counts) = render(true);
    save_png("gpu_cull", &culled);
    let differing = plain.chunks(4).zip(culled.chunks(4)).filter(|(a, b)| a != b).count();
    assert!(differing < 20, "{differing} pixels differ with GPU culling");
    let (early, late, total) = counts.expect("the frame was GPU-culled");
    assert_eq!(total, draws.len() as u32);
    let drawn = early + late;
    eprintln!("gpu cull: {early} early + {late} late of {total}; {differing} pixels differ");
    assert!(drawn > 50, "the cubes beside the wall are drawn: {drawn}");
    assert!(drawn < total / 3, "most cubes are culled: {drawn} of {total} drawn");
    assert!(early > late, "after a few frames the early set carries the scene: {early} early, {late} late");
}

/// web3d-M7: render a floor seen from straight above with only the given
/// point / spot lights (no sun, no ambient). Returns the pixels and a
/// world (x, z) → pixel mapping.
fn floor_with_lights(lights: &[twec::kernel::render::PointLightU]) -> (Vec<u8>, impl Fn(f32, f32) -> (u32, u32)) {
    use twec::kernel::render::{Camera3d, PostFx, RenderSnapshot, ShadowSettings, Tonemapper};
    use twec::render3d_types::{DrawCall3d, Primitive};
    let mut renderer = headless().expect("gpu");
    let height = 20.0f32;
    let draws = [DrawCall3d {
        primitive: Primitive::Cube,
        at: [0.0, -50.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
        size: 100.0,
        texture: 0,
        yaw: 0.0,
        material: 0,
    }];
    let anim = |_: u32| Default::default();
    let mut assets = twec::play3d::NativeAssets::default();
    let snap = RenderSnapshot {
        lut: None,
        point_lights: lights,
        fog: None,
        particles: Default::default(),
        camera: Camera3d::new([0.0, height, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        environment: None,
        background: [0.0; 3],
        lights: bytemuck::Zeroable::zeroed(),
        shadow: ShadowSettings {
            enabled: false,
            extent: 10.0,
        },
        post: PostFx {
            tonemapper: Tonemapper::None,
            ..PostFx::default()
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
    let rgba = renderer.read_pixels().expect("pixels");
    // Looking straight down with -z up the screen, 60° vertical field.
    let half_h = height * 30f32.to_radians().tan();
    let half_w = half_h * W as f32 / H as f32;
    let to_pixel = move |x: f32, z: f32| {
        (
            ((0.5 + 0.5 * x / half_w) * W as f32) as u32,
            ((0.5 + 0.5 * z / half_h) * H as f32) as u32,
        )
    };
    (rgba, to_pixel)
}

/// web3d-M7: clustered lighting drops no light: 300 small lights, each
/// lighting only its own patch of floor, all show, and the floor
/// between them stays dark.
#[test]
fn hundreds_of_lights_all_shine() {
    use twec::kernel::render::PointLightU;
    if headless().is_none() {
        return;
    }
    let spacing = 1.2;
    let mut lights = Vec::new();
    for i in 0..20 {
        for j in 0..15 {
            let (x, z) = ((i as f32 - 9.5) * spacing, (j as f32 - 7.0) * spacing);
            let color = [4.0 * (i % 3) as f32 / 2.0 + 1.0, 4.0 * (j % 2) as f32 + 1.0, 3.0];
            lights.push(PointLightU::point([x, 0.3, z], color, 0.6));
        }
    }
    let (rgba, to_pixel) = floor_with_lights(&lights);
    save_png("many_lights", &rgba);
    let luma = |p: [u8; 3]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    let mut dark = Vec::new();
    for l in &lights {
        let (px, py) = to_pixel(l.pos[0], l.pos[2]);
        if luma(pixel(&rgba, px, py)) < 120 {
            dark.push((l.pos[0], l.pos[2]));
        }
    }
    assert!(dark.is_empty(), "{} of 300 lights didn't light their patch: {:?}", dark.len(), &dark[..dark.len().min(5)]);
    // Diagonally between four lights, beyond every radius.
    let (px, py) = to_pixel(0.0, 0.6);
    assert!(luma(pixel(&rgba, px, py)) < 30, "between lights: {:?}", pixel(&rgba, px, py));
}

/// web3d-M7: a spot light lights inside its cone and not outside it,
/// though both are within its radius.
#[test]
fn spot_lights_light_their_cone() {
    use twec::kernel::render::PointLightU;
    if headless().is_none() {
        return;
    }
    let mut spot = PointLightU::point([0.0, 4.0, 0.0], [3.0, 3.0, 3.0], 12.0);
    let (outer, inner) = (20f32.to_radians(), 16f32.to_radians());
    spot.cone = [0.0, -1.0, 0.0, outer.cos()];
    spot.params = [inner.cos(), 0.0, 0.0, 0.0];
    let (rgba, to_pixel) = floor_with_lights(&[spot]);
    save_png("spot_light", &rgba);
    let luma = |p: [u8; 3]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    let (cx, cy) = to_pixel(0.0, 0.0);
    // The cone reaches the floor within tan(20°) · 4 ≈ 1.46 of the centre.
    let (ox, oy) = to_pixel(3.0, 0.0);
    assert!(luma(pixel(&rgba, cx, cy)) > 300, "under the spot: {:?}", pixel(&rgba, cx, cy));
    assert!(luma(pixel(&rgba, ox, oy)) < 10, "outside the cone: {:?}", pixel(&rgba, ox, oy));
}

/// web3d-M7: a fountain of sparks over a floor, seen from above. The
/// sparks fall through the floor (hidden) unless they `collide`.
fn spark_scene(collide: bool, size: &str) -> String {
    format!(
        "camera.eye = vec3(0, 8, 0.001)
camera.target = vec3(0, 0, 0)
postfx.tonemap(\"none\")
var big = 0.08

particles Sparks:
    count: 3000
    lifetime: 5.0
    collide: {collide}

    on_spawn(p):
        let a = random.float() * 6.2832
        let r = random.float() * 2.0
        p.velocity = (math.cos(a) * r, 2.0, math.sin(a) * r)
        p.color = (1.0, 0.45, 0.0, 1.0)
        p.size = {size}

    on_update(p, dt):
        p.velocity = (p.velocity.x, p.velocity.y - 9.8 * dt, p.velocity.z)
        p.pos = (p.pos.x + p.velocity.x * dt, p.pos.y + p.velocity.y * dt, p.pos.z + p.velocity.z * dt)

var fired = false
on update(dt):
    if not fired:
        spawn Sparks at vec3(0, 1, 0)
        fired = true

on render():
    cube(at: vec3(0, -50, 0), size: 100, color: (0.3, 0.3, 0.3, 1))
"
    )
}

fn orange_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4)
        .filter(|p| p[0] > 120 && i32::from(p[0]) - i32::from(p[2]) > 80)
        .count()
}

/// web3d-M7: a `particles` block that compiles runs on the GPU: its
/// sparks draw, and with `collide: true` they bounce on the floor
/// instead of falling through it.
#[test]
fn gpu_particles_draw_and_bounce() {
    if headless().is_none() {
        return;
    }
    let (early, env) = render_frames_env(&spark_scene(true, "0.06"), 10);
    save_png("particles_early", &early);
    assert_eq!(env.particle_programs.len(), 1, "the block runs on the GPU");
    assert!(orange_pixels(&early) > 200, "sparks in the air: {}", orange_pixels(&early));
    let bounced = render_frames(&spark_scene(true, "0.06"), 70);
    let fell = render_frames(&spark_scene(false, "0.06"), 70);
    save_png("particles_bounced", &bounced);
    save_png("particles_fell", &fell);
    let (on_floor, through) = (orange_pixels(&bounced), orange_pixels(&fell));
    assert!(on_floor > 500, "sparks resting on the floor: {on_floor}");
    assert!(through < on_floor / 20, "without collide they fall through: {through} vs {on_floor}");
}

/// web3d-M7: a block that can't compile (it reads a global) runs on the
/// CPU and still draws in 3D.
#[test]
fn cpu_particles_still_draw_in_3d() {
    if headless().is_none() {
        return;
    }
    let (rgba, env) = render_frames_env(&spark_scene(true, "big"), 10);
    save_png("particles_cpu", &rgba);
    assert!(env.particle_programs.is_empty(), "the block runs on the CPU");
    assert!(orange_pixels(&rgba) > 200, "CPU sparks draw: {}", orange_pixels(&rgba));
}

/// web3d-M7: a million particles fit the pool and draw.
#[test]
fn a_million_particles() {
    let Some(mut renderer) = headless() else {
        return;
    };
    let src = "camera.eye = vec3(0, 0, 12)
camera.target = vec3(0, 0, 0)
postfx.tonemap(\"none\")

particles Cloud:
    count: 1000000
    lifetime: 10.0

    on_spawn(p):
        p.pos = (random.float() * 8.0 - 4.0, random.float() * 8.0 - 4.0, random.float() * 2.0 - 1.0)
        p.color = (0.2, 0.6, 1.0, 0.05)
        p.size = 0.02

spawn Cloud at vec3(0, 0, 0)
";
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    env.gpu_particles = true;
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    let mut assets = twec::play3d::NativeAssets::default();
    for _ in 0..3 {
        twec::eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
        twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).expect("render");
    }
    assert_eq!(renderer.particle_capacity(), 1 << 20);
    let rgba = renderer.read_pixels().expect("pixels");
    save_png("particles_million", &rgba);
    // The cloud covers the middle of the view, blue; the corners don't.
    let centre = pixel(&rgba, W / 2, H / 2);
    let corner = pixel(&rgba, 4, 4);
    assert!(centre[2] > corner[2] + 20, "cloud {centre:?} vs corner {corner:?}");
}

/// web3d-M7: a glTF with one square in the y = 0 plane, x, z ∈ [-4, 4],
/// facing up, with `material`.
fn floor_glb(material: &str) -> Vec<u8> {
    let positions = [-4.0f32, 0.0, -4.0, -4.0, 0.0, 4.0, 4.0, 0.0, 4.0, 4.0, 0.0, -4.0];
    let normals = [0.0f32, 1.0, 0.0].repeat(4);
    let indices = [0u16, 1, 2, 0, 2, 3];
    let mut bin: Vec<u8> = positions.iter().flat_map(|f| f.to_le_bytes()).collect();
    bin.extend(normals.iter().flat_map(|f| f.to_le_bytes()));
    bin.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
        "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1}},"indices":2,"material":0}}]}}],
        "materials":[{material}],
        "accessors":[
            {{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-4,0,-4],"max":[4,0,4]}},
            {{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"}},
            {{"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"}}],
        "bufferViews":[
            {{"buffer":0,"byteOffset":0,"byteLength":48}},
            {{"buffer":0,"byteOffset":48,"byteLength":48}},
            {{"buffer":0,"byteOffset":96,"byteLength":12}}],
        "buffers":[{{"byteLength":{{bin_len}}}}]}}"#
    );
    glb(&json, &bin)
}

/// web3d-M7: with `postfx.ssr`, a mirror floor reflects the red block
/// standing on it (screen-space reflections); without, only the
/// surroundings. Pixels away from the floor don't change.
#[test]
fn screen_space_reflections_show_the_scene() {
    if headless().is_none() {
        return;
    }
    let mirror = r#"{"pbrMetallicRoughness":{"baseColorFactor":[0.9,0.9,0.9,1],"metallicFactor":1,"roughnessFactor":0.05}}"#;
    let scene = |ssr: f32| {
        format!(
            "light.clear()
light.ambient((0.3, 0.3, 0.3, 1.0))
postfx.tonemap(\"none\")
postfx.ssr({ssr})
camera.eye = vec3(0, 1.6, 4)
camera.target = vec3(0, 0.4, 0)
on render():
    mesh(\"mirror.glb\", at: vec3(0, 0, 0), color: (1, 1, 1, 1), size: 1.0)
    cube(at: vec3(0, 0.5, 0), size: 1, color: (1, 0.1, 0.1, 1))
"
        )
    };
    let off = render_glb("mirror", floor_glb(mirror), &scene(0.0));
    let on = render_glb("mirror", floor_glb(mirror), &scene(1.0));
    save_png("ssr_off", &off);
    save_png("ssr_on", &on);
    let red = |p: &[u8]| p[0] > 90 && i32::from(p[0]) > i32::from(p[1]) + 50;
    let (mut reflected, mut changed_above) = (0, 0);
    for (i, (a, b)) in off.chunks_exact(4).zip(on.chunks_exact(4)).enumerate() {
        let y = i as u32 / W;
        if red(b) && !red(a) {
            reflected += 1;
        }
        // The top rows show the backdrop, above the floor's horizon.
        if y < 20 && (i32::from(a[0]) - i32::from(b[0])).abs() > 2 {
            changed_above += 1;
        }
    }
    assert!(reflected > 800, "the floor reflects the block: {reflected} pixels");
    assert_eq!(changed_above, 0, "the backdrop is untouched");
}

/// web3d-M7: `light.volumetric` lights the fog per point. With no shadow
/// in the way it matches the closed-form fog it replaces; with the sun
/// shadowed by a wall, the fog in the wall's shadow is darker (a light
/// shaft's edge).
#[test]
fn volumetric_fog_matches_and_casts_shafts() {
    if headless().is_none() {
        return;
    }
    let scene = |volumetric: bool, shadow: bool| {
        format!(
            "light.clear()
light.ambient((0.2, 0.2, 0.2, 1.0))
sun.direction(vec3(-0.6, 0.5, 0.2))
sun.shadow({shadow})
sun.shadow_extent(20.0)
light.fog(0.15, 0.0, (0.8, 0.8, 0.8))
light.volumetric({volumetric})
postfx.tonemap(\"none\")
camera.eye = vec3(0, 2, 8)
camera.target = vec3(0, 2, 0)
on render():
    cube(at: vec3(0, -50, 0), size: 100, color: (0.3, 0.3, 0.3, 1))
    cube(at: vec3(-3, 3, -2), size: 6, color: (0.5, 0.5, 0.5, 1))
"
        )
    };
    let mut renderer = headless().expect("gpu");
    let closed = render_source(&mut renderer, "fog_closed", &scene(false, false));
    let volume = render_source(&mut renderer, "fog_volume", &scene(true, false));
    let closed_shadowed = render_source(&mut renderer, "fog_closed_shadowed", &scene(false, true));
    let volume_shadowed = render_source(&mut renderer, "fog_volume_shadowed", &scene(true, true));
    save_png("fog_closed", &closed);
    save_png("fog_volume", &volume);
    save_png("fog_volume_shadowed", &volume_shadowed);
    let mean = |rgba: &[u8]| rgba.chunks_exact(4).map(|p| f64::from(p[0])).sum::<f64>() / f64::from(W * H);
    assert!(
        (mean(&closed) - mean(&volume)).abs() < 3.0,
        "unshadowed, the volume matches the closed form: {} vs {}",
        mean(&volume),
        mean(&closed)
    );
    let darker = closed_shadowed
        .chunks_exact(4)
        .zip(volume_shadowed.chunks_exact(4))
        .filter(|(a, b)| i32::from(a[0]) - i32::from(b[0]) > 8)
        .count();
    assert!(darker as u32 > W * H / 50, "the wall's shadow darkens the fog: {darker} pixels");
}
