//! Lanternfall E3: the `look:` keys `rotation:`, a vec3 `scale:` and
//! `glow:`, and the `quat.from_euler` / `quat.look_along` builders —
//! what the script hands the renderer, checked without a GPU (the pixel
//! checks are in `tests/kernel_render.rs`).

use twec::render3d_types::DrawCall3d;

/// Run `src` and one 3D render frame; return the queued draws or the
/// render error's message and help.
fn draws(src: &str) -> Result<Vec<DrawCall3d>, String> {
    let program = twec::parser::parse(&twec::lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).map_err(|e| e.message)?;
    match twec::eval::render_frame3d(&mut env) {
        Ok(()) => Ok(env.render_queue3d.clone()),
        Err(e) => Err(format!("{} | {}", e.message, e.help.unwrap_or_default())),
    }
}

fn entity(keys: &str) -> String {
    format!("entity Thing:\n    var pos = vec3(1, 2, 3)\n    look:\n        mesh: \"cube\"\n{keys}spawn Thing\n")
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

#[test]
fn defaults_leave_a_look_untouched() {
    let d = draws(&entity("")).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].rotation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(d[0].scale, [1.0; 3]);
    assert_eq!(d[0].glow, [0.0; 3]);
    assert_eq!(d[0].size, 1.0);
}

#[test]
fn a_vec3_scale_is_per_axis_and_a_number_is_uniform() {
    let d = draws(&entity("        scale: vec3(0.1, 0.2, 4.0)\n")).unwrap();
    assert_eq!((d[0].size, d[0].scale), (1.0, [0.1, 0.2, 4.0]));
    let d = draws(&entity("        scale: 2.5\n")).unwrap();
    assert_eq!((d[0].size, d[0].scale), (2.5, [1.0; 3]));
}

#[test]
fn glow_takes_a_number_or_a_colour() {
    let d = draws(&entity("        glow: 3.0\n")).unwrap();
    assert_eq!(d[0].glow, [3.0; 3]);
    let d = draws(&entity("        glow: (1.0, 0.5, 0.0)\n")).unwrap();
    assert_eq!(d[0].glow, [1.0, 0.5, 0.0]);
}

#[test]
fn per_entity_keys_follow_each_entity() {
    let src = "entity Hit:\n    var pos = vec3(0, 0, 0)\n    var hurt = 0.0\n    look:\n        mesh: \"cube\"\n        glow: hurt * 4.0\n\
               spawn Hit\nspawn Hit at vec3(2, 0, 0)\nfor h in entities.of(Hit):\n    if h.pos.x > 1.0:\n        h.hurt = 1.0\n";
    let mut glows: Vec<f32> = draws(src).unwrap().iter().map(|d| d.glow[0]).collect();
    glows.sort_by(f32::total_cmp);
    assert_eq!(glows, vec![0.0, 4.0]);
}

#[test]
fn euler_yaw_matches_axis_angle_and_look_along_points_the_front() {
    let src = entity("        rotation: quat.from_euler(0.0, 1.0, 0.0)\n");
    let a = draws(&src).unwrap()[0].rotation;
    let src = entity("        rotation: quat.from_axis_angle(vec3(0, 1, 0), 1.0)\n");
    let b = draws(&src).unwrap()[0].rotation;
    assert!(close(&a, &b), "{a:?} vs {b:?}");
    // Looking along +X is a quarter turn about +Y (as `facing` 90°).
    let src = entity("        rotation: quat.look_along(vec3(5, 0, 0))\n");
    let q = draws(&src).unwrap()[0].rotation;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    assert!(close(&q, &[0.0, h, 0.0, h]), "{q:?}");
    // Straight down: the front (+Z) turns to -Y.
    let src = entity("        rotation: quat.look_along(vec3(0, -1, 0))\n");
    let q = draws(&src).unwrap()[0].rotation;
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    // q rotates +Z to (2(xz + wy), 2(yz - wx), 1 - 2(x² + y²)).
    let front = [
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    ];
    assert!(close(&front, &[0.0, -1.0, 0.0]), "{front:?}");
}

#[test]
fn bad_values_name_the_fix() {
    let err = draws(&entity("        rotation: 5\n")).unwrap_err();
    assert!(err.contains("quat.from_euler"), "{err}");
    let err = draws(&entity("        scale: \"big\"\n")).unwrap_err();
    assert!(err.contains("vec3"), "{err}");
    let err = draws(&entity("        glow: \"bright\"\n")).unwrap_err();
    assert!(err.contains("glow:"), "{err}");
}
