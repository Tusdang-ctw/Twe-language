//! Phase 9 session 10: tests for the WGSL codegen.

use insta::assert_snapshot;
use twec::{lexer, parser, visual_wgsl};

fn compile(src: &str) -> Result<Vec<(String, String)>, visual_wgsl::WgslError> {
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    visual_wgsl::compile_program(&program)
}

#[test]
fn compiles_visual_fire() {
    // Snapshot the full WGSL output for the canonical Example 5
    // (session-8 visual_fire.twe). Locks the WGSL surface so any
    // future codegen change has to acknowledge the diff via insta.
    let src = std::fs::read_to_string("tests/programs/visual_fire.twe").unwrap();
    let modules = compile(&src).expect("should compile");
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].0, "Fire");
    assert_snapshot!(modules[0].1);
}

#[test]
fn empty_program_compiles_to_no_modules() {
    let modules = compile("print(\"hi\")\n").expect("should compile");
    assert!(modules.is_empty());
}

#[test]
fn missing_pixel_method_errors() {
    let src = "visual Foo:\n\
        \x20   size: (64, 64)\n";
    let err = compile(src).expect_err("should fail");
    assert!(err.message.contains("missing a `pixel` method"));
}

#[test]
fn color_constants_inline_as_vec4() {
    let src = "visual Solid:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       return color.red\n";
    let modules = compile(src).expect("should compile");
    let wgsl = &modules[0].1;
    assert!(
        wgsl.contains("vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
        "expected color.red inlined as vec4. got:\n{wgsl}"
    );
}

#[test]
fn integer_literals_emit_as_floats() {
    let src = "visual Solid:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let n = 4 * uv.x\n\
        \x20       return color.red\n";
    let modules = compile(src).expect("should compile");
    let wgsl = &modules[0].1;
    assert!(
        wgsl.contains("4.0 * (uv).x") || wgsl.contains("(4.0 * (uv).x)"),
        "expected integer 4 to emit as 4.0. got:\n{wgsl}"
    );
}

#[test]
fn tuple_2_emits_vec2() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let p = (1, 2)\n\
        \x20       return color.red\n";
    let modules = compile(src).expect("should compile");
    let wgsl = &modules[0].1;
    assert!(
        wgsl.contains("vec2<f32>(1.0, 2.0)"),
        "got:\n{wgsl}"
    );
}

#[test]
fn math_dot_sin_calls_through() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let n = math.sin(time)\n\
        \x20       return color.red\n";
    let modules = compile(src).expect("should compile");
    let wgsl = &modules[0].1;
    assert!(
        wgsl.contains("sin(time)"),
        "got:\n{wgsl}"
    );
}

#[test]
fn module_includes_vertex_uniform_noise_and_fs_main() {
    // Sanity: the emitted module should include all four sections
    // a wgpu pipeline needs.
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       return color.red\n";
    let modules = compile(src).expect("should compile");
    let wgsl = &modules[0].1;
    for needle in [
        "fn vs_main",
        "fn fs_main",
        "fn twe_pixel",
        "fn noise(",
        "@group(0) @binding(0) var<uniform>",
    ] {
        assert!(
            wgsl.contains(needle),
            "expected `{needle}` in WGSL output, got:\n{wgsl}"
        );
    }
}
