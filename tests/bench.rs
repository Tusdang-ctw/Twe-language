//! web3d-M5 session 2: the benchmark's grader and task validator,
//! through the real `twec` binary (grading runs in child processes).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use twec::bench::{grade_in_child, load_task, parse_task, validate, Grader, Stage};

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_twec"))
}

fn temp_task(name: &str, toml: &str, solution: &str) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("twec_bench_{name}_{ts}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("task.toml"), toml).unwrap();
    std::fs::write(dir.join("solution.twe"), solution).unwrap();
    dir
}

/// Every task in the repository is valid: its solution passes, its
/// starter (if any) fails, and each check is caught by a mutant.
#[test]
fn every_committed_task_is_valid() {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir("bench/tasks")
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("task.toml").exists())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty());
    for dir in dirs {
        let task = load_task(&dir).unwrap();
        for file in ["task.md", "twe.md"] {
            assert!(dir.join(file).exists(), "{}: missing {file}", task.id);
        }
        let v = validate(&Grader::twe(exe()), &task, Duration::from_secs(30), 8).unwrap();
        assert!(v.ok(), "{}: {:?}", task.id, v.problems);
    }
}

#[test]
fn a_program_that_never_ends_times_out() {
    let task = load_task(Path::new("bench/tasks/move_player")).unwrap();
    let start = Instant::now();
    let g = grade_in_child(
        exe(),
        &task,
        "var player = vec3(0, 0, 0)\nvar n = 0\non update(dt):\n    while true:\n        n = n + 1\n",
        Duration::from_secs(2),
    );
    assert_eq!(g.stage, Stage::Timeout, "{g:?}");
    assert!(!g.passed);
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn child_grades_match_in_process_grades() {
    let task = load_task(Path::new("bench/tasks/move_player")).unwrap();
    let solution = std::fs::read_to_string("bench/tasks/move_player/solution.twe").unwrap();
    let child = grade_in_child(exe(), &task, &solution, Duration::from_secs(30));
    assert!(child.passed, "{child:?}");
    assert_eq!(child, twec::bench::grade(&task, &solution));
    // A plausible wrong answer: W moves toward +z.
    let flipped = solution.replace("dz = dz - 1.0", "dz = dz + 1.0");
    let g = grade_in_child(exe(), &task, &flipped, Duration::from_secs(30));
    assert!(!g.passed);
    assert_eq!(g.stage, Stage::Checks);
    assert!(g.failed_checks().contains(&"W for 0.5 s moves 2.5 toward -z"), "{g:?}");
}

#[test]
fn validation_rejects_a_vacuous_check_and_a_failing_solution() {
    let toml = "ticks = 10\n\
        [[check]]\nname = \"counts ticks\"\nexpr = \"n == 10\"\n\
        [[check]]\nname = \"always true\"\nexpr = \"n >= 0 or n < 0\"\n";
    let solution = "var n = 0\non update(dt):\n    n = n + 1\n";
    let dir = temp_task("vacuous", toml, solution);
    let task = load_task(&dir).unwrap();
    let v = validate(&Grader::twe(exe()), &task, Duration::from_secs(30), 4).unwrap();
    assert!(v.solution.passed);
    assert_eq!(v.problems.len(), 1, "{:?}", v.problems);
    assert!(v.problems[0].contains("`always true`"));
    assert!(v.strengths[0].killed > 0);

    std::fs::write(dir.join("solution.twe"), "var n = 0\n").unwrap();
    std::fs::write(dir.join("starter.twe"), "var n = 0\non update(dt):\n    n = n + 1\n").unwrap();
    let v = validate(&Grader::twe(exe()), &task, Duration::from_secs(30), 4).unwrap();
    assert!(v.problems.iter().any(|p| p.starts_with("the solution fails")), "{:?}", v.problems);
    assert!(v.problems.iter().any(|p| p == "the starter already passes"), "{:?}", v.problems);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hand_written_mutants_count() {
    // Mechanical mutants don't change strings, and deleting either line
    // stops the program loading or leaves `greeting` alone, so only a
    // hand-written mutant can fail this check.
    let toml = "ticks = 3\n[[check]]\nname = \"named\"\nexpr = \"greeting == \\\"twe\\\"\"\n";
    let dir = temp_task("hand", toml, "let greeting = \"twe\"\nprint(greeting)\n");
    let task = load_task(&dir).unwrap();
    let v = validate(&Grader::twe(exe()), &task, Duration::from_secs(30), 2).unwrap();
    assert!(!v.ok(), "no mechanical mutant changes a string");
    std::fs::create_dir_all(dir.join("mutants")).unwrap();
    std::fs::write(dir.join("mutants/wrong_greeting.twe"), "let greeting = \"two\"\nprint(greeting)\n").unwrap();
    let v = validate(&Grader::twe(exe()), &task, Duration::from_secs(30), 2).unwrap();
    assert!(v.ok(), "{:?}", v.problems);
    let _ = std::fs::remove_dir_all(&dir);
    // parse_task is reachable from the public API too.
    assert!(parse_task("x", Path::new("."), "ticks = 1\n").is_err());
}
