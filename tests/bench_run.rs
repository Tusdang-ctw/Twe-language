//! web3d-M5 session 4: `bench_run` end to end, with stand-in models
//! (no network): the loop, grading in child processes, the cache,
//! resuming, re-grading and the scorecard.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use twe_llm::{Error, Provider, Reply, Request};
use twec::bench::{load_task, Grader};
use twec::bench_run::{read_samples, regrade, run, RunOptions};

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_twec"))
}

fn temp_dir(name: &str) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let d = std::env::temp_dir().join(format!("twec_bench_run_{name}_{ts}"));
    std::fs::create_dir_all(&d).unwrap();
    d
}

const TASKS: [&str; 3] = ["move_player", "countdown", "door"];

/// Answers each task with its reference solution, recognised by the
/// task text in the request. `typo_first` makes round 1 misspell a
/// name, so verify feedback is needed. Counts its calls.
struct Solutions {
    typo_first: bool,
    calls: &'static AtomicU32,
}

impl Provider for Solutions {
    fn complete(&mut self, request: &Request) -> Result<Reply, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let prompt = &request.messages[0].text;
        for id in TASKS {
            let dir = Path::new("bench/tasks").join(id);
            let first = std::fs::read_to_string(dir.join("task.md")).unwrap();
            if prompt.contains(first.lines().next().unwrap()) {
                let mut src = std::fs::read_to_string(dir.join("solution.twe")).unwrap();
                if self.typo_first && request.messages.len() == 1 {
                    src.push_str("\nvar oops = no_such_name\n");
                }
                return Ok(Reply::text_only(format!("```twe\n{src}\n```"), "stand-in"));
            }
        }
        Err(Error::Config("unknown task".into()))
    }
    fn id(&self) -> String {
        format!("stand-in{}", if self.typo_first { "-typo" } else { "" })
    }
}

fn tasks() -> Vec<twec::bench::Task> {
    TASKS.iter().map(|t| load_task(&Path::new("bench/tasks").join(t)).unwrap()).collect()
}

#[test]
fn a_run_grades_resumes_and_regrades() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let dir = temp_dir("plain");
    let options = RunOptions {
        grader: Grader::twe(exe()),
        samples: 2,
        jobs: 3,
        out_dir: dir.join("run"),
        cache_dir: Some(dir.join("cache")),
        grade_limit: Duration::from_secs(30),
        ..Default::default()
    };
    let make = || Ok(Box::new(Solutions { typo_first: true, calls: &CALLS }) as Box<dyn Provider>);
    let records = run(&tasks(), &make, "stand-in-typo", &options).unwrap();
    assert_eq!(records.len(), 6);
    assert!(records.iter().all(|r| r.passed() && r.rounds == 2), "{records:#?}");
    assert!(records.iter().all(|r| r.first_verify_errors > 0 && r.final_verify_errors == 0));
    assert_eq!(CALLS.load(Ordering::SeqCst), 12);

    let summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("run/summary.json")).unwrap()).unwrap();
    assert_eq!(summary["pass_at_k"][0]["mean"], 1.0);
    assert_eq!(summary["first_round_verify_clean_rate"], 0.0);
    assert_eq!(summary["mean_rounds"], 2.0);
    assert!(std::fs::read_to_string(dir.join("run/summary.md")).unwrap().contains("| pass@1 | 100.0%"));
    assert!(dir.join("run/programs/door/1.twe").exists());
    assert!(std::fs::read_dir(dir.join("run/transcripts/door/0")).unwrap().count() == 1);

    // Resuming a finished run does nothing; a fresh run directory with
    // the same cache answers from the cache.
    run(&tasks(), &make, "stand-in-typo", &options).unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst), 12);
    let again = RunOptions { out_dir: dir.join("run2"), ..options.clone() };
    let records = run(&tasks(), &make, "stand-in-typo", &again).unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst), 12, "every reply came from the cache");
    assert!(records.iter().all(|r| r.cache_hits == 2));

    // A run directory refuses different settings.
    let other = RunOptions { max_rounds: 1, ..options.clone() };
    assert!(run(&tasks(), &make, "stand-in-typo", &other).unwrap_err().contains("different settings"));

    // Re-grading reads the programs, not the model: break one program
    // and its sample fails.
    std::fs::write(dir.join("run/programs/door/0.twe"), "var door_state = \"closed\"\n").unwrap();
    let records = regrade(&Grader::twe(exe()), &dir.join("run"), Path::new("bench/tasks"), Duration::from_secs(30), 4).unwrap();
    assert_eq!(records.iter().filter(|r| !r.passed()).count(), 1);
    assert_eq!(read_samples(&dir.join("run/samples.jsonl")).iter().filter(|r| r.passed()).count(), 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_feedback_a_broken_first_answer_fails() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let dir = temp_dir("single");
    let options = RunOptions {
        grader: Grader::twe(exe()),
        samples: 1,
        max_rounds: 1,
        jobs: 3,
        out_dir: dir.join("run"),
        cache_dir: None,
        ..Default::default()
    };
    let make = || Ok(Box::new(Solutions { typo_first: true, calls: &CALLS }) as Box<dyn Provider>);
    let records = run(&tasks(), &make, "stand-in-typo", &options).unwrap();
    assert!(records.iter().all(|r| !r.passed() && r.rounds == 1));
    assert!(records.iter().all(|r| r.grade.as_ref().unwrap().stage == twec::bench::Stage::Load));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_provider_stops_the_run() {
    struct Down;
    impl Provider for Down {
        fn complete(&mut self, _: &Request) -> Result<Reply, Error> {
            Err(Error::Transport("connection refused".into()))
        }
        fn id(&self) -> String {
            "down".into()
        }
    }
    let dir = temp_dir("down");
    let options = RunOptions { grader: Grader::twe(exe()), samples: 3, jobs: 1, out_dir: dir.join("run"), cache_dir: None, ..Default::default() };
    let make = || Ok(Box::new(Down) as Box<dyn Provider>);
    let err = run(&tasks(), &make, "down", &options).unwrap_err();
    assert!(err.contains("connection refused"), "{err}");
    // The failures are recorded, and retried on resume.
    assert_eq!(read_samples(&dir.join("run/samples.jsonl")).len(), 5);
    let _ = std::fs::remove_dir_all(&dir);
}
