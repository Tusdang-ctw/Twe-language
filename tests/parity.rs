//! Differential parity harness: the tree-walker (`eval.rs`) and the
//! bytecode VM (`vm.rs` / `compiler.rs`) are two independent
//! interpreters of the same language. The tree-walker is the
//! production runtime; the VM is opt-in via `--vm bytecode`. Before
//! this harness they were only *proven* to agree on a handful of
//! hand-picked programs (`tests/cli.rs`), so anywhere else they could
//! silently diverge.
//!
//! This test runs EVERY program in `tests/programs/*.twe` on both
//! backends and asserts byte-identical stdout — both with no frames
//! (top-level evaluation) and with `--frames N` (scene / state / clock
//! ticking). It is the safety net for the stdlib split and the VM
//! perf work: any change that makes the two backends disagree turns
//! this red.
//!
//! As of its introduction the corpus has **zero** divergences and an
//! **empty** unsupported list — the VM's documented compile-time
//! rejections (`on render()` / dialogue / typed holes in
//! `src/compiler.rs`) degrade to warnings at the whole-program level
//! rather than hard-failing `twec run`. `VM_UNSUPPORTED` exists so
//! that if a future program *does* exercise a VM-rejected construct,
//! the skip is explicit and cites the `compiler.rs` line that
//! justifies it — that list is the VM parity backlog, measured rather
//! than guessed.

use std::process::Command;
use std::sync::Once;

static BUILD: Once = Once::new();

fn twec_bin() -> String {
    let exe = if cfg!(windows) { "twec.exe" } else { "twec" };
    let target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    let release_path = format!("{target_dir}/release/{exe}");
    if std::path::Path::new(&release_path).exists() {
        format!("{target_dir}/release/{exe}")
    } else {
        format!("{target_dir}/debug/{exe}")
    }
}

fn build_once() {
    BUILD.call_once(|| {
        let status = Command::new("cargo")
            .args(["build"])
            .status()
            .expect("cargo build twec");
        assert!(status.success(), "twec build failed");
    });
}

/// Run `twec` with `args`; return `Ok(stdout)` on exit 0, otherwise
/// `Err(stderr)`. Unlike `cli.rs::run_cli` we do not panic on a
/// non-zero exit — a backend that legitimately rejects a construct
/// should surface as a comparison result, not a test-harness crash.
fn run(args: &[&str]) -> Result<String, String> {
    build_once();
    let output = Command::new(twec_bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn twec: {e}"));
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// Programs that exercise a construct the bytecode VM rejects at
/// compile time. Each entry MUST cite the `src/compiler.rs` line that
/// justifies the skip so this list stays honest and shrinks as the VM
/// gains parity. Empty today: every shipped test program runs on both
/// backends.
const VM_UNSUPPORTED: &[(&str, &str)] = &[
    // ("example.twe", "compiler.rs:NNN — <rejected construct>"),
];

/// Frame counts to exercise. 0 covers top-level evaluation; 10 ticks
/// scenes / states / `every`-clocks so frame-driven divergences show.
const FRAME_COUNTS: &[u32] = &[0, 10];

fn program_paths() -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir("tests/programs")
        .expect("read tests/programs")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "twe"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn bytecode_matches_tree_on_all_programs() {
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    let mut skipped = 0usize;

    for path in program_paths() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let rel = path.to_string_lossy().replace('\\', "/");

        if let Some((_, reason)) = VM_UNSUPPORTED.iter().find(|(n, _)| *n == name) {
            skipped += 1;
            eprintln!("skip {name}: {reason}");
            continue;
        }

        for &frames in FRAME_COUNTS {
            let frames_str = frames.to_string();
            let tree_args: Vec<&str> = if frames == 0 {
                vec!["run", &rel]
            } else {
                vec!["run", "--frames", &frames_str, &rel]
            };
            let mut vm_args = tree_args.clone();
            // insert `--vm bytecode` right after the `run` subcommand
            vm_args.insert(1, "bytecode");
            vm_args.insert(1, "--vm");

            let tree = run(&tree_args);
            let vm = run(&vm_args);
            checked += 1;

            match (tree, vm) {
                (Ok(t), Ok(v)) if t == v => {}
                (Ok(t), Ok(v)) => failures.push(format!(
                    "STDOUT DIFFERS  {name} (frames={frames})\n  tree: {t:?}\n  vm:   {v:?}"
                )),
                // Both backends reject the program: that is *agreement*,
                // not divergence. "this program should succeed" is the
                // job of the eval / cli suites, not the parity harness;
                // comparing stderr verbatim would be fragile (line/col
                // text drift). The corpus has zero such cases today.
                (Err(_), Err(_)) => {}
                (Ok(_), Err(v)) => failures.push(format!(
                    "VM FAILED ONLY  {name} (frames={frames}) — tree ran, VM rejected\n  vm stderr: {}\n  (if intentional, add to VM_UNSUPPORTED with a compiler.rs cite)",
                    v.lines().next().unwrap_or("")
                )),
                (Err(t), Ok(_)) => failures.push(format!(
                    "TREE FAILED ONLY {name} (frames={frames}) — VM ran, tree rejected\n  tree stderr: {}",
                    t.lines().next().unwrap_or("")
                )),
            }
        }
    }

    eprintln!("parity: {checked} comparisons across {} programs, {skipped} skipped", program_paths().len() - skipped);

    assert!(
        failures.is_empty(),
        "tree-walker / bytecode VM divergence ({} case(s)):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
