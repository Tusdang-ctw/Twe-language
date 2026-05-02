//! Phase 9 session 9: tests for the `visual` block subset typechecker.

use twec::{lexer, parser, visual_check};

fn check(src: &str) -> Vec<visual_check::VisualError> {
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    visual_check::check_program(&program)
}

#[test]
fn example_5_subset_passes() {
    // The session-8 visual_fire.twe is the canonical happy path.
    let src = std::fs::read_to_string("tests/programs/visual_fire.twe").unwrap();
    let errors = check(&src);
    assert!(
        errors.is_empty(),
        "expected no errors, got: {errors:#?}"
    );
}

#[test]
fn empty_program_is_accepted() {
    let errors = check("print(\"hi\")\n");
    assert!(errors.is_empty(), "got: {errors:#?}");
}

#[test]
fn rejects_string_literal_in_pixel_body() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let s = \"oops\"\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert_eq!(errors.len(), 1, "got: {errors:#?}");
    assert!(errors[0].message.contains("string literals"));
}

#[test]
fn rejects_list_literal() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let xs = [1, 2, 3]\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("list literals"));
}

#[test]
fn rejects_print_call() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       print(\"hi\")\n\
        \x20       return color.red\n";
    let errors = check(src);
    // print itself errors (callable), and the string arg errors too.
    assert!(errors.iter().any(|e| e.message.contains("`print`")), "got: {errors:#?}");
}

#[test]
fn rejects_load_call() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let h = load(\"x.png\")\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(errors.iter().any(|e| e.message.contains("`load`")), "got: {errors:#?}");
}

#[test]
fn rejects_while_loop() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       while true:\n\
        \x20           return color.red\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(errors.iter().any(|e| e.message.contains("`while`")), "got: {errors:#?}");
}

#[test]
fn rejects_assignment() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let x = 1\n\
        \x20       x = 2\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(errors.iter().any(|e| e.message.contains("assignment")), "got: {errors:#?}");
}

#[test]
fn accepts_math_dot_sin() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let n = math.sin(time)\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(errors.is_empty(), "got: {errors:#?}");
}

#[test]
fn rejects_math_dot_unknown() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let n = math.gamma(time)\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("math.gamma")),
        "got: {errors:#?}"
    );
}

#[test]
fn rejects_color_constructor_call() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let c = color.from_hex(\"#fff\")\n\
        \x20       return c\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("color.from_hex")),
        "got: {errors:#?}"
    );
}

#[test]
fn requires_pixel_method() {
    let src = "visual Foo:\n\
        \x20   size: (64, 64)\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("requires a `pixel")),
        "got: {errors:#?}"
    );
}

#[test]
fn enforces_pixel_arity() {
    let src = "visual Foo:\n\
        \x20   pixel(uv) -> color:\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("exactly two parameters")),
        "got: {errors:#?}"
    );
}
