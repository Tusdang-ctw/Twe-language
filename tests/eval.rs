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
    let err = run_program_str("on click(e):\n    print(e)\n").expect_err("should fail");
    assert!(err.contains("only `on update(dt):`"), "got: {err}");
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
fn time_dt_reflects_real_frame_delta() {
    // 3 frames of dt = 0.05s. Every-clock fires every 16ms, so it
    // fires once per frame and `time.dt` returns the frame's dt.
    let out = run_program_frames("tests/programs/time_dt.twe", 3, 0.05)
        .expect("program should run");
    assert_eq!(out, "0.05\n0.1\n0.15000000000000002\n");
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
