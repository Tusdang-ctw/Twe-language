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
        rotation: twec::render3d_types::NO_ROTATION,
        scale: [1.0; 3],
        glow: [0.0; 3],
        anim: 0,
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
    let post = PostFx {
        frustum_cull: cull,
        ..PostFx::default()
    };
    measure_with(name, camera, draws, point_lights, post, None, cull)
}

/// `measure_lit` with any post-processing and fog.
fn measure_with(
    name: &str,
    camera: Camera3d,
    draws: &[DrawCall3d],
    point_lights: &[twec::kernel::render::PointLightU],
    post: PostFx,
    fog: Option<twec::kernel::render::FogSettings>,
    cull: bool,
) -> (f64, f64) {
    measure_materials(name, camera, draws, point_lights, (post, fog, cull), &[])
}

/// `measure_with`, the draws' `material` ids indexing `materials`
/// (material WGSL; index 0 unused).
fn measure_materials(
    name: &str,
    camera: Camera3d,
    draws: &[DrawCall3d],
    point_lights: &[twec::kernel::render::PointLightU],
    (post, fog, cull): (PostFx, Option<twec::kernel::render::FogSettings>, bool),
    materials: &[String],
) -> (f64, f64) {
    let mut renderer = pollster::block_on(Renderer::new_headless(W, H)).expect("gpu");
    let anim = |_: u32| Default::default();
    let mut assets = twec::play3d::NativeAssets::default();
    let snap = || RenderSnapshot {
        lut: None,
        point_lights,
        fog,
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
        post,
        draws,
        draws_generation: None,
        mesh_paths: &[],
        texture_paths: &[],
        time: 0.0,
        materials,
        hud: &[],
        anim: &anim,
        instance_anims: &[],
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
    let (cpu_ms, frame_ms) = (
        cpu * 1e3 / f64::from(FRAMES),
        total * 1e3 / f64::from(FRAMES),
    );
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
        small.push(cube(
            r * a.cos(),
            0.5,
            r * a.sin(),
            1.0,
            [0.9, 0.4, 0.3, 1.0],
        ));
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
                twec::kernel::render::PointLightU::point(
                    [r * a.cos(), 1.0, r * a.sin()],
                    [1.0, 0.7, 0.4],
                    3.0,
                )
            })
            .collect();
        measure_lit(
            if n == 100 {
                "100 lights"
            } else {
                "1000 lights"
            },
            top,
            &small,
            &lights,
            true,
        );
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
        small.push(cube(
            r * a.cos(),
            0.5,
            r * a.sin(),
            1.0,
            [0.9, 0.4, 0.3, 1.0],
        ));
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
            draws_generation: None,
            mesh_paths: &[],
            texture_paths: &[],
            time: frame as f32 / 60.0,
            materials: &[],
            hud: &[],
            anim: &anim,
            instance_anims: &[],
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

/// web3d-M7 session 14: screen-space reflections and volumetric fog
/// over the small scene (300 cubes and a floor, top-down).
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn render_ssr_and_fog() {
    if pollster::block_on(Renderer::new_headless(64, 64)).is_err() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    let mut small = vec![cube(0.0, -50.0, 0.0, 100.0, [0.3, 0.3, 0.3, 1.0])];
    for i in 0..300 {
        let a = i as f32 * 0.37;
        let r = 3.0 + (i % 17) as f32;
        small.push(cube(
            r * a.cos(),
            0.5,
            r * a.sin(),
            1.0,
            [0.9, 0.4, 0.3, 1.0],
        ));
    }
    let top = Camera3d {
        far: 200.0,
        ..Camera3d::new([0.0, 18.0, 12.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    };
    let lights: Vec<_> = (0..100)
        .map(|i| {
            let a = i as f32 * 2.399;
            let r = 20.0 * ((i as f32 + 0.5) / 100.0).sqrt();
            twec::kernel::render::PointLightU::point(
                [r * a.cos(), 1.0, r * a.sin()],
                [1.0, 0.7, 0.4],
                3.0,
            )
        })
        .collect();
    let fog = |volumetric| {
        Some(twec::kernel::render::FogSettings {
            density: 0.03,
            falloff: 0.2,
            color: [0.7, 0.75, 0.8],
            volumetric,
        })
    };
    let ssr = PostFx {
        ssr: 1.0,
        ..PostFx::default()
    };
    measure_with("plain", top, &small, &lights, PostFx::default(), None, true);
    measure_with("ssr", top, &small, &lights, ssr, None, true);
    measure_with(
        "fog",
        top,
        &small,
        &lights,
        PostFx::default(),
        fog(false),
        true,
    );
    measure_with(
        "vol fog",
        top,
        &small,
        &lights,
        PostFx::default(),
        fog(true),
        true,
    );
}

/// web3d-M7 session 15: procedural surface materials over the small
/// scene (300 cubes and a floor, top-down): every draw plain, with a
/// `surface` material (noise, tiles, grout), and with that material
/// displacing its vertices too.
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn render_procedural_materials() {
    if pollster::block_on(Renderer::new_headless(64, 64)).is_err() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    let material = |src: &str| -> String {
        let tokens = twec::lexer::lex(src).expect("lex");
        let program = twec::parser::parse(&tokens).expect("parse");
        let twec::ast::Stmt::Decl { name, members, .. } = &program.stmts[0] else {
            panic!("a visual");
        };
        twec::visual_wgsl::compile_material(name, members).expect("compile")
    };
    let surface = r#"    surface(uv, time, pos) -> material:
        let cell = (math.mod(pos.x, 1), math.mod(pos.z, 1))
        let odd = math.mod(math.floor(pos.x) + math.floor(pos.z), 2)
        let edge = math.min(math.min(cell.x, 1 - cell.x), math.min(cell.y, 1 - cell.y))
        let grout = 1 - smoothstep(0.02, 0.05, edge)
        let stone = mix((0.86, 0.84, 0.8), (0.16, 0.16, 0.18), odd) + noise((pos.x, pos.z) * 5) * 0.06
        return material(albedo: mix(stone, (0.35, 0.32, 0.3), grout), roughness: mix(0.2 + odd * 0.3, 0.95, grout))
"#;
    let displace = r#"    displace(uv, time, pos, normal) -> vec3:
        return normal * noise((pos.x + pos.y, pos.z) * 4) * 0.05
"#;
    let materials = vec![
        String::new(),
        material(&format!("visual Tiles:\n{surface}")),
        material(&format!("visual Bumpy:\n{surface}{displace}")),
    ];
    let scene = |mat: u32| -> Vec<DrawCall3d> {
        let mut out = vec![DrawCall3d {
            material: mat,
            ..cube(0.0, -50.0, 0.0, 100.0, [0.3, 0.3, 0.3, 1.0])
        }];
        for i in 0..300 {
            let a = i as f32 * 0.37;
            let r = 3.0 + (i % 17) as f32;
            out.push(DrawCall3d {
                material: mat,
                ..cube(r * a.cos(), 0.5, r * a.sin(), 1.0, [0.9, 0.4, 0.3, 1.0])
            });
        }
        out
    };
    let top = Camera3d {
        far: 200.0,
        ..Camera3d::new([0.0, 18.0, 12.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    };
    let post = (PostFx::default(), None, true);
    measure_materials("plain", top, &scene(0), &[], post, &materials);
    measure_materials("surface", top, &scene(1), &[], post, &materials);
    measure_materials("displaced", top, &scene(2), &[], post, &materials);
    // Again, in case the first run paid for clocks ramping up.
    measure_materials("plain", top, &scene(0), &[], post, &materials);
}

/// web3d-M7 session 16: the stress scene (examples/stress_3d.twe)
/// through the whole native path: script, looks, snapshot, kernel.
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn render_stress_scene() {
    let Ok(mut renderer) = pollster::block_on(Renderer::new_headless(W, H)) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // `TWE_STRESS_SCRIPT` swaps in a variant (for splitting the cost).
    let path =
        std::env::var("TWE_STRESS_SCRIPT").unwrap_or_else(|_| "examples/stress_3d.twe".to_string());
    let src = std::fs::read_to_string(&path).expect("read");
    let out_dir = std::env::current_dir()
        .expect("cwd")
        .join("target/kernel-render");
    // Asset paths are relative to the game's directory, as when it's played
    // (a multi-file game like survive3d loads `assets/…`).
    if let Some(dir) = std::path::Path::new(&path)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        std::env::set_current_dir(dir).expect("cd into the script's directory");
    }
    let tokens = twec::lexer::lex(&src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    env.gpu_particles = true;
    let t = std::time::Instant::now();
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    eprintln!("top level: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let mut assets = twec::play3d::NativeAssets::default();
    for _ in 0..5 {
        twec::eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
        twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).expect("render");
    }
    // `TWE_STRESS_RAMP=1`: step down through the tiers and scales the
    // automatic governor would (G5), a few frames each, then measure at
    // the last — to compare with starting there.
    if std::env::var_os("TWE_STRESS_RAMP").is_some() {
        use twec::kernel::quality::{AutoQuality, Tier};
        let mut governor = AutoQuality::new(Tier::High, 60.0);
        // The governor's floor: Low at half of Low's own scale.
        let floor = twec::kernel::quality::Quality::tier(Tier::Low).render_scale * 0.5;
        for _ in 0..2000 {
            if !governor.update(0.05) {
                continue;
            }
            let q = governor.quality();
            twec::stdlib::set_auto_quality(Some(q));
            for _ in 0..3 {
                twec::eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
                twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).expect("render");
            }
            // Sync each step, as a presenting host's frame pacing would;
            // without it the CPU queues hundreds of frames ahead and the
            // targets each step frees can't be released in time.
            renderer.read_pixels().expect("sync");
            if q.tier == Tier::Low && (q.render_scale - floor).abs() < 1e-4 {
                break;
            }
        }
        eprintln!("ramped to {:?}", renderer.quality());
    }
    renderer.read_pixels().expect("sync");
    let (mut tick, mut script, mut kernel) = (0.0, 0.0, 0.0);
    let start = std::time::Instant::now();
    // `TWE_STRESS_FRAMES` measures longer (the GPU profile prints every 60).
    let frames: u32 = std::env::var("TWE_STRESS_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(FRAMES);
    for _ in 0..frames {
        let t = std::time::Instant::now();
        twec::eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
        tick += t.elapsed().as_secs_f64();
        let times =
            twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).expect("render");
        script += times.script_ms;
        kernel += times.kernel_ms;
    }
    let rgba = renderer.read_pixels().expect("sync");
    std::fs::create_dir_all(&out_dir).expect("output dir");
    image::save_buffer(
        out_dir.join("stress.png"),
        &rgba,
        W,
        H,
        image::ColorType::Rgba8,
    )
    .expect("png");
    let n = f64::from(frames);
    eprintln!(
        "stress: tick {:.2} ms, looks {:.2} ms, kernel {:.2} ms (CPU), {:.2} ms/frame",
        tick * 1e3 / n,
        script / n,
        kernel / n,
        start.elapsed().as_secs_f64() * 1e3 / n
    );
}

/// Lanternfall E4: the cost of per-entity animation — 40 skinned heroes
/// (the battle royale's crowd), each posed by its own animation, against
/// the same 40 sharing the mesh's one. Run through the real script →
/// snapshot → kernel path (`host3d::render_frame`); prints kernel ms per
/// frame (CPU: posing, palette, instance upload, encoding).
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn render_animated_crowd() {
    twec::bundle::set_asset_root(Some("examples/action3d".into()));
    let crowd = |own: bool| {
        format!(
            "camera.eye = vec3(0, 14, 14)\ncamera.target = vec3(0, 0, 0)\n\
             entity Hero:\n    var pos = vec3(0, 0, 0)\n    look:\n        mesh: \"assets/hero.glb\"\n\
             for i in 0..<40:\n    spawn Hero at vec3(math.mod(i, 8) * 2.0 - 7.0, 0, math.floor(i / 8) * 2.0 - 4.0)\n\
             var k = 0\nfor h in entities.of(Hero):\n    k += 1\n\
             \x20   if {own}:\n        mesh_anim.play(h, \"run\", true)\n        mesh_anim.advance(0.013)\n\
             mesh_anim.play(\"assets/hero.glb\", \"walk\", true)\n\
             on update(dt):\n    mesh_anim.advance(dt)\n"
        )
    };
    for own in [false, true] {
        let src = crowd(own);
        let program = twec::parser::parse(&twec::lexer::lex(&src).unwrap()).unwrap();
        let mut env = twec::value::Env::new();
        twec::stdlib::install(&mut env);
        twec::eval::run_top_level(&mut env, &program).unwrap();
        let mut renderer = pollster::block_on(Renderer::new_headless(W, H)).expect("gpu");
        let mut assets = twec::play3d::NativeAssets::default();
        for _ in 0..200 {
            twec::eval::tick_frame(&mut env, 1.0 / 60.0).unwrap();
            twec::host3d::render_frame(&mut renderer, &mut env, &mut assets).unwrap();
            if assets.pending() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mut kernel = 0.0;
        for _ in 0..FRAMES {
            twec::eval::tick_frame(&mut env, 1.0 / 60.0).unwrap();
            kernel += twec::host3d::render_frame(&mut renderer, &mut env, &mut assets)
                .unwrap()
                .kernel_ms;
        }
        renderer.read_pixels().expect("sync");
        let label = if own {
            "own animation each"
        } else {
            "one shared animation"
        };
        eprintln!(
            "40 skinned heroes, {label:<22}: kernel {:6.3} ms/frame",
            kernel / f64::from(FRAMES)
        );
    }
}
