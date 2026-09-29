//! web3d-M7: Twe's half of the graphics comparison harness
//! (`bench/graphics/`, see its README).
//!
//! Renders every scenario fetched into `bench/graphics/cache/` with the
//! kernel renderer, headless, at the reference's pixel size, and writes
//! `bench/graphics/out/twe/<scenario>.png` for `score.py`. Opt-in (it
//! needs the fetched suite and a GPU):
//!
//!     cargo test --release --test graphics_bench -- --ignored --nocapture
//!
//! The scene is what the Khronos Render Fidelity goldens use: the
//! scenario's orbit camera and vertical field of view, clip planes from
//! the model's bounding sphere, a black background. Lighting is Twe's
//! default (the renderer has no environment lighting yet: M7 session
//! 4), so these images are the baseline M7 improves on.

use std::path::{Path, PathBuf};

use twec::json::Value;
use twec::kernel::render::{
    Camera3d, PostFx, RenderSnapshot, Renderer, ShadowSettings,
};
use twec::render3d_types::{DrawCall3d, Primitive};

fn num(v: &Value, key: &str) -> f32 {
    match v.get(key) {
        Some(Value::Int(i)) => *i as f32,
        Some(Value::Float(f)) => *f as f32,
        other => panic!("scene.json: `{key}` is not a number: {other:?}"),
    }
}

struct Scene {
    name: String,
    dir: PathBuf,
    width: u32,
    height: u32,
    target: [f32; 3],
    theta: f32,
    phi: f32,
    radius: f32,
    fov_y: f32,
}

fn read_scene(dir: &Path) -> Scene {
    let text = std::fs::read_to_string(dir.join("scene.json")).expect("read scene.json");
    let v = twec::json::parse(&text).expect("scene.json is JSON");
    let get = |k: &str| v.get(k).unwrap_or_else(|| panic!("scene.json: no `{k}`"));
    let (px, t, o) = (get("pixels"), get("target"), get("orbit"));
    Scene {
        name: get("name").as_str().expect("name").to_string(),
        dir: dir.to_path_buf(),
        width: num(px, "width") as u32,
        height: num(px, "height") as u32,
        target: [num(t, "x"), num(t, "y"), num(t, "z")],
        theta: num(o, "theta").to_radians(),
        phi: num(o, "phi").to_radians(),
        radius: num(o, "radius"),
        fov_y: match v.get("verticalFoV") {
            Some(Value::Int(i)) => (*i as f32).to_radians(),
            Some(Value::Float(f)) => (*f as f32).to_radians(),
            _ => 45_f32.to_radians(),
        },
    }
}

fn render(scene: &Scene) -> Result<Vec<u8>, String> {
    let model = std::fs::read(scene.dir.join("model.glb")).map_err(|e| e.to_string())?;
    let (_, sphere_r) = twec::kernel::render::parse_glb_bytes(&model)?.bounding_sphere();
    twec::bundle::set_asset_root(Some(scene.dir.clone()));

    // The generator's orbit: theta around +Y from +Z, phi down from +Y.
    // Clip planes as its Cycles reference sets them.
    // The camera looks along -dir rather than *at* the target: the same
    // view for r > 0, and still defined when r = 0 (Sponza's camera
    // stands at its target).
    let [tx, ty, tz] = scene.target;
    let r = scene.radius;
    let dir = [
        scene.phi.sin() * scene.theta.sin(),
        scene.phi.cos(),
        scene.phi.sin() * scene.theta.cos(),
    ];
    let eye = [tx + r * dir[0], ty + r * dir[1], tz + r * dir[2]];
    let look = [eye[0] - dir[0], eye[1] - dir[1], eye[2] - dir[2]];
    let reach = r.max(sphere_r).max(1e-5);
    let camera = Camera3d {
        fov_y: scene.fov_y,
        near: 2.0 * reach / 1000.0,
        far: 2.0 * reach + r,
        ..Camera3d::new(eye, look, [0.0, 1.0, 0.0])
    };

    let mut renderer = pollster::block_on(Renderer::new_headless(scene.width, scene.height))?;
    let draws = [DrawCall3d {
        primitive: Primitive::Mesh(0),
        at: [0.0, 0.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
        size: 1.0,
        texture: 0,
        yaw: 0.0,
        material: 0,
    }];
    let mesh_paths = vec!["model.glb".to_string()];
    let anim = |_: u32| Default::default();
    let mut assets = twec::play3d::NativeAssets::default();
    for frame in 0.. {
        let snap = RenderSnapshot {
            camera,
            background: [0.0, 0.0, 0.0],
            lights: twec::stdlib::lights_snapshot(),
            shadow: ShadowSettings {
                enabled: twec::stdlib::shadow_enabled(),
                extent: twec::stdlib::shadow_extent(),
            },
            post: PostFx {
                tonemap_aces: true,
                vignette: 0.0,
                vignette_color: [0.0; 3],
                bloom_intensity: 0.0,
                bloom_threshold: 1.0,
                frustum_cull: false,
            },
            draws: &draws,
            mesh_paths: &mesh_paths,
            texture_paths: &[],
            time: 0.0,
            materials: &[],
            hud: &[],
            anim: &anim,
        };
        renderer.render(&snap, &mut assets)?;
        // Draw until the model has loaded, then once more.
        if assets.pending() == 0 && frame > 0 {
            break;
        }
        if frame > 3000 {
            return Err("model never finished loading".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    renderer.read_pixels()
}

#[test]
#[ignore = "needs bench/graphics/cache (node bench/graphics/fetch.mjs) and a GPU"]
fn render_the_graphics_suite() {
    let cache = Path::new("bench/graphics/cache");
    let out = Path::new("bench/graphics/out/twe");
    std::fs::create_dir_all(out).expect("create out dir");
    let wanted: Vec<String> = std::env::var("TWE_BENCH_SCENES")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(cache)
        .expect("bench/graphics/cache missing: run `node bench/graphics/fetch.mjs`")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("scene.json").exists())
        .collect();
    dirs.sort();
    let mut failures = Vec::new();
    for dir in dirs {
        let scene = read_scene(&dir);
        if !wanted.is_empty() && !wanted.contains(&scene.name) {
            continue;
        }
        let t0 = std::time::Instant::now();
        match render(&scene) {
            Ok(rgba) => {
                image::save_buffer(
                    out.join(format!("{}.png", scene.name)),
                    &rgba,
                    scene.width,
                    scene.height,
                    image::ColorType::Rgba8,
                )
                .expect("write png");
                eprintln!("{}: {:.1} s", scene.name, t0.elapsed().as_secs_f32());
            }
            Err(e) => {
                eprintln!("{}: FAILED: {e}", scene.name);
                failures.push(scene.name);
            }
        }
    }
    assert!(failures.is_empty(), "scenes Twe could not render: {failures:?}");
}
