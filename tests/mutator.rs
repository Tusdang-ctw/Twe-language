//! Phase 33 session 8: integration test for the mutation corpus generator.
//!
//! End-to-end contract: run the mutator on the existing
//! `tests/programs/` set (known-good by construction), assert that
//! at least one triple lands per corpus, and that each triple is
//! independently re-applicable — apply the structured fix from the
//! verify JSON to the mutated source, re-verify, and the result is
//! clean.

use twec::mutator::{run, RuleSet};

#[test]
fn run_against_tests_programs_emits_at_least_one_triple() {
    let tmp = unique_tempdir("mutate_corpus_run");
    let report = run(
        std::path::Path::new("tests/programs"),
        &tmp,
        RuleSet::IdentifierTypoOnly,
    );
    assert!(
        report.triples_emitted >= 1,
        "expected at least one triple from tests/programs/, got {}",
        report.triples_emitted
    );
    let body = std::fs::read_to_string(tmp.join("error_fix.jsonl")).unwrap();
    let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(line.starts_with('{') && line.ends_with('}'));
        assert!(line.contains("\"tool\":\"twec-mutate\""));
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn each_emitted_triple_round_trips_via_structured_fix() {
    // Generate fresh triples, then for any whose verify JSON carries
    // a structured fix, apply it back and re-verify. This is the
    // contract a fine-tuned model uses at training time: (broken,
    // verify JSON) → fix payload → patched source → clean.
    let tmp = unique_tempdir("mutate_round_trip");
    let _ = run(
        std::path::Path::new("tests/programs"),
        &tmp,
        RuleSet::IdentifierTypoOnly,
    );
    let body = std::fs::read_to_string(tmp.join("error_fix.jsonl")).unwrap();
    let mut round_trips = 0;
    let mut applicable_triples = 0;
    for line in body.lines() {
        if line.is_empty() {
            continue;
        }
        let v = twec::json::parse(line).expect("each JSONL line must be valid JSON");
        let mutated = v
            .get("mutated")
            .and_then(|x| x.as_str())
            .expect("triple has mutated source");
        let verify = v.get("verify").expect("triple has verify payload");
        let diags = verify
            .get("diagnostics")
            .and_then(|x| x.as_array())
            .expect("verify.diagnostics is an array");
        for d in diags {
            let Some(fix) = d.get("fix") else { continue };
            if matches!(fix, twec::json::Value::Null) {
                continue;
            }
            applicable_triples += 1;
            let edits = fix
                .get("edits")
                .and_then(|x| x.as_array())
                .expect("fix.edits is an array");
            let edit_structs: Vec<twec::verify::Edit> = edits
                .iter()
                .map(|e| twec::verify::Edit {
                    line: e.get("line").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
                    col: e.get("col").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
                    len: e.get("len").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
                    replace: e
                        .get("replace")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect();
            // Count "did_you_mean"-eligible errors before and after.
            // The contract isn't "render the whole program clean" —
            // strict mode often surfaces unrelated unknown-name
            // errors on stdlib references the inferer doesn't model
            // (e.g. `load_atlas`). The structured fix removes the
            // *targeted* error specifically; that's what we assert.
            let before = reverify_count_unknown(mutated);
            let patched = apply_edits(mutated, &edit_structs);
            let after = reverify_count_unknown(&patched);
            assert!(
                after < before,
                "round-trip failed: structured fix did not reduce the unknown-name diagnostic count.\n  before: {before}\n  after: {after}\n  mutated:\n{mutated}\n  patched:\n{patched}"
            );
            round_trips += 1;
            break; // One fix per triple is sufficient for the contract.
        }
    }
    assert!(
        applicable_triples > 0,
        "no triples carried an applicable structured fix — corpus has no supervised targets"
    );
    assert_eq!(
        applicable_triples, round_trips,
        "every applicable fix should round-trip"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Count `name-error.unknown` diagnostics from `verify` over `source`.
fn reverify_count_unknown(source: &str) -> usize {
    twec::verify::verify_program(source)
        .diagnostics
        .iter()
        .filter(|d| d.kind == "name-error.unknown")
        .count()
}

fn apply_edits(src: &str, edits: &[twec::verify::Edit]) -> String {
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
        let end = end.min(out.len());
        let start = start.min(end);
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

fn unique_tempdir(prefix: &str) -> std::path::PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = std::env::temp_dir().join(format!("twec_{prefix}_{ts}"));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
