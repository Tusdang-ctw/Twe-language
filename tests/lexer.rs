use insta::assert_debug_snapshot;
use twec::lexer::{lex, TokenKind};

const EXAMPLE_1_FIRST_CHUNK: &str = r#"let hero = load("hero.png")
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
fn sprite_is_an_identifier_not_a_keyword() {
    let tokens = lex("sprite").expect("lex should succeed");
    assert!(
        matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "sprite"),
        "expected Ident(\"sprite\"), got {:?}",
        tokens[0].kind
    );
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
    let newlines = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Newline))
        .count();
    assert_eq!(newlines, 1, "exactly one Newline after the closing paren");
}

#[test]
fn newlines_inside_brackets_and_braces_are_suppressed() {
    let tokens = lex("[\n1,\n2\n]\n{\na: 1\n}\n").expect("lex should succeed");
    let newlines = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Newline))
        .count();
    assert_eq!(newlines, 2, "one Newline after each top-level closer");
}

#[test]
fn comments_are_skipped() {
    let tokens = lex("let x = 1 # tail comment\n# whole-line comment\nlet y = 2\n")
        .expect("lex should succeed");
    let comment_token_present = tokens.iter().any(|t| {
        matches!(&t.kind, TokenKind::Ident(s) if s == "tail" || s == "whole")
    });
    assert!(!comment_token_present, "comment text leaked into tokens");
    let lets = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Let))
        .count();
    assert_eq!(lets, 2);
}

#[test]
fn comparison_and_logical_operators_lex() {
    let tokens =
        lex("not a == b != c < d > e <= f >= g and h or i").expect("lex should succeed");
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    for expected in [
        TokenKind::Not,
        TokenKind::EqEq,
        TokenKind::NotEq,
        TokenKind::Lt,
        TokenKind::Gt,
        TokenKind::LtEq,
        TokenKind::GtEq,
        TokenKind::And,
        TokenKind::Or,
    ] {
        assert!(
            kinds.contains(&expected),
            "expected token kind missing: {expected:?}"
        );
    }
}

#[test]
fn brackets_and_braces_lex() {
    let tokens = lex("[1, 2] {a: 3}").expect("lex should succeed");
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    for expected in [
        TokenKind::LBracket,
        TokenKind::RBracket,
        TokenKind::LBrace,
        TokenKind::RBrace,
    ] {
        assert!(kinds.contains(&expected), "missing: {expected:?}");
    }
}

#[test]
fn range_literals_lex() {
    let tokens = lex("0..10  3..<8").expect("lex should succeed");
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert!(kinds.contains(&TokenKind::DotDot));
    assert!(kinds.contains(&TokenKind::DotDotLt));
}

#[test]
fn percent_literals_lex() {
    let tokens = lex("5% 25.5%").expect("lex should succeed");
    let mut percents = tokens.iter().filter_map(|t| match t.kind {
        TokenKind::PercentLit(v) => Some(v),
        _ => None,
    });
    assert_eq!(percents.next(), Some(5.0));
    assert_eq!(percents.next(), Some(25.5));
}

#[test]
fn unit_literals_lex() {
    let tokens = lex("3kg 200ms 0.5s 90deg 100px").expect("lex should succeed");
    let units: Vec<_> = tokens
        .iter()
        .filter_map(|t| match &t.kind {
            TokenKind::UnitLit { value, unit } => Some((*value, unit.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        units,
        vec![
            (3.0, "kg".to_string()),
            (200.0, "ms".to_string()),
            (0.5, "s".to_string()),
            (90.0, "deg".to_string()),
            (100.0, "px".to_string()),
        ]
    );
}

#[test]
fn unknown_unit_errors() {
    let err = lex("5xyz").expect_err("should fail");
    assert!(err.message.contains("unknown unit"));
    assert!(err.help.is_some());
}

#[test]
fn hex_and_binary_literals_lex() {
    let tokens = lex("0xFF 0xff_ff 0b1010 0b1111_0000").expect("lex should succeed");
    let ints: Vec<i64> = tokens
        .iter()
        .filter_map(|t| match t.kind {
            TokenKind::Int(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(ints, vec![0xFF, 0xFFFF, 0b1010, 0b1111_0000]);
}

#[test]
fn digit_separators_lex() {
    let tokens = lex("1_000_000 1_2.5_5 1.5e1_0").expect("lex should succeed");
    let mut nums = tokens.iter().filter_map(|t| match t.kind {
        TokenKind::Int(n) => Some(n as f64),
        TokenKind::Float(f) => Some(f),
        _ => None,
    });
    assert_eq!(nums.next(), Some(1_000_000.0));
    assert_eq!(nums.next(), Some(12.55));
    assert_eq!(nums.next(), Some(1.5e10));
}

#[test]
fn string_escape_sequences_lex() {
    let tokens =
        lex(r#""line one\nline two\ttab\\back\"q""#).expect("lex should succeed");
    let s = match &tokens[0].kind {
        TokenKind::Str(s) => s,
        other => panic!("expected Str, got {other:?}"),
    };
    assert_eq!(s, "line one\nline two\ttab\\back\"q");
}

#[test]
fn unknown_escape_errors() {
    let err = lex(r#""\q""#).expect_err("should fail");
    assert!(err.message.contains(r"\q"), "got: {}", err.message);
}

#[test]
fn triple_string_lexes_multiline_content() {
    let src = "let x = \"\"\"line 1\nline 2\nline 3\"\"\"\n";
    let tokens = lex(src).expect("lex should succeed");
    let s = tokens
        .iter()
        .find_map(|t| match &t.kind {
            TokenKind::Str(s) => Some(s.clone()),
            _ => None,
        })
        .expect("should have a Str token");
    assert_eq!(s, "line 1\nline 2\nline 3");
}

#[test]
fn block_comment_spans_lines() {
    let src = "let x = 1\n#- block\ncomment\nspans -#\nlet y = 2\n";
    let tokens = lex(src).expect("lex should succeed");
    let lets = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Let))
        .count();
    assert_eq!(lets, 2);
}

#[test]
fn unterminated_block_comment_errors() {
    let err = lex("#- never closes").expect_err("should fail");
    assert!(err.message.contains("block comment"));
}

#[test]
fn mixed_tabs_and_spaces_in_indent_errors() {
    let err = lex("if true:\n \tx = 1\n").expect_err("should fail");
    assert_eq!(err.message, "mixed tabs and spaces in indentation");
    assert!(err.help.is_some());
}

#[test]
fn dedent_to_unknown_level_errors() {
    let err = lex("if a:\n    if b:\n        x = 1\n      y = 2\n")
        .expect_err("should fail");
    assert_eq!(
        err.message,
        "dedent does not match any outer indentation level"
    );
}

#[test]
fn stray_bang_errors_with_help() {
    let err = lex("a ! b").expect_err("should fail");
    assert!(err.message.contains("'!'"), "got: {}", err.message);
    let help = err.help.expect("error should carry a help string");
    assert!(help.contains("'!='"), "got: {help}");
}

const NESTED_INDENT: &str = r#"if a:
    if b:
        x = 1
    y = 2
z = 3
"#;

#[test]
fn lexes_nested_indent() {
    let tokens = lex(NESTED_INDENT).expect("lex should succeed");
    assert_debug_snapshot!(tokens);
}
