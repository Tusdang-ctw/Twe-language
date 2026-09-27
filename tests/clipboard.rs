//! Phase 10 session 5b: clipboard surface.
//!
//! web3d-M0: isolated in its own test binary (its own process). The
//! native clipboard backend (`arboard` → Win32 clipboard) corrupted
//! the process heap (`STATUS_HEAP_CORRUPTION`) when exercised on one
//! libtest thread while other tests ran on sibling threads — measured
//! 9/80 crashes of the eval binary with these tests in it, 0/80
//! without. The game runtime only touches the clipboard from its
//! single main thread, so this is a test-harness hazard; keeping the
//! calls in one `#[test]`, alone in this binary, means they never run
//! concurrently with anything.
//!
//! Functional round-trips are skipped because CI runners typically
//! lack a display server / clipboard daemon (X11 / Wayland /
//! NSPasteboard). `os.clipboard.read` returns the empty string in that
//! case rather than erroring — exercised here to confirm the surface is
//! registered and returns the documented shapes.

use twec::{eval, lexer, parser};

fn run_program_str(src: &str) -> Result<String, String> {
    let tokens = lexer::lex(src).map_err(|e| format!("lex: {e}"))?;
    let program = parser::parse(&tokens).map_err(|e| format!("parse: {e}"))?;
    eval::run(&program).map_err(|e| format!("eval: {e}"))
}

#[test]
fn clipboard_surface_is_registered() {
    // Write succeeds-or-fails-silently; the return value is nil
    // either way so the script can chain calls without checking.
    // Writing replaces the developer's real clipboard, so it only runs
    // when explicitly opted in (`TWE_TEST_CLIPBOARD=1`, e.g. in CI).
    if std::env::var("TWE_TEST_CLIPBOARD").is_ok_and(|v| v == "1") {
        let out = run_program_str("os.clipboard.write(\"hello\")\nprint(\"done\")\n")
            .expect("program should run");
        assert_eq!(out, "done\n");
    }

    // Either the runner has a clipboard with text in it (then `out`
    // is whatever's there + newline) or it doesn't (`out == "\n"`).
    // Either way the call returns a string and the program exits
    // cleanly.
    let out = run_program_str("print(os.clipboard.read())\n").expect("program should run");
    assert!(out.ends_with('\n'), "got: {out:?}");
}
