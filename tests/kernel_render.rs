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
