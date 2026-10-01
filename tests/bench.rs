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

fn task_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir("bench/tasks")
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("task.toml").exists())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty());
    dirs
}

/// Every task in the repository is well-formed, and its Twe reference
/// solution passes while its starter (if any) fails. Graded in
/// parallel; the full mutant validation is `every_check_is_caught_by_a_mutant`.
#[test]
fn every_committed_task_is_sound() {
    let dirs = task_dirs();
    let tasks: Vec<_> = dirs.iter().map(|d| load_task(d).unwrap()).collect();
    for (dir, task) in dirs.iter().zip(&tasks) {
        for file in ["task.md", "twe.md", "python.md", "solution.py"] {
            assert!(dir.join(file).exists(), "{}: missing {file}", task.id);
        }
        // The canary marks the benchmark in any corpus it leaks into; it
        // is in the task and both reference solutions, never in a prompt.
        assert_eq!(task.canary.as_deref(), Some(twec::bench::CANARY), "{}: canary", task.id);
        for file in ["solution.twe", "solution.py"] {
            let src = std::fs::read_to_string(dir.join(file)).unwrap();
            assert!(src.lines().next().unwrap_or("").contains(twec::bench::CANARY), "{}: {file} lacks the canary", task.id);
        }
        for file in ["task.md", "twe.md", "python.md", "starter.twe", "starter.py"] {
            let text = std::fs::read_to_string(dir.join(file)).unwrap_or_default();
            assert!(!text.contains("canary GUID"), "{}: the canary would reach a model through {file}", task.id);
        }
    }
    std::thread::scope(|s| {
        for task in &tasks {
            s.spawn(move || {
                let read = |f: &str| std::fs::read_to_string(task.dir.join(f));
                let solution = grade_in_child(exe(), task, &read("solution.twe").unwrap(), Duration::from_secs(60));
                assert!(solution.passed, "{}: the solution fails: {solution:?}", task.id);
                if let Ok(starter) = read("starter.twe") {
                    assert!(!grade_in_child(exe(), task, &starter, Duration::from_secs(60)).passed, "{}: the starter passes", task.id);
                }
            });
        }
    });
}

/// The full validation of every task: each check is failed by some
/// running mutant of its solution. About 4 minutes for 60 tasks, so it
/// runs on request: `cargo test --release --test bench -- --ignored`
/// (and `twec bench check --lang python --all` for the Python twins).
#[test]
#[ignore]
fn every_check_is_caught_by_a_mutant() {
    for dir in task_dirs() {
        let task = load_task(&dir).unwrap();
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
