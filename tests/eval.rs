use std::fs;
use std::path::Path;

use twec::{eval, lexer, parser};

fn run_program(path: &str) -> Result<String, String> {
    let src = fs::read_to_string(Path::new(path))
        .unwrap_or_else(|e| panic!("could not read {path}: {e}"));
    let tokens = lexer::lex(&src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    eval::run(&program).map_err(|e| format!("eval: {e}"))
}

fn run_program_frames(path: &str, frames: u32, dt: f64) -> Result<String, String> {
    let src = fs::read_to_string(Path::new(path))
        .unwrap_or_else(|e| panic!("could not read {path}: {e}"));
    let tokens = lexer::lex(&src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    eval::run_with_frames(&program, frames, dt).map_err(|e| format!("eval: {e}"))
}

#[test]
fn runs_hello() {
    let out = run_program("tests/programs/hello.twe").expect("program should run");
    assert_eq!(out, "hello, twe\n");
}

#[test]
fn runs_let_int() {
    let out = run_program("tests/programs/let_int.twe").expect("program should run");
    assert_eq!(out, "42\n");
}

#[test]
fn undefined_name_errors() {
    let err = run_program_str("print(missing)\n").expect_err("should fail");
    assert!(err.contains("'missing' is not defined"), "got: {err}");
}

#[test]
fn runs_arithmetic() {
    let out = run_program("tests/programs/arithmetic.twe").expect("program should run");
    assert_eq!(
        out,
        "14\n20\n4\n4\n-7\ntrue\ntrue\n42\n"
    );
}

#[test]
fn comparison_chaining_errors() {
    let err = run_program_str("print(1 < 2 < 3)\n").expect_err("should fail");
    assert!(
        err.contains("comparison operators do not chain"),
        "got: {err}"
    );
}

#[test]
fn division_by_zero_errors() {
    let err = run_program_str("print(1 / 0)\n").expect_err("should fail");
    assert!(err.contains("division by zero"), "got: {err}");
}

#[test]
fn type_mismatch_in_arithmetic_errors() {
    let err = run_program_str("print(1 + \"two\")\n").expect_err("should fail");
    assert!(err.contains("'+'"), "got: {err}");
    assert!(err.contains("string"), "got: {err}");
}

#[test]
fn runs_tuples_and_fields() {
    let out = run_program("tests/programs/tuples_and_fields.twe").expect("program should run");
    assert_eq!(
        out,
        "(3, 4)\n3\n4\n200\n150\n(200, 150)\n(250, 130)\n"
    );
}

#[test]
fn invalid_assignment_target_errors() {
    let err = run_program_str("1 + 2 = 3\n").expect_err("should fail");
    assert!(err.contains("invalid assignment target"), "got: {err}");
}

#[test]
fn missing_field_errors() {
    let err = run_program_str(
        "let h = load(\"tests/assets/hero.png\")\nprint(h.glubjorm)\n",
    )
    .expect_err("should fail");
    assert!(err.contains("'glubjorm'"), "got: {err}");
}

#[test]
fn runs_floats() {
    let out = run_program("tests/programs/floats.twe").expect("program should run");
    assert_eq!(out, "3.14\n6.28\n1.5\n0.0015\n2.5\ntrue\ntrue\n");
}

#[test]
fn runs_if_else_chain() {
    let src = r#"
let x = 5
if x < 3:
    print("small")
elif x < 10:
    print("medium")
else:
    print("large")
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "medium\n");
}

#[test]
fn runs_single_line_if() {
    let src = "let x = 1\nif x == 1: print(\"one\")\n";
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "one\n");
}

#[test]
fn runs_example_1_three_frames() {
    let out = run_program_frames("tests/programs/example_1.twe", 3, 0.1)
        .expect("program should run");
    assert_eq!(
        out,
        "(220.0, 150)\n(240.0, 150)\n(260.0, 150)\n"
    );
}

#[test]
fn on_update_outside_v01_event_set_errors() {
    // v0.1 accepts `on update(dt):` and `on render():` at the top
    // level; anything else (named events, predicates) is reserved
    // for state bodies.
    let err = run_program_str("on click(e):\n    print(e)\n").expect_err("should fail");
    assert!(
        err.contains("`on click` is not supported"),
        "got: {err}"
    );
}

#[test]
fn runs_literals() {
    let out = run_program("tests/programs/literals.twe").expect("program should run");
    assert_eq!(out, "10..15\n0..<5\n5%\n3kg\n1.5s\n");
}

#[test]
fn runs_example_2_simplified() {
    let out = run_program("tests/programs/example_2_simplified.twe")
        .expect("program should run");
    assert_eq!(out, "20..30\n5%\n3kg\nrare\n");
}

#[test]
fn runs_methods_and_self() {
    let out = run_program("tests/programs/methods.twe").expect("program should run");
    assert_eq!(out, "0\n5\n12\n");
}

#[test]
fn extending_undefined_parent_errors() {
    let err = run_program_str("item Foo extends Missing:\n    x: 1\n")
        .expect_err("should fail");
    assert!(err.contains("Missing"), "got: {err}");
    assert!(err.contains("not defined"), "got: {err}");
}

#[test]
fn calling_class_with_args_errors_in_v01() {
    let err = run_program_str("item Foo:\n    x: 1\nlet a = Foo(1, 2)\n")
        .expect_err("should fail");
    assert!(err.contains("constructor"), "got: {err}");
}

#[test]
fn runs_functions_and_recursion() {
    let out = run_program("tests/programs/functions.twe").expect("program should run");
    assert_eq!(out, "5\n6\n42\n0\n1\n13\n");
}

#[test]
fn return_at_top_level_errors() {
    let err = run_program_str("return 1\n").expect_err("should fail");
    assert!(err.contains("`return`"), "got: {err}");
}

#[test]
fn runs_loops() {
    let out = run_program("tests/programs/loops.twe").expect("program should run");
    // Inclusive `0..2` iterates 0, 1, 2 (three rounds), so the nested
    // section prints 6 lines (a=0,b=0; a=1,b=0; a=2,b=0).
    assert_eq!(
        out,
        "0\n1\n2\n---\n0\n1\n2\n3\n---\n0\n1\n2\n---\n0\n1\n3\n---\n0\n0\n1\n0\n2\n0\n"
    );
}

#[test]
fn break_at_top_level_errors() {
    let err = run_program_str("break\n").expect_err("should fail");
    assert!(err.contains("`break`"), "got: {err}");
}

#[test]
fn continue_at_top_level_errors() {
    let err = run_program_str("continue\n").expect_err("should fail");
    assert!(err.contains("`continue`"), "got: {err}");
}

#[test]
fn runs_type_annotations() {
    let out = run_program("tests/programs/type_annotations.twe")
        .expect("program should run");
    assert_eq!(out, "12\nhi\n");
}

#[test]
fn runs_math_stdlib() {
    let out = run_program("tests/programs/math.twe").expect("program should run");
    assert_eq!(
        out,
        "7\n3.14\n3.0\n1.4142135623730951\n2\n3\n1\n3\n0.5\n"
    );
}

#[test]
fn runs_lists() {
    let out = run_program("tests/programs/lists.twe").expect("program should run");
    let expected = "[1, 2, 3]\n3\n1\n2\n3\n\
                    [1, 2, 3, 4]\n[0, 1, 2, 3, 4]\n\
                    4\n[0, 1, 2, 3]\n\
                    0\n[1, 2, 3]\n\
                    true\nfalse\ntrue\n\
                    true\n\
                    true\nfalse\nfalse\n\
                    2\n3\n\
                    true\nfalse\n\
                    6\n";
    assert_eq!(out, expected);
}

#[test]
fn list_index_out_of_bounds_errors() {
    let err = run_program_str("let xs = [1, 2]\nprint(xs[5])\n").expect_err("should fail");
    assert!(err.contains("out of bounds"), "got: {err}");
}

#[test]
fn pop_back_on_empty_errors() {
    let err = run_program_str("let xs = []\nxs.pop_back()\n").expect_err("should fail");
    assert!(err.contains("empty list"), "got: {err}");
}

#[test]
fn runs_random_stdlib() {
    let out = run_program("tests/programs/random.twe").expect("program should run");
    assert_eq!(out, "ok int\nok float\nok choice\nok determinism\n");
}

#[test]
fn random_int_on_empty_range_errors() {
    let err = run_program_str("print(random.int(5..<5))\n").expect_err("should fail");
    assert!(err.contains("empty range"), "got: {err}");
}

#[test]
fn random_choice_on_empty_list_errors() {
    let err = run_program_str("print(random.choice([]))\n").expect_err("should fail");
    assert!(err.contains("empty list"), "got: {err}");
}

#[test]
fn runs_interpolation() {
    let out = run_program("tests/programs/interpolation.twe").expect("program should run");
    let expected = "hello, Twe!\n\
                    Score: 42\n\
                    42 + 42 = 84\n\
                    at (10, 20)\n\
                    first = 1, length = 3\n\
                    a \\ b \"q\" c\n\
                    brace: {not interpolated}\n";
    assert_eq!(out, expected);
}

#[test]
fn unterminated_interpolation_errors() {
    let err = run_program_str("print(\"{x\")\n").expect_err("should fail");
    assert!(err.contains("interpolation") || err.contains("unterminated"), "got: {err}");
}

#[test]
fn runs_scene_counter_with_state_machine() {
    // 10 frames of dt=0.1s. Counter ticks at 100ms, so each frame
    // should fire exactly once. After 3 ticks, `-> done` transitions
    // out and the remaining 7 frames are silent.
    let out = run_program_frames("tests/programs/scene_counter.twe", 10, 0.1)
        .expect("program should run");
    assert_eq!(out, "1\n2\n3\n");
}

#[test]
fn runs_scene_with_render_handler_headlessly() {
    // `on render():` is registered but never called by `twec run`; the
    // headless harness only ticks scenes via `tick_frame` (no render
    // context). The every-clock body should still run normally.
    let out = run_program_frames("tests/programs/scene_with_render.twe", 10, 0.016)
        .expect("program should run");
    assert_eq!(out, "5\n10\n15\n20\n25\n30\n");
}

#[test]
fn tuple_arithmetic() {
    let src = r#"
let head = (10, 7)
let dir = (1, 0)
print(head + dir)
print(head - dir)
print(head * 3)
print(2 * head)
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "(11, 7)\n(9, 7)\n(30, 21)\n(20, 14)\n");
}

#[test]
fn for_over_list_iterates_each_element() {
    let src = r#"
let xs = [10, 20, 30]
let total = 0
for x in xs:
    total += x
print(total)
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "60\n");
}

#[test]
fn for_over_tuple_iterates_each_element() {
    let src = r#"
let t = (1, 2, 3, 4)
let s = 0
for x in t:
    s += x
print(s)
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "10\n");
}

#[test]
fn key_press_handler_fires_when_pressed() {
    let src = r#"
scene S:
    var counter = 0

    initial: a

    state a:
        on key_press.right:
            counter += 1
            print(counter)
            if counter >= 2:
                -> b

    state b:
        on key_press.right:
            print("done")
"#;
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");

    // Simulate three frames where key_press.right is set.
    set_key_press(&env, "right", true);
    twec::eval::tick_frame(&mut env, 0.016).expect("frame 1");
    twec::eval::tick_frame(&mut env, 0.016).expect("frame 2");
    twec::eval::tick_frame(&mut env, 0.016).expect("frame 3");
    assert_eq!(env.out, "1\n2\ndone\n");
}

fn set_key_press(env: &twec::value::Env, key: &str, value: bool) {
    use twec::value::Value;
    if let Some(Value::Object(rc)) = env.get("key_press") {
        rc.borrow_mut()
            .fields
            .insert(key.to_string(), Value::Bool(value));
    }
}

// --- v0.2 session 3: mouse surface ---

#[test]
fn stdlib_installs_mouse_objects() {
    use twec::value::Value;
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);

    // mouse: x, y, pos, wheel
    let Some(Value::Object(rc)) = env.get("mouse") else {
        panic!("mouse object missing after stdlib::install");
    };
    let m = rc.borrow();
    assert!(matches!(m.fields.get("x"), Some(Value::Float(_))));
    assert!(matches!(m.fields.get("y"), Some(Value::Float(_))));
    assert!(matches!(m.fields.get("pos"), Some(Value::Tuple(_))));
    assert!(matches!(m.fields.get("wheel"), Some(Value::Float(_))));

    // mouse_held / mouse_press: left, middle, right
    for name in ["mouse_held", "mouse_press"] {
        let Some(Value::Object(rc)) = env.get(name) else {
            panic!("{name} object missing after stdlib::install");
        };
        let o = rc.borrow();
        for btn in ["left", "middle", "right"] {
            assert!(
                matches!(o.fields.get(btn), Some(Value::Bool(false))),
                "{name}.{btn} missing or non-bool after install"
            );
        }
    }
}

#[test]
fn mouse_position_drives_on_update_logic() {
    // Simulate the pattern `play.rs` uses: write to the mouse
    // ambient before each tick_frame, then have the script read
    // and react. Here the script accumulates `mouse.x` into a
    // running total via on_update — proves the field is reachable
    // from the frame loop the same way `key.right` is.
    use twec::value::Value;
    let src = r#"
var total = 0.0
on update(dt):
    total = total + mouse.x
"#;
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");

    set_mouse_x(&env, 10.0);
    twec::eval::tick_frame(&mut env, 0.016).expect("tick");
    set_mouse_x(&env, 20.0);
    twec::eval::tick_frame(&mut env, 0.016).expect("tick");
    set_mouse_x(&env, 30.0);
    twec::eval::tick_frame(&mut env, 0.016).expect("tick");

    let total = match env.get("total") {
        Some(Value::Float(f)) => *f,
        other => panic!("expected total to be Float, got {other:?}"),
    };
    assert_eq!(total, 60.0);
}

#[test]
fn mouse_press_left_drives_branching() {
    // Edge-triggered mouse_press.left fires the body once per
    // press. Three frames: press, no-press, press again.
    use twec::value::Value;
    let src = r#"
var clicks = 0
on update(dt):
    if mouse_press.left:
        clicks = clicks + 1
"#;
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");

    set_mouse_press(&env, "left", true);
    twec::eval::tick_frame(&mut env, 0.016).expect("tick");
    set_mouse_press(&env, "left", false);
    twec::eval::tick_frame(&mut env, 0.016).expect("tick");
    set_mouse_press(&env, "left", true);
    twec::eval::tick_frame(&mut env, 0.016).expect("tick");

    let clicks = match env.get("clicks") {
        Some(Value::Int(n)) => *n,
        other => panic!("expected clicks to be Int, got {other:?}"),
    };
    assert_eq!(clicks, 2);
}

fn set_mouse_x(env: &twec::value::Env, x: f64) {
    use twec::value::Value;
    if let Some(Value::Object(rc)) = env.get("mouse") {
        rc.borrow_mut()
            .fields
            .insert("x".to_string(), Value::Float(x));
    }
}

fn set_mouse_press(env: &twec::value::Env, button: &str, value: bool) {
    use twec::value::Value;
    if let Some(Value::Object(rc)) = env.get("mouse_press") {
        rc.borrow_mut()
            .fields
            .insert(button.to_string(), Value::Bool(value));
    }
}

// --- v0.2 session 4: save_to / load_from ---

#[test]
fn save_to_and_load_from_round_trip_a_tuple() {
    let dir = std::env::temp_dir();
    let path = dir.join("twec_eval_save_round_trip.json");
    let _ = std::fs::remove_file(&path);
    let path_str = path.to_str().expect("temp path is valid UTF-8");

    let src = format!(
        r#"
let p = "{}"
save_to(p, (42, 3.14, "hi", true))
let back = load_from(p)
print(back)
"#,
        path_str.replace('\\', "\\\\")
    );
    let out = run_program_str(&src).expect("program should run");
    let _ = std::fs::remove_file(&path);
    // Tuple round-trips back as a tuple. Display format mirrors
    // `Value::display`'s tuple printer.
    assert!(
        out.contains("42") && out.contains("3.14") && out.contains("hi") && out.contains("true"),
        "expected round-tripped tuple in output, got: {out:?}"
    );
}

#[test]
fn load_from_missing_file_errors_at_runtime() {
    let src = r#"let _ = load_from("nope-not-a-real-save.json")"#;
    let err = run_program_str(src).expect_err("should fail");
    assert!(err.contains("cannot read"), "got: {err}");
}

// --- v0.2 session 5: audio v2 ---

#[test]
fn stdlib_installs_audio_v2_surface() {
    use twec::value::Value;
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);

    // sound module: load + play (v0.1) + play_at + stop +
    // set_volume (v0.2 session 5).
    let Some(Value::Object(rc)) = env.get("sound") else {
        panic!("sound object missing after stdlib::install");
    };
    let s = rc.borrow();
    for name in ["load", "play", "play_at", "stop", "set_volume"] {
        assert!(
            matches!(s.fields.get(name), Some(Value::Builtin { .. })),
            "sound.{name} missing or not a builtin"
        );
    }

    // music module: play, play_at, stop. New in v0.2 session 5.
    let Some(Value::Object(rc)) = env.get("music") else {
        panic!("music object missing after stdlib::install");
    };
    let m = rc.borrow();
    for name in ["play", "play_at", "stop"] {
        assert!(
            matches!(m.fields.get(name), Some(Value::Builtin { .. })),
            "music.{name} missing or not a builtin"
        );
    }
}

// --- v0.2 session 6: tilemap ---

#[test]
fn tilemap_builds_grid_from_layout_and_specs() {
    let src = r###"
let layout = "##.\n.~.\n..."
let map = tilemap(
    layout: layout,
    tile_size: 16,
    tiles: [
        ("#", "wall", ["solid"]),
        (".", "floor", ["walkable"]),
        ("~", "water", ["walkable", "slow"]),
    ]
)
print(map.width)
print(map.height)
print(map.tile_size)
"###;
    let out = run_program_str(src).expect("should run");
    assert_eq!(out, "3\n3\n16\n");
}

#[test]
fn tilemap_at_reports_tile_name_per_pixel() {
    // 3x3 map, tile_size = 16. Pixel (0,0) in row 0 col 0 -> '#'.
    // Pixel (16,0) -> col 1 row 0 -> '#'. Pixel (32,16) -> col 2 row 1 -> '.'.
    let src = r###"
let map = tilemap(
    layout: "##.\n.~.\n...",
    tile_size: 16,
    tiles: [
        ("#", "wall", ["solid"]),
        (".", "floor", ["walkable"]),
        ("~", "water", ["walkable", "slow"]),
    ]
)
print(tilemap_at(map, 0, 0))
print(tilemap_at(map, 16, 0))
print(tilemap_at(map, 32, 16))
print(tilemap_at(map, 999, 999))
"###;
    let out = run_program_str(src).expect("should run");
    assert_eq!(out, "wall\nwall\nfloor\n\n");
}

#[test]
fn tilemap_solid_at_reflects_solid_trait() {
    let src = r###"
let map = tilemap(
    layout: "#.\n..",
    tile_size: 8,
    tiles: [
        ("#", "wall", ["solid"]),
        (".", "floor", ["walkable"]),
    ]
)
print(tilemap_solid_at(map, 0, 0))
print(tilemap_solid_at(map, 8, 0))
print(tilemap_solid_at(map, 99, 99))
"###;
    let out = run_program_str(src).expect("should run");
    assert_eq!(out, "true\nfalse\nfalse\n");
}

#[test]
fn tilemap_rejects_malformed_tiles_arg() {
    let src = r###"
let map = tilemap(
    layout: "...",
    tile_size: 16,
    tiles: [(".", "floor")]
)
"###;
    let err = run_program_str(src).expect_err("3-tuple required");
    assert!(
        err.contains("3 fields"),
        "got: {err}"
    );
}

#[test]
fn audio_builtins_reject_non_sound_handles() {
    // Each new audio builtin should error clearly when given
    // something that isn't a sound handle. Pin it for sound.stop
    // (the only builtin reachable without actually decoding the
    // file — others fail later at filesystem read).
    let src = r#"sound.stop("not a handle")"#;
    let err = run_program_str(src).expect_err("string isn't a handle");
    assert!(
        err.contains("sound.stop") && err.contains("sound handle"),
        "got: {err}"
    );
}

#[test]
fn save_to_refuses_a_function_value() {
    let src = r#"
function greet(): nil
save_to("ignored.json", greet)
"#;
    let err = run_program_str(src).expect_err("functions can't save");
    assert!(
        err.contains("function") && err.contains("data, not code"),
        "got: {err}"
    );
}

#[test]
fn spawn_and_despawn_drive_entity_updates() {
    let out = run_program_frames("tests/programs/spawn_entities.twe", 5, 0.016)
        .expect("program should run");
    // Two entities, each ticking. Frame 1: both print 1. Frame 2: both
    // print 2 then despawn. Frames 3-5 are empty.
    assert_eq!(out, "1\n1\n2\n2\n");
}

#[test]
fn survive_parses_and_ticks() {
    // Smoke test: examples/survive.twe parses and ticks without error.
    // After ~2 seconds of 16ms frames the spawn_timer (which the
    // scene increments by 0.016 per tick) crosses 0.8 and a monster
    // gets spawned.
    let src = std::fs::read_to_string("examples/survive.twe")
        .expect("examples/survive.twe must exist");
    let tokens = twec::lexer::lex(&src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");
    for _ in 0..120 {
        twec::eval::tick_frame(&mut env, 0.016).expect("tick");
    }
    assert!(
        !env.active_entities.is_empty(),
        "expected at least one monster to spawn within ~2s"
    );
}

#[test]
fn load_fails_fast_on_missing_asset() {
    let err = run_program_str("let h = load(\"nope-not-a-real-path.png\")\n")
        .expect_err("should fail");
    assert!(err.contains("cannot find asset"), "got: {err}");
    assert!(err.contains("nope-not-a-real-path.png"), "got: {err}");
}

#[test]
fn load_returns_handle_with_path_field() {
    use twec::value::Value;
    let src = r#"
let h = load("tests/assets/hero.png")
print(h.path)
print(h.x)
print(h.y)
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "tests/assets/hero.png\n0\n0\n");
    let _ = Value::Nil;
}

#[test]
fn sound_load_returns_handle_with_path_field() {
    let src = r#"
let s = sound.load("tests/assets/silence.wav")
print(s.path)
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "tests/assets/silence.wav\n");
}

#[test]
fn sound_load_fails_fast_on_missing_asset() {
    let err = run_program_str("let s = sound.load(\"nope.wav\")\n")
        .expect_err("should fail");
    assert!(err.contains("cannot find asset"), "got: {err}");
}

#[test]
fn sound_play_rejects_non_handle() {
    let err = run_program_str("sound.play(42)\n").expect_err("should fail");
    assert!(err.contains("sound.play"), "got: {err}");
    assert!(err.contains("handle"), "got: {err}");
}

#[test]
fn sprite_outside_render_errors() {
    let src = r#"let h = load("tests/assets/hero.png")
on update(dt):
    sprite(h, (0, 0))
"#;
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let err = twec::eval::run_with_frames(&program, 1, 0.016).expect_err("should fail");
    assert!(err.message.contains("on render"), "got: {}", err.message);
}

#[test]
fn or_and_value_returning_semantics() {
    // `or` / `and` are value-returning, short-circuit, Python-like.
    // `not` is strict-Bool. Only `false` is falsy in Twe (Principle 3),
    // so `0`, `nil`, "" are all truthy. Locks in the F11 decision per
    // docs/changes/2026-04-28-or-and-keep-value-returning.md.
    let src = r#"
print(true and 42)
print(false and 42)
print(true or 99)
print(false or 99)
print(0 or "default")
print(false or "default")
print(not true)
print(not false)
print(not 0)
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(
        out,
        "42\nfalse\ntrue\n99\n0\ndefault\nfalse\ntrue\nfalse\n"
    );
}

#[test]
fn state_scoped_on_update_fires_per_frame_with_dt() {
    // Top-level on_update runs first (prints "top N"), then the
    // active state's on_update (prints "a N dt=..."). After 2 fires
    // in state a, transition to b. Frame 3 prints "top 3" then the
    // new state's on_update.
    let out = run_program_frames("tests/programs/state_on_update.twe", 3, 0.5)
        .expect("program should run");
    assert_eq!(
        out,
        "top 1\na 1 dt=0.5\ntop 2\na 2 dt=0.5\ntop 3\nb dt=0.5\n"
    );
}

#[test]
fn every_clock_catches_up_when_dt_covers_multiple_intervals() {
    // dt=0.5, interval=100ms: each frame deserves 5 fires. Two frames
    // → counter reaches 10. (Was 2 with the F4 bug.)
    let out = run_program_frames("tests/programs/catchup.twe", 2, 0.5)
        .expect("program should run");
    let lines: Vec<&str> = out.trim_end().split('\n').collect();
    assert_eq!(lines, vec!["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]);
}

#[test]
fn every_clock_catchup_caps_at_eight_per_frame() {
    // dt=2.0, interval=100ms: 20 deserved fires per frame, but the cap
    // is 8 and residual is dropped. Two frames → 16 fires total.
    let out = run_program_frames("tests/programs/catchup_capped.twe", 2, 2.0)
        .expect("program should run");
    let lines: Vec<&str> = out.trim_end().split('\n').collect();
    let nums: Vec<i32> = lines.iter().map(|s| s.parse().unwrap()).collect();
    assert_eq!(nums.len(), 16, "expected 16 fires (8 per frame, capped)");
    assert_eq!(nums.last().copied(), Some(16));
}

#[test]
fn time_dt_reflects_real_frame_delta() {
    // 3 frames of dt = 0.05s. Every-clock fires every 16ms, so it
    // fires once per frame and `time.dt` returns the frame's dt.
    let out = run_program_frames("tests/programs/time_dt.twe", 3, 0.05)
        .expect("program should run");
    assert_eq!(out, "0.05\n0.1\n0.15000000000000002\n");
}

#[test]
fn particles_emitter_ages_and_despawns() {
    // Frame 1: on update prints count=1; tick ages particles to 0.05.
    // Frame 2: prints count=1; tick ages to 0.11 > 0.1 lifetime, all
    //           particles dead, emitter despawns, prune drops it.
    // Frame 3: prints count=0.
    let out = run_program_frames("tests/programs/particles_block.twe", 3, 0.05)
        .expect("program should run");
    let lines: Vec<&str> = out.trim_end().split('\n').collect();
    assert_eq!(lines, vec!["1", "1", "0"]);
}

#[test]
fn particles_block_creates_count_particles_with_defaults() {
    use twec::value::Value;
    let src = r#"
particles Spark:
    count: 4
    lifetime: 5.0

spawn Spark at (50.0, 60.0)
"#;
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");
    assert_eq!(env.active_entities.len(), 1);
    let inst = env.active_entities[0].borrow();
    let particles = inst.fields.get("__particles").expect("__particles");
    let n = match particles {
        Value::List(rc) => rc.borrow().len(),
        _ => panic!("__particles should be a list"),
    };
    assert_eq!(n, 4);
}

#[test]
fn keyword_args_distribute_to_function_params_by_name() {
    let src = r#"
function rect_area(w, h):
    return w * h

print(rect_area(w: 4, h: 3))
print(rect_area(h: 5, w: 6))
print(rect_area(7, h: 8))
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "12\n30\n56\n");
}

#[test]
fn keyword_args_unknown_name_errors() {
    let src = r#"function add(a, b):
    return a + b
print(add(a: 1, c: 2))
"#;
    let err = run_program_str(src).expect_err("should fail");
    assert!(err.contains("no parameter named `c`"), "got: {err}");
}

#[test]
fn keyword_args_duplicate_binding_errors() {
    let src = r#"function add(a, b):
    return a + b
print(add(1, a: 2))
"#;
    let err = run_program_str(src).expect_err("should fail");
    assert!(err.contains("already bound"), "got: {err}");
}

#[test]
fn keyword_args_missing_param_errors() {
    let src = r#"function add(a, b):
    return a + b
print(add(a: 1))
"#;
    let err = run_program_str(src).expect_err("should fail");
    assert!(err.contains("missing argument"), "got: {err}");
    assert!(err.contains("`b`"), "got: {err}");
}

#[test]
fn positional_after_keyword_is_a_parse_error() {
    let src = "print(a: 1, 2)\n";
    let err = run_program_str(src).expect_err("should fail");
    assert!(
        err.contains("positional argument cannot follow keyword"),
        "got: {err}"
    );
}

#[test]
fn keyword_args_on_variadic_builtin_errors() {
    let err =
        run_program_str("print(label: \"hi\")\n").expect_err("should fail");
    assert!(
        err.contains("doesn't accept keyword arguments"),
        "got: {err}"
    );
}

#[test]
fn entities_of_returns_only_live_instances_of_class() {
    let out = run_program("tests/programs/entity_query.twe").expect("program should run");
    // 3 mobs spawned, 1 bullet. entities.count returns 3, 1.
    // entities.of(Mob).length is 3. Each mob's hp default is 1.
    assert_eq!(out, "3\n1\n3\n1\n1\n1\n");
}

#[test]
fn bullet_despawns_overlapping_monster_and_bumps_kills() {
    // Per tick_frame: top-level on update(dt) runs before entities.
    // Frame 1: prints kills=0, then bullet collides, kills becomes 1.
    // Frame 2: prints kills=1; bullet is gone, nothing else happens.
    let out = run_program_frames("tests/programs/bullet_collision.twe", 2, 0.016)
        .expect("program should run");
    assert_eq!(out, "0\n1\n");
}

#[test]
fn entities_of_with_non_class_errors() {
    let err = run_program_str("print(entities.of(42))\n").expect_err("should fail");
    assert!(err.contains("entities.of"), "got: {err}");
    assert!(err.contains("class"), "got: {err}");
}

#[test]
fn spawn_at_sets_pos_field() {
    use twec::value::Value;
    let src = r#"
entity Pin:
    var pos = (0, 0)
    function update(dt):
        # nothing
spawn Pin at (12, 34)
"#;
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");
    assert_eq!(env.active_entities.len(), 1);
    let inst = env.active_entities[0].borrow();
    let pos = inst.fields.get("pos").expect("pos field");
    let elems = match pos {
        Value::Tuple(elems) => elems.clone(),
        _ => panic!("pos should be a tuple"),
    };
    assert!(matches!(elems[0], Value::Int(12)));
    assert!(matches!(elems[1], Value::Int(34)));
}

#[test]
fn scene_methods_callable_by_bare_name() {
    let out = run_program_frames("tests/programs/scene_methods.twe", 20, 0.1)
        .expect("program should run");
    // a: bump() three times → 1, 2, 3, transition to b.
    // b: bump_by(10) three times → 13, 23, 33, transition to done.
    assert_eq!(out, "1\n2\n3\n13\n23\n33\n");
}

#[test]
fn snake_advances_right_by_default() {
    use twec::value::Value;
    let src = std::fs::read_to_string("examples/snake.twe")
        .expect("examples/snake.twe must exist");
    let tokens = twec::lexer::lex(&src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");

    // No keys held, no presses. Tick exactly one full step (150ms).
    twec::eval::tick_frame(&mut env, 0.150).expect("tick");

    let scene = env.active_scene.as_ref().expect("scene");
    let inst = scene.borrow();
    let snake = inst.fields.get("snake").expect("snake field");
    let head = match snake {
        Value::List(rc) => rc.borrow()[0].clone(),
        _ => panic!("snake should be a list"),
    };
    let (hx, hy) = match head {
        Value::Tuple(elems) => match (&elems[0], &elems[1]) {
            (Value::Int(x), Value::Int(y)) => (*x, *y),
            _ => panic!("head should be (Int, Int)"),
        },
        _ => panic!("head should be a tuple"),
    };
    // Snake starts at (10, 7) heading right; after one 150ms tick the
    // head should be at (11, 7).
    assert_eq!((hx, hy), (11, 7));
}

#[test]
fn snake_dies_into_a_wall() {
    use twec::value::Value;
    let src = std::fs::read_to_string("examples/snake.twe")
        .expect("examples/snake.twe must exist");
    let tokens = twec::lexer::lex(&src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");

    // Snake heads right. Grid is 20 wide. From x=10 it takes 9
    // ticks to land on x=19, and the 10th to walk off the east wall.
    for _ in 0..10 {
        twec::eval::tick_frame(&mut env, 0.150).expect("tick");
    }
    let scene = env.active_scene.as_ref().expect("scene");
    let state_name = scene
        .borrow()
        .current_state
        .clone()
        .expect("current state");
    assert_eq!(state_name, "game_over");
    // Snake eats the food at (15, 7) on the way, so score is 1 by the
    // time it walks off the east wall at x=20.
    let inst = scene.borrow();
    let score = inst.fields.get("score").expect("score field");
    assert!(matches!(score, Value::Int(1)), "got: {score:?}");
}

#[test]
fn rect_outside_render_errors() {
    let src = r#"on update(dt):
    rect((0, 0), (10, 10), (1.0, 0.0, 0.0))
"#;
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let err = twec::eval::run_with_frames(&program, 1, 0.016).expect_err("should fail");
    assert!(err.message.contains("on render"), "got: {}", err.message);
}

#[test]
fn transition_to_unknown_state_errors() {
    let src = r#"scene S:
    initial: a
    state a:
        every 100ms:
            -> nope
"#;
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    // The error fires at scene tick time when the transition runs.
    let err = twec::eval::run_with_frames(&program, 1, 0.1).expect_err("should fail");
    assert!(err.message.contains("nope"), "got: {}", err.message);
}

#[test]
fn range_roll_returns_value_in_range() {
    // Deterministic: same seed → same sequence. Just check the values
    // fall inside the range.
    let src = r#"
let r = 1..6
let total = 0
for i in 0..<10:
    let n = r.roll()
    if n < 1 or n > 6:
        print("out of range")
        return
print("ok")
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "ok\n");
}

fn run_program_str(src: &str) -> Result<String, String> {
    let tokens = lexer::lex(src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    eval::run(&program).map_err(|e| format!("eval: {e}"))
}

// --- Phase 5 task 2: cooperative fibers via `wait <duration>` ---

#[test]
fn wait_in_state_suspends_until_duration_elapses() {
    // Frame 1 (dt=0.25): enter `alert` → "alert enter" → wait 0.5s.
    //                    Suspended with 0.5s remaining.
    // Frame 2 (dt=0.25): tick scene → 0.25s left, still suspended.
    //                    No prints.
    // Frame 3 (dt=0.25): tick scene → wait elapses, resume from
    //                    statement after `wait` → "alert resume" →
    //                    -> done → enter `done` → "done enter".
    let out = run_program_frames("tests/programs/wait_in_state.twe", 3, 0.25)
        .expect("program should run");
    assert_eq!(out, "alert enter\nalert resume\ndone enter\n");
}

#[test]
fn wait_resumes_in_one_frame_when_dt_covers_duration() {
    // Same program, but a single frame with dt = 1.0 covers the
    // full 0.5s wait — the resume runs in the same frame and
    // produces all three lines back-to-back.
    let out = run_program_frames("tests/programs/wait_in_state.twe", 1, 1.0)
        .expect("program should run");
    assert_eq!(out, "alert enter\nalert resume\ndone enter\n");
}

#[test]
fn wait_outside_state_body_is_a_runtime_error() {
    // `wait` only works at the top level of a state body in v0.1.
    // Using it inside a function body should error with a clear
    // help message rather than silently doing nothing.
    let src = r#"
function pause():
    wait 0.5s

pause()
"#;
    let err = run_program_str(src).expect_err("should fail");
    assert!(
        err.contains("`wait` is only supported"),
        "expected the wait-context error, got: {err}"
    );
}

#[test]
fn wait_inside_if_then_branch_resumes() {
    // v0.2 session 2a: `wait` now works inside `if` / `elif` /
    // `else` / `while` blocks at the top level of a state's
    // on_entry body. Two frames with dt=0.1: frame 1 hits the
    // wait inside the if-then; frame 2's tick elapses the wait
    // and resumes from the next stmt in the then-body.
    let src = r#"
scene Demo:
    initial: a
    state a:
        if true:
            print("before")
            wait 0.1s
            print("after")
        print("done")
"#;
    let out = run_program_frames_str(src, 2, 0.1).expect("should run");
    assert_eq!(out, "before\nafter\ndone\n");
}

#[test]
fn wait_inside_if_else_resumes_in_else_branch() {
    // The runner must remember which branch was taken at
    // suspension time so resume re-enters the same body
    // (without re-evaluating the condition / its side effects).
    let src = r#"
scene Demo:
    initial: a
    state a:
        if false:
            print("then-side")
        else:
            print("else-before")
            wait 0.1s
            print("else-after")
        print("done")
"#;
    let out = run_program_frames_str(src, 2, 0.1).expect("should run");
    assert_eq!(out, "else-before\nelse-after\ndone\n");
}

#[test]
fn wait_inside_elif_arm_resumes_same_arm() {
    // Pin the elif-arm preservation: arm index is recorded on
    // suspension so resume picks the same arm without
    // re-evaluating elif conditions.
    let src = r#"
scene Demo:
    initial: a
    state a:
        if false:
            print("first")
        elif true:
            print("elif-before")
            wait 0.1s
            print("elif-after")
        else:
            print("else")
        print("done")
"#;
    let out = run_program_frames_str(src, 2, 0.1).expect("should run");
    assert_eq!(out, "elif-before\nelif-after\ndone\n");
}

#[test]
fn wait_inside_while_loop_resumes_each_iteration() {
    // 3 iterations × 0.1s wait each = 0.3s of wait. With
    // dt=0.1 we need 4 frames: frame 1 hits the first wait,
    // frame 2 resumes + prints + hits 2nd wait, etc. (the
    // resumed iteration eagerly re-enters the body when the
    // wait elapses inside the same frame, then loops on cond).
    let src = r#"
scene Demo:
    initial: a
    state a:
        var i = 0
        while i < 3:
            print("step")
            wait 0.1s
            i = i + 1
        print("done")
"#;
    let out = run_program_frames_str(src, 4, 0.1).expect("should run");
    assert_eq!(out, "step\nstep\nstep\ndone\n");
}

#[test]
fn wait_nested_blocks_program_runs() {
    // Real .twe program checked into tests/programs to keep an
    // on-disk reference for the nested-wait machinery. Same
    // shape as the inline wait_inside_while_inside_if test
    // but committed so future readers see the expected trace.
    let out = run_program_frames("tests/programs/wait_nested_blocks.twe", 4, 0.2)
        .expect("program should run");
    assert_eq!(
        out,
        "outer\ninner-pre\ninner-post\ninner-pre\ninner-post\nouter-after\ndone\n"
    );
}

#[test]
fn wait_inside_while_inside_if_two_levels_deep() {
    // Nesting: wait suspended at depth 2 (top → if-then →
    // while-body). Each level pushes its own PathEntry so
    // resume navigates back through both descents.
    let src = r#"
scene Demo:
    initial: a
    state a:
        if true:
            var i = 0
            while i < 2:
                print("inner")
                wait 0.1s
                i = i + 1
        print("done")
"#;
    let out = run_program_frames_str(src, 3, 0.1).expect("should run");
    assert_eq!(out, "inner\ninner\ndone\n");
}

#[test]
fn wait_in_function_program_runs() {
    // Real .twe program checked into tests/programs as a
    // committed reference for the function-body wait machinery.
    let out = run_program_frames("tests/programs/wait_in_function.twe", 3, 0.1)
        .expect("program should run");
    assert_eq!(
        out,
        "entry\nfirst\nfirst\nsecond\nsecond\nafter-call\ndone\n"
    );
}

#[test]
fn wait_inside_function_called_from_state_entry_resumes() {
    // v0.2 session 2b: a function called as `Stmt::Expr` from a
    // state's on_entry can `wait`. The fiber stack holds the
    // function frame on top of the state-entry frame; on resume
    // the function body completes, then the state-entry's
    // post-call statements run.
    let src = r#"
function pause_then_log():
    print("pre-wait")
    wait 0.1s
    print("post-wait")

scene Demo:
    initial: a
    state a:
        print("entry")
        pause_then_log()
        print("after-call")
"#;
    let out = run_program_frames_str(src, 2, 0.1).expect("should run");
    assert_eq!(out, "entry\npre-wait\npost-wait\nafter-call\n");
}

#[test]
fn wait_inside_function_inside_if_resumes() {
    // The function call sits inside an if-then branch of the
    // state entry. Two frames on the fiber stack: state entry
    // (path through the if) + function body (path past the wait).
    let src = r#"
function nap(label: string):
    print(label)
    wait 0.1s
    print(label)

scene Demo:
    initial: a
    state a:
        if true:
            print("inside-if")
            nap("napping")
        print("done")
"#;
    let out = run_program_frames_str(src, 2, 0.1).expect("should run");
    assert_eq!(
        out,
        "inside-if\nnapping\nnapping\ndone\n"
    );
}

#[test]
fn two_sequential_waiting_calls_run_in_order() {
    // Two function calls back-to-back, each with a wait.
    // Frame 1: enter, first call's pre-wait. Suspended.
    // Frame 2: first call's post-wait, second call's pre-wait.
    //          Suspended.
    // Frame 3: second call's post-wait, "done".
    let src = r#"
function step(label: string):
    print(label)
    wait 0.1s
    print(label)

scene Demo:
    initial: a
    state a:
        step("first")
        step("second")
        print("done")
"#;
    let out = run_program_frames_str(src, 3, 0.1).expect("should run");
    assert_eq!(
        out,
        "first\nfirst\nsecond\nsecond\ndone\n"
    );
}

#[test]
fn function_calls_function_with_wait() {
    // Two function frames on top of the state-entry frame at
    // suspension time. Resume drains them outermost-first
    // (innermost — `inner` — finishes, then `outer` finishes,
    // then the state-entry continues).
    let src = r#"
function inner():
    print("inner-pre")
    wait 0.1s
    print("inner-post")

function outer():
    print("outer-pre")
    inner()
    print("outer-post")

scene Demo:
    initial: a
    state a:
        outer()
        print("done")
"#;
    let out = run_program_frames_str(src, 2, 0.1).expect("should run");
    assert_eq!(
        out,
        "outer-pre\ninner-pre\ninner-post\nouter-post\ndone\n"
    );
}

#[test]
fn wait_in_function_called_from_outside_state_still_errors() {
    // Calling a wait-bearing function from top-level (not
    // inside a state on_entry) still errors — there's no fiber
    // context to suspend into. Same error as before.
    let src = r#"
function pause():
    wait 0.5s

pause()
"#;
    let err = run_program_str(src).expect_err("should fail");
    assert!(
        err.contains("`wait` is only supported"),
        "expected the wait-context error, got: {err}"
    );
}

fn run_program_frames_str(src: &str, frames: u32, dt: f64) -> Result<String, String> {
    let tokens = lexer::lex(src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    eval::run_with_frames(&program, frames, dt).map_err(|e| format!("eval: {e}"))
}

// --- Phase 5 task 4: predicate hooks ---

#[test]
fn predicate_hook_fires_on_false_to_true_transition() {
    // chase's every-clock decrements hp by 25 each 100ms; once hp
    // hits 30, the `on hp <= 30:` predicate fires and transitions
    // to flee. flee's clock decrements hp by 50; `on hp <= 0:`
    // transitions to dead.
    let out = run_program_frames("tests/programs/predicate_hook.twe", 50, 0.020)
        .expect("program should run");
    let lines: Vec<&str> = out.trim_end().split('\n').collect();
    assert!(
        lines.starts_with(&["chase"]),
        "expected to start in chase, got {lines:?}"
    );
    assert!(
        lines.contains(&"flee"),
        "expected predicate to fire and reach flee, got {lines:?}"
    );
    assert!(
        lines.contains(&"dead"),
        "expected second predicate to reach dead, got {lines:?}"
    );
    // Edge-triggered: each state's print lines appear exactly once
    // (no re-firing while predicate stays true).
    assert_eq!(lines.iter().filter(|s| **s == "flee").count(), 1);
    assert_eq!(lines.iter().filter(|s| **s == "dead").count(), 1);
}

#[test]
fn predicate_hook_does_not_re_fire_while_stable_true() {
    // Predicate is true on first frame and stays true. Body must
    // run exactly once (false → true edge), not every frame.
    let src = r#"
scene S:
    var fired: int = 0
    initial: a
    state a:
        on true:
            fired += 1
            print(fired)

on update(dt):
    pass_var = 0
"#;
    // The dummy `on update(dt)` lets us tick frames; body is a no-op
    // assignment. Run several frames and verify the predicate fired
    // exactly once.
    let _ = src; // The above program won't compile (`pass_var = 0` requires var declaration).
    // Use a simpler shape:
    let src = r#"
scene S:
    var fired: int = 0
    initial: a
    state a:
        on true:
            fired += 1
            print(fired)
"#;
    let out = run_program_frames_str(src, 5, 0.020).expect("program should run");
    // Edge-triggered: only one "1" print across 5 frames.
    assert_eq!(out, "1\n");
}

// --- Phase 5 task 3: dialogue runtime (minimum viable) ---

#[test]
fn dialogue_minimal_runs_say_choice_and_first_branch() {
    let out = run_program("tests/programs/dialogue_minimal.twe")
        .expect("program should run");
    // Bare `say "..."` prints just the text.
    // `say <actor>: "..."` prints `Actor: text`.
    // `choice:` prints the labels (numbered) and runs the first branch.
    assert_eq!(
        out,
        "Welcome, traveler.\n\
         Merchant: Looking to trade?\n\
         \x20\x20[1] Yes, show me your wares.\n\
         \x20\x20[2] Just browsing.\n\
         Merchant: Gold first.\n"
    );
}

#[test]
fn actor_keyword_is_alias_for_let() {
    // `actor merchant = scene.npc(...)` reads cleaner inside a
    // dialogue than `let merchant = ...`. Implementation: lex
    // `actor` as a keyword that dispatches to `parse_let`. No
    // semantic difference.
    let src = r#"
entity Merchant:
    name: "default"

dialogue Trade:
    actor merchant = Merchant()
    say merchant: "Welcome."

Trade()
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "Merchant: Welcome.\n");
}

#[test]
fn dialogue_with_actor_identifier_uses_class_name() {
    // Per the design, an instance actor renders as its class name
    // (Wren-style). This makes `say merchant: "..."` produce
    // `Merchant: ...` when `merchant` is a `merchant` instance.
    let src = r#"
entity Merchant:
    name: "default"

let merchant = Merchant()

dialogue Trade:
    say merchant: "Welcome to the shop."

Trade()
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "Merchant: Welcome to the shop.\n");
}

#[test]
fn dialogue_choice_first_branch_is_deterministic() {
    // V0.1 always picks branch 0. This test pins that behavior so
    // a future change to interactive selection doesn't silently
    // break programs that rely on the deterministic shape.
    let src = r#"
dialogue Pick:
    choice:
        "first":
            print("a")
        "second":
            print("b")

Pick()
"#;
    let out = run_program_str(src).expect("program should run");
    // The labels come first, then the body of branch 0.
    assert_eq!(out, "  [1] first\n  [2] second\na\n");
}

#[test]
fn dialogue_wait_inside_dialogue_body_is_a_runtime_error() {
    // V0.1 ships dialogue without per-dialogue suspension. `wait`
    // inside a dialogue body still hits the same runtime error
    // every other non-state-entry context produces, so the
    // limitation is consistent.
    let src = r#"
dialogue Pause:
    say "before"
    wait 0.1s
    say "after"

Pause()
"#;
    let err = run_program_str(src).expect_err("should fail");
    assert!(
        err.contains("`wait` is only supported"),
        "expected the wait-context error, got: {err}"
    );
}
