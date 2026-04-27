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
