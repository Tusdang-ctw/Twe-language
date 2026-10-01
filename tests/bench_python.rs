//! web3d-M5 session 5: the Python + pygame-ce baseline, through the
//! real harness. These need the project venv (`bench/python/.venv`, from
//! `bench/python/requirements.txt`); without it they print why and
//! pass, so a machine without Python can still run the suite.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use twe_llm::{Error, Provider, Reply, Request};
use twec::bench::{default_python, load_task, Grader, Lang, Stage, PYTHON_HARNESS};
use twec::bench_run::{run, RunOptions};

fn grader() -> Option<Grader> {
    let python = default_python();
    let ok = std::process::Command::new(&python)
        .args(["-c", "import pygame, pyflakes"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipped: no Python with pygame-ce and pyflakes (make bench/python/.venv from bench/python/requirements.txt)");
        return None;
    }
    Some(Grader::python(Path::new(env!("CARGO_BIN_EXE_twec")), &python, Path::new(PYTHON_HARNESS)))
}

#[test]
fn the_harness_grades_like_the_twe_grader() {
    let Some(g) = grader() else { return };
    let task = load_task(Path::new("bench/tasks/move_player")).unwrap();
    let solution = std::fs::read_to_string("bench/tasks/move_player/solution.py").unwrap();
    let pass = g.grade(&task, &solution, Duration::from_secs(60));
    assert!(pass.passed, "{pass:?}");
    // W moving toward +z fails the same check the Twe version does.
    let flipped = solution.replace("(1 if keys[pygame.K_w] else 0)", "(-1 if keys[pygame.K_w] else 0)");
    assert_ne!(flipped, solution);
    let g2 = g.grade(&task, &flipped, Duration::from_secs(60));
    assert!(!g2.passed && g2.failed_checks().contains(&"W for 0.5 s moves 2.5 toward -z"), "{g2:?}");
    // Stages: syntax, load (no Game), run, timeout.
    assert_eq!(g.grade(&task, "def broken(:\n", Duration::from_secs(60)).stage, Stage::Parse);
    assert_eq!(g.grade(&task, "x = 1\n", Duration::from_secs(60)).stage, Stage::Load);
    let crash = "class Game:\n    def __init__(self):\n        self.player = None\n    def update(self, dt, events):\n        [][1]\n    def draw(self, s):\n        pass\n";
    let c = g.grade(&task, crash, Duration::from_secs(60));
    assert_eq!(c.stage, Stage::Run);
    assert!(c.error.as_deref().unwrap().starts_with("tick 0: line 5: IndexError"), "{c:?}");
    let hang = "class Game:\n    def __init__(self):\n        pass\n    def update(self, dt, events):\n        while True:\n            pass\n    def draw(self, s):\n        pass\n";
    assert_eq!(g.grade(&task, hang, Duration::from_secs(3)).stage, Stage::Timeout);
}

#[test]
fn the_static_check_is_compile_and_pyflakes() {
    let Some(g) = grader() else { return };
    let (errors, report) = g.static_check("import os\nprint(undefined_name)\n", Duration::from_secs(60));
    assert_eq!(errors, 1, "{report}");
    assert!(report.contains("undefined name 'undefined_name'") && report.contains("'os' imported but unused"));
    let (errors, report) = g.static_check("def f(:\n", Duration::from_secs(60));
    assert_eq!(errors, 1);
    assert!(report.contains("SyntaxError"));
    assert_eq!(g.static_check("x = 1\nprint(x)\n", Duration::from_secs(60)).0, 0);
}

/// Answers with each task's Python solution; round 1 adds a misspelt
/// name, which pyflakes reports.
struct PySolutions {
    calls: &'static AtomicU32,
}

impl Provider for PySolutions {
    fn complete(&mut self, request: &Request) -> Result<Reply, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(request.system.contains("class `Game`"), "the Python system prompt");
        assert!(!request.system.contains("Golden rules"), "no Twe primer for Python");
        let prompt = &request.messages[0].text;
        for id in ["countdown", "door"] {
            let dir = Path::new("bench/tasks").join(id);
            let first = std::fs::read_to_string(dir.join("task.md")).unwrap();
            if prompt.contains(first.lines().next().unwrap()) {
                assert!(prompt.contains("self."), "the Python interface");
                let mut src = std::fs::read_to_string(dir.join("solution.py")).unwrap();
                if request.messages.len() == 1 {
                    src.push_str("\nVALUE = misspelt_name\n");
                }
                return Ok(Reply::text_only(format!("```python\n{src}\n```"), "stand-in"));
            }
        }
        Err(Error::Config("unknown task".into()))
    }
    fn id(&self) -> String {
        "stand-in-py".into()
    }
}

#[test]
fn a_python_run_uses_its_checker_as_feedback() {
    let Some(g) = grader() else { return };
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("twec_bench_py_{ts}"));
    let tasks = vec![
        load_task(Path::new("bench/tasks/countdown")).unwrap(),
        load_task(Path::new("bench/tasks/door")).unwrap(),
    ];
    let options = RunOptions {
        grader: g,
        samples: 1,
        jobs: 2,
        out_dir: dir.join("run"),
        cache_dir: None,
        grade_limit: Duration::from_secs(60),
        ..Default::default()
    };
    let make = || Ok(Box::new(PySolutions { calls: &CALLS }) as Box<dyn Provider>);
    let records = run(&tasks, &make, "stand-in-py", &options).unwrap();
    assert!(records.iter().all(|r| r.passed() && r.rounds == 2), "{records:#?}");
    assert!(records.iter().all(|r| r.first_verify_errors == 1 && r.final_verify_errors == 0));
    assert!(dir.join("run/programs/door/0.py").exists());
    let settings = std::fs::read_to_string(dir.join("run/run.json")).unwrap();
    assert!(settings.contains("\"lang\": \"python\""), "{settings}");
    assert_eq!(options.grader.lang, Lang::Python);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every task has its Python side, and every Python reference solution
/// passes. (The full mutant validation of the Python checks takes about
/// 90 s: `twec bench check --lang python --all`.)
#[test]
fn every_python_solution_passes() {
    let Some(g) = grader() else { return };
    let mut dirs: Vec<_> = std::fs::read_dir("bench/tasks")
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("task.toml").exists())
        .collect();
    dirs.sort();
    let tasks: Vec<_> = dirs.iter().map(|d| load_task(d).unwrap()).collect();
    for task in &tasks {
        assert!(task.dir.join("python.md").exists(), "{}: no python.md", task.id);
        assert!(task.checks.iter().all(|c| c.py.is_some()), "{}: a check has no `py`", task.id);
    }
    std::thread::scope(|s| {
        for task in &tasks {
            let g = &g;
            s.spawn(move || {
                let src = std::fs::read_to_string(task.dir.join("solution.py")).unwrap();
                let grade = g.grade(task, &src, Duration::from_secs(120));
                assert!(grade.passed, "{}: {grade:?}", task.id);
            });
        }
    });
}
