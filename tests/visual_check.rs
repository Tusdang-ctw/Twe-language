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
    assert!(errors.is_empty(), "expected no errors, got: {errors:#?}");
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
    assert!(
        errors.iter().any(|e| e.message.contains("`print`")),
        "got: {errors:#?}"
    );
}

#[test]
fn rejects_load_call() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let h = load(\"x.png\")\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("`load`")),
        "got: {errors:#?}"
    );
}

#[test]
fn rejects_while_loop() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       while true:\n\
        \x20           return color.red\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("`while`")),
        "got: {errors:#?}"
    );
}

#[test]
fn rejects_assignment() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let x = 1\n\
        \x20       x = 2\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("assignment")),
        "got: {errors:#?}"
    );
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
        errors
            .iter()
            .any(|e| e.message.contains("requires a `pixel")),
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
        errors
            .iter()
            .any(|e| e.message.contains("exactly two parameters")),
        "got: {errors:#?}"
    );
}

// web3d-M7: procedural surface materials.

const SURFACE_OK: &str = "visual Rock:\n\
    \x20   surface(uv, time, pos, normal) -> material:\n\
    \x20       let n = noise((pos.x, pos.z) * 3)\n\
    \x20       return material(albedo: (0.5, 0.4, 0.3), roughness: 0.8 + n * 0.1, emission: color.orange * 2)\n\
    \x20   displace(uv, time, pos, normal) -> vec3:\n\
    \x20       return normal * math.clamp(noise((pos.x, pos.z)), 0, 1) * 0.1\n";

#[test]
fn accepts_surface_and_displace() {
    let errors = check(SURFACE_OK);
    assert!(errors.is_empty(), "got: {errors:#?}");
}

#[test]
fn surface_may_leave_off_trailing_inputs() {
    let src = "visual Plain:\n\
        \x20   surface(uv, time) -> material:\n\
        \x20       return material(metalness: 1)\n";
    let errors = check(src);
    assert!(errors.is_empty(), "got: {errors:#?}");
}

#[test]
fn rejects_unknown_material_output_with_suggestion() {
    let src = "visual Foo:\n\
        \x20   surface(uv, time) -> material:\n\
        \x20       return material(roughnes: 0.5)\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("no output `roughnes`")
            && e.help.as_deref().is_some_and(|h| h.contains("roughness"))),
        "got: {errors:#?}"
    );
}

#[test]
fn surface_must_return_material() {
    let src = "visual Foo:\n\
        \x20   surface(uv, time) -> material:\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("returns `material(...)`")),
        "got: {errors:#?}"
    );
}

#[test]
fn material_only_as_surface_return() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let m = material(albedo: color.red)\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("only valid as what `surface` returns")),
        "got: {errors:#?}"
    );
}

#[test]
fn material_takes_named_arguments_only() {
    let src = "visual Foo:\n\
        \x20   surface(uv, time) -> material:\n\
        \x20       return material(color.red)\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("named arguments only")),
        "got: {errors:#?}"
    );
}

#[test]
fn rejects_pixel_and_surface_together() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       return color.red\n\
        \x20   surface(uv, time) -> material:\n\
        \x20       return material()\n";
    let errors = check(src);
    assert!(errors.iter().any(|e| e.message.contains("not both")), "got: {errors:#?}");
}

#[test]
fn rejects_unknown_visual_method() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       return color.red\n\
        \x20   vertex(uv, time) -> vec3:\n\
        \x20       return (0, 0, 0)\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("`vertex` is not a visual method")),
        "got: {errors:#?}"
    );
}

#[test]
fn reports_type_errors_through_codegen() {
    // A tuple where roughness (a number) belongs, and a displacement
    // that isn't a 3-vector: caught by the checker, before a look draws.
    let src = "visual Foo:\n\
        \x20   surface(uv, time) -> material:\n\
        \x20       return material(roughness: (0.5, 0.5))\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("`roughness` must be a number")),
        "got: {errors:#?}"
    );
    let src = "visual Bar:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       return color.red\n\
        \x20   displace(uv, time) -> vec3:\n\
        \x20       return 0.1\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("`displace` returns an (x, y, z) offset")),
        "got: {errors:#?}"
    );
}

#[test]
fn rejects_named_arguments_outside_material() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let n = noise(p: uv)\n\
        \x20       return color.red\n";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("named argument `p:`")),
        "got: {errors:#?}"
    );
}
