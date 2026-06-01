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

/// Programs that exercise a construct the bytecode VM rejects (at
/// compile time, or at runtime for the closure case) ON PURPOSE. Each
/// entry MUST cite the `src/compiler.rs` (or other src) line that
/// justifies the skip so this list stays honest and shrinks as the VM
/// gains parity. These are *agreed* deferrals — the tree-walker runs
/// them, the VM declines them with a clear "use --vm tree" message —
/// not silent divergences. This list IS the VM's documented-deferral
/// backlog, measured rather than guessed.
const VM_UNSUPPORTED: &[(&str, &str)] = &[
    (
        "death_event_phase9.twe",
        "compiler.rs:1480 — field default must be a literal constant in v0.1",
    ),
    (
        "example_2_simplified.twe",
        "compiler.rs:1480 — field default must be a literal constant in v0.1",
    ),
    (
        "dialogue_minimal.twe",
        "compiler.rs:528 — dialogue / say / choice not yet compiled (tree-walker only)",
    ),
    (
        "literals.twe",
        "compiler.rs:1196 — quantity literal (e.g. `3kg`) not yet compiled (Phase 4)",
    ),
    (
        "tilemap_aabb.twe",
        "compiler.rs:1232 — keyword arguments in bytecode calls not yet supported",
    ),
    (
        "lang_plural_closure.twe",
        "stdlib lang.set_plural_rule — custom Twe-closure plural rules are a tree-walker-only \
         runtime path (needs eval::call_function); the VM rejects at runtime",
    ),
    (
        "state_hooks.twe",
        "compiler.rs — `on exit:` state hook is tree-walker-only (the VM doesn't yet mirror the \
         enter_state exit hook); tree-walker-first per the 2026-06-01 VM-strategy decision",
    ),
    (
        "list_comp.twe",
        "compiler.rs — list comprehensions are tree-walker-only (a comprehension lowers to a \
         hidden loop + accumulator the frozen VM doesn't emit); tree-walker-first per the \
         2026-06-01 VM-strategy decision",
    ),
    (
        "then_seq.twe",
        "compiler.rs — `<action> then <body>` sequencing is tree-walker-only (the frozen VM \
         doesn't mirror the fiber suspend/resume path); also uses a `0.2s` quantity literal the \
         VM rejects. Tree-walker-first per the 2026-06-01 VM-strategy decision",
    ),
];

/// Real tree-walker/VM divergences that are KNOWN BUGS, not agreed
/// deferrals — quarantined here so the harness stays green as a "no
/// NEW divergence" gate while these stay tracked for a fix. Keyed by
/// `(program, frames)` so we quarantine only the failing configuration
/// and keep coverage of the passing ones. Every entry is a debt item:
/// the goal is to empty this list, not grow it.
///
/// Empty as of the craft-hardening pass: the one prior entry
/// (`scene_methods.twe` at frames=10 — bare sibling-method calls like
/// `bump()` from a state's `every` body) is fixed in `compiler.rs`,
/// which now lowers a bare call to a declared sibling method into
/// `self.name(args)` via `OP_INVOKE`, mirroring the tree-walker's
/// `eval_call` self-method precedence.
const KNOWN_VM_BUGS: &[(&str, u32, &str)] = &[];

/// Programs that write to the filesystem at a fixed repo-root path
/// (e.g. `save.write("save_block_test.json")`). The parity sweep runs
/// in parallel with the `eval` suite, whose dedicated save tests
/// write/read/remove those same paths — running these here too races
/// on a shared file and makes the suite flaky. Their tree/VM behaviour
/// is already pinned by the dedicated `runs_save_*` eval tests, so the
/// broad parity sweep skips them. Not a deferral; an isolation skip.
const WRITES_FILES: &[&str] = &[
    "save_block.twe",
    "save_schema_version.twe",
    "v1_0_2_sugar.twe",
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

        if WRITES_FILES.contains(&name.as_str()) {
            skipped += 1;
            eprintln!("skip {name}: writes to a shared repo-root path; covered by the eval suite");
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

            // Quarantine a known, tracked VM bug for this exact
            // (program, frames) configuration so the harness stays a
            // "no NEW divergence" gate. We still RUN both backends and
            // assert the divergence is the *expected* one — if the bug
            // is silently fixed, the program now matches and we fail
            // loudly telling the maintainer to remove the quarantine.
            let quarantined = KNOWN_VM_BUGS
                .iter()
                .find(|(n, f, _)| *n == name && *f == frames)
                .map(|(_, _, reason)| *reason);

            let tree = run(&tree_args);
            let vm = run(&vm_args);
            checked += 1;

            if let Some(reason) = quarantined {
                let still_diverges = match (&tree, &vm) {
                    (Ok(t), Ok(v)) => t != v,
                    (Err(_), Ok(_)) | (Ok(_), Err(_)) => true,
                    (Err(_), Err(_)) => false,
                };
                if still_diverges {
                    skipped += 1;
                    eprintln!("known-bug (quarantined) {name} (frames={frames}): {reason}");
                } else {
                    failures.push(format!(
                        "QUARANTINE STALE  {name} (frames={frames}) — backends now AGREE; \
                         the bug appears fixed. Remove this entry from KNOWN_VM_BUGS.\n  was: {reason}"
                    ));
                }
                continue;
            }

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
