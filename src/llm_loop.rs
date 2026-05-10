//! Phase 33 session 4: end-to-end LLM authoring loop.
//!
//! The loop closes the contract Tier 1 set up:
//!
//!   prompt → generate → write file → `verify` → if errors,
//!   feed structured JSON back → repeat → pass / give up
//!
//! and logs every round-trip to `traces/<timestamp>.jsonl` so the
//! same machinery doubles as a fine-tuning corpus generator. A single
//! converged session is one labelled training datum.
//!
//! ## Provider abstraction (no network deps in the binary)
//!
//! The HTTP path lands in a follow-on `--features llm-loop-http`
//! session — pulling `reqwest` + `tokio` would inflate the default
//! build by ~120 crates. Phase 33 ships two zero-dep providers
//! sufficient to prove and exercise the loop:
//!
//! - [`FixtureProvider`] — in-memory canned responses, used in
//!   `tests/llm_loop.rs`. Deterministic, network-free, fast.
//! - [`CommandProvider`] — shells out to a user-configured command
//!   (e.g. `claude code -p $PROMPT`, `python my_wrapper.py`,
//!   `curl -s ...`). The command receives the prompt on stdin and
//!   returns the model's reply on stdout. Lets a contributor wire
//!   any provider — including local `llama.cpp` with our exported
//!   GBNF grammar — without rebuilding `twec`.
//!
//! Custom providers in third-party tooling implement the
//! [`LlmProvider`] trait directly.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::verify::{verify_program_with_path, VerifyReport};

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// One synchronous round-trip with an LLM. Implementations are
/// blocking — the loop driver runs sequentially on the calling
/// thread because the cost is dominated by the model's latency,
/// not local compute.
pub trait LlmProvider: Send {
    /// Send `prompt` and return the model's text reply, or an error
    /// describing why no reply could be produced. Errors stop the
    /// loop immediately — they're treated as infrastructure failures,
    /// not model mistakes the loop should retry past.
    fn complete(&mut self, prompt: &str) -> Result<String, String>;

    /// Short identifier used in trace files (`provider: "claude"`).
    /// Default `"unknown"` for ad-hoc providers in tests.
    fn name(&self) -> &str {
        "unknown"
    }
}

// ---------------------------------------------------------------------------
// FixtureProvider — for tests. Returns canned responses in order.
// ---------------------------------------------------------------------------

/// Deterministic in-memory provider. Each `complete` call returns
/// the next item from the pre-loaded queue. Panics if the queue
/// runs dry — that indicates a test scenario that didn't model
/// the loop's iteration count correctly.
pub struct FixtureProvider {
    pub responses: std::collections::VecDeque<String>,
    pub captured_prompts: Vec<String>,
}

impl FixtureProvider {
    pub fn new(responses: impl IntoIterator<Item = String>) -> Self {
        Self {
            responses: responses.into_iter().collect(),
            captured_prompts: Vec::new(),
        }
    }
}

impl LlmProvider for FixtureProvider {
    fn complete(&mut self, prompt: &str) -> Result<String, String> {
        self.captured_prompts.push(prompt.to_string());
        self.responses
            .pop_front()
            .ok_or_else(|| "FixtureProvider queue is empty".to_string())
    }
    fn name(&self) -> &str {
        "fixture"
    }
}

// ---------------------------------------------------------------------------
// CommandProvider — shells out, no native deps.
// ---------------------------------------------------------------------------

/// Spawn a configured command, write the prompt to its stdin, and
/// read the reply from stdout. The command is the contributor's
/// integration point — point it at a `claude` CLI, a Python wrapper
/// over the OpenAI client, a local `llama-cli` with `--grammar twe.gbnf`
/// set, or anything else that fits the pipe.
pub struct CommandProvider {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandProvider {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
        }
    }
}

impl LlmProvider for CommandProvider {
    fn complete(&mut self, prompt: &str) -> Result<String, String> {
        use std::io::Write;
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn `{}` failed: {e}", self.program))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.as_bytes())
                .map_err(|e| format!("writing prompt to `{}` stdin failed: {e}", self.program))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("waiting on `{}` failed: {e}", self.program))?;
        if !output.status.success() {
            return Err(format!(
                "`{}` exited with {}: {}",
                self.program,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        String::from_utf8(output.stdout)
            .map_err(|e| format!("`{}` produced non-UTF-8 output: {e}", self.program))
    }
    fn name(&self) -> &str {
        "command"
    }
}

// ---------------------------------------------------------------------------
// Loop driver
// ---------------------------------------------------------------------------

/// Settings for one [`run_loop`] invocation.
#[derive(Clone, Debug)]
pub struct LoopOptions {
    /// Maximum prompt+verify rounds before giving up. Includes the
    /// initial generation; `max_rounds = 1` runs once with no
    /// retries on errors.
    pub max_rounds: u32,
    /// Where to log per-round JSONL traces. `None` disables tracing.
    pub trace_dir: Option<PathBuf>,
    /// Path to use in verify diagnostics + trace metadata. Doesn't
    /// have to exist on disk; verify is run on the source string.
    pub source_path: Option<String>,
    /// If true, every prompt sent to the provider is appended to
    /// the trace under `prompt`. Costs a few KB per round per
    /// trace. Default true.
    pub log_prompts: bool,
}

impl Default for LoopOptions {
    fn default() -> Self {
        Self {
            max_rounds: 5,
            trace_dir: None,
            source_path: None,
            log_prompts: true,
        }
    }
}

/// One round of the loop. Captured both for return-value introspection
/// and for trace serialization.
#[derive(Clone, Debug)]
pub struct LoopRound {
    pub round: u32,
    pub prompt: String,
    pub response: String,
    pub verify_json: String,
    pub passed: bool,
}

/// Result of running the loop to convergence (or to the round limit).
/// `final_source` is the most recent generation, `passed` says whether
/// it was clean under verify. `rounds` is the audit trail.
#[derive(Clone, Debug)]
pub struct LoopOutcome {
    pub final_source: String,
    pub passed: bool,
    pub rounds: Vec<LoopRound>,
    pub trace_path: Option<PathBuf>,
}

/// Run the loop. The initial prompt is the user's task description;
/// each subsequent round appends the previous reply *and* the verify
/// JSON to nudge the model toward a passing program. Returns when
/// verify is clean or after `max_rounds` attempts, whichever comes
/// first.
///
/// Failure modes:
/// - Provider returns an error — propagated immediately, loop stops.
/// - Tracing IO fails — logged to stderr but does not stop the loop.
pub fn run_loop(
    provider: &mut dyn LlmProvider,
    initial_prompt: &str,
    options: &LoopOptions,
) -> Result<LoopOutcome, String> {
    let trace_path = options
        .trace_dir
        .as_ref()
        .map(|d| trace_filename(d, provider.name()));
    if let Some(p) = trace_path.as_ref() {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let mut rounds: Vec<LoopRound> = Vec::new();
    let mut current_prompt = initial_prompt.to_string();

    for round in 1..=options.max_rounds {
        let response = provider.complete(&current_prompt)?;
        let source = extract_twe_source(&response);
        let report = verify_program_with_path(&source, options.source_path.as_deref());
        let verify_json = report.to_json();
        let passed = report.ok();

        let logged_prompt = if options.log_prompts {
            current_prompt.clone()
        } else {
            String::new()
        };
        let rec = LoopRound {
            round,
            prompt: logged_prompt,
            response: response.clone(),
            verify_json: verify_json.clone(),
            passed,
        };
        if let Some(p) = trace_path.as_ref() {
            if let Err(e) = append_trace(p, &rec, provider.name(), &report) {
                eprintln!("[twec llm-loop] trace write failed: {e}");
            }
        }
        rounds.push(rec);

        if passed {
            return Ok(LoopOutcome {
                final_source: source,
                passed: true,
                rounds,
                trace_path,
            });
        }

        if round == options.max_rounds {
            return Ok(LoopOutcome {
                final_source: source,
                passed: false,
                rounds,
                trace_path,
            });
        }

        current_prompt = build_followup_prompt(&source, &verify_json);
    }

    // Unreachable: the loop returns inside on every iteration.
    unreachable!("run_loop iteration order is exhaustive")
}

/// Pull a `.twe` source out of an LLM reply. Models are sloppy about
/// fences — the contract here is permissive: if the reply contains a
/// triple-backtick block (with or without a `twe` language tag), we
/// take the *first* one. Otherwise the reply is treated as raw Twe.
///
/// This is the single most common LLM-side failure mode in code-gen
/// loops; centralising it here keeps individual providers simple.
pub fn extract_twe_source(reply: &str) -> String {
    if let Some(after) = reply.find("```twe") {
        let body = &reply[after + "```twe".len()..];
        // Skip any newline immediately after the opening fence.
        let body = body.strip_prefix('\n').unwrap_or(body);
        if let Some(end) = body.find("```") {
            return body[..end].trim_end().to_string();
        }
    }
    if let Some(after) = reply.find("```") {
        let body = &reply[after + 3..];
        // Optional language tag on its own line.
        let body = match body.find('\n') {
            Some(nl) if !body[..nl].contains("```") => &body[nl + 1..],
            _ => body,
        };
        if let Some(end) = body.find("```") {
            return body[..end].trim_end().to_string();
        }
    }
    reply.trim().to_string()
}

/// Build the follow-up prompt for the next round. Includes the
/// previous source and the structured verify JSON — the latter is
/// the LLM's machine-readable feedback channel. Prompts the model
/// to produce a corrected version inside a `twe` fence.
fn build_followup_prompt(prev_source: &str, verify_json: &str) -> String {
    format!(
        "Your previous Twe program failed verification. Below is the program you produced and the structured diagnostics from `twec verify`. Apply the suggested fixes (each diagnostic carries a `fix.edits` array with anchored replacements you can apply mechanically) and emit a corrected program inside a single ```twe fenced block.\n\nPrevious program:\n```twe\n{prev_source}\n```\n\nVerify diagnostics (JSON v2):\n{verify_json}\n\nReply with the corrected program only — no commentary outside the code fence."
    )
}

// ---------------------------------------------------------------------------
// Trace logging
// ---------------------------------------------------------------------------

fn trace_filename(dir: &Path, provider_name: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(format!("llm_loop_{provider_name}_{ts}.jsonl"))
}

fn append_trace(
    path: &Path,
    rec: &LoopRound,
    provider_name: &str,
    report: &VerifyReport,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut s = String::with_capacity(rec.response.len() + rec.verify_json.len() + 128);
    s.push('{');
    s.push_str("\"tool\":\"twec-llm-loop\",\"version\":1");
    s.push_str(",\"provider\":");
    write_json_string(&mut s, provider_name);
    s.push_str(",\"round\":");
    s.push_str(&rec.round.to_string());
    s.push_str(",\"passed\":");
    s.push_str(if rec.passed { "true" } else { "false" });
    s.push_str(",\"errors\":");
    s.push_str(&report.errors().to_string());
    s.push_str(",\"warnings\":");
    s.push_str(&report.warnings().to_string());
    s.push_str(",\"prompt\":");
    write_json_string(&mut s, &rec.prompt);
    s.push_str(",\"response\":");
    write_json_string(&mut s, &rec.response);
    s.push_str(",\"verify\":");
    s.push_str(&rec.verify_json);
    s.push_str("}\n");

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(s.as_bytes())
}

fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_handles_twe_fenced_block() {
        let reply = "Sure, here's the program:\n\n```twe\nlet x = 1\nlet y = 2\n```\n\nHope that helps!";
        assert_eq!(extract_twe_source(reply), "let x = 1\nlet y = 2");
    }

    #[test]
    fn extract_handles_unlanguaged_fence() {
        let reply = "```\nlet x = 1\n```";
        assert_eq!(extract_twe_source(reply), "let x = 1");
    }

    #[test]
    fn extract_returns_raw_when_no_fence() {
        let reply = "let x = 1\n";
        assert_eq!(extract_twe_source(reply), "let x = 1");
    }

    #[test]
    fn fixture_provider_returns_canned_responses_in_order() {
        let mut p = FixtureProvider::new(["one".into(), "two".into()]);
        assert_eq!(p.complete("a").unwrap(), "one");
        assert_eq!(p.complete("b").unwrap(), "two");
        assert_eq!(p.captured_prompts, vec!["a", "b"]);
        // Empty queue: error, not panic, so the loop can report
        // cleanly.
        assert!(p.complete("c").is_err());
    }

    #[test]
    fn loop_passes_on_first_round_when_program_clean() {
        let mut p = FixtureProvider::new(["```twe\nlet x = 1\n```".into()]);
        let outcome = run_loop(
            &mut p,
            "Write a Twe program that binds x to 1.",
            &LoopOptions::default(),
        )
        .unwrap();
        assert!(outcome.passed);
        assert_eq!(outcome.rounds.len(), 1);
        assert_eq!(outcome.final_source, "let x = 1");
    }

    #[test]
    fn loop_iterates_until_verify_clean() {
        // Round 1 has a typo; round 2 fixes it.
        let mut p = FixtureProvider::new([
            "```twe\n# verified\nlet apple = 1\nlet y = aple\n```".into(),
            "```twe\n# verified\nlet apple = 1\nlet y = apple\n```".into(),
        ]);
        let outcome = run_loop(
            &mut p,
            "Write a Twe program with a verified header.",
            &LoopOptions {
                max_rounds: 5,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(outcome.passed);
        assert_eq!(outcome.rounds.len(), 2);
        // The second prompt must include the verify JSON so the
        // model knows what to fix.
        assert!(p.captured_prompts[1].contains("\"version\":2"));
        assert!(p.captured_prompts[1].contains("aple"));
    }

    #[test]
    fn loop_gives_up_after_max_rounds_when_unfixed() {
        let mut p = FixtureProvider::new([
            "```twe\n# verified\nlet x = oops\n```".into(),
            "```twe\n# verified\nlet x = oops\n```".into(),
        ]);
        let outcome = run_loop(
            &mut p,
            "task",
            &LoopOptions {
                max_rounds: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!outcome.passed);
        assert_eq!(outcome.rounds.len(), 2);
    }

    #[test]
    fn provider_error_propagates() {
        struct ErrProvider;
        impl LlmProvider for ErrProvider {
            fn complete(&mut self, _: &str) -> Result<String, String> {
                Err("provider down".into())
            }
        }
        let mut p = ErrProvider;
        assert!(run_loop(&mut p, "task", &LoopOptions::default()).is_err());
    }
}
