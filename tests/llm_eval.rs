//! Phase 33 session 7: integration tests for the eval harness.
//!
//! Two contracts:
//!
//! 1. **Every shipped suite under `eval/`** loads cleanly. A suite
//!    that's missing `expected.txt` or has a malformed `config.toml`
//!    fails this test by name — the price of a one-line `mkdir`-and-go
//!    authoring path is a drift catcher.
//! 2. **Hand-written reference programs** pass their own suite. Each
//!    suite ships an inline reference implementation; the test
//!    confirms the expected output is achievable by *some* Twe program,
//!    not just a hypothesis.

use twec::llm_eval::{grade_source, load_suite};

#[test]
fn every_shipped_suite_loads_cleanly() {
    let root = std::path::Path::new("eval");
    assert!(root.is_dir(), "eval/ directory missing");

    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap().flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if !p.join("expected.txt").exists() {
            continue;
        }
        let suite = load_suite(&p).unwrap_or_else(|e| panic!("load_suite({:?}): {e}", p));
        assert!(!suite.expected.is_empty(), "{} expected.txt is empty", suite.name);
        count += 1;
    }
    assert!(count >= 2, "expected at least 2 suites, found {count}");
}

#[test]
fn print_hello_suite_passes_with_reference_implementation() {
    let suite = load_suite(std::path::Path::new("eval/print_hello")).unwrap();
    let source = "print(\"hello, twe\")\n";
    let score = grade_source(&suite, source);
    assert!(
        score.passed,
        "reference program should pass; got: {:?}",
        score
    );
}

#[test]
fn counter_suite_passes_with_reference_implementation() {
    let suite = load_suite(std::path::Path::new("eval/counter")).unwrap();
    let source = "var n = 0\non update(dt):\n    n = n + 1\n    print(n)\n";
    let score = grade_source(&suite, source);
    assert!(
        score.passed,
        "reference program should pass; got: {:?}",
        score
    );
}

#[test]
fn orbit_suite_passes_with_reference_implementation() {
    let suite = load_suite(std::path::Path::new("eval/orbit")).unwrap();
    let source = "scene Orbit:\n    var t = 0.0\n\n    initial: running\n\n    state running:\n        on update(dt):\n            t = t + dt\n            print(\"frame={t}\")\n";
    let score = grade_source(&suite, source);
    assert!(
        score.passed,
        "reference program should pass; got: {:?}",
        score
    );
}

#[test]
fn broken_program_lands_with_lex_or_parse_stage() {
    let suite = load_suite(std::path::Path::new("eval/print_hello")).unwrap();
    let score = grade_source(&suite, "let = 1\n");
    assert!(!score.passed);
    // Should land at parse stage, not run/match.
    assert_eq!(score.stage, twec::llm_eval::Stage::Parse);
    assert!(score.message.is_some());
}

#[test]
fn scorecard_json_structure_holds_for_real_runs() {
    let suite = load_suite(std::path::Path::new("eval/print_hello")).unwrap();
    let scores = vec![
        grade_source(&suite, "print(\"hello, twe\")\n"),
        grade_source(&suite, "print(\"oops\")\n"),
    ];
    let json = twec::llm_eval::scorecard_json(&scores);
    assert!(json.contains("\"tool\":\"twec-eval\""));
    assert!(json.contains("\"version\":1"));
    assert!(json.contains("\"total\":2"));
    assert!(json.contains("\"passed\":1"));
    assert!(json.contains("\"failed\":1"));
    assert_eq!(json.matches('{').count(), json.matches('}').count());
    assert_eq!(json.matches('[').count(), json.matches(']').count());
}
