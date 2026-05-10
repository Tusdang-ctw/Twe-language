//! Phase 33 session 9: integration tests for typed holes (`???`).
//!
//! Five contracts:
//!
//! 1. A program containing `???` lexes and parses cleanly.
//! 2. `twec verify` reports each hole as a `kind: "hole"` Warning
//!    (not an Error — verify still says ok() in the absence of
//!    real errors).
//! 3. Running the program errors at runtime with a clear message
//!    naming the hole's location.
//! 4. The bytecode VM compile path rejects holes cleanly with a
//!    "use --vm tree" message (honest deferral per the plan).
//! 5. The printer / formatter round-trips `???` verbatim.

use twec::lexer::lex;
use twec::parser::parse;
use twec::verify::{verify_program, Severity};

#[test]
fn hole_lexes_and_parses() {
    let src = "let x = ???\n";
    let tokens = lex(src).unwrap();
    let program = parse(&tokens).unwrap();
    // The let's value should be an Expr::Hole.
    use twec::ast::{Expr, Stmt};
    let stmt = &program.stmts[0];
    let Stmt::Let { value, .. } = stmt else {
        panic!("expected Let statement, got {stmt:?}");
    };
    assert!(
        matches!(value, Expr::Hole { .. }),
        "expected Expr::Hole, got {value:?}"
    );
}

#[test]
fn hole_triggers_verify_warning_with_kind_hole() {
    let report = verify_program("let x = ???\n");
    // Holes are warnings — they don't trigger a non-zero exit code.
    assert!(report.ok(), "verify should pass with only hole warnings");
    let hole_diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.kind == "hole")
        .collect();
    assert_eq!(hole_diags.len(), 1, "expected one hole diagnostic");
    let d = hole_diags[0];
    assert_eq!(d.severity, Severity::Warning);
    assert!(d.message.contains("???"));
    assert!(d.help.is_some());
}

#[test]
fn multiple_holes_each_get_their_own_warning() {
    let src = "let x = ???\nlet y = ???\nprint(???)\n";
    let report = verify_program(src);
    let count = report.diagnostics.iter().filter(|d| d.kind == "hole").count();
    assert_eq!(count, 3);
}

#[test]
fn running_program_with_hole_errors_at_runtime_with_location() {
    // Use the tree-walker entry point so we exercise the runtime
    // error path on `Expr::Hole`.
    use twec::eval::run;
    let tokens = lex("print(???)\n").unwrap();
    let program = parse(&tokens).unwrap();
    let err = run(&program).expect_err("running a hole should fail");
    assert!(err.message.contains("???") || err.message.contains("hole"));
    assert_eq!(err.line, 1, "error should be located at the hole");
    assert!(err.help.is_some());
}

#[test]
fn bytecode_compile_rejects_hole_with_clear_message() {
    use twec::compiler::compile_program;
    let tokens = lex("let x = ???\n").unwrap();
    let program = parse(&tokens).unwrap();
    let err = compile_program(&program).expect_err("bytecode compile should reject holes");
    assert!(
        err.message.contains("???") || err.message.contains("hole"),
        "compile error should mention holes; got: {err:?}"
    );
}

#[test]
fn hole_round_trips_through_formatter() {
    use twec::printer::print_program;
    let tokens = lex("let x = ???\n").unwrap();
    let program = parse(&tokens).unwrap();
    let formatted = print_program(&program);
    assert!(formatted.contains("???"), "formatter must round-trip ???");
}

#[test]
fn one_or_two_question_marks_are_lex_errors() {
    let one = lex("let x = ?\n");
    assert!(one.is_err(), "single `?` should be a lex error");

    let two = lex("let x = ??\n");
    assert!(two.is_err(), "double `??` should also be a lex error");
}

#[test]
fn hole_in_strict_mode_does_not_error() {
    // Strict mode types holes as fresh variables; the only signal
    // is the warning. This is the "purely additive" guarantee —
    // adding `???` to a strict file doesn't break the type check.
    let report = verify_program("# strict\nlet x: int = ???\n");
    let errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "strict mode should not error on holes: {:?}",
        errors
    );
    let warnings: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.kind == "hole")
        .collect();
    assert_eq!(warnings.len(), 1);
}

#[test]
fn hole_appears_in_verify_v2_json_output() {
    let report = verify_program("let x = ???\n");
    let json = report.to_json();
    assert!(json.contains("\"kind\":\"hole\""));
    assert!(json.contains("\"severity\":\"warning\""));
    assert!(json.contains("\"version\":2"));
}
