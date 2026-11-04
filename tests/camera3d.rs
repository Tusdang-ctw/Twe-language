//! Lanternfall E1: the 3D camera's world <-> HUD mapping.
//!
//! `camera.project` / `camera.ray` / `camera.ground_point` use the same
//! view and projection math the renderer does (`twec::camera3d`), at
//! the aspect of the view the host reports through `InputState`. These
//! tests check the math (round trips at 4:3 and 16:9, the nil cases,
//! `camera.fov`) and the script surface end to end through `sim_tick`.

use twec::camera3d::{ground_point, hud_size, project, ray, Camera3d};
use twec::host3d::{sim_tick, InputState};

fn top_down() -> Camera3d {
    Camera3d::new([0.0, 13.0, 9.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0])
}

fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
}

#[test]
fn hud_is_480_tall_and_as_wide_as_the_view() {
    assert_eq!(hud_size(640, 480), [640.0, 480.0]);
    assert_eq!(hud_size(1280, 960), [640.0, 480.0]);
    let [w, h] = hud_size(1920, 1080);
    assert_eq!(h, 480.0);
    assert!((w - 853.333).abs() < 0.01, "16:9 width {w}");
    // No view yet: the 2D canvas.
    assert_eq!(hud_size(0, 0), [640.0, 480.0]);
}

#[test]
fn the_target_projects_to_the_hud_centre() {
    let cam = top_down();
    for (w, h) in [(640, 480), (1920, 1080), (1080, 1920)] {
        let hud = hud_size(w, h);
        let aspect = w as f32 / h as f32;
        let p = project(&cam, aspect, hud, cam.target).expect("in front");
        assert!(close(p, [hud[0] * 0.5, hud[1] * 0.5]), "{w}x{h}: {p:?}");
    }
}

#[test]
fn ground_point_and_project_round_trip() {
    let cam = top_down();
    for (w, h) in [(640, 480), (1920, 1080)] {
        let hud = hud_size(w, h);
        let aspect = w as f32 / h as f32;
        for xy in [[10.0, 20.0], [hud[0] * 0.5, 400.0], [hud[0] - 5.0, 300.0]] {
            let g = ground_point(&cam, aspect, hud, xy, 0.0).expect("hits the ground");
            assert!(g[1].abs() < 1e-5);
            let back = project(&cam, aspect, hud, g).expect("in front");
            assert!(close(back, xy), "{w}x{h}: {xy:?} -> {g:?} -> {back:?}");
        }
    }
}

#[test]
fn ray_points_at_what_project_maps() {
    let cam = top_down();
    let hud = hud_size(1920, 1080);
    let aspect = 1920.0 / 1080.0;
    let p = [3.0, 1.5, -2.0];
    let xy = project(&cam, aspect, hud, p).expect("in front");
    let (origin, dir) = ray(&cam, aspect, hud, xy);
    let to_p = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
    let len = (to_p[0] * to_p[0] + to_p[1] * to_p[1] + to_p[2] * to_p[2]).sqrt();
    let cos = (to_p[0] * dir[0] + to_p[1] * dir[1] + to_p[2] * dir[2]) / len;
    assert!(cos > 0.99999, "ray misses the point: cos {cos}");
}

#[test]
fn nil_behind_the_camera_and_for_rays_that_never_reach_the_plane() {
    let cam = top_down();
    let hud = hud_size(640, 480);
    // Behind the eye.
    assert!(project(&cam, 4.0 / 3.0, hud, [0.0, 20.0, 20.0]).is_none());
    // Above the eye: the ray through the top of the view goes up and
    // away, never reaching y = 50 in front of the camera.
    assert!(ground_point(&cam, 4.0 / 3.0, hud, [320.0, 0.0], 50.0).is_none());
    // A level camera's centre ray runs parallel to the ground.
    let level = Camera3d::new([0.0, 2.0, 0.0], [0.0, 2.0, -10.0], [0.0, 1.0, 0.0]);
    assert!(ground_point(&level, 4.0 / 3.0, hud, [320.0, 240.0], 0.0).is_none());
}

#[test]
fn fov_widens_the_view() {
    let hud = hud_size(640, 480);
    let p = [4.0, 0.0, 0.0];
    let narrow = {
        let mut c = top_down();
        c.fov_y = 30_f32.to_radians();
        project(&c, 4.0 / 3.0, hud, p).unwrap()
    };
    let wide = {
        let mut c = top_down();
        c.fov_y = 90_f32.to_radians();
        project(&c, 4.0 / 3.0, hud, p).unwrap()
    };
    // The same point sits further from the centre with a narrower lens.
    assert!(
        narrow[0] - 320.0 > wide[0] - 320.0,
        "{narrow:?} vs {wide:?}"
    );
}

/// Run `src`, give the simulation a `w × h` view and one tick, then
/// return what the probe printed.
fn run_with_view(src: &str, w: u32, h: u32) -> String {
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top level");
    let mut input = InputState::default();
    input.set_view(w, h);
    input.pointer(0.5, 0.5);
    sim_tick(&mut env, &mut input, 1.0 / 60.0).expect("tick");
    env.out.clear();
    let probe = r#"
let c = camera.project(camera.target)
let g = camera.ground_point(mouse.pos, 0.0)
let r = camera.ray(mouse.pos)
print("{math.floor(c.x)} {math.floor(c.y)} {math.floor(screen.size.x)} {math.floor(screen.aspect * 100)}")
print("{math.floor(g.x * 100)} {math.floor(g.z * 100)} {math.floor(r.dir.y * 100)}")
print("{camera.project(vec3(0, 50, 50))}")
"#;
    let program = twec::parser::parse(&twec::lexer::lex(probe).expect("lex")).expect("parse");
    twec::eval::run_top_level(&mut env, &program).expect("probe");
    env.out.clone()
}

#[test]
fn the_script_sees_the_view_it_is_drawn_to() {
    let src = "camera.eye = vec3(0, 13, 9)\ncamera.target = vec3(0, 0, 0)\n";
    let out = run_with_view(src, 1920, 1080);
    let lines: Vec<&str> = out.lines().collect();
    // The target sits at the centre of an 853×480 HUD; aspect 1.77.
    assert_eq!(lines[0], "426 240 853 177", "{out}");
    // The cursor in the middle of the view looks at the target.
    assert_eq!(lines[1], "0 0 -83", "{out}");
    // A point behind the camera projects to nil.
    assert_eq!(lines[2], "nil", "{out}");
}

#[test]
fn camera_fov_is_a_script_field() {
    let out_default = run_with_view(
        "camera.eye = vec3(0, 13, 9)\ncamera.target = vec3(0, 0, 0)\n",
        640,
        480,
    );
    let out_wide = run_with_view(
        "camera.eye = vec3(0, 13, 9)\ncamera.target = vec3(0, 0, 0)\ncamera.fov = 90.0\n",
        640,
        480,
    );
    // The lens doesn't move the centre, or what is under a centred
    // cursor; it moves everything off-centre.
    assert_eq!(out_default.lines().next(), out_wide.lines().next());
    let probe = |fov: &str| {
        let src = format!(
            "camera.eye = vec3(0, 13, 9)\ncamera.target = vec3(0, 0, 0)\n{fov}\nprint(\"{{math.floor(camera.project(vec3(4, 0, 0)).x)}}\")\n"
        );
        let program = twec::parser::parse(&twec::lexer::lex(&src).expect("lex")).expect("parse");
        let mut env = twec::value::Env::new();
        twec::stdlib::install(&mut env);
        twec::eval::run_top_level(&mut env, &program).expect("run");
        env.out.trim().parse::<i64>().expect("number")
    };
    assert!(probe("camera.fov = 30.0") > probe("camera.fov = 90.0"));
}
