//! Phase 33 session 4: integration tests for the LLM loop driver.
//!
//! Exercises the loop end-to-end against a fixture provider so the
//! contract — "model emits, verify checks, broken JSON feeds back,
//! corrected emit passes" — is independently locked down even if
//! the unit tests in `src/llm_loop.rs` regress.

use std::io::BufRead;

use twec::llm_loop::{run_loop, FixtureProvider, LoopOptions};

#[test]
fn end_to_end_recovery_from_did_you_mean_typo() {
    // The classic loop-of-the-loop: round 1 ships a typo, the
    // structured fix in verify v2 lands in the follow-up prompt,
    // round 2 ships the corrected source.
    let mut p = FixtureProvider::new([
        "```twe\n# verified\nlet apple = 1\nlet y = aple\n```".into(),
        "```twe\n# verified\nlet apple = 1\nlet y = apple\n```".into(),
    ]);
    let outcome = run_loop(
        &mut p,
        "Implement a Twe scene that binds two variables.",
        &LoopOptions {
            max_rounds: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(outcome.passed, "expected loop to converge by round 2");
    assert_eq!(outcome.rounds.len(), 2);
    // Round 2's prompt must include the structured fix payload —
    // that's the contract the LLM is supposed to act on.
    assert!(
        p.captured_prompts[1].contains("\"replace\":\"apple\""),
        "follow-up prompt missing structured fix payload"
    );
}

#[test]
fn loop_writes_jsonl_trace_per_round() {
    let dir = tempdir_unique("twec_llm_loop_trace");
    let mut p = FixtureProvider::new(["```twe\nlet x = 1\n```".into()]);
    let outcome = run_loop(
        &mut p,
        "task",
        &LoopOptions {
            max_rounds: 1,
            trace_dir: Some(dir.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(outcome.passed);
    let trace_path = outcome.trace_path.expect("trace path should be set");
    let file = std::fs::File::open(&trace_path).expect("trace file must exist");
    let lines: Vec<String> = std::io::BufReader::new(file)
        .lines()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(lines.len(), 1, "expected one JSONL line per round");
    let line = &lines[0];
    assert!(line.starts_with('{') && line.ends_with('}'));
    assert!(line.contains("\"tool\":\"twec-llm-loop\""));
    assert!(line.contains("\"version\":1"));
    assert!(line.contains("\"round\":1"));
    assert!(line.contains("\"passed\":true"));
    assert!(line.contains("\"verify\":"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn loop_traces_each_round_when_recovering() {
    let dir = tempdir_unique("twec_llm_loop_trace_multi");
    let mut p = FixtureProvider::new([
        "```twe\n# verified\nlet x = oops\n```".into(),
        "```twe\n# verified\nlet x = 1\n```".into(),
    ]);
    let outcome = run_loop(
        &mut p,
        "task",
        &LoopOptions {
            max_rounds: 4,
            trace_dir: Some(dir.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(outcome.passed);
    let trace_path = outcome.trace_path.unwrap();
    let file = std::fs::File::open(&trace_path).unwrap();
    let lines: Vec<String> = std::io::BufReader::new(file)
        .lines()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"round\":1"));
    assert!(lines[0].contains("\"passed\":false"));
    assert!(lines[1].contains("\"round\":2"));
    assert!(lines[1].contains("\"passed\":true"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn seed_prompts_directory_exists_with_at_least_one_file() {
    // Phase 33 session 4 ships seed prompts so the loop has a
    // canonical exercise corpus. Their existence is part of the
    // contract — `twec llm-loop --prompt examples/llm_prompts/snake.md`
    // should always have something to point at.
    let dir = std::path::Path::new("examples/llm_prompts");
    assert!(dir.is_dir(), "examples/llm_prompts/ directory missing");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        if entry.path().extension().is_some_and(|e| e == "md") {
            count += 1;
        }
    }
    assert!(
        count >= 2,
        "expected at least two seed prompts, found {count}"
    );
}

fn tempdir_unique(prefix: &str) -> std::path::PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = std::env::temp_dir().join(format!("{prefix}_{ts}"));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
