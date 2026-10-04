//! web3d-M6: input through the 2D frame step (`frame2d::tick` with a
//! shell's `InputState`), driven by a replay log, headless.
//!
//! The native 2D shell (`play2d`) feeds input exactly this way, so these
//! pin what a player sees: a click reaches a render-time widget, and a
//! pause menu still gets its clicks while the simulation is paused (the
//! paused frames are recorded and replayed too).

use twec::host3d::InputState;

/// Run `script` for `frames` frames of one tick each, playing `replay`
/// (lines after the header), and return the env.
fn run(name: &str, script: &str, replay: &[String], frames: u32) -> twec::value::Env {
    let dir = std::path::Path::new("target/frame2d-input");
    std::fs::create_dir_all(dir).expect("dir");
    let log = dir.join(format!("{name}.replay"));
    let mut text = String::from("TWE-REPLAY v2\n");
    for line in replay {
        text.push_str(line);
        text.push('\n');
    }
    std::fs::write(&log, text).expect("write replay");
    twec::replay::start_playing(log.to_str().unwrap()).expect("start replay");
    let mut env = twec::frame2d::load(script).expect("load");
    let mut input = InputState::default();
    let mut acc = 0.0;
    let dt = twec::eval::PHYSICS_DT;
    for _ in 0..frames {
        twec::frame2d::tick(&mut env, Some(&mut input), &mut acc, dt).expect("tick");
        let (_, result) = twec::frame2d::render(&mut env, dt, 640.0, 480.0);
        result.expect("render");
    }
    twec::replay::stop();
    env
}

fn idle(x: u32, y: u32, n: usize) -> Vec<String> {
    vec![format!("||{x}|{y}|||0|0|0|||"); n]
}

fn click(x: u32, y: u32) -> Vec<String> {
    vec![
        format!("||{x}|{y}|left|left|0|0|0|||"),
        format!("||{x}|{y}|left||0|0|0|||"),
    ]
}

fn global_int(env: &twec::value::Env, name: &str) -> i64 {
    env.get(name).expect(name).as_int()
}

#[test]
fn clicks_reach_a_render_time_button() {
    let mut log = Vec::new();
    for _ in 0..3 {
        log.extend(idle(100, 160, 10));
        log.extend(click(100, 160));
    }
    let env = run("clicks", "examples/button_demo.twe", &log, 60);
    assert_eq!(global_int(&env, "count"), 3);
}

#[test]
fn a_pause_menu_gets_clicks_while_paused() {
    // Escape pauses (from `on update`); the menu's Resume button, drawn
    // only while paused, unpauses.
    let mut log = idle(200, 300, 30);
    log.push("escape|escape|200|300|||0|0|0|||".to_string());
    log.extend(idle(200, 426, 29));
    log.extend(click(200, 426));
    log.extend(idle(200, 426, 40));
    let env = run("pause_menu", "examples/pause_menu_demo.twe", &log, 100);
    assert!(!twec::stdlib::is_paused(), "Resume should have unpaused");
    let elapsed = env.get("elapsed").expect("elapsed").as_float();
    // 31 running ticks before the pause, 40ish after; none while paused.
    assert!((1.0..1.3).contains(&elapsed), "elapsed {elapsed}");
}

#[test]
fn dimension_is_detected_from_the_game() {
    use twec::frame2d::Dimension;
    let two = ["pong", "snake", "tetris", "button_demo", "particles_demo"];
    for name in two {
        let path = format!("examples/{name}.twe");
        let mut env = twec::frame2d::load(&path).expect("load");
        assert_eq!(
            twec::frame2d::detect_dimension(&mut env),
            Dimension::Two,
            "{name} should be 2D"
        );
    }
    // A 3D example uses cube()/sphere() or a look: block.
    let three = ["hello_3d", "hello_glb"];
    for name in three {
        let path = format!("examples/{name}.twe");
        let mut env = twec::frame2d::load(&path).unwrap_or_else(|e| panic!("load {name}: {e}"));
        assert_eq!(
            twec::frame2d::detect_dimension(&mut env),
            Dimension::Three,
            "{name} should be 3D"
        );
    }
}
