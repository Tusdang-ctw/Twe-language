//! End-to-end CLI tests for the `--vm bytecode` flag.
//!
//! These run the actual `twec` binary as a subprocess and compare
//! its stdout against the tree-walker's stdout on the same input.
//! That verifies both the flag plumbing in `cli.rs` and the
//! end-to-end output of the bytecode VM driving real test programs.

use std::process::Command;

fn twec_bin() -> &'static str {
    env!("CARGO_BIN_EXE_twec")
}

/// Run the binary with the given args; return stdout if the exit
/// code is 0, otherwise panic with the captured stderr.
fn run_cli(args: &[&str]) -> String {
    let output = Command::new(twec_bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn twec: {e}"));
    if !output.status.success() {
        panic!(
            "twec {args:?} failed (exit {:?}): stderr = {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn run_with_default_backend_uses_tree_walker() {
    let out = run_cli(&["run", "tests/programs/hello.twe"]);
    assert_eq!(out, "hello, twe\n");
}

#[test]
fn run_with_vm_bytecode_executes_via_vm() {
    let out = run_cli(&["run", "--vm", "bytecode", "tests/programs/hello.twe"]);
    assert_eq!(out, "hello, twe\n");
}

#[test]
fn run_vm_bytecode_matches_tree_on_arithmetic() {
    let tree = run_cli(&["run", "tests/programs/arithmetic.twe"]);
    let bc = run_cli(&["run", "--vm", "bytecode", "tests/programs/arithmetic.twe"]);
    assert_eq!(tree, bc);
}

#[test]
fn run_vm_bytecode_matches_tree_on_methods() {
    let tree = run_cli(&["run", "tests/programs/methods.twe"]);
    let bc = run_cli(&["run", "--vm", "bytecode", "tests/programs/methods.twe"]);
    assert_eq!(tree, bc);
}

#[test]
fn run_vm_bytecode_matches_tree_on_lists() {
    let tree = run_cli(&["run", "tests/programs/lists.twe"]);
    let bc = run_cli(&["run", "--vm", "bytecode", "tests/programs/lists.twe"]);
    assert_eq!(tree, bc);
}

#[test]
fn run_vm_bytecode_matches_tree_on_for_loops() {
    let tree = run_cli(&["run", "tests/programs/loops.twe"]);
    let bc = run_cli(&["run", "--vm", "bytecode", "tests/programs/loops.twe"]);
    assert_eq!(tree, bc);
}

#[test]
fn run_vm_bytecode_matches_tree_on_functions() {
    let tree = run_cli(&["run", "tests/programs/functions.twe"]);
    let bc = run_cli(&["run", "--vm", "bytecode", "tests/programs/functions.twe"]);
    assert_eq!(tree, bc);
}

#[test]
fn run_vm_bytecode_matches_tree_on_math() {
    let tree = run_cli(&["run", "tests/programs/math.twe"]);
    let bc = run_cli(&["run", "--vm", "bytecode", "tests/programs/math.twe"]);
    assert_eq!(tree, bc);
}

#[test]
fn run_vm_bytecode_matches_tree_on_interpolation() {
    let tree = run_cli(&["run", "tests/programs/interpolation.twe"]);
    let bc = run_cli(&["run", "--vm", "bytecode", "tests/programs/interpolation.twe"]);
    assert_eq!(tree, bc);
}

#[test]
fn run_vm_bytecode_with_frames_drives_scene() {
    // scene_counter.twe: `every 100ms:` printing 1..3 then idling.
    // Five frames of 100ms each should produce "1\n2\n3\n" via both.
    let tree = run_cli(&[
        "run",
        "--frames",
        "5",
        "tests/programs/scene_counter.twe",
    ]);
    let bc = run_cli(&[
        "run",
        "--vm",
        "bytecode",
        "--frames",
        "5",
        "tests/programs/scene_counter.twe",
    ]);
    assert_eq!(tree, bc);
}

#[test]
fn vm_bytecode_with_frames_runs_spawn_entities() {
    let tree = run_cli(&[
        "run",
        "--frames",
        "5",
        "tests/programs/spawn_entities.twe",
    ]);
    let bc = run_cli(&[
        "run",
        "--vm",
        "bytecode",
        "--frames",
        "5",
        "tests/programs/spawn_entities.twe",
    ]);
    assert_eq!(tree, bc);
}

#[test]
fn vm_alias_accepts_bc_shorthand() {
    let out = run_cli(&["run", "--vm", "bc", "tests/programs/hello.twe"]);
    assert_eq!(out, "hello, twe\n");
}

#[test]
fn unknown_vm_value_errors() {
    let output = Command::new(twec_bin())
        .args(["run", "--vm", "haskell", "tests/programs/hello.twe"])
        .output()
        .expect("spawn");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("--vm"), "stderr did not mention --vm: {err}");
}

#[test]
fn frames_is_only_valid_for_run() {
    let output = Command::new(twec_bin())
        .args(["play", "--frames", "5", "tests/programs/hello.twe"])
        .output()
        .expect("spawn");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("--frames"), "stderr did not mention --frames: {err}");
}
