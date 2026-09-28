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
    let src = std::fs::read_to_string(path).expect("read script");
    let tokens = twec::lexer::lex(&src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    for _ in 0..frames {
        twec::eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
    }
    let mut assets = twec::play3d::NativeAssets::default();
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
