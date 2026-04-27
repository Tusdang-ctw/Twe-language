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
    let err = run_program_str("let h = load(\"x.png\")\nprint(h.glubjorm)\n")
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

fn run_program_str(src: &str) -> Result<String, String> {
    let tokens = lexer::lex(src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    eval::run(&program).map_err(|e| format!("eval: {e}"))
}
