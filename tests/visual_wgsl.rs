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
    assert!(wgsl.contains("vec2<f32>(1.0, 2.0)"), "got:\n{wgsl}");
}

#[test]
fn math_dot_sin_calls_through() {
    let src = "visual Foo:\n\
        \x20   pixel(uv, time) -> color:\n\
        \x20       let n = math.sin(time)\n\
        \x20       return color.red\n";
    let modules = compile(src).expect("should compile");
    let wgsl = &modules[0].1;
    assert!(wgsl.contains("sin(time)"), "got:\n{wgsl}");
}

#[test]
fn example_5_wgsl_validates_through_naga() {
    // Phase 9 session 11: prove the WGSL we emit actually parses
    // through naga (wgpu's WGSL frontend). naga's accept-set is a
    // superset of what wgpu::create_shader_module accepts at runtime,
    // so a parse failure here would fail at GPU-init time too.
    let src = std::fs::read_to_string("tests/programs/visual_fire.twe").unwrap();
    let modules = compile(&src).expect("should compile");
    let wgsl = &modules[0].1;
    let parsed = naga::front::wgsl::parse_str(wgsl);
    if let Err(e) = &parsed {
        panic!("naga parse failed:\n{}\n--- WGSL ---\n{}", e, wgsl);
    }
    // naga also wants validation to confirm bindings + types are sound.
    let module = parsed.unwrap();
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    );
    if let Err(e) = validator.validate(&module) {
        panic!("naga validate failed:\n{:?}\n--- WGSL ---\n{}", e, wgsl);
    }
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

// web3d-M7: procedural surface materials.

fn material(src: &str) -> String {
    let tokens = lexer::lex(src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    let decl = program.stmts.iter().find_map(|s| match s {
        twec::ast::Stmt::Decl {
            kind: twec::ast::DeclKind::Visual,
            name,
            members,
            ..
        } => Some((name, members)),
        _ => None,
    });
    let (name, members) = decl.expect("a visual block");
    visual_wgsl::compile_material(name, members).expect("compile material")
}

fn validate(label: &str, wgsl: &str) {
    let module = match naga::front::wgsl::parse_str(wgsl) {
        Ok(m) => m,
        Err(e) => panic!("{label}: naga parse failed:\n{}\n--- WGSL ---\n{wgsl}", e.emit_to_string(wgsl)),
    };
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    );
    if let Err(e) = validator.validate(&module) {
        panic!("{label}: naga validate failed:\n{e:?}\n--- WGSL ---\n{wgsl}");
    }
}

const ROCK: &str = "visual Rock:\n\
    \x20   surface(uv, time, pos, normal) -> material:\n\
    \x20       let cell = (math.mod(pos.x, 1), math.mod(pos.z, 1))\n\
    \x20       let edge = math.min(cell.x, cell.y)\n\
    \x20       let bent = math.normalize(normal + (noise(cell * 8), 0, 0) * 0.2)\n\
    \x20       let tone = if edge < 0.05: 0.2 elif edge < 0.1: 0.5 else: (0.6, 0.5, 0.4)\n\
    \x20       return material(albedo: tone, normal: bent, roughness: math.clamp(edge * 4, 0.1, 1), metalness: 0, emission: 0.5)\n\
    \x20   displace(uv, time, pos, normal) -> vec3:\n\
    \x20       return normal * noise((pos.x, pos.z) + (time, 0)) * 0.05\n";

#[test]
fn surface_and_displace_compile_to_kernel_shaders_that_validate() {
    let wgsl = material(ROCK);
    for needle in ["struct TweSurface", "fn twe_surface(", "fn twe_displace(", "select("] {
        assert!(wgsl.contains(needle), "expected `{needle}` in:\n{wgsl}");
    }
    assert!(twec::kernel::render::material_displaces(&wgsl));
    validate("material", &twec::kernel::render::material_shader_source(&wgsl));
    validate("material depth", &twec::kernel::render::material_depth_source(&wgsl));
}

#[test]
fn pixel_visual_is_a_material_without_displacement() {
    let src = std::fs::read_to_string("tests/programs/visual_fire.twe").unwrap();
    let wgsl = material(&src);
    assert!(wgsl.contains("fn twe_surface("), "pixel wraps as a surface:\n{wgsl}");
    assert!(!twec::kernel::render::material_displaces(&wgsl));
    validate("fire material", &twec::kernel::render::material_shader_source(&wgsl));
}

#[test]
fn material_outputs_are_coerced_and_defaulted() {
    let wgsl = material(
        "visual Foo:\n\
         \x20   surface(uv, time) -> material:\n\
         \x20       return material(albedo: (1, 0, 0), emission: color.red)\n",
    );
    // (r, g, b) gains alpha 1; the normal defaults to the input (named
    // internally when left off); roughness 0.5, metalness 0; a
    // four-component emission drops its alpha.
    assert!(
        wgsl.contains(
            "TweSurface(vec4<f32>(vec3<f32>(1.0, 0.0, 0.0), 1.0), twe_normal, 0.5, 0.0, (vec4<f32>(1.0, 0.0, 0.0, 1.0)).rgb)"
        ),
        "got:\n{wgsl}"
    );
}

#[test]
fn math_mod_matches_rem_euclid() {
    let wgsl = material(
        "visual Foo:\n\
         \x20   surface(uv, time) -> material:\n\
         \x20       return material(roughness: math.mod(time, 2))\n",
    );
    assert!(wgsl.contains("(time - abs(2.0) * floor(time / abs(2.0)))"), "got:\n{wgsl}");
}
