// web3d-M10 session 1: the `twec debug` stdio protocol. Drive the
// debugger via the binary: set a breakpoint, run, read the paused
// snapshot, continue. Verifies the line-JSON contract Studio depends on.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_debug(src: &str, commands: &str) -> Vec<twec::json::Value> {
    // One file per call: the tests run in parallel in one process, and a
    // shared path let one test's program overwrite the other's mid-run.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("twe_dbg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("prog{n}.twe"));
    std::fs::write(&file, src).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_twec"))
        .args(["debug", "--frames", "0"])
        .arg(&file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn twec debug");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(commands.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.trim_start().starts_with('{'))
        .map(|l| twec::json::parse(l).expect("event JSON"))
        .collect()
}

#[test]
fn breakpoint_pause_and_continue_over_stdio() {
    let events = run_debug(
        "let a = 1\nlet b = 2\nlet c = a + b\n",
        "{\"cmd\":\"set_breakpoints\",\"lines\":[3]}\n{\"cmd\":\"run\"}\n{\"cmd\":\"continue\"}\n",
    );
    let ev = |i: usize| events[i].get("event").and_then(|e| e.as_str());
    assert_eq!(ev(0), Some("ready"));
    assert_eq!(ev(1), Some("ok"));
    // The pause is at line 3, a breakpoint, with user globals a=1, b=2
    // (c is not yet declared).
    let paused = &events[2];
    assert_eq!(paused.get("event").and_then(|e| e.as_str()), Some("paused"));
    assert_eq!(paused.get("line").and_then(|l| l.as_i64()), Some(3));
    assert_eq!(
        paused.get("reason").and_then(|r| r.as_str()),
        Some("breakpoint")
    );
    let globals = paused.get("globals").and_then(|g| g.as_array()).unwrap();
    // Only the two user globals, not the stdlib surface.
    assert_eq!(globals.len(), 2);
    assert_eq!(
        events
            .last()
            .and_then(|e| e.get("event"))
            .and_then(|e| e.as_str()),
        Some("exited")
    );
}

#[test]
fn stop_before_run_exits_cleanly() {
    let events = run_debug("let a = 1\n", "{\"cmd\":\"stop\"}\n");
    assert_eq!(
        events[0].get("event").and_then(|e| e.as_str()),
        Some("ready")
    );
    assert_eq!(
        events
            .last()
            .and_then(|e| e.get("event"))
            .and_then(|e| e.as_str()),
        Some("exited")
    );
}
