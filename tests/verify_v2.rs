//! Phase 33 session 2: integration tests for verify JSON v2.
//!
//! Two contracts the schema bump must hold:
//!
//! 1. **Backward compatibility.** Every v1 field is preserved in v2.
//!    A v1 consumer reading our v2 output sees the same `kind`,
//!    `severity`, `line`, `col`, `message`, `help` it has always
//!    seen. The new `fix` field is additive.
//!
//! 2. **Self-correction round-trip.** When a high-confidence
//!    diagnostic carries a `fix`, applying that fix to the source
//!    and re-running verify produces a clean report. This is the
//!    contract an LLM needs to close the loop without parsing free
//!    text.

use twec::verify::{verify_program, verify_program_with_path};

#[test]
fn schema_version_is_v2() {
    let report = verify_program("# verified\nlet x = 1\n");
    let json = report.to_json();
    assert!(json.contains("\"tool\":\"twec-verify\""));
    assert!(
        json.contains("\"version\":2"),
        "schema must be v2, got: {json}"
    );
}

#[test]
fn v1_fields_still_present_in_v2_output() {
    // A program with one strict-mode error: unknown identifier.
    let src = "# verified\nlet x = unkown_thing\n";
    let report = verify_program_with_path(src, Some("test.twe"));
    let json = report.to_json();
    // Top-level v1 fields.
    for field in &[
        "\"file\":",
        "\"strict\":",
        "\"verified\":",
        "\"summary\":",
        "\"diagnostics\":",
    ] {
        assert!(json.contains(field), "missing top-level field {field}");
    }
    // Per-diagnostic v1 fields.
    for field in &[
        "\"kind\":",
        "\"severity\":",
        "\"line\":",
        "\"col\":",
        "\"message\":",
        "\"help\":",
    ] {
        assert!(json.contains(field), "missing diagnostic field {field}");
    }
    // The new v2 field.
    assert!(json.contains("\"fix\":"), "v2 must include fix field");
}

#[test]
fn did_you_mean_emits_structured_fix() {
    // The classic LLM authoring mistake: typo on a previously-bound
    // name. With strict mode active, infer emits an `unknown name`
    // diagnostic with a `did you mean` help; verify must promote it
    // to a structured fix the LLM can apply mechanically.
    let src = "# verified\nlet apple = 1\nlet y = aple\n";
    let report = verify_program(src);
    assert!(!report.ok(), "expected an error for the typo");

    // Find the unknown-name diagnostic.
    let diag = report
        .diagnostics
        .iter()
        .find(|d| d.kind == "name-error.unknown")
        .expect("expected a name-error.unknown diagnostic");

    let fix = diag
        .fix
        .as_ref()
        .expect("did_you_mean diagnostic must carry a structured fix");

    assert_eq!(fix.edits.len(), 1, "expected one edit");
    let edit = &fix.edits[0];
    assert_eq!(
        edit.replace, "apple",
        "replacement should be the suggested name"
    );
    assert_eq!(edit.len, "aple".len() as u32, "len should match the typo");
    assert!(
        fix.rationale.contains("did_you_mean"),
        "rationale should mention did_you_mean"
    );
}

#[test]
fn fix_applies_and_reverify_is_clean() {
    // The full round-trip the LLM loop depends on: take a broken
    // program, take its first fix, apply the edit to the source,
    // re-run verify, assert clean.
    let src = "# verified\nlet apple = 1\nlet y = aple\n";
    let report = verify_program(src);
    let diag = report
        .diagnostics
        .iter()
        .find(|d| d.kind == "name-error.unknown")
        .unwrap();
    let fix = diag.fix.as_ref().unwrap();

    let fixed = apply_edits(src, &fix.edits);
    let reverify = verify_program(&fixed);
    assert!(
        reverify.ok(),
        "after applying fix, verify must be clean. got diagnostics: {:?}",
        reverify.diagnostics
    );
}

/// Apply a list of edits to a source string. Mirrors the simplest
/// reasonable implementation an LLM-side patch applier would write —
/// edits with 1-based line/col anchors, sorted reverse so earlier
/// offsets aren't perturbed by later applications.
fn apply_edits(src: &str, edits: &[twec::verify::Edit]) -> String {
    // Convert to byte offsets first, then apply right-to-left.
    let mut byte_edits: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let start = line_col_to_byte(src, e.line, e.col);
            (start, start + e.len as usize, e.replace.as_str())
        })
        .collect();
    byte_edits.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
    let mut out = src.to_string();
    for (start, end, repl) in byte_edits {
        out.replace_range(start..end, repl);
    }
    out
}

fn line_col_to_byte(src: &str, line: u32, col: u32) -> usize {
    let mut current_line = 1u32;
    let mut line_start = 0;
    for (i, b) in src.bytes().enumerate() {
        if current_line == line {
            return line_start + (col as usize - 1);
        }
        if b == b'\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }
    if current_line == line {
        line_start + (col as usize - 1)
    } else {
        src.len()
    }
}

#[test]
fn json_diagnostic_with_fix_is_well_formed() {
    let src = "# verified\nlet apple = 1\nlet y = aple\n";
    let report = verify_program(src);
    let json = report.to_json();
    // Brace + bracket balance check (catches accidental syntax breakage
    // in the hand-rolled emitter).
    assert_eq!(
        json.matches('{').count(),
        json.matches('}').count(),
        "unbalanced braces in JSON: {json}"
    );
    assert_eq!(
        json.matches('[').count(),
        json.matches(']').count(),
        "unbalanced brackets in JSON: {json}"
    );
    // The fix payload should appear in the JSON.
    assert!(json.contains("\"replace\":\"apple\""), "got: {json}");
    assert!(json.contains("\"rationale\":"), "got: {json}");
    assert!(json.contains("\"edits\":["), "got: {json}");
}

// ---------- web3d-M1: lexical scope errors ----------

#[test]
fn verify_reports_scope_errors_that_run_would_reject() {
    // `twec run` rejects these before executing, so verify must flag
    // them too — an LLM must never get a clean verify for a program
    // that won't run.
    let report = verify_program(
        "function g():\n    return y\nfunction h(y):\n    return g()\nprint(h(99))\n",
    );
    assert!(!report.ok());
    let d = &report.diagnostics[0];
    assert_eq!(d.kind, "scope-error.frame-leak");
    assert_eq!((d.line, d.col), (2, 12));

    let report = verify_program("function f():\n    total = 3\n");
    assert_eq!(report.diagnostics[0].kind, "scope-error.assign-undeclared");
}

#[test]
fn verify_suggests_a_rename_for_an_undefined_name_inside_a_function() {
    let report = verify_program("let speed = 3\nfunction f():\n    return sped * 2\nprint(f())\n");
    let d = &report.diagnostics[0];
    assert_eq!(d.kind, "name-error.unknown");
    let fix = d.fix.as_ref().expect("did-you-mean fix");
    assert_eq!(fix.edits[0].replace, "speed");
    assert_eq!(
        (fix.edits[0].line, fix.edits[0].col, fix.edits[0].len),
        (3, 12, 4)
    );
}

/// web3d-M7: `visual` problems are reported statically as
/// `visual-error`, including type errors found by the WGSL codegen; the
/// procedural-materials example is clean.
#[test]
fn verify_reports_visual_errors() {
    let kinds = |src: &str| -> Vec<String> {
        verify_program(src)
            .diagnostics
            .into_iter()
            .map(|d| d.kind)
            .collect()
    };
    // The call and its string literal: two violations.
    assert_eq!(
        kinds("visual Bad:\n    pixel(uv, time) -> color:\n        print(\"x\")\n        return color.red\n"),
        ["visual-error", "visual-error"]
    );
    assert_eq!(
        kinds("visual Bad:\n    surface(uv, time) -> material:\n        return material(metalness: (1, 1))\n"),
        ["visual-error"]
    );
    let path = "examples/procedural_materials_3d.twe";
    let r = verify_program(&std::fs::read_to_string(path).unwrap());
    assert!(r.ok(), "{path}: {}", r.to_json());
}

/// web3d-M3: `look:` problems are reported statically with stable
/// kinds; a key typo carries a rename fix that round-trips to clean.
#[test]
fn look_block_diagnostics_and_fix_round_trip() {
    let src = "entity E:\n    var pos = vec3(0, 0, 0)\n    look:\n        scal: 2\n";
    let report = verify_program(src);
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.kind == "look-error.unknown-key")
        .expect("unknown key reported");
    let fixed = apply_edits(src, &d.fix.as_ref().expect("rename fix").edits);
    assert!(fixed.contains("scale: 2"), "{fixed}");
    assert!(verify_program(&fixed).ok(), "fixed program verifies clean");

    let kinds = |src: &str| -> Vec<String> {
        verify_program(src)
            .diagnostics
            .into_iter()
            .map(|d| d.kind)
            .collect()
    };
    let head = "entity E:\n    var pos = vec3(0, 0, 0)\n    look:\n";
    // Every planned key has shipped: `material` takes a visual block.
    assert!(verify_program(&format!(
        "visual Glow:\n    pixel(uv, time) -> color:\n        return color.red\n{head}        material: Glow\n"
    ))
    .ok());
    assert_eq!(
        kinds(&format!("{head}        mesh: \"cone\"\n")),
        ["look-error.mesh"]
    );
    assert_eq!(
        kinds(&format!("{head}        scale: 1\n        scale: 2\n")),
        ["look-error.duplicate"]
    );
    // Key expressions are resolved like a method body: fields are
    // visible, an undefined name is not.
    assert!(verify_program(&format!("{head}        scale: pos.x\n")).ok());
    assert_eq!(
        kinds(&format!("{head}        tint: colr.red\n")),
        ["name-error.unknown"]
    );
    // The benchmark and test programs are clean.
    for path in ["tests/programs/look_block.twe", "examples/swarm_3d.twe"] {
        let r = verify_program(&std::fs::read_to_string(path).unwrap());
        assert!(r.ok(), "{path}: {}", r.to_json());
    }
}

/// web3d-M7 session 16: a `particles` block the GPU can't run is a
/// `particles-cpu` warning naming the reason; one it can run is clean.
#[test]
fn verify_warns_when_particles_fall_back_to_the_cpu() {
    let block = |body: &str| {
        format!(
            "let spread = 3.0\nparticles Sparks:\n    count: 100\n    lifetime: 1.0\n\n    on_spawn(p):\n        {body}\n"
        )
    };
    let report = verify_program(&block("p.velocity = (spread, 1, 0)"));
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.kind == "particles-cpu")
        .expect("a CPU fallback warning");
    assert!(
        d.message.contains("`spread`"),
        "names the reason: {}",
        d.message
    );
    assert!(report.ok(), "a warning, not an error");
    let clean = verify_program(&block("p.velocity = (3.0, 1, 0)"));
    assert!(clean.diagnostics.is_empty(), "{}", clean.to_json());
    for path in [
        "examples/stress_3d.twe",
        "examples/particles_3d.twe",
        "examples/survive3d/main.twe",
    ] {
        let r = verify_program(&std::fs::read_to_string(path).unwrap());
        assert!(r.diagnostics.is_empty(), "{path}: {}", r.to_json());
    }
}

/// The `kind-error.*` diagnostics of `src`, as `(kind, line, message)`.
fn kind_errors(src: &str) -> Vec<(String, u32, String)> {
    verify_program(src)
        .diagnostics
        .into_iter()
        .filter(|d| d.kind.starts_with("kind-error"))
        .map(|d| (d.kind, d.line, d.message))
        .collect()
}

#[test]
fn verify_reports_a_vector_mixed_with_a_colour() {
    // docs/06 §2.5.7: the run-time error, reported before running.
    let src = "let p = vec3(1, 2, 3)\n\
               let c = color.red * 0.5\n\
               let bad = p + c\n\
               let fine = p + (0, 1, 0)\n\
               print(mix(color.blue, vec3(0, 1, 0), 0.5))\n";
    let e = kind_errors(src);
    assert_eq!(e.len(), 2, "{e:?}");
    assert_eq!(e[0].0, "kind-error.mix");
    assert_eq!(e[0].1, 3);
    assert!(
        e[0].2
            .contains("can't add a vector (from `vec3`) and a colour"),
        "{e:?}"
    );
    assert!(e[1].2.contains("mix: can't blend a colour"), "{e:?}");
}

#[test]
fn verify_reports_a_vector_as_a_colour_argument_and_a_colour_as_a_position() {
    let src = "entity E:\n    var pos = vec3(0, 0, 0)\n    look:\n        tint: vec3(1, 0, 0)\n\
               let up = vec3(0, 1, 0)\n\
               on render():\n    rect(at: (0, 0), size: (10, 10), color: up)\n\
                   circle(color.white, 4.0, color.red)\n";
    let e = kind_errors(src);
    let messages: Vec<&str> = e.iter().map(|x| x.2.as_str()).collect();
    assert!(
        messages.contains(&"look tint expects a colour, got a vector (from `vec3`)"),
        "{e:?}"
    );
    assert!(
        messages.contains(&"rect.color expects a colour, got a vector (from `vec3`)"),
        "{e:?}"
    );
    assert!(
        messages.contains(&"circle.at expects a position, got a colour (from `color.*`)"),
        "{e:?}"
    );
    assert_eq!(e.len(), 3, "{e:?}");
}

#[test]
fn verify_kind_check_has_no_false_positives() {
    // Unknowns never report: parameters, reassigned names, fields,
    // plain tuples, redefined builtins, and visual blocks (WGSL).
    let src = "function shade(c):\n    return c + vec3(1, 0, 0)\n\
               var tint = color.red\n\
               tint = vec3(0, 0, 1)\n\
               let a = tint + vec3(1, 1, 1)\n\
               let b = (1.0, 0.5, 0.0) + vec3(1, 1, 1)\n\
               entity E:\n    var color = (1, 1, 1)\n    function f():\n        return color + vec3(0, 0, 0)\n\
               visual Glow:\n    pixel(uv, time) -> color:\n        return mix(color.red, color.yellow, uv.y)\n";
    assert_eq!(kind_errors(src), vec![]);
    // A program that runs clean verifies clean: every example and test
    // program (the run-time check would have failed them otherwise).
    let mut checked = 0;
    for dir in ["examples", "tests/programs"] {
        for path in twe_files(std::path::Path::new(dir)) {
            let src = std::fs::read_to_string(&path).unwrap();
            let e = kind_errors(&src);
            assert!(e.is_empty(), "{}: {e:?}", path.display());
            checked += 1;
        }
    }
    assert!(checked > 50, "only {checked} files");
}

#[test]
fn kind_check_parameter_tables_name_real_builtin_parameters() {
    // The tables mirror the run-time decoders; a renamed builtin or
    // parameter must not silently drop out of the check.
    let manifest = twec::stdlib::manifest();
    for label in twec::kind_check::COLOUR_PARAMS
        .iter()
        .chain(twec::kind_check::POSITION_PARAMS)
    {
        let (name, param) = label.rsplit_once('.').unwrap();
        let spec = manifest.iter().find(|s| s.name == name);
        assert!(
            spec.is_some_and(|s| s.params.iter().any(|p| p == param)),
            "`{label}` is not a builtin parameter"
        );
    }
}

fn twe_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name()
                .is_some_and(|n| n != "dist" && n != "node_modules")
            {
                out.extend(twe_files(&p));
            }
        } else if p.extension().is_some_and(|e| e == "twe") {
            out.push(p);
        }
    }
    out
}
