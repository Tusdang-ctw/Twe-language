//! Lanternfall E2: `fx.*`, screen shake and hit-stop in 3D games, plus
//! the three bugs fixed alongside — the screen shake drew from the
//! script's random numbers, physics stepped while paused, and the 3D
//! path never ran `fx.hit_stop`.

use twec::draw2d::Draw2d;
use twec::host3d::{sim_tick, InputState};

fn load(src: &str) -> twec::value::Env {
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::fx::clear();
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    env
}

fn probe(env: &mut twec::value::Env, src: &str) -> String {
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    env.out.clear();
    twec::eval::run_top_level(env, &program).expect("probe");
    env.out.trim().to_string()
}

#[test]
fn hit_stop_freezes_3d_ticks() {
    let mut env = load("var n = 0\non update(dt):\n    n += 1\nfx.hit_stop(0.05)\n");
    let mut input = InputState::default();
    for _ in 0..5 {
        sim_tick(&mut env, &mut input, 1.0 / 60.0).expect("tick");
    }
    // 0.05 s is 3 ticks frozen; the other 2 ran.
    assert_eq!(probe(&mut env, "print(n)"), "2");
}

#[test]
fn physics_holds_still_while_paused() {
    let mut env = load(
        "physics.reset()\nphysics.gravity(vec3(0, -9.8, 0))\nlet b = physics.body(\"box\", vec3(0, 10, 0), 1.0)\npause(true)\n",
    );
    let mut input = InputState::default();
    for _ in 0..20 {
        sim_tick(&mut env, &mut input, 1.0 / 60.0).expect("tick");
    }
    assert_eq!(probe(&mut env, "print(math.floor(physics.position(b).y * 100))"), "1000");
    probe(&mut env, "pause(false)");
    for _ in 0..20 {
        sim_tick(&mut env, &mut input, 1.0 / 60.0).expect("tick");
    }
    let y: i64 = probe(&mut env, "print(math.floor(physics.position(b).y * 100))").parse().unwrap();
    assert!(y < 1000, "falls once unpaused: {y}");
}

#[test]
fn screen_shake_leaves_the_scripts_random_numbers_alone() {
    let draw = |shake: bool| {
        let mut env = load(if shake { "fx.screen_shake(8.0, 1.0)\n" } else { "let x = 1\n" });
        for _ in 0..10 {
            let (_, r) = twec::frame2d::render(&mut env, 1.0 / 60.0, 640.0, 480.0);
            r.expect("render");
        }
        probe(&mut env, "print(random.float())")
    };
    assert_eq!(draw(true), draw(false));
}

#[test]
fn a_vec3_anchors_an_effect_in_the_world() {
    load("fx.damage_number(vec3(2, 0, 0), 42, color.red)\nfx.damage_number((10, 20), 7, color.red)\n");
    let mut out = Vec::new();
    // Wherever the world point projects this frame, the number is there.
    twec::fx::fx_draw_overlay_3d(&mut out, &|p| {
        assert_eq!(p, [2.0, 0.0, 0.0]);
        Some([300.0, 200.0])
    });
    let texts: Vec<(String, f32, f32)> = out
        .iter()
        .filter_map(|d| match d {
            Draw2d::Text { text, x, y, .. } => Some((text.clone(), *x, *y)),
            _ => None,
        })
        .collect();
    assert!(texts.contains(&("42".to_string(), 300.0, 200.0)), "{texts:?}");
    // A 2-tuple position is a canvas point, as in 2D.
    assert!(texts.contains(&("7".to_string(), 10.0, 20.0)), "{texts:?}");
    // Behind the camera: not drawn.
    let mut hidden = Vec::new();
    twec::fx::fx_draw_overlay_3d(&mut hidden, &|_| None);
    assert_eq!(hidden.len(), 1, "only the canvas-point effect");
}

#[test]
fn camera_project_follows_the_shake() {
    let mut env = load("camera.eye = vec3(0, 10, 10)\ncamera.target = vec3(0, 0, 0)\n");
    let still = probe(&mut env, "print(math.floor(camera.project(vec3(0, 0, 0)).x))");
    let cam = twec::camera3d::read_camera(&env);
    twec::camera3d::set_shake(twec::camera3d::shake_offset(&cam, 6.0, 0.0));
    let shaken = probe(&mut env, "print(math.floor(camera.project(vec3(0, 0, 0)).x))");
    // A 6-unit shake to the right moves the world 6 units left on screen.
    assert_eq!(still, "320");
    assert_eq!(shaken, "314");
    // Aiming ignores the shake.
    let aim = probe(&mut env, "print(math.floor(camera.ground_point((320, 240), 0.0).x * 100))");
    assert_eq!(aim, "0");
    twec::camera3d::set_shake([0.0; 3]);
}

#[test]
fn time_scale_slows_the_game_and_its_physics() {
    let mut env = load(
        "var t = 0.0\non update(dt):\n    t += dt\nphysics.reset()\nphysics.gravity(vec3(0, -9.8, 0))\nlet b = physics.body(\"box\", vec3(0, 50, 0), 1.0)\n",
    );
    let mut input = InputState::default();
    let run = |env: &mut twec::value::Env, input: &mut InputState, n: u32| {
        for _ in 0..n {
            sim_tick(env, input, 1.0 / 60.0).expect("tick");
        }
    };
    run(&mut env, &mut input, 60);
    let normal_t: f64 = probe(&mut env, "print(t)").parse().unwrap();
    let y1: f64 = probe(&mut env, "print(physics.position(b).y)").parse().unwrap();
    probe(&mut env, "time.set_scale(0.25)");
    assert_eq!(probe(&mut env, "print(time.scale)"), "0.25");
    run(&mut env, &mut input, 60);
    let slow_t: f64 = probe(&mut env, "print(t)").parse().unwrap();
    let y2: f64 = probe(&mut env, "print(physics.position(b).y)").parse().unwrap();
    // A second of ticks at a quarter speed is a quarter second of game.
    assert!((normal_t - 1.0).abs() < 1e-6, "{normal_t}");
    assert!((slow_t - 1.25).abs() < 1e-6, "{slow_t}");
    // The box fell less in that second than in the first one at speed.
    assert!(50.0 - y1 > (y1 - y2) * 1.5, "fell {} then {}", 50.0 - y1, y1 - y2);
    probe(&mut env, "time.set_scale(1.0)");
}
