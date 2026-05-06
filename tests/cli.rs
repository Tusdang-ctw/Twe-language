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
fn play_visual_subcommand_recognized() {
    // Phase 9 session 11: `play_visual` is a real CLI subcommand.
    // Invoking it with no args returns exit-2 (missing-path), proving
    // the dispatcher reaches handle_play_visual rather than the
    // unknown-command branch (which would also exit 2 but with a
    // different stderr — assert on the stderr to disambiguate).
    let output = Command::new(twec_bin())
        .arg("play_visual")
        .output()
        .expect("spawn twec");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`twec play_visual` requires a file path"),
        "got: {stderr}"
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn play_visual_rejects_unknown_flag() {
    let output = Command::new(twec_bin())
        .args(["play_visual", "--bogus", "foo.twe"])
        .output()
        .expect("spawn twec");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown flag for `play_visual`"),
        "got: {stderr}"
    );
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
    let bc = run_cli(&[
        "run",
        "--vm",
        "bytecode",
        "tests/programs/interpolation.twe",
    ]);
    assert_eq!(tree, bc);
}

#[test]
fn run_vm_bytecode_with_frames_drives_scene() {
    // scene_counter.twe: `every 100ms:` printing 1..3 then idling.
    // Five frames of 100ms each should produce "1\n2\n3\n" via both.
    let tree = run_cli(&["run", "--frames", "5", "tests/programs/scene_counter.twe"]);
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
    let tree = run_cli(&["run", "--frames", "5", "tests/programs/spawn_entities.twe"]);
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

// --- `twec types` (Phase 4a) ---

#[test]
fn types_subcommand_prints_let_int() {
    // Use a small temp file rather than touching tests/programs/
    // so the assertion can be exact without creating a .twe file
    // that's part of the wider test suite.
    let tmp = std::env::temp_dir().join("twec_types_let_int.twe");
    std::fs::write(&tmp, "let n = 42\nlet name = \"hi\"\nlet ok = true\n").expect("write");
    let out = run_cli(&["types", tmp.to_str().unwrap()]);
    let _ = std::fs::remove_file(&tmp);
    // Sorted alphabetically by handle_types.
    assert_eq!(out, "n: int\nname: string\nok: bool\n");
}

#[test]
fn types_subcommand_handles_real_test_program() {
    // arithmetic.twe — only lets and prints; every top-level let
    // should resolve to a known scalar type.
    let out = run_cli(&["types", "tests/programs/arithmetic.twe"]);
    // arithmetic.twe declares no top-level lets — only print
    // expressions. So the output is empty (zero bindings).
    // Confirm by parsing the file and asserting no bindings,
    // then assert empty output.
    assert_eq!(out, "");
}

#[test]
fn types_subcommand_records_class_and_instance() {
    let tmp = std::env::temp_dir().join("twec_types_class.twe");
    std::fs::write(
        &tmp,
        "item Counter:\n    value: 0\n\n    bump(amount):\n        self.value = self.value + amount\n\nlet c = Counter()\n",
    )
    .expect("write");
    let out = run_cli(&["types", tmp.to_str().unwrap()]);
    let _ = std::fs::remove_file(&tmp);
    // Counter binds as <class Counter>; c is an instance.
    assert!(out.contains("Counter: <class Counter>"), "got: {out}");
    assert!(out.contains("c: Counter"), "got: {out}");
}

#[test]
fn types_subcommand_unknown_for_unresolved_names() {
    let tmp = std::env::temp_dir().join("twec_types_unknown.twe");
    std::fs::write(&tmp, "let x = unknown_thing\n").expect("write");
    let out = run_cli(&["types", tmp.to_str().unwrap()]);
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(out, "x: ?\n");
}

#[test]
fn types_subcommand_errors_on_missing_path() {
    let output = std::process::Command::new(twec_bin())
        .args(["types"])
        .output()
        .expect("spawn");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("requires a file") || err.contains("single file"),
        "stderr: {err}"
    );
}

// --- end Phase 4a tests ---

#[test]
fn frames_is_only_valid_for_run() {
    let output = Command::new(twec_bin())
        .args(["play", "--frames", "5", "tests/programs/hello.twe"])
        .output()
        .expect("spawn");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("--frames"),
        "stderr did not mention --frames: {err}"
    );
}

// --- Phase 13 session 8: `twec verify <file>` ---

#[test]
fn verify_subcommand_clean_program_exits_zero_with_json() {
    // A clean `# verified` file should print a JSON document with
    // an empty diagnostics array and exit 0. The combined contract
    // of "stdout is JSON" + "exit 0 means OK" is what an LLM
    // self-correction loop reads.
    let dir = std::env::temp_dir().join(format!("twec-verify-clean-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("clean.twe");
    std::fs::write(&file, "# verified\nlet x: int = 42\n").unwrap();
    let output = Command::new(twec_bin())
        .args(["verify", file.to_str().unwrap()])
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "stdout: {stdout}");
    assert!(stdout.contains("\"tool\":\"twec-verify\""));
    assert!(stdout.contains("\"verified\":true"));
    assert!(stdout.contains("\"diagnostics\":[]"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_subcommand_dirty_program_exits_one_with_diagnostic() {
    let dir = std::env::temp_dir().join(format!("twec-verify-dirty-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("dirty.twe");
    std::fs::write(&file, "# verified\nlet x: int = \"hi\"\n").unwrap();
    let output = Command::new(twec_bin())
        .args(["verify", file.to_str().unwrap()])
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "stdout: {stdout}");
    assert!(stdout.contains("\"errors\":1"));
    assert!(stdout.contains("\"kind\":\"type-error.let-annotation\""));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_subcommand_missing_file_emits_io_error_diagnostic() {
    // The error path also produces JSON — an LLM consumer doesn't
    // need to special-case "the file didn't exist", just read the
    // stable shape and surface the diagnostic to the user.
    let output = Command::new(twec_bin())
        .args(["verify", "/nonexistent/path/that/should/not/exist.twe"])
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("\"kind\":\"io-error\""));
    assert!(stdout.contains("cannot read"));
}

#[test]
fn verify_warn_deprecated_flag_emits_warning() {
    // Phase 13 session 10: `--warn-deprecated` surfaces a
    // `deprecation` warning per use site of a `@deprecated` symbol.
    // Exit code stays 0 because warnings aren't errors.
    let dir = std::env::temp_dir().join(format!("twec-verify-dep-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("dep.twe");
    std::fs::write(
        &file,
        "@deprecated(\"since v0.7\")\nfunction old(): return 1\nlet x = old()\n",
    )
    .unwrap();
    let output = Command::new(twec_bin())
        .args(["verify", "--warn-deprecated", file.to_str().unwrap()])
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Exit 0 because warnings only — no errors.
    assert_eq!(output.status.code(), Some(0), "stdout: {stdout}");
    assert!(stdout.contains("\"warnings\":1"), "stdout: {stdout}");
    assert!(stdout.contains("\"kind\":\"deprecation\""), "stdout: {stdout}");
    assert!(stdout.contains("`old` is deprecated"), "stdout: {stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_without_warn_deprecated_omits_deprecation_warnings() {
    // Symmetry check: without the flag, the same input produces
    // no deprecation warnings.
    let dir = std::env::temp_dir().join(format!("twec-verify-nodep-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("dep.twe");
    std::fs::write(
        &file,
        "@deprecated(\"since v0.7\")\nfunction old(): return 1\nlet x = old()\n",
    )
    .unwrap();
    let output = Command::new(twec_bin())
        .args(["verify", file.to_str().unwrap()])
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("\"warnings\":0"), "stdout: {stdout}");
    assert!(stdout.contains("\"diagnostics\":[]"), "stdout: {stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_subcommand_no_args_errors() {
    let output = Command::new(twec_bin())
        .arg("verify")
        .output()
        .expect("spawn");
    assert_eq!(output.status.code(), Some(2));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("requires a file path"),
        "stderr: {err}"
    );
}
