//! Integration tests for `twec fmt`: every real `.twe` file in
//! `tests/programs/` and `examples/` must format cleanly,
//! re-parse, and be idempotent under repeated formatting.
//!
//! This is the round-trip safety net for Principle 4 (AI-legible
//! by design — round-trippable AST). If a new AST node ever ships
//! without a matching printer arm, the print-then-parse step here
//! will surface it immediately.

use std::process::Command;

use twec::{lexer, parser, printer};

fn twec_bin() -> &'static str {
    env!("CARGO_BIN_EXE_twec")
}

/// Read a file, parse, format. Returns the canonical text.
/// Panics with the file path on lex/parse error so the test
/// output points at the offender immediately.
fn fmt_file(path: &str) -> String {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("could not read {path}: {e}"));
    let tokens = lexer::lex(&src)
        .unwrap_or_else(|e| panic!("{path}: lex: {e}"));
    let program = parser::parse(&tokens)
        .unwrap_or_else(|e| panic!("{path}: parse: {e}"));
    printer::print_program(&program)
}

/// All `.twe` files under `tests/programs/` plus the user-facing
/// examples. List each explicitly so adding a new file is a
/// deliberate test addition (not a silent skip).
const PROGRAMS: &[&str] = &[
    "tests/programs/arithmetic.twe",
    "tests/programs/bullet_collision.twe",
    "tests/programs/catchup.twe",
    "tests/programs/catchup_capped.twe",
    "tests/programs/entity_query.twe",
    "tests/programs/example_1.twe",
    "tests/programs/example_2_simplified.twe",
    "tests/programs/floats.twe",
    "tests/programs/functions.twe",
    "tests/programs/hello.twe",
    "tests/programs/interpolation.twe",
    "tests/programs/let_int.twe",
    "tests/programs/lists.twe",
    "tests/programs/literals.twe",
    "tests/programs/loops.twe",
    "tests/programs/math.twe",
    "tests/programs/methods.twe",
    "tests/programs/particles_block.twe",
    "tests/programs/random.twe",
    "tests/programs/scene_counter.twe",
    "tests/programs/scene_methods.twe",
    "tests/programs/scene_with_render.twe",
    "tests/programs/spawn_entities.twe",
    "tests/programs/state_on_update.twe",
    "tests/programs/time_dt.twe",
    "tests/programs/tuples_and_fields.twe",
    "tests/programs/type_annotations.twe",
];

const EXAMPLES: &[&str] = &[
    "examples/hero.twe",
    "examples/snake.twe",
    "examples/sprite_demo.twe",
    "examples/survive.twe",
    "examples/particles_demo.twe",
];

/// Format every test program and assert it parses back. Catches
/// any printer arm that emits invalid Twe.
#[test]
fn formatted_test_programs_re_parse() {
    for path in PROGRAMS {
        let formatted = fmt_file(path);
        let tokens = lexer::lex(&formatted)
            .unwrap_or_else(|e| panic!("{path}: re-lex of formatted output: {e}\n--- formatted ---\n{formatted}"));
        let _program = parser::parse(&tokens)
            .unwrap_or_else(|e| panic!("{path}: re-parse of formatted output: {e}\n--- formatted ---\n{formatted}"));
    }
}

/// Same for the user-facing examples — these are the "the
/// examples are the spec" programs from CLAUDE.md.
#[test]
fn formatted_examples_re_parse() {
    for path in EXAMPLES {
        let formatted = fmt_file(path);
        let tokens = lexer::lex(&formatted)
            .unwrap_or_else(|e| panic!("{path}: re-lex: {e}\n--- formatted ---\n{formatted}"));
        let _program = parser::parse(&tokens)
            .unwrap_or_else(|e| panic!("{path}: re-parse: {e}\n--- formatted ---\n{formatted}"));
    }
}

/// Idempotence: format, parse, format again — the second pass
/// must produce identical bytes. Catches printers that emit
/// non-canonical text (extra whitespace, unstable orderings).
#[test]
fn formatting_is_idempotent_on_test_programs() {
    for path in PROGRAMS {
        let once = fmt_file(path);
        let tokens = lexer::lex(&once).expect("re-lex");
        let program = parser::parse(&tokens).expect("re-parse");
        let twice = printer::print_program(&program);
        assert_eq!(once, twice, "{path}: fmt is not idempotent");
    }
}

#[test]
fn formatting_is_idempotent_on_examples() {
    for path in EXAMPLES {
        let once = fmt_file(path);
        let tokens = lexer::lex(&once).expect("re-lex");
        let program = parser::parse(&tokens).expect("re-parse");
        let twice = printer::print_program(&program);
        assert_eq!(once, twice, "{path}: fmt is not idempotent");
    }
}

// --- CLI integration tests ---

#[test]
fn fmt_cli_writes_to_stdout_by_default() {
    let output = Command::new(twec_bin())
        .args(["fmt", "tests/programs/hello.twe"])
        .output()
        .expect("spawn twec");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "print(\"hello, twe\")\n");
}

#[test]
fn fmt_check_exits_zero_when_already_formatted() {
    // hello.twe is one statement; canonical form == source.
    let output = Command::new(twec_bin())
        .args(["fmt", "--check", "tests/programs/hello.twe"])
        .output()
        .expect("spawn twec");
    assert!(
        output.status.success(),
        "expected exit 0, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn fmt_check_exits_nonzero_on_unformatted_input() {
    // Write a tiny file with non-canonical whitespace, run --check,
    // expect nonzero exit. Use a temp file co-located with the
    // crate root so cleanup is simple.
    let tmp_path = std::env::temp_dir().join("twec_fmt_check_test.twe");
    std::fs::write(&tmp_path, "let   x   =    5\n").expect("write tmp");
    let output = Command::new(twec_bin())
        .args(["fmt", "--check", tmp_path.to_str().unwrap()])
        .output()
        .expect("spawn twec");
    let _ = std::fs::remove_file(&tmp_path);
    assert!(
        !output.status.success(),
        "expected nonzero exit on unformatted file"
    );
}

#[test]
fn fmt_in_place_rewrites_file() {
    let tmp_path = std::env::temp_dir().join("twec_fmt_in_place_test.twe");
    std::fs::write(&tmp_path, "let   x  =  5\n").expect("write tmp");
    let output = Command::new(twec_bin())
        .args(["fmt", "--in-place", tmp_path.to_str().unwrap()])
        .output()
        .expect("spawn twec");
    let final_contents = std::fs::read_to_string(&tmp_path).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_path);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(final_contents, "let x = 5\n");
}

#[test]
fn fmt_check_and_in_place_are_mutually_exclusive() {
    let output = Command::new(twec_bin())
        .args(["fmt", "--check", "--in-place", "tests/programs/hello.twe"])
        .output()
        .expect("spawn twec");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("mutually exclusive"),
        "stderr did not flag the conflict: {err}"
    );
}
