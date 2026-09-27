//! web3d-M1: every example must run.
//!
//! Runs each `examples/*.twe` and each project `examples/<name>/main.twe`
//! from the repo root for 10 headless frames, and fails on any runtime
//! error. Project-relative asset paths (`assets/...`, the form a shipped
//! bundle uses) resolve through the CLI's asset root.
//!
//! Before this test, the multi-file projects were never executed by CI.
//! Writing it found four of them broken (`survive_demo`,
//! `survive_beta_mobile`, and both module demos — `import` only worked
//! through `twec run <dir>`, which never ticked frames), plus latent
//! undefined-name bugs in single-file examples found by the lexical
//! resolver (`src/resolve.rs`).
//!
//! `examples/experimental/` is skipped: those demos need
//! `--features experimental`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Examples known to fail, each with the error text they must still
/// produce. If one starts passing, the test fails and asks for the
/// entry to be removed, so this list can only shrink.
const EXPECTED_FAILURES: &[(&str, &str, &str)] = &[];

/// (display name, working dir, script path relative to that dir)
fn examples() -> Vec<(String, PathBuf, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir("examples")
        .expect("read examples")
        .flatten()
    {
        let p = entry.path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() {
            if name == "experimental" || !p.join("main.twe").is_file() {
                continue;
            }
            // Run from the repo root, like the single files: the asset
            // root (the script's directory) makes project-relative
            // `assets/...` paths resolve too.
            out.push((
                name,
                PathBuf::from("."),
                p.join("main.twe").to_string_lossy().into_owned(),
            ));
        } else if p.extension().is_some_and(|x| x == "twe") {
            out.push((name, PathBuf::from("."), p.to_string_lossy().into_owned()));
        }
    }
    out.sort();
    out
}

fn run(dir: &Path, script: &str) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_twec"))
        .current_dir(dir)
        .args(["run", "--frames", "10", script])
        .env_remove("TWE_GC_STRESS")
        .output()
        .expect("spawn twec");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn every_example_runs_headless() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for (name, dir, script) in examples() {
        checked += 1;
        let (ok, stderr) = run(&dir, &script);
        let expected = EXPECTED_FAILURES.iter().find(|(n, _, _)| *n == name);
        match (ok, expected) {
            (true, None) => {}
            (false, None) => failures.push(format!("{name}: {}", stderr.trim())),
            (false, Some((_, needle, _))) if stderr.contains(needle) => {}
            (false, Some((_, needle, _))) => failures.push(format!(
                "{name}: expected failure containing {needle:?}, got: {}",
                stderr.trim()
            )),
            (true, Some(_)) => failures.push(format!(
                "{name} now PASSES — remove it from EXPECTED_FAILURES in tests/examples_run.rs"
            )),
        }
    }
    assert!(checked > 40, "only found {checked} examples");
    assert!(
        failures.is_empty(),
        "{} example(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Every example and test program resolves lexically — no name used
/// outside the scope that declares it, and no undefined names. Keeps
/// the corpus ready for (and, after M1, correct under) lexical scoping.
#[test]
fn corpus_has_no_lexical_scope_issues() {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "experimental") {
                    continue;
                }
                collect(&p, out);
            } else if p.extension().is_some_and(|x| x == "twe") {
                out.push(p);
            }
        }
    }
    let builtins = twec::resolve::known_globals();
    let mut paths = Vec::new();
    for d in ["examples", "tests/programs", "eval"] {
        collect(Path::new(d), &mut paths);
    }
    let mut problems = Vec::new();
    for p in &paths {
        let src = std::fs::read_to_string(p).unwrap();
        let tokens = twec::lexer::lex(&src).expect("lex");
        let program = twec::parser::parse(&tokens).expect("parse");
        for i in twec::resolve::check(&program, &builtins) {
            problems.push(format!(
                "{}:{}:{}: {} `{}`",
                p.display(),
                i.line,
                i.col,
                i.kind.as_str(),
                i.name
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} scope issue(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}
