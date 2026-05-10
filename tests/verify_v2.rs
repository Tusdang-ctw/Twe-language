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
    assert!(json.contains("\"version\":2"), "schema must be v2, got: {json}");
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
    assert_eq!(edit.replace, "apple", "replacement should be the suggested name");
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
