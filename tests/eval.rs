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

fn run_program_str(src: &str) -> Result<String, String> {
    let tokens = lexer::lex(src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    eval::run(&program).map_err(|e| format!("eval: {e}"))
}
