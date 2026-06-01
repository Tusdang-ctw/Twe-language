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
    assert_eq!(out, "14\n20\n4\n4\n-7\ntrue\ntrue\n42\n");
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
    assert_eq!(out, "(3, 4)\n3\n4\n200\n150\n(200, 150)\n(250, 130)\n");
}

#[test]
fn invalid_assignment_target_errors() {
    let err = run_program_str("1 + 2 = 3\n").expect_err("should fail");
    assert!(err.contains("invalid assignment target"), "got: {err}");
}

#[test]
fn missing_field_errors() {
    let err = run_program_str("let h = load(\"tests/assets/hero.png\")\nprint(h.glubjorm)\n")
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
fn runs_if_expression_basic() {
    // Phase 9 follow-on: if-expression form `if c: a else: b`. Closes
    // the latent parser bug where `examples/gamepad_demo.twe` line 9
    // (`let conn_color = if gamepad.connected: color.green else: color.red`)
    // failed with "expected expression, got If."
    let out =
        run_program_str("let n = 5\nlet label = if n > 0: \"pos\" else: \"neg\"\nprint(label)\n")
            .expect("program should run");
    assert_eq!(out, "pos\n");
}

#[test]
fn runs_if_expression_else_branch() {
    let out =
        run_program_str("let n = -2\nlet label = if n > 0: \"pos\" else: \"neg\"\nprint(label)\n")
            .expect("program should run");
    assert_eq!(out, "neg\n");
}

#[test]
fn runs_if_expression_elif_chain() {
    let src = r#"
let x = 7
let bucket = if x < 3: "small" elif x < 10: "medium" else: "large"
print(bucket)
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "medium\n");
}

#[test]
fn if_expression_requires_else() {
    let err = run_program_str("let x = if 1 == 1: \"yes\"\n").expect_err("should fail");
    assert!(
        err.contains("`else:`") || err.contains("else"),
        "got: {err}"
    );
}

#[test]
fn if_expression_inside_call_arg_works() {
    // Use as a function-call argument — the headline use case is
    // `text("count: {x}", ..., color: if dark: color.white else: color.black)`.
    let out = run_program_str("let dark = true\nprint(if dark: \"white\" else: \"black\")\n")
        .expect("program should run");
    assert_eq!(out, "white\n");
}

#[test]
fn runs_example_1_three_frames() {
    let out =
        run_program_frames("tests/programs/example_1.twe", 3, 0.1).expect("program should run");
    assert_eq!(out, "(220.0, 150)\n(240.0, 150)\n(260.0, 150)\n");
}

#[test]
fn on_update_outside_v01_event_set_errors() {
    // v0.1 accepts `on update(dt):` and `on render():` at the top
    // level; anything else (named events, predicates) is reserved
    // for state bodies.
    let err = run_program_str("on click(e):\n    print(e)\n").expect_err("should fail");
    assert!(err.contains("`on click` is not supported"), "got: {err}");
}

#[test]
fn runs_literals() {
    let out = run_program("tests/programs/literals.twe").expect("program should run");
    assert_eq!(out, "10..15\n0..<5\n5%\n3kg\n1.5s\n");
}

#[test]
fn runs_example_2_simplified() {
    let out = run_program("tests/programs/example_2_simplified.twe").expect("program should run");
    assert_eq!(out, "20..30\n5%\n3kg\nrare\n");
}

#[test]
fn runs_methods_and_self() {
    let out = run_program("tests/programs/methods.twe").expect("program should run");
    assert_eq!(out, "0\n5\n12\n");
}

#[test]
fn extending_undefined_parent_errors() {
    let err = run_program_str("item Foo extends Missing:\n    x: 1\n").expect_err("should fail");
    assert!(err.contains("Missing"), "got: {err}");
    assert!(err.contains("not defined"), "got: {err}");
}

#[test]
fn calling_class_with_args_errors_in_v01() {
    let err = run_program_str("item Foo:\n    x: 1\nlet a = Foo(1, 2)\n").expect_err("should fail");
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
    let out = run_program("tests/programs/type_annotations.twe").expect("program should run");
    assert_eq!(out, "12\nhi\n");
}

#[test]
fn runs_math_stdlib() {
    let out = run_program("tests/programs/math.twe").expect("program should run");
    assert_eq!(out, "7\n3.14\n3.0\n1.4142135623730951\n2\n3\n1\n3\n0.5\n");
}

// Phase 27 session 4: Euclidean modulo. Negative operands wrap to
// the [0, b) range — needed for examples/tetris.twe rotation.
#[test]
fn runs_math_mod() {
    let out = run_program("tests/programs/math_mod.twe").expect("program should run");
    assert_eq!(out, "1\n0\n0\n3\n1\n1.5\n0.5\n1\n3\n0\n");
}

#[test]
fn math_mod_by_zero_errors() {
    let err = run_program_str("print(math.mod(7, 0))\n").expect_err("should fail");
    assert!(err.contains("math.mod by zero"), "got: {err}");
}

#[test]
fn vm_death_event_handler_fires_once() {
    // Phase 11 session 10: bytecode-VM mirror of the tree-walker
    // death-event hook. Uses a plain entity that despawns itself
    // (the v0.1 VM compiler rejects `lifetime: 0.1s` particle
    // defaults, hence the separate-from-eval test program).
    use twec::{compiler, lexer, parser, vm};
    let src = fs::read_to_string("tests/programs/death_event_vm.twe").expect("read");
    let tokens = lexer::lex(&src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    let chunk = compiler::compile_program(&program).expect("compile");
    let mut machine = vm::VM::new();
    machine.run(&chunk).expect("vm boot");
    let dt = 0.05;
    for _ in 0..3 {
        machine.tick(dt).expect("tick");
    }
    let out = machine.take_out();
    // Handler fires exactly once even though the entity stays
    // marked despawned across multiple frames before pruning.
    assert_eq!(out, "doomed died\n", "VM output: {out:?}");
}

#[test]
fn runs_death_event_phase9_handler_fires_once() {
    // Phase 9 session 7b: `on <Class>.death(e):` fires when the
    // entity transitions despawned → pruned. We tick frames until
    // the particles emitter ages out (>= 0.1s), then assert the
    // handler ran exactly once. Frame count is conservative — three
    // frames at 0.05s each gets us past the 0.1s lifetime.
    let out = run_program_frames("tests/programs/death_event_phase9.twe", 3, 0.05)
        .expect("program should run");
    assert_eq!(out, "burst died\n");
}

#[test]
fn death_event_unknown_keyword_errors() {
    // Use a non-keyword identifier so the parser reaches the
    // "unknown class event" branch — `spawn` is a Twe keyword and
    // hits the "expected event name" branch first (also a clean
    // error, but tests a different code path).
    let err = run_program_str("on Foo.fire(e):\n    print(\"x\")\n").expect_err("should fail");
    assert!(err.contains("unknown class event"), "got: {err}");
}

#[test]
fn runs_visual_block_phase9_parses_and_no_ops() {
    // Phase 9 session 8: the `visual` keyword + DeclKind::Visual + parser
    // dispatch should accept Example 5's shape and execute the surrounding
    // top-level statements unchanged. The block body itself is parsed but
    // never invoked; the runtime ships in session 11 (after the WGSL
    // codegen in session 10). The trailing `print("ok")` is the proof
    // that nothing exploded.
    let out = run_program("tests/programs/visual_fire.twe").expect("program should run");
    assert_eq!(out, "ok\n");
}

#[test]
fn visual_is_now_a_reserved_keyword() {
    // Adding `visual` to the keyword table means scripts that try to
    // use it as a let-binding name fail at parse time. Scripts have
    // never been able to in practice (docs/06 §10.1 listed it as
    // reserved since v0.1), but the lexer wasn't enforcing it.
    let err = run_program_str("let visual = 5\n").expect_err("should fail");
    // The exact error wording is parser-driven; we just need to
    // confirm `visual` isn't bound as an identifier.
    assert!(
        err.contains("visual") || err.contains("expected") || err.contains("let"),
        "got: {err}"
    );
}

#[test]
fn runs_color_phase9_pipeline() {
    // Phase 9 session 6: color pipeline — from_hex, hsv, gamma helpers,
    // and the two lerp variants (sRGB perceptual + gamma-correct linear).
    // Float reference values pinned to the IEC 61966-2-1 transfer
    // function so the future WGSL counterpart in Phase 9 session 10
    // produces bit-identical output.
    let out = run_program("tests/programs/color_phase9.twe").expect("program should run");
    let expected = "(1.0, 0.0, 0.0, 1.0)\n\
        (0.0, 1.0, 0.0, 1.0)\n\
        (1.0, 0.5333333333333333, 0.0, 1.0)\n\
        (0.5019607843137255, 0.5019607843137255, 0.5019607843137255, 0.5019607843137255)\n\
        (1.0, 0.0, 0.0, 1.0)\n\
        (0.0, 1.0, 0.0, 1.0)\n\
        (0.0, 0.0, 1.0, 1.0)\n\
        (1.0, 1.0, 0.0, 1.0)\n\
        (0.5, 0.5, 0.5, 1.0)\n\
        true\n\
        (0.0, 0.21404114048223255, 1.0, 0.5)\n\
        (0.0, 0.7353569830524495, 0.9999999999999999, 0.5)\n\
        (0.3, 0.6, 0.9, 1.0)\n\
        (0.5, 0.5, 0.5, 1.0)\n\
        (0.5, 0.0, 0.5, 1.0)\n\
        (0.7353569830524495, 0.7353569830524495, 0.7353569830524495, 1.0)\n";
    assert_eq!(out, expected);
}

#[test]
fn color_from_hex_errors_on_bad_input() {
    let err = run_program_str("color.from_hex(\"#zzz\")\n").expect_err("should fail");
    assert!(err.contains("color.from_hex"), "got: {err}");

    let err = run_program_str("color.from_hex(\"#abc\")\n").expect_err("should fail");
    assert!(err.contains("expected 6 or 8 hex digits"), "got: {err}");
}

#[test]
fn runs_gamepad_phase9_surface_defaults() {
    // Phase 9 session 5: `gamepad` / `gamepad_press` / `gamepad_axis`
    // ambients install with all-false / all-zero defaults. The
    // polling impl lives in play.rs (gilrs-driven, requires the
    // macroquad window context) so headless `twec run` is the
    // perfect place to assert nothing else got accidentally set.
    let out = run_program("tests/programs/gamepad_phase9.twe").expect("program should run");
    let mut expected = String::new();
    // 15 booleans (connected + 14 buttons), 1 edge-trigger, 6 axes.
    for _ in 0..16 {
        expected.push_str("false\n");
    }
    for _ in 0..6 {
        expected.push_str("0.0\n");
    }
    assert_eq!(out, expected);
}

#[test]
fn load_font_errors_on_missing_file() {
    // Phase 9 session 4: load_font surface. Positive path needs a real
    // TTF + GL context (text_with_font) so it's exercised by hand via
    // examples/font_demo.twe. Headless tests cover the load-side
    // sanity checks.
    let err = run_program_str("load_font(\"no_such_font.ttf\")\n").expect_err("should fail");
    assert!(err.contains("cannot find asset"), "got: {err}");
}

#[test]
fn load_font_errors_on_bad_format() {
    // hero.png is not a TTF; load_font's parse step should reject it
    // with a clear "not a valid TTF/OTF" message rather than panicking
    // or silently returning a junk handle.
    let err =
        run_program_str("load_font(\"examples/assets/hero.png\")\n").expect_err("should fail");
    assert!(err.contains("is not a valid TTF/OTF font"), "got: {err}");
}

#[test]
fn text_with_font_outside_render_fails_clearly() {
    // require_render fires before the font-handle type check, so the
    // bogus 0 in the font slot never gets validated.
    let err = run_program_str("text_with_font(\"hi\", (0, 0), 24, color.white, 0)\n")
        .expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn runs_atlas_phase9_load_handle() {
    // Phase 9 session 3: load_atlas builds an atlas-kind handle with
    // path + grid fields. The sprite_frame / sprite_frame_at draw
    // calls need a GL context (require_render guards them) so they
    // can't run headless — exercised by hand via examples/atlas_demo.twe.
    let out = run_program("tests/programs/atlas_phase9.twe").expect("program should run");
    let expected = "examples/assets/hero.png\n(4, 2)\n(8, 8)\n(4, 2)\n";
    assert_eq!(out, expected);
}

#[test]
fn load_atlas_errors_on_missing_file() {
    let err =
        run_program_str("load_atlas(\"no_such_file.png\", (4, 2))\n").expect_err("should fail");
    assert!(err.contains("cannot find asset"), "got: {err}");
}

#[test]
fn load_atlas_errors_on_zero_grid() {
    let err = run_program_str("load_atlas(\"examples/assets/hero.png\", (0, 4))\n")
        .expect_err("should fail");
    assert!(err.contains("grid must be positive"), "got: {err}");
}

#[test]
fn sprite_frame_outside_render_fails_clearly() {
    // sprite_frame is require_render-guarded so calling it outside
    // an `on render():` body fails fast with the standard message —
    // the atlas-handle type-check never gets reached headless.
    let err =
        run_program_str("let s = load(\"examples/assets/hero.png\")\nsprite_frame(s, (0, 0), 0)\n")
            .expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn button_outside_render_fails_clearly() {
    // Phase 10 session 1: `button(at:, size:, label:) -> bool` is
    // require_render-guarded — the rendering path can't run outside
    // `on render():`. Hit-test logic + click latching are exercised
    // by hand via `examples/button_demo.twe` (needs a real mouse +
    // GL context); the pure point-in-rect helper has unit tests in
    // `src/stdlib.rs`.
    let err = run_program_str("button((0, 0), (100, 40), \"Resume\")\n").expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn label_outside_render_fails_clearly() {
    // Phase 10 session 2.
    let err = run_program_str("label((0, 0), (100, 40), \"Hello\")\n").expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn progress_bar_outside_render_fails_clearly() {
    // Phase 10 session 2.
    let err = run_program_str("progress_bar((0, 0), (100, 20), 0.5)\n").expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn slider_outside_render_fails_clearly() {
    // Phase 10 session 3.
    let err =
        run_program_str("slider((0, 0), (200, 28), 0.5, 0.0, 1.0)\n").expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn checkbox_outside_render_fails_clearly() {
    // Phase 10 session 4.
    let err = run_program_str("checkbox((0, 0), (24, 24), true)\n").expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn dropdown_outside_render_fails_clearly() {
    // Phase 10 session 4.
    let err = run_program_str("dropdown((0, 0), (200, 28), [\"Low\", \"High\"], 0)\n")
        .expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn text_input_outside_render_fails_clearly() {
    // Phase 10 session 5.
    let err = run_program_str("text_input((0, 0), (200, 28), \"\")\n").expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

// Phase 10 session 5b: clipboard. Functional tests are skipped
// because CI runners typically lack a display server / clipboard
// daemon (X11 / Wayland / NSPasteboard). The `os.clipboard.read`
// path returns the empty string in that case rather than erroring
// — exercised here to confirm the surface is registered.
#[test]
fn clipboard_read_returns_string_or_empty() {
    let out = run_program_str("print(os.clipboard.read())\n").expect("program should run");
    // Either the runner has a clipboard with text in it (then
    // `out` is whatever's there + newline) or it doesn't (then
    // `out == "\n"`). Either way, the call returns a string and
    // the program exits cleanly.
    assert!(out.ends_with('\n'), "got: {out:?}");
}

#[test]
fn clipboard_write_returns_nil() {
    // Write succeeds-or-fails-silently; the return value is nil
    // either way so the script can chain calls without checking.
    let out = run_program_str("os.clipboard.write(\"hello\")\nprint(\"done\")\n")
        .expect("program should run");
    assert_eq!(out, "done\n");
}

#[test]
fn panel_outside_render_fails_clearly() {
    // Phase 10 session 6.
    let err = run_program_str("panel((0, 0), (100, 50))\n").expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn stack_returns_layout_object() {
    // Phase 10 session 6: `stack(at:, size:, count:, index:, gap:)`
    // returns a {at, size} object. `from_float` yields float-display
    // (`10.0` not `10`) for whole values — that's the contract.
    let src = r#"
let slot = stack((10, 20), (200, 200), 4, 0, 4)
print(slot.at)
print(slot.size)
"#;
    let out = run_program_str(src).expect("program should run");
    // total_gap = 4 * 3 = 12, slot_h = (200-12)/4 = 47. y of slot 0 = 20.
    assert!(out.contains("(10.0, 20.0)"), "got: {out:?}");
    assert!(out.contains("(200.0, 47.0)"), "got: {out:?}");
}

#[test]
fn flex_returns_layout_object() {
    let src = r#"
let slot = flex((10, 20), (300, 40), 3, 1, 8)
print(slot.at)
print(slot.size)
"#;
    let out = run_program_str(src).expect("program should run");
    // total_gap = 16, slot_w = (300-16)/3 = 94.666..., slot 1
    // x = 10 + 1 * (slot_w + 8). We assert the height is 40 and
    // y is the input y without pinning the slot_w float exactly.
    assert!(out.contains(", 40.0)"), "got: {out:?}");
    assert!(out.contains(", 20.0)"), "got: {out:?}");
}

#[test]
fn grid_row_major_index_zero_is_top_left() {
    let src = r#"
let slot = grid((100, 200), (240, 120), 4, 2, 0, 0)
print(slot.at)
"#;
    let out = run_program_str(src).expect("program should run");
    assert!(out.contains("(100.0, 200.0)"), "got: {out:?}");
}

#[test]
fn grid_row_major_advances_by_row_then_column() {
    // Phase 10 session 7: cols=4, rows=2, gap=0 means index 4 is the
    // start of row 1 (i.e., directly below index 0 at y = 200 + h/2).
    let src = r#"
let slot = grid((100, 200), (240, 120), 4, 2, 4, 0)
print(slot.at)
"#;
    let out = run_program_str(src).expect("program should run");
    // index 4 → col=0, row=1; slot_h = 120/2 = 60; y = 200 + 60 = 260.
    assert!(out.contains("(100.0, 260.0)"), "got: {out:?}");
}

#[test]
fn scroll_outside_render_works_returns_object() {
    // `scroll` doesn't draw anything — it's a positioning helper
    // backed by per-rect state. Unlike rendering widgets it works
    // outside `on render():`; useful for non-graphical tests and
    // for scripts that compute scroll math during update.
    let src = r#"
let s = scroll((10, 20), (200, 100), 500)
print(s.at)
print(s.size)
print(s.scroll_y)
"#;
    let out = run_program_str(src).expect("program should run");
    assert!(out.contains("(10.0, 20.0)"), "got: {out:?}");
    assert!(out.contains("(200.0, 100.0)"), "got: {out:?}");
    // Initial scroll is 0 — no wheel input under headless.
    assert!(out.contains("0\n") || out.contains("0.0"), "got: {out:?}");
}

#[test]
fn pause_round_trips() {
    // Phase 10 session 8: pause primitives are runtime-state so
    // they're testable without a render loop.
    let src = r#"
print(is_paused())
pause(true)
print(is_paused())
pause(false)
print(is_paused())
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "false\ntrue\nfalse\n");
}

// --- Phase 10 session 9: settings system ---

#[test]
fn settings_set_get_round_trip() {
    let src = r#"
settings.set("audio.master", 0.8)
settings.set("display.fullscreen", true)
settings.set("player.name", "Hero")
print(settings.get("audio.master"))
print(settings.get("display.fullscreen"))
print(settings.get("player.name"))
print(settings.get("missing"))
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "0.8\ntrue\nHero\nnil\n");
}

#[test]
fn settings_has_reports_presence() {
    let src = r#"
print(settings.has("k"))
settings.set("k", 1)
print(settings.has("k"))
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "false\ntrue\n");
}

#[test]
fn settings_set_default_does_not_overwrite() {
    let src = r#"
settings.set("k", 1)
settings.set_default("k", 99)
print(settings.get("k"))
settings.set_default("new_k", 7)
print(settings.get("new_k"))
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "1\n7\n");
}

#[test]
fn settings_save_and_load_round_trip() {
    // Pick a unique path under temp_dir so concurrent test runs
    // don't collide.
    let dir = std::env::temp_dir();
    let path = dir.join("twec_phase10_settings_round_trip.json");
    let _ = std::fs::remove_file(&path);
    let path_lit = path.display().to_string().replace('\\', "/");

    let src = format!(
        r#"
settings.set("audio.master", 0.7)
settings.set("display.fullscreen", true)
settings.save("{path}")
"#,
        path = path_lit
    );
    run_program_str(&src).expect("save program should run");

    // Fresh process state — settings start empty until load.
    let src2 = format!(
        r#"
print(settings.has("audio.master"))
settings.load("{path}")
print(settings.get("audio.master"))
print(settings.get("display.fullscreen"))
"#,
        path = path_lit
    );
    let out = run_program_str(&src2).expect("load program should run");
    let _ = std::fs::remove_file(&path);
    // The settings ambient is process-fresh per `eval::run`, so
    // `has` is false before load. After load, the values come back.
    assert_eq!(out, "false\n0.7\ntrue\n");
}

#[test]
fn settings_load_missing_file_errors_clearly() {
    let err = run_program_str("settings.load(\".twec_no_such_settings_file.json\")\n")
        .expect_err("missing file should error");
    assert!(err.contains("cannot read"), "got: {err}");
}

// --- Phase 10 session 10: localization scaffolding ---

#[test]
fn auto_pause_when_idle_round_trips_threshold() {
    // Phase 11 session 11: setting a non-zero threshold and reading
    // it via the public accessor should round-trip the seconds value.
    // Headless `twec run` doesn't actually pause — that's the
    // play-loop's IdleAutoPause path; the test confirms the surface
    // exists and stores the configured value.
    twec::stdlib::set_paused(false);
    let src = r#"
auto_pause_when_idle(2.5)
"#;
    run_program_str(src).expect("program should run");
    let t = twec::stdlib::auto_pause_idle_threshold();
    assert!((t - 2.5).abs() < 1e-9, "got {t}");
    // 0 disables.
    run_program_str("auto_pause_when_idle(0)\n").expect("disable");
    let t = twec::stdlib::auto_pause_idle_threshold();
    assert_eq!(t, 0.0);
}

#[test]
fn auto_pause_when_idle_rejects_negative() {
    let err =
        run_program_str("auto_pause_when_idle(-1.5)\n").expect_err("negative seconds should error");
    assert!(err.contains("non-negative"), "got: {err}");
}

#[test]
fn auto_pause_on_blur_round_trips_flag() {
    // Phase 11 follow-on (deeper): setting the bool and reading it via
    // the public accessor should round-trip. Headless `twec run` has
    // no play loop, so the `BlurAutoPause` state machine never fires;
    // this test confirms the builtin / accessor surface and the
    // disable path. Reset to default-off after we're done so we don't
    // leak state to other tests in this binary.
    twec::stdlib::set_paused(false);
    run_program_str("auto_pause_on_blur(true)\n").expect("enable");
    assert!(twec::stdlib::auto_pause_on_blur_enabled());
    run_program_str("auto_pause_on_blur(false)\n").expect("disable");
    assert!(!twec::stdlib::auto_pause_on_blur_enabled());
}

#[test]
fn auto_pause_on_blur_rejects_non_bool() {
    let err = run_program_str("auto_pause_on_blur(1)\n").expect_err("integer should error");
    assert!(err.contains("bool"), "got: {err}");
}

#[test]
fn window_focus_is_focused_does_not_panic() {
    // Phase 11 follow-on (deeper): the focus poll should be safe to
    // call from any thread on every supported platform. On Windows it
    // hits Win32 (`GetForegroundWindow` + `GetWindowThreadProcessId`);
    // on macOS / Linux it returns true unconditionally. We only check
    // it doesn't panic — the actual focus state on a CI runner is
    // platform-dependent and not assertable.
    let _ = twec::window_focus::is_focused();
}

#[test]
fn screenshot_queues_path_for_play_loop() {
    // Phase 11 session 1: `screenshot(path)` is a deferred call —
    // it queues the path in a thread-local that the play loop
    // drains after rendering. Headless `twec run` doesn't have a
    // play loop, so the call is a no-op that returns nil. Test
    // that the surface exists and the queued path is visible to
    // `take_pending_screenshot`.
    let src = r#"
screenshot("test_shot.png")
print("queued")
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "queued\n");
    let queued = twec::stdlib::take_pending_screenshot();
    assert_eq!(queued.as_deref(), Some("test_shot.png"));
}

#[test]
fn key_input_outside_render_fails_clearly() {
    // Phase 10 session 11.
    let err =
        run_program_str("key_input((0, 0), (160, 28), \"right\")\n").expect_err("should fail");
    assert!(
        err.contains("must be called from inside `on render():`"),
        "got: {err}"
    );
}

#[test]
fn key_held_dynamic_lookup_returns_bool() {
    // Phase 10 session 11: dynamic key lookup. Headless `twec run`
    // doesn't poll input, so every key reads false; the test only
    // confirms the surface returns a bool and falls back to false
    // for unknown names.
    let src = r#"
print(key_held("right"))
print(key_held("not_a_real_key"))
print(key_pressed("space"))
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "false\nfalse\nfalse\n");
}

#[test]
fn lang_default_locale_is_en() {
    let out = run_program_str("print(lang.locale())\n").expect("program should run");
    assert_eq!(out, "en\n");
}

#[test]
fn lang_set_locale_updates_active() {
    let src = r#"
lang.set_locale("ja")
print(lang.locale())
"#;
    let out = run_program_str(src).expect("program should run");
    assert_eq!(out, "ja\n");
}

#[test]
fn lang_t_falls_back_to_key_when_no_bundle() {
    let src = r#"
print(lang.t("menu.resume"))
"#;
    let out = run_program_str(src).expect("program should run");
    // No bundle loaded — fall back to the key itself.
    assert_eq!(out, "menu.resume\n");
}

#[test]
fn lang_load_and_translate_round_trip() {
    let dir = std::env::temp_dir();
    let path = dir.join("twec_phase10_lang_en.json");
    // Hand-write a minimal JSON bundle: it's just a flat object of
    // key → string. Templates use positional `{0}`, `{1}`, etc.
    std::fs::write(
        &path,
        r#"{"menu.resume":"Resume","menu.quit":"Quit","greet":"Hi {0}!"}"#,
    )
    .expect("write bundle");
    let path_lit = path.display().to_string().replace('\\', "/");

    let src = format!(
        r#"
lang.load("en", "{path}")
lang.set_locale("en")
print(lang.t("menu.resume"))
print(lang.t("menu.quit"))
print(lang.tf("greet", ["Alice"]))
print(lang.t("not.in.bundle"))
"#,
        path = path_lit
    );
    let out = run_program_str(&src).expect("program should run");
    let _ = std::fs::remove_file(&path);
    assert_eq!(out, "Resume\nQuit\nHi Alice!\nnot.in.bundle\n");
}

#[test]
fn lang_tf_unknown_placeholder_is_emitted_literally() {
    // If a placeholder index isn't in the args list, emit it as
    // `{2}` so missing-data is visible at runtime instead of
    // silently dropped.
    let dir = std::env::temp_dir();
    let path = dir.join("twec_phase10_lang_tf_missing.json");
    std::fs::write(&path, r#"{"hi":"Hello {0} and {1}"}"#).expect("write bundle");
    let path_lit = path.display().to_string().replace('\\', "/");
    let src = format!(
        r#"
lang.load("en", "{path}")
print(lang.tf("hi", ["Alice"]))
"#,
        path = path_lit
    );
    let out = run_program_str(&src).expect("program should run");
    let _ = std::fs::remove_file(&path);
    assert_eq!(out, "Hello Alice and {1}\n");
}

#[test]
fn lang_tf_requires_list_args() {
    let err = run_program_str("lang.tf(\"k\", \"not a list\")\n")
        .expect_err("should fail on non-list args");
    assert!(err.contains("expects a list"), "got: {err}");
}

#[test]
fn runs_camera_phase9_follow_shake_reset() {
    // Phase 9 session 2: 2D camera ambient shipped with pos/zoom +
    // follow/shake/reset. Headless run tests the script-visible state
    // mutations; the macroquad render-loop integration (set_camera,
    // shake offset, default-camera carve-out) is exercised by hand
    // via `twec play` and not snapshot-tested.
    let out = run_program("tests/programs/camera_phase9.twe").expect("program should run");
    let expected = "(0.0, 0.0)\n\
        1.0\n\
        (200, 150)\n\
        1.5\n\
        (100.0, 75.0)\n\
        (50.0, 37.5)\n\
        (50.0, 37.5)\n\
        (42.0, 42.0)\n\
        (42.0, 42.0)\n\
        (0.0, 0.0)\n\
        1.0\n";
    assert_eq!(out, expected);
}

#[test]
fn camera_shake_decays_with_camera_tick() {
    // Direct-API test for the runtime decay path that `twec play`
    // drives but no Twe surface exposes (the script can only set
    // shake; the offset/decay live entirely in stdlib).
    use twec::stdlib::{camera_shake_remaining, camera_tick, clear_asset_caches};
    clear_asset_caches();
    assert_eq!(camera_shake_remaining(), 0.0);
    // Trigger a shake via a tiny Twe program.
    run_program_str("camera.shake(5, 0.5)\n").expect("should run");
    assert_eq!(camera_shake_remaining(), 0.5);
    camera_tick(0.2);
    assert!((camera_shake_remaining() - 0.3).abs() < 1e-9);
    camera_tick(1.0);
    assert_eq!(camera_shake_remaining(), 0.0);
}

#[test]
fn runs_math_phase9_smoothstep_mix_noise() {
    // Phase 9 session 1: noise / smoothstep / mix on the CPU surface.
    // Smoothstep + mix outputs are exact (algebraic). Noise is asserted
    // by property (deterministic + range) so the hash internals can
    // change without a brittle test.
    let out = run_program("tests/programs/math_phase9.twe").expect("program should run");
    let expected = "0.0\n\
        1.0\n\
        0.5\n\
        0.0\n\
        1.0\n\
        0.5\n\
        1.0\n\
        0.5\n\
        0.0\n\
        10.0\n\
        5.0\n\
        -0.5\n\
        (0.5, 0.0, 0.5, 1.0)\n\
        (5.0, 10.0)\n\
        true\n\
        true\n\
        true\n\
        true\n";
    assert_eq!(out, expected);
}

#[test]
fn runs_camera2d_phase_v1_0_1_session_8() {
    // v1.0.1 session 8: camera2d.* follow + zoom + pan + bounds.
    // Animation tick (cinematic_pan, zoom_to non-zero duration)
    // requires the play loop's camera2d_tick(env, dt); covered by
    // Rust-side has_camera2d_zoom_anim / has_camera2d_pan_anim.
    let out =
        run_program("tests/programs/camera2d.twe").expect("program should run");
    let expected = "(0.0, 0.0)\n\
        1.0\n\
        (0.0, 0.0)\n\
        (140.0, 0.0)\n\
        1.5\n\
        bounds ok\n\
        anims registered\n";
    assert_eq!(out, expected);
    assert!(
        twec::stdlib::has_camera2d_zoom_anim(),
        "expected an active zoom animation registered at the script's tail"
    );
    assert!(
        twec::stdlib::has_camera2d_pan_anim(),
        "expected an active pan animation registered at the script's tail"
    );
}

#[test]
fn runs_persistent_state_phase_v1_0_1_session_6() {
    // v1.0.1 session 6: persistent-state registry. The parser-sugar
    // form (`state X: pause: false`) defers to v1.0.2; this MVP
    // closes the functional gap with the stdlib registry consulted
    // by `eval::tick_frame` / `tick_entities` to skip non-persistent
    // states under the global pause flag.
    let out =
        run_program("tests/programs/persistent_state.twe").expect("program should run");
    let expected = "false\n\
        true\n\
        true\n\
        false\n\
        false\n\
        true\n\
        false\n\
        false\n\
        true\n\
        false\n\
        true\n\
        true\n\
        false\n\
        true\n";
    assert_eq!(out, expected);
}

#[test]
fn runs_save_schema_version_phase_v1_0_1_session_5() {
    // v1.0.1 session 5: schema-version stamping on write +
    // surfacing via `save.loaded_version()` on read. The block-syntax
    // form (`save SaveSlot:` + `migration from N:`) defers to v1.0.2;
    // this MVP covers the call-and-go API path.
    let out =
        run_program("tests/programs/save_schema_version.twe").expect("program should run");
    let expected = "1\n\
        3\n\
        nil\n\
        nil\n\
        3\n\
        100\n\
        7\n\
        warrior\n\
        false\n";
    assert_eq!(out, expected);
    // Cleanup — best-effort.
    let _ = std::fs::remove_file("save_schema_version_test.json");
}

#[test]
fn runs_v1_0_2_sugar_exit_gate_phase_v1_0_2_session_11() {
    // v1.0.2 Session 11 EXIT GATE: every shipped sugar form ran
    // end-to-end in one program. Pins both Session 1 (save block +
    // migrations) and Session 2 (state persistent / pause: false)
    // so a regression in either parser-sugar path breaks here
    // rather than at a release-tag smoke test.
    let out =
        run_program("tests/programs/v1_0_2_sugar.twe").expect("program should run");
    let expected = "3\n\
        1\n\
        25\n\
        true\n\
        true\n\
        false\n";
    assert_eq!(out, expected);
    let _ = std::fs::remove_file("v1_0_2_sugar_test.json");
}

#[test]
fn runs_lang_plural_closure_phase_v1_0_2_session_4() {
    // v1.0.2 session 4: lang.set_plural_rule accepts a Twe closure
    // `(n: int) -> string` for the long-tail locales the CLDR
    // built-ins don't cover. Closes the v1.0.1 Session 12 alias-only
    // deferral. Side-effect trace verifies the closure fires (and
    // does NOT fire when the locale is switched back to an alias).
    let out =
        run_program("tests/programs/lang_plural_closure.twe").expect("program should run");
    let expected = "other\n\
        one\n\
        two\n\
        other\n\
        4\n\
        0\n\
        two\n";
    assert_eq!(out, expected);
}

#[test]
fn runs_persistent_state_sugar_phase_v1_0_2_session_2() {
    // v1.0.2 session 2: `state X: pause: false` / `state X: persistent`
    // parser sugar. Both forms strip the sentinel from the state body
    // and inject `persistent_state("X")` calls right after the
    // enclosing declaration; `pause: true` is the default and does
    // NOT register. Pure parser sugar over the v1.0.1 registry.
    let out =
        run_program("tests/programs/persistent_state_sugar.twe").expect("program should run");
    let expected = "true\n\
        true\n\
        false\n\
        false\n";
    assert_eq!(out, expected);
}

#[test]
fn runs_save_block_phase_v1_0_2_session_1() {
    // v1.0.2 session 1: `save SaveSlot:` block (Path B, anchor-only).
    // The block is pure parser sugar over the v1.0.1 stateless
    // schema-version primitives — no new builtins, no new AST nodes.
    // This test bootstraps a v1 save on disk, re-reads it, then runs
    // a v3 SaveSlot with two migrations that should fire in order.
    let out = run_program("tests/programs/save_block.twe").expect("program should run");
    let expected = "1\n\
        3\n\
        1\n\
        50\n\
        100\n";
    assert_eq!(out, expected);
    let _ = std::fs::remove_file("save_block_test.json");
}

#[test]
fn runs_save_block_no_load_phase_v1_0_2_session_1() {
    // v1.0.2 session 1: same block, no prior `save.read`. Each
    // `migration from N:` condition compares `nil == K` which is
    // always false under Eq, so no migration body runs.
    let out =
        run_program("tests/programs/save_block_no_load.twe").expect("program should run");
    let expected = "nil\n\
        3\n\
        false\n";
    assert_eq!(out, expected);
}

#[test]
fn runs_tween_phase_v1_0_1_session_2() {
    // v1.0.1 session 2: tween.* — pure deterministic easing primitives.
    // Outputs are byte-identical algebraic values (no fp drift in the
    // chosen cases) so the test catches any regression in the curve
    // math. The final `60` line is the 60-tick determinism gate from
    // the plan — each iteration computes `lerp_eased` twice and
    // asserts equality, so a hidden-state regression flips this to
    // a smaller number.
    let out = run_program("tests/programs/tween.twe").expect("program should run");
    let expected = "0.0\n\
        0.5\n\
        1.0\n\
        0.875\n\
        0.5\n\
        0.0\n\
        1.0\n\
        25.0\n\
        50.0\n\
        25.0\n\
        0.0\n\
        100.0\n\
        0.0\n\
        75.0\n\
        0.0\n\
        true\n\
        14\n\
        linear\n\
        60\n";
    assert_eq!(out, expected);
}

#[test]
fn runs_lang_plural_phase_v1_0_1_session_12() {
    // v1.0.1 session 12: localization plurals — CLDR cardinal rules
    // for en/es/de/ja/pl plus alias support via lang.set_plural_rule.
    // The test asserts category-selection per locale; bundle-driven
    // formatting is exercised through the missing-key fallback path
    // (returns the bare key, identical to lang.t).
    let out = run_program("tests/programs/lang_plural.twe").expect("program should run");
    let expected = "other\n\
        one\n\
        other\n\
        other\n\
        one\n\
        other\n\
        other\n\
        other\n\
        other\n\
        one\n\
        few\n\
        few\n\
        few\n\
        many\n\
        many\n\
        many\n\
        many\n\
        few\n\
        many\n\
        one\n\
        other\n\
        one\n\
        other\n\
        missing_key\n\
        missing_key\n";
    assert_eq!(out, expected);
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

// Phase 27 session 4: Fisher-Yates in-place shuffle. Tests
// determinism (re-seeding gives the same permutation), permutation
// preservation (no element added or dropped), and the no-op edge
// cases (empty / single-element).
#[test]
fn runs_random_shuffle() {
    let out = run_program("tests/programs/random_shuffle.twe").expect("program should run");
    assert_eq!(out, "0\n42\nok determinism\nok permutation\n");
}

#[test]
fn random_shuffle_on_non_list_errors() {
    let err = run_program_str("random.shuffle(42)\n").expect_err("should fail");
    assert!(err.contains("expected a list"), "got: {err}");
}

// Phase 27 session 4: AABB-vs-tile collision queries. Replaces the
// 4-corner sample pattern examples/platformer.twe was repeating.
#[test]
fn runs_tilemap_aabb() {
    let out = run_program("tests/programs/tilemap_aabb.twe").expect("program should run");
    assert_eq!(
        out,
        "ok empty\nok solid_at_corner\nok solid_straddle\nok spike_touch\nok no_spike\n"
    );
}

// Phase 28 session 3 + 4: postfx state setters. Verifies the
// new builtins are callable from Twe and the side-effect persists
// in the thread-local store the play3d render loop reads. The
// render loop itself is exercised by `twec play3d ...`, which a
// terminal can't drive — these are the closest-to-end-to-end
// checks short of opening a wgpu window.
#[test]
fn postfx_bloom_setters() {
    let _ = run_program_str("postfx.bloom(0.42)\n").expect("should run");
    assert!(
        (twec::stdlib::bloom_intensity() - 0.42).abs() < 1e-5,
        "bloom intensity = {}",
        twec::stdlib::bloom_intensity()
    );
    let _ = run_program_str("postfx.bloom_threshold(0.7)\n").expect("should run");
    assert!(
        (twec::stdlib::bloom_threshold() - 0.7).abs() < 1e-5,
        "bloom threshold = {}",
        twec::stdlib::bloom_threshold()
    );
}

#[test]
fn postfx_vignette_color_setter() {
    let _ = run_program_str("postfx.vignette_color((0.10, 0.20, 0.30))\n").expect("should run");
    let c = twec::stdlib::vignette_color();
    assert!((c[0] - 0.10).abs() < 1e-5, "vignette r = {}", c[0]);
    assert!((c[1] - 0.20).abs() < 1e-5, "vignette g = {}", c[1]);
    assert!((c[2] - 0.30).abs() < 1e-5, "vignette b = {}", c[2]);
}

#[test]
fn postfx_bloom_clamps_negative_intensity_to_zero() {
    let _ = run_program_str("postfx.bloom(-0.5)\n").expect("should run");
    assert_eq!(twec::stdlib::bloom_intensity(), 0.0);
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
    assert!(
        err.contains("interpolation") || err.contains("unterminated"),
        "got: {err}"
    );
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
    if let Some(t) = env.get("key_press") {
        if t.is_object() {
            t.as_object()
                .borrow_mut()
                .insert_field(key.to_string(), Value::from_bool(value));
        }
    }
}

// --- v0.2 session 3: mouse surface ---

#[test]
fn stdlib_installs_mouse_objects() {
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);

    // mouse: x, y, pos, wheel
    let rc = match env.get("mouse") {
        Some(t) if t.is_object() => t.as_object(),
        _ => panic!("mouse object missing after stdlib::install"),
    };
    let m = rc.borrow();
    assert!(m.get_field("x").as_ref().is_some_and(|t| t.is_float()));
    assert!(m.get_field("y").as_ref().is_some_and(|t| t.is_float()));
    assert!(m.get_field("pos").as_ref().is_some_and(|t| t.is_tuple()));
    assert!(m.get_field("wheel").as_ref().is_some_and(|t| t.is_float()));

    // mouse_held / mouse_press: left, middle, right
    for name in ["mouse_held", "mouse_press"] {
        let rc = match env.get(name) {
            Some(t) if t.is_object() => t.as_object(),
            _ => panic!("{name} object missing after stdlib::install"),
        };
        let o = rc.borrow();
        for btn in ["left", "middle", "right"] {
            assert!(
                o.get_field(btn).as_ref().is_some_and(|t| t.is_bool()),
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
        Some(t) if t.is_float() => t.as_float(),
        other => panic!("expected total to be Float, got {other:?}"),
    };
    assert_eq!(total, 60.0);
}

#[test]
fn mouse_press_left_drives_branching() {
    // Edge-triggered mouse_press.left fires the body once per
    // press. Three frames: press, no-press, press again.

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
        Some(t) if t.is_int_or_boxed_int() => t.as_int(),
        other => panic!("expected clicks to be Int, got {other:?}"),
    };
    assert_eq!(clicks, 2);
}

fn set_mouse_x(env: &twec::value::Env, x: f64) {
    use twec::value::Value;
    if let Some(t) = env.get("mouse") {
        if t.is_object() {
            t.as_object()
                .borrow_mut()
                .insert_field("x", Value::from_float(x));
        }
    }
}

fn set_mouse_press(env: &twec::value::Env, button: &str, value: bool) {
    use twec::value::Value;
    if let Some(t) = env.get("mouse_press") {
        if t.is_object() {
            t.as_object()
                .borrow_mut()
                .insert_field(button.to_string(), Value::from_bool(value));
        }
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
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);

    // sound module: load + play (v0.1) + play_at + stop +
    // set_volume (v0.2 session 5).
    let rc = match env.get("sound") {
        Some(t) if t.is_object() => t.as_object(),
        _ => panic!("sound object missing after stdlib::install"),
    };
    let s = rc.borrow();
    for name in ["load", "play", "play_at", "stop", "set_volume"] {
        assert!(
            s.get_field(name).as_ref().is_some_and(|t| t.is_builtin()),
            "sound.{name} missing or not a builtin"
        );
    }

    // music module: play, play_at, stop. New in v0.2 session 5.
    let rc = match env.get("music") {
        Some(t) if t.is_object() => t.as_object(),
        _ => panic!("music object missing after stdlib::install"),
    };
    let m = rc.borrow();
    for name in ["play", "play_at", "stop"] {
        assert!(
            m.get_field(name).as_ref().is_some_and(|t| t.is_builtin()),
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
    assert!(err.contains("3 fields"), "got: {err}");
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
    let src =
        std::fs::read_to_string("examples/survive.twe").expect("examples/survive.twe must exist");
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
    let err =
        run_program_str("let h = load(\"nope-not-a-real-path.png\")\n").expect_err("should fail");
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
    let _ = Value::NIL;
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
    let err = run_program_str("let s = sound.load(\"nope.wav\")\n").expect_err("should fail");
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
    assert_eq!(out, "42\nfalse\ntrue\n99\n0\ndefault\nfalse\ntrue\nfalse\n");
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
    let out = run_program_frames("tests/programs/catchup.twe", 2, 0.5).expect("program should run");
    let lines: Vec<&str> = out.trim_end().split('\n').collect();
    assert_eq!(
        lines,
        vec!["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]
    );
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
    let out =
        run_program_frames("tests/programs/time_dt.twe", 3, 0.05).expect("program should run");
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
    let particles = inst.get_field("__particles").expect("__particles");
    let n = if particles.is_list() {
        let rc = particles.as_list();
        let len = rc.borrow().len();
        len
    } else {
        panic!("__particles should be a list")
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
    let err = run_program_str("print(label: \"hi\")\n").expect_err("should fail");
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
    let pos = inst.get_field("pos").expect("pos field");
    let elems = if pos.is_tuple() {
        let elems = pos.as_tuple();
        elems.clone()
    } else {
        panic!("pos should be a tuple")
    };
    assert!(elems[0].is_int_or_boxed_int());
    assert!(elems[1].is_int_or_boxed_int());
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
fn vm_scene_methods_callable_by_bare_name() {
    // craft-hardening: the bytecode VM used to compile a bare sibling-
    // method call (`bump()` inside a state's `every` body) to
    // OP_GET_GLOBAL and fail at runtime with "name not defined". The
    // compiler now lowers it to `self.bump(args)` via OP_INVOKE,
    // matching the tree-walker exactly (see the tree-walker assertion
    // above). Covers both the no-arg (`bump`) and with-arg (`bump_by`)
    // forms, and both state transitions.
    use twec::{compiler, lexer, parser, vm};
    let src = fs::read_to_string("tests/programs/scene_methods.twe").expect("read");
    let tokens = lexer::lex(&src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    let chunk = compiler::compile_program(&program).expect("compile");
    let mut machine = vm::VM::new();
    machine.run(&chunk).expect("vm boot");
    for _ in 0..20 {
        machine.tick(0.1).expect("tick");
    }
    let out = machine.take_out();
    assert_eq!(out, "1\n2\n3\n13\n23\n33\n", "VM output: {out:?}");
}

#[test]
fn snake_advances_right_by_default() {
    let src = std::fs::read_to_string("examples/snake.twe").expect("examples/snake.twe must exist");
    let tokens = twec::lexer::lex(&src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");

    // No keys held, no presses. Tick exactly one full step (150ms).
    twec::eval::tick_frame(&mut env, 0.150).expect("tick");

    let scene = env.active_scene.as_ref().expect("scene");
    let inst = scene.borrow();
    let snake = inst.get_field("snake").expect("snake field");
    let head = if snake.is_list() {
        let rc = snake.as_list();
        let h = rc.borrow()[0];
        h
    } else {
        panic!("snake should be a list")
    };
    let (hx, hy) = if head.is_tuple() {
        let elems = head.as_tuple();
        if elems[0].is_int_or_boxed_int() && elems[1].is_int_or_boxed_int() {
            (elems[0].as_int(), elems[1].as_int())
        } else {
            panic!("head should be (Int, Int)")
        }
    } else {
        panic!("head should be a tuple")
    };
    // Snake starts at (10, 7) heading right; after one 150ms tick the
    // head should be at (11, 7).
    assert_eq!((hx, hy), (11, 7));
}

#[test]
fn snake_dies_into_a_wall() {
    let src = std::fs::read_to_string("examples/snake.twe").expect("examples/snake.twe must exist");
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
    let state_name = scene.borrow().current_state.clone().expect("current state");
    assert_eq!(state_name, "game_over");
    // Snake eats the food at (15, 7) on the way, so score is 1 by the
    // time it walks off the east wall at x=20.
    let inst = scene.borrow();
    let score = inst.get_field("score").expect("score field");
    assert!(score.is_int_or_boxed_int(), "got: {score:?}");
}

// v0.2 Phase 8.5 session 8h: stress-test the bytecode VM safepoint
// and roots wiring. Spawn entities, tick, and force collect on every
// bytecode-instruction safepoint. If VM::scan_roots misses the
// stack / globals / active_entities / active_scene / fiber_stack, a
// still-live TaggedValue gets swept and the tick crashes or produces
// wrong output.
#[test]
fn vm_entity_tick_runs_under_aggressive_gc() {
    let src = "entity Mob:\n\
               \x20   var n = 0\n\
               \x20   update(dt):\n\
               \x20       n += 1\n\
               \n\
               var i = 0\n\
               while i < 50:\n\
               \x20   spawn Mob at (0, 0)\n\
               \x20   i += 1\n";
    let tokens = twec::lexer::lex(src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let chunk = twec::compiler::compile_program(&program).expect("compile");
    let mut vm = twec::vm::VM::new();
    vm.run(&chunk).expect("run");

    // Force collect on every safepoint.
    twec::heap::gc_set_threshold(0);

    for _ in 0..10 {
        vm.tick(0.016).expect("tick under aggressive GC");
    }
}

// v0.2 Phase 8.5 session 8h: stress-test the safepoint and roots
// wiring by lowering the GC threshold to 0 so every statement-boundary
// safepoint actually collects. Snake walking off the east wall is
// 10 ticks of state-machine + entity logic — exercises
// Env::scan_roots, active_scene fields, fiber frames, and global
// stdlib state. If any root is missing, a still-live TaggedValue gets
// swept and the program crashes or produces wrong output.
#[test]
fn snake_runs_under_aggressive_gc() {
    let src = std::fs::read_to_string("examples/snake.twe").expect("examples/snake.twe must exist");
    let tokens = twec::lexer::lex(&src).expect("lex");
    let program = twec::parser::parse(&tokens).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).expect("top-level");

    // Force collect on every statement boundary from now on.
    twec::heap::gc_set_threshold(0);

    for _ in 0..10 {
        twec::eval::tick_frame(&mut env, 0.150).expect("tick under aggressive GC");
    }

    // Same observable as `snake_dies_into_a_wall`: 10 ticks → game_over.
    let scene = env.active_scene.as_ref().expect("scene");
    let state_name = scene.borrow().current_state.clone().expect("current state");
    assert_eq!(state_name, "game_over");
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

#[test]
fn os_data_dir_returns_app_scoped_created_path() {
    // ship-pipeline: `os.data_dir(app)` gives a shipped game a writable,
    // per-user location for saves/settings. Assert the path is app-scoped
    // and the directory is actually created (so a later `save.write` into
    // it succeeds). Uses a unique app name and cleans up after itself.
    let out = run_program_str("print(os.data_dir(\"TweUnitTestDataDir\"))\n")
        .expect("program should run");
    let path = out.trim();
    assert!(!path.is_empty(), "data dir path should be non-empty");
    assert!(
        path.ends_with("TweUnitTestDataDir"),
        "path should be app-scoped, got: {path}"
    );
    assert!(
        std::path::Path::new(path).is_dir(),
        "os.data_dir should create the directory, missing: {path}"
    );
    // Cleanup — best-effort; the dir lives in the real user data dir.
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn os_data_dir_rejects_path_separators() {
    let err = run_program_str("print(os.data_dir(\"evil/escape\"))\n")
        .expect_err("path separators must be rejected");
    assert!(err.contains("path separator"), "got: {err}");
}

#[test]
fn os_data_dir_rejects_empty_name() {
    let err = run_program_str("print(os.data_dir(\"\"))\n")
        .expect_err("empty app name must be rejected");
    assert!(err.contains("non-empty"), "got: {err}");
}

#[test]
fn physics2d_collision_primitives() {
    // genre coverage: hand-rolled 2D collision queries. Pins correctness;
    // the parity harness separately confirms the bytecode VM agrees.
    // Box = (x, y, w, h), top-left origin.
    let out = run_program("tests/programs/physics2d.twe").expect("program should run");
    let expected = "true\n\
        false\n\
        false\n\
        (-5.0, 0.0)\n\
        true\n\
        0.25\n\
        -1.0\n\
        0.0\n\
        false\n\
        1.0\n\
        true\n\
        false\n\
        [0, 2]\n";
    assert_eq!(out, expected);
}

#[test]
fn list_comprehensions_map_filter_and_scope() {
    // Snake NP3: [<elem> for <var> in <iter> (if <cond>)?]. Covers map,
    // filter, list iteration, expression elements, and that the loop var
    // is scoped to the comprehension (the outer `x = 99` is unchanged).
    let out = run_program("tests/programs/list_comp.twe").expect("program should run");
    assert_eq!(
        out,
        "[0, 2, 4, 6, 8, 10]\n[6, 7, 8, 9, 10]\n[a, b, c]\n[1, 4, 9]\n[0, 1, 2, 3]\n99\n"
    );
}

#[test]
fn list_comprehension_rejects_non_iterable() {
    let err = run_program_str("print([x for x in 5])\n")
        .expect_err("comprehending over a non-iterable must error");
    assert!(err.contains("range, list, or tuple"), "got: {err}");
}

#[test]
fn state_enter_exit_hooks_fire_in_order() {
    // Snake NP9: `on enter:` folds into the on-entry body (so the bare
    // "body a" and "enter a" both print on entry), and `on exit:` runs
    // when a state is left, before the next state's entry.
    let out = run_program_frames("tests/programs/state_hooks.twe", 4, 0.1)
        .expect("program should run");
    assert_eq!(
        out,
        "body a\nenter a\ntick a\nexit a\nenter b\nexit b\nenter done\n"
    );
}

#[test]
fn rect_outline_is_registered_and_render_gated() {
    // The finalized drawing set adds rect_outline (the missing outline
    // counterpart to `rect`, mirroring circle/circle_outline). Confirm
    // it's a known builtin (not "name not defined") and render-gated
    // exactly like every other drawing primitive.
    let err = run_program_str("rect_outline((0, 0), (10, 10), 2, color.white)\n")
        .expect_err("drawing outside a render handler must error");
    assert!(err.contains("on render"), "got: {err}");
}

#[test]
fn physics2d_broadphase_and_move_and_slide() {
    // Broad-phase spatial grid (build/query/near/free) + dynamic
    // move_and_slide (swept collision response with sliding). Pins
    // correctness; the parity harness confirms the VM agrees.
    let out =
        run_program("tests/programs/physics2d_dynamics.twe").expect("program should run");
    let expected = "[0, 2]\n\
        [0, 2]\n\
        [3]\n\
        40.0\n\
        0.0\n\
        true\n\
        50.0\n\
        40.0\n\
        50.0\n\
        0.0\n";
    assert_eq!(out, expected);
}

#[test]
fn physics2d_rigidbody_impulse_response() {
    // Rigid-body-lite: bounce (reflect off static surface w/ restitution)
    // and collide (mass-weighted two-body impulse). Pins correctness; the
    // parity harness confirms the VM agrees.
    let out =
        run_program("tests/programs/physics2d_rigidbody.twe").expect("program should run");
    let expected = "(3.0, -8.0)\n\
        (3.0, -10.0)\n\
        (-5.0, 0.0)\n\
        (0.0, 0.0)\n\
        -1.0\n\
        0.0\n\
        1.0\n\
        0.0\n\
        -1.0\n\
        1.0\n\
        0.0\n\
        0.0\n";
    assert_eq!(out, expected);
}

#[test]
fn physics2d_collide_rejects_nonpositive_mass() {
    let err = run_program_str(
        "print(physics2d.collide((0, 0), (1, 0), 0, (5, 0), (-1, 0), 1, 1))\n",
    )
    .expect_err("zero mass should error");
    assert!(err.contains("masses must be positive"), "got: {err}");
}

#[test]
fn physics2d_grid_free_then_query_errors() {
    // Querying a freed grid is a tracked footgun, not a silent empty
    // result (Principle 3).
    let err = run_program_str(
        "let g = physics2d.broadphase([(0, 0, 8, 8)], 16)\n\
         physics2d.grid_free(g)\n\
         print(physics2d.grid_query(g, (0, 0, 8, 8)))\n",
    )
    .expect_err("query after free should error");
    assert!(err.contains("not found"), "got: {err}");
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
    let out =
        run_program_frames("tests/programs/wait_in_state.twe", 1, 1.0).expect("program should run");
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
    assert_eq!(out, "inside-if\nnapping\nnapping\ndone\n");
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
    assert_eq!(out, "first\nfirst\nsecond\nsecond\ndone\n");
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
    assert_eq!(out, "outer-pre\ninner-pre\ninner-post\nouter-post\ndone\n");
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
    let out = run_program("tests/programs/dialogue_minimal.twe").expect("program should run");
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

#[test]
fn time_physics_dt_is_60hz_constant() {
    // Phase 29 session 1: `time.physics_dt` is the fixed simulation
    // rate the engine guarantees. Scripts read it at top level — when
    // `time.dt` is still 0.0 — to size velocity-per-step state. The
    // value must equal 1/60 exactly.
    let out = run_program("tests/programs/physics_dt.twe").expect("program should run");
    let printed: f64 = out.trim().parse().expect("expected a single float");
    let expected = 1.0_f64 / 60.0;
    assert!(
        (printed - expected).abs() < 1e-12,
        "expected {expected}, got {printed}"
    );
}

#[test]
fn sound_now_advances_with_fixed_step_ticks() {
    // Phase 29 session 5: `sound.now()` is the simulation clock that
    // sound.schedule deadlines compare against. After 60 ticks at
    // dt=1/60, it should read ~1.0 (modulo float accumulation
    // error ≤ 1 ULP per tick).
    twec::stdlib::reset_audio_schedule();
    let src = r#"
print(sound.now())
"#;
    // Drive 60 ticks via run_with_frames — tick_frame calls
    // tick_audio_schedule under the hood.
    let prog = parser::parse(&lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    eval::run_top_level(&mut env, &prog).expect("top");
    for _ in 0..60 {
        eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
    }
    // After 60 ticks, sim time should be ~1.0s. Tolerance covers
    // f64 accumulation drift.
    let src2 = r#"
print(sound.now())
"#;
    let prog2 = parser::parse(&lexer::lex(src2).expect("lex")).expect("parse");
    eval::run_top_level(&mut env, &prog2).expect("read");
    let printed = env
        .out
        .lines()
        .last()
        .expect("at least one print")
        .parse::<f64>()
        .expect("float");
    assert!(
        (printed - 1.0).abs() < 1e-9,
        "expected sim time ≈ 1.0 after 60 ticks, got {printed}"
    );
}

#[test]
fn sound_schedule_drains_when_deadline_passes() {
    // Phase 29 session 5: schedule three one-shots at staggered
    // deadlines and tick past the latest one. The schedule queue
    // must drain in deadline order, and end at zero entries.
    //
    // Test uses non-multiples-of-1/60 deadlines (0.1s, 0.2s, 0.4s)
    // so float accumulation in SIM_TIME_S can't park exactly on a
    // boundary. After enough ticks each `when` is comfortably
    // exceeded.
    //
    // Headless: macroquad's audio backend requires a window
    // (THREAD_ID assertion). The dispatch step is suppressed via
    // `set_audio_dispatch_disabled(true)`; SOUND_DISPATCHED_COUNT
    // still increments per fired entry so we can assert on
    // dispatches without invoking macroquad.
    twec::stdlib::reset_audio_schedule();
    twec::stdlib::set_audio_dispatch_disabled(true);
    let src = r#"
let snd = sound.load("tests/programs/physics_dt.twe")
sound.schedule(snd, 0.4, 1.0)
sound.schedule(snd, 0.1, 1.0)
sound.schedule(snd, 0.2, 1.0)
"#;
    let prog = parser::parse(&lexer::lex(src).expect("lex")).expect("parse");
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    eval::run_top_level(&mut env, &prog).expect("top");

    // Three entries queued; none dispatched.
    let count_prog =
        parser::parse(&lexer::lex("print(sound.scheduled_count())").expect("lex")).expect("parse");
    eval::run_top_level(&mut env, &count_prog).expect("read");
    let count: i64 = env.out.lines().last().unwrap().parse().expect("count");
    env.out.clear();
    assert_eq!(count, 3, "expected 3 queued entries before any tick");

    // 7 ticks ≈ 0.117s — only the 0.1s entry fires.
    for _ in 0..7 {
        eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
    }
    eval::run_top_level(&mut env, &count_prog).expect("read");
    let after_7: i64 = env.out.lines().last().unwrap().parse().expect("count");
    env.out.clear();
    assert_eq!(after_7, 2, "0.1s entry should have drained by tick 7");

    // Drive past 0.4s. 30 more ticks ≈ 0.5s sim time → all entries
    // gone.
    for _ in 0..30 {
        eval::tick_frame(&mut env, 1.0 / 60.0).expect("tick");
    }
    eval::run_top_level(&mut env, &count_prog).expect("read");
    let after_37: i64 = env.out.lines().last().unwrap().parse().expect("count");
    assert_eq!(after_37, 0, "all entries should have drained past 0.4s");
    // Restore default so other tests on the same thread aren't
    // surprised by dispatch suppression.
    twec::stdlib::set_audio_dispatch_disabled(false);
    // Three entries fired — assert via the dispatched counter.
    let dispatched = twec::stdlib::sound_dispatched_count();
    assert_eq!(dispatched, 3, "expected 3 dispatches, got {dispatched}");
}
