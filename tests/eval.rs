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

fn run_program_str(src: &str) -> Result<String, String> {
    let tokens = lexer::lex(src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    eval::run(&program).map_err(|e| format!("eval: {e}"))
}
