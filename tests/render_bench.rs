//! web3d-M7 session 11: renderer throughput on large scenes.
//!
//! Draws 100 000 cubes headless at 1280×720 through the kernel directly
//! (no interpreter), in three scenes:
//!
//! - **open**: every cube in view — the cost of drawing them all;
//! - **field**: a camera low over a wide grid — most cubes are outside
//!   the view (frustum culling's case);
//! - **occluded**: the field behind a wall — most cubes are in the
//!   frustum but hidden (occlusion culling's case).
//!
//! Prints the CPU time of `Renderer::render` and the whole frame
//! (render + GPU, measured over a batch ended by a readback). Opt-in:
//!
//!     cargo test --release --test render_bench -- --ignored --nocapture

use twec::kernel::render::{Camera3d, PostFx, RenderSnapshot, Renderer, ShadowSettings};
use twec::render3d_types::{DrawCall3d, Primitive};

const W: u32 = 1280;
const H: u32 = 720;
const FRAMES: u32 = 60;

fn cube(x: f32, y: f32, z: f32, size: f32, color: [f32; 4]) -> DrawCall3d {
    DrawCall3d {
        primitive: Primitive::Cube,
        at: [x, y, z],
        color,
        size,
        texture: 0,
        yaw: 0.0,
        material: 0,
    }
}

/// 100 000 cubes on a 316 × 316 grid, 2 units apart, centred on the
/// origin (x, z ∈ [-316, 316]).
fn grid() -> Vec<DrawCall3d> {
    let n = 316;
    let mut out = Vec::with_capacity(n * n + 1);
    for i in 0..n {
        for j in 0..n {
            let x = (i as f32 - n as f32 / 2.0) * 2.0;
            let z = (j as f32 - n as f32 / 2.0) * 2.0;
            let shade = 0.4 + 0.6 * ((i * 7 + j * 13) % 10) as f32 / 10.0;
            out.push(cube(x, 0.5, z, 1.0, [shade, 0.5, 1.0 - shade, 1.0]));
        }
    }
    out
}

/// Frames per second-ish numbers for one scene: (CPU ms in `render`,
/// total ms per frame).
fn measure(name: &str, camera: Camera3d, draws: &[DrawCall3d], cull: bool) -> (f64, f64) {
    measure_lit(name, camera, draws, &[], cull)
}

/// `measure` with point lights.
fn measure_lit(
    name: &str,
    camera: Camera3d,
    draws: &[DrawCall3d],
    point_lights: &[twec::kernel::render::PointLightU],
    cull: bool,
) -> (f64, f64) {
    let mut renderer = pollster::block_on(Renderer::new_headless(W, H)).expect("gpu");
    let anim = |_: u32| Default::default();
    let mut assets = twec::play3d::NativeAssets::default();
    let snap = || RenderSnapshot {
        lut: None,
        point_lights,
        fog: None,
        particles: Default::default(),
        camera,
        environment: None,
        background: [0.05, 0.07, 0.1],
        lights: {
            let mut l: twec::render3d_types::LightsUniform = bytemuck::Zeroable::zeroed();
            l.ambient = [0.3, 0.3, 0.3, 0.0];
            l.sun_dir = [0.4, 0.8, 0.3, 1.0];
            l
        },
        shadow: ShadowSettings {
            enabled: false,
            extent: 10.0,
        },
        post: PostFx {
            frustum_cull: cull,
            ..PostFx::default()
        },
        draws,
        mesh_paths: &[],
        texture_paths: &[],
        time: 0.0,
        materials: &[],
        hud: &[],
        anim: &anim,
    };
    // Warm up (pipelines, buffer growth).
    for _ in 0..5 {
        renderer.render(&snap(), &mut assets).expect("render");
    }
    renderer.read_pixels().expect("sync");
    let mut cpu = 0.0;
    let start = std::time::Instant::now();
    for _ in 0..FRAMES {
        let t = std::time::Instant::now();
        renderer.render(&snap(), &mut assets).expect("render");
        cpu += t.elapsed().as_secs_f64();
    }
    renderer.read_pixels().expect("sync");
    let total = start.elapsed().as_secs_f64();
    let (cpu_ms, frame_ms) = (cpu * 1e3 / f64::from(FRAMES), total * 1e3 / f64::from(FRAMES));
    eprintln!("{name:>10} cull={cull:<5}: render() {cpu_ms:6.2} ms CPU, {frame_ms:6.2} ms/frame");
    (cpu_ms, frame_ms)
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn render_large_scenes() {
    if pollster::block_on(Renderer::new_headless(64, 64)).is_err() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    let field = grid();
    // Everything in view: high above the grid, looking down.
    let open = Camera3d {
        far: 1000.0,
        ..Camera3d::new([0.0, 520.0, 1.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    };
    // Low over the grid's edge, looking along it.
    let low = Camera3d {
        far: 1000.0,
        ..Camera3d::new([0.0, 3.0, 330.0], [0.0, 1.0, 300.0], [0.0, 1.0, 0.0])
    };
    // The same camera behind a wall.
    let mut walled = field.clone();
    // A wall 6 units in front of the camera, filling the view.
    walled.push(cube(0.0, 5.0, 312.0, 24.0, [0.8, 0.8, 0.8, 1.0]));
    // A game-sized scene: a floor and 300 cubes around a top-down camera
    // (survive3d's scale), where culling has little to win.
    let mut small = vec![cube(0.0, -50.0, 0.0, 100.0, [0.3, 0.3, 0.3, 1.0])];
    for i in 0..300 {
        let a = i as f32 * 0.37;
        let r = 3.0 + (i % 17) as f32;
        small.push(cube(r * a.cos(), 0.5, r * a.sin(), 1.0, [0.9, 0.4, 0.3, 1.0]));
    }
    let top = Camera3d {
        far: 200.0,
        ..Camera3d::new([0.0, 18.0, 12.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    };
    measure("small", top, &small, false);
    measure("small", top, &small, true);
    // Clustered lights over the small scene: 100 and 1000 lights of
    // radius 3 scattered over the view.
    for n in [100usize, 1000] {
        let lights: Vec<_> = (0..n)
            .map(|i| {
                let a = i as f32 * 2.399;
                let r = 20.0 * ((i as f32 + 0.5) / n as f32).sqrt();
                twec::kernel::render::PointLightU::point([r * a.cos(), 1.0, r * a.sin()], [1.0, 0.7, 0.4], 3.0)
            })
            .collect();
        measure_lit(if n == 100 { "100 lights" } else { "1000 lights" }, top, &small, &lights, true);
    }
    // Fixed costs: nothing to draw, and everything behind the camera.
    measure("empty", low, &[], true);
    let away = Camera3d {
        far: 1000.0,
        ..Camera3d::new([0.0, 3.0, 330.0], [0.0, 3.0, 400.0], [0.0, 1.0, 0.0])
    };
    measure("behind", away, &field, true);
    measure("behind", away, &field, false);
    for cull in [false, true] {
        measure("open", open, &field, cull);
        measure("field", low, &field, cull);
        measure("occluded", low, &walled, cull);
    }
}

/// web3d-M7 session 13: GPU particles over the small scene — `n`
/// particles emitted once, then simulated (gravity, drag) and drawn
/// every frame, optionally bouncing off the depth buffer.
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn render_particles() {
    use twec::kernel::particles::{ParticleEmission, ParticleFrame, ParticleProgram};
    if pollster::block_on(Renderer::new_headless(64, 64)).is_err() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    let mut small = vec![cube(0.0, -50.0, 0.0, 100.0, [0.3, 0.3, 0.3, 1.0])];
    for i in 0..300 {
        let a = i as f32 * 0.37;
        let r = 3.0 + (i % 17) as f32;
        small.push(cube(r * a.cos(), 0.5, r * a.sin(), 1.0, [0.9, 0.4, 0.3, 1.0]));
    }
    let camera = Camera3d {
        far: 200.0,
        ..Camera3d::new([0.0, 18.0, 12.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    };
    for (n, collide) in [(100_000u32, false), (1_000_000, false), (1_000_000, true)] {
        let programs = [ParticleProgram {
            spawn: "    pt.velocity = vec3<f32>(twe_rand() * 16.0 - 8.0, twe_rand() * 12.0, twe_rand() * 16.0 - 8.0);\n    \
                    pt.color = vec4<f32>(1.0, 0.5, 0.1, 0.6);\n    pt.size = 0.05;\n"
                .into(),
            update: "    pt.velocity.y -= 9.8 * dt;\n    pt.velocity *= 0.995;\n    pt.pos += pt.velocity * dt;\n".into(),
            collide,
        }];
        let emission = [ParticleEmission {
            program: 0,
            at: [0.0, 2.0, 0.0],
            count: n,
            lifetime: 1000.0,
            seed: 1,
        }];
        let mut renderer = pollster::block_on(Renderer::new_headless(W, H)).expect("gpu");
        let anim = |_: u32| Default::default();
        let mut assets = twec::play3d::NativeAssets::default();
        let snap = |frame: u32| RenderSnapshot {
            lut: None,
            point_lights: &[],
            fog: None,
            particles: ParticleFrame {
                programs: &programs,
                emissions: if frame == 0 { &emission } else { &[] },
                cpu: &[],
            },
            camera,
            environment: None,
            background: [0.05, 0.07, 0.1],
            lights: {
                let mut l: twec::render3d_types::LightsUniform = bytemuck::Zeroable::zeroed();
                l.ambient = [0.3, 0.3, 0.3, 0.0];
                l.sun_dir = [0.4, 0.8, 0.3, 1.0];
                l
            },
            shadow: ShadowSettings {
                enabled: false,
                extent: 10.0,
            },
            post: PostFx::default(),
            draws: &small,
            mesh_paths: &[],
            texture_paths: &[],
            time: frame as f32 / 60.0,
            materials: &[],
            hud: &[],
            anim: &anim,
        };
        for f in 0..30 {
            renderer.render(&snap(f), &mut assets).expect("render");
        }
        renderer.read_pixels().expect("sync");
        let start = std::time::Instant::now();
        for f in 30..30 + FRAMES {
            renderer.render(&snap(f), &mut assets).expect("render");
        }
        renderer.read_pixels().expect("sync");
        let ms = start.elapsed().as_secs_f64() * 1e3 / f64::from(FRAMES);
        eprintln!("{n:>8} particles collide={collide:<5}: {ms:6.2} ms/frame");
    }
    measure("no particles", camera, &small, true);
}
