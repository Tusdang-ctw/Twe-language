//! Phase 33 session 6: integration tests for the examples corpus.
//!
//! Drift catches:
//!
//! 1. Every example in `examples/` has a complete header.
//! 2. No two examples share an `@task` (a duplicate task means the
//!    fine-tune corpus would have a near-clone, which over-weights
//!    that pattern at training time).
//! 3. Every `@category` value comes from the documented vocabulary —
//!    new categories should land in code intentionally, not by
//!    typo in a header.
//! 4. Every `@difficulty` is one of trivial / small / medium / large.
//!
//! Adding a new example without a header makes test 1 fail, naming
//! the offending file. Adding a header with a typo'd category fails
//! test 3. The cost of the discipline is one comment block per file;
//! the payoff is a labelled corpus that fine-tunes cleanly.

use twec::corpus::{scan_corpus, to_json, CorpusEntry};

const ALLOWED_CATEGORIES: &[&str] = &[
    "2d", "3d", "ui", "audio", "input", "net", "visual", "tilemap", "save",
    "dialogue", "lifecycle", "core",
    // Phase 39 mobile-specific examples (touch + safe-area + joystick).
    "mobile",
];

const ALLOWED_DIFFICULTIES: &[&str] = &[
    "trivial",
    "small",
    "medium",
    "large",
    // Phase 36 + 37 introduced multi-system multiplayer examples that
    // are a tier above `large` in cognitive load (lockstep determinism
    // + rollback + lobbies).
    "hard",
];

#[test]
fn every_example_has_complete_header() {
    let entries = scan_corpus(std::path::Path::new("examples"));
    assert!(
        entries.len() >= 30,
        "examples corpus unexpectedly small: {}",
        entries.len()
    );
    let mut incomplete: Vec<(String, Vec<&'static str>)> = Vec::new();
    for e in &entries {
        if !e.is_complete() {
            incomplete.push((
                e.path.display().to_string(),
                e.missing_fields(),
            ));
        }
    }
    assert!(
        incomplete.is_empty(),
        "examples missing corpus headers (add to src/corpus.rs vocabulary): {:#?}",
        incomplete
    );
}

#[test]
fn no_duplicate_tasks() {
    let entries = scan_corpus(std::path::Path::new("examples"));
    let mut seen: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for e in &entries {
        if let Some(t) = &e.task {
            if let Some(prev) = seen.insert(t.clone(), e.path.display().to_string()) {
                panic!(
                    "duplicate @task across examples:\n  - {}\n  - {}\ntask: {t}",
                    prev,
                    e.path.display()
                );
            }
        }
    }
}

#[test]
fn every_category_is_in_vocabulary() {
    let entries = scan_corpus(std::path::Path::new("examples"));
    let allowed: std::collections::HashSet<&str> =
        ALLOWED_CATEGORIES.iter().copied().collect();
    let mut bad: Vec<(String, String)> = Vec::new();
    for e in &entries {
        if let Some(c) = &e.category {
            if !allowed.contains(c.as_str()) {
                bad.push((e.path.display().to_string(), c.clone()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "examples with unknown @category (extend ALLOWED_CATEGORIES if intentional): {:#?}",
        bad
    );
}

#[test]
fn every_difficulty_is_in_vocabulary() {
    let entries = scan_corpus(std::path::Path::new("examples"));
    let allowed: std::collections::HashSet<&str> =
        ALLOWED_DIFFICULTIES.iter().copied().collect();
    let mut bad: Vec<(String, String)> = Vec::new();
    for e in &entries {
        if let Some(d) = &e.difficulty {
            if !allowed.contains(d.as_str()) {
                bad.push((e.path.display().to_string(), d.clone()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "examples with unknown @difficulty: {:#?}",
        bad
    );
}

#[test]
fn json_emit_is_balanced_and_versioned() {
    let entries = scan_corpus(std::path::Path::new("examples"));
    let json = to_json(&entries);
    assert!(json.contains("\"tool\":\"twec-corpus\""));
    assert!(json.contains("\"version\":1"));
    let needle = format!("\"count\":{}", entries.len());
    assert!(json.contains(&needle));
    assert_eq!(json.matches('{').count(), json.matches('}').count());
    assert_eq!(json.matches('[').count(), json.matches(']').count());
}

#[test]
fn parse_header_handles_unicode_in_values() {
    // Just in case a future @expected ever carries a non-ASCII
    // character (e.g. an arrow → for state-transition prose), the
    // parser shouldn't choke.
    let src = "# @task: arrows → work\n# @inputs: none\n# @expected: → renders\n# @category: 2d\n# @difficulty: trivial\n";
    let e = twec::corpus::parse_header(src);
    assert_eq!(e.task.as_deref(), Some("arrows → work"));
    assert_eq!(e.expected.as_deref(), Some("→ renders"));
}

/// Sanity: scanning a non-existent directory returns empty rather
/// than panicking. Lets `twec corpus --root /missing` produce a
/// clean empty document instead of crashing.
#[test]
fn scan_missing_directory_returns_empty() {
    let out: Vec<CorpusEntry> = scan_corpus(std::path::Path::new(
        "this-path-definitely-does-not-exist-anywhere",
    ));
    assert!(out.is_empty());
}
