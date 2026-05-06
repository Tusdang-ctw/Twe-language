use insta::assert_snapshot;
use twec::{ast_json, lexer, parser};

fn parse_to_json(src: &str) -> String {
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    ast_json::to_json(&program)
}

#[test]
fn json_dump_for_example_1() {
    let src = std::fs::read_to_string("tests/programs/example_1.twe").unwrap();
    assert_snapshot!(parse_to_json(&src));
}

#[test]
fn json_dump_for_example_2_simplified() {
    let src = std::fs::read_to_string("tests/programs/example_2_simplified.twe").unwrap();
    assert_snapshot!(parse_to_json(&src));
}

#[test]
fn json_dump_for_functions() {
    let src = std::fs::read_to_string("tests/programs/functions.twe").unwrap();
    assert_snapshot!(parse_to_json(&src));
}

#[test]
fn json_dump_for_visual_fire() {
    // Phase 9 session 8: lock the AST shape for a visual block so
    // future parser/AST changes have to acknowledge any structural
    // shift via an `insta` snapshot review.
    let src = std::fs::read_to_string("tests/programs/visual_fire.twe").unwrap();
    assert_snapshot!(parse_to_json(&src));
}

#[test]
fn parses_bare_import() {
    // Phase 13 session 1: `import "<path>"` with no alias.
    let src = "import \"math/vec2\"\n";
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    assert_eq!(program.stmts.len(), 1);
    match &program.stmts[0] {
        twec::ast::Stmt::Import { path, alias, .. } => {
            assert_eq!(path, "math/vec2");
            assert!(alias.is_none());
        }
        other => panic!("expected import, got {other:?}"),
    }
}

#[test]
fn parses_import_with_alias() {
    // Phase 13 session 1: `import "<path>" as <Alias>`. `as` is a
    // contextual identifier here, not a reserved keyword.
    let src = "import \"physics/forces\" as Forces\n";
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    match &program.stmts[0] {
        twec::ast::Stmt::Import { path, alias, .. } => {
            assert_eq!(path, "physics/forces");
            assert_eq!(alias.as_deref(), Some("Forces"));
        }
        other => panic!("expected import, got {other:?}"),
    }
}

#[test]
fn import_requires_string_literal_path() {
    // A bareword path like `import math` is rejected with a help
    // pointing at the canonical form. This forces the load-bearing
    // distinction between identifiers and module paths to be visible
    // at the call site rather than emerging as a confusing later
    // error.
    let src = "import math\n";
    let tokens = lexer::lex(src).expect("lex");
    let err = parser::parse(&tokens).expect_err("parse should fail");
    assert!(
        err.message.contains("string literal"),
        "expected string-literal mention in {:?}",
        err.message
    );
    assert!(err.help.is_some());
}

#[test]
fn import_round_trips_through_fmt() {
    // `twec fmt` (printer) must reproduce both forms verbatim so
    // session-1 imports survive a save / re-format cycle once the
    // editor integration is reading the AST. The trailing newline
    // is the printer's statement terminator.
    let src = "import \"math/vec2\"\nimport \"physics/forces\" as Forces\n";
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    let printed = twec::printer::print_program(&program);
    assert_eq!(printed, src, "round-trip: got {printed:?}, want {src:?}");
}
