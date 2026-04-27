use insta::assert_debug_snapshot;
use twec::lexer::lex;

const EXAMPLE_1_FIRST_CHUNK: &str = r#"sprite hero = load("hero.png")
hero.pos = (200, 150)

on update(dt):
    if key.right: hero.x += 200 * dt
    if key.left:  hero.x -= 200 * dt
    if key.up:    hero.y -= 200 * dt
    if key.down:  hero.y += 200 * dt
"#;

#[test]
fn lexes_example_1_first_chunk() {
    let tokens = lex(EXAMPLE_1_FIRST_CHUNK).expect("lex should succeed");
    assert_debug_snapshot!(tokens);
}

#[test]
fn rejects_unterminated_string() {
    let err = lex("name = \"hello\n").expect_err("should fail");
    assert_eq!(err.message, "unterminated string literal");
    assert!(err.help.is_some());
}

#[test]
fn rejects_unmatched_close_paren() {
    let err = lex(")").expect_err("should fail");
    assert_eq!(err.message, "unmatched ')'");
}

#[test]
fn newlines_inside_parens_are_suppressed() {
    let tokens = lex("(1,\n 2)\n").expect("lex should succeed");
    let newline_count = tokens
        .iter()
        .filter(|t| matches!(t.kind, twec::lexer::TokenKind::Newline))
        .count();
    assert_eq!(newline_count, 1, "exactly one Newline after the closing paren");
}
