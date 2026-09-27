//! web3d-M0: differential GC stress harness.
//!
//! Runs every `tests/programs/*.twe` and top-level `examples/*.twe`
//! twice through the real `twec run` binary: once normally, once with
//! `TWE_GC_STRESS=1`. Stress mode collects at *every* safepoint with an
//! unlimited sweep budget and poisons swept objects instead of freeing
//! them (`HeapObject::freed`), so a missing GC root turns into a
//! deterministic "GC use-after-free" panic instead of silent memory
//! reuse. Any difference in exit status or output between the two runs
//! is a GC soundness bug.
//!
//! Uses `CARGO_BIN_EXE_twec`, which cargo guarantees is the binary
//! built for this test run (never a stale `target/release` copy).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Programs that write to a shared repo-root path; running them in
/// parallel with the eval suite races on the file. Same list as
/// `tests/parity.rs` — an isolation skip, not a GC exemption.
const WRITES_FILES: &[&str] = &[
    "save_block.twe",
    "save_schema_version.twe",
    "v1_0_2_sugar.twe",
];

/// Frame counts: 0 = top-level evaluation only; 10 = also tick
/// scenes, states, clocks and entities.
const FRAME_COUNTS: &[u32] = &[0, 10];

fn twe_files(dir: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {dir}: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "twe"))
        .collect();
    paths.sort();
    paths
}

struct Outcome {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run(path: &Path, frames: u32, stress: bool) -> Outcome {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_twec"));
    cmd.arg("run");
    if frames > 0 {
        cmd.args(["--frames", &frames.to_string()]);
    }
    cmd.arg(path);
    if stress {
        cmd.env("TWE_GC_STRESS", "1");
    } else {
        cmd.env_remove("TWE_GC_STRESS");
    }
    let out = cmd.output().expect("spawn twec");
    Outcome {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn check_dir(dir: &str) {
    let mut failures = Vec::new();
    let mut checked = 0;
    for path in twe_files(dir) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if WRITES_FILES.contains(&name.as_str()) {
            continue;
        }
        for &frames in FRAME_COUNTS {
            let normal = run(&path, frames, false);
            let stressed = run(&path, frames, true);
            checked += 1;
            let same = normal.ok == stressed.ok
                && normal.stdout == stressed.stdout
                && (normal.ok || stable_stderr(&normal.stderr) == stable_stderr(&stressed.stderr));
            if !same {
                failures.push(format!(
                    "{dir}/{name} (frames={frames})\n  normal: ok={} stdout={:?}\n  stress: ok={} stdout={:?}\n  stress stderr: {}",
                    normal.ok,
                    tail(&normal.stdout),
                    stressed.ok,
                    tail(&stressed.stdout),
                    tail(&stressed.stderr),
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} runs diverged under TWE_GC_STRESS=1:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// stderr minus lines that legitimately differ run-to-run: the crash
/// reporter's dump path embeds a timestamp and pid, and panic lines
/// carry the thread id.
fn stable_stderr(s: &str) -> String {
    s.lines()
        .filter(|l| !l.contains("dump written to") && !l.starts_with("thread '"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn tail(s: &str) -> &str {
    let start = s.len().saturating_sub(400);
    let mut i = start;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}

#[test]
fn programs_agree_under_gc_stress() {
    check_dir("tests/programs");
}

#[test]
fn examples_agree_under_gc_stress() {
    check_dir("examples");
}
