//! Phase 33 session 4: end-to-end LLM authoring loop. web3d-M5: on
//! the shared `twe-llm` crate.
//!
//! ```text
//! task → model → edit applied → `twec verify` → clean? done
//!                                             → else the diagnostics go back → repeat
//! ```
//!
//! The rounds are one conversation: each failed round appends the
//! model's reply and the verify JSON (or why its edit didn't apply), so
//! the model sees its own previous attempt rather than a restatement.
//! Replies follow the one edit protocol in [`twe_llm::edit`]:
//! SEARCH/REPLACE blocks against the current file, or a whole file in a
//! ```` ```twe ```` block. A reply with neither is taken as raw Twe, for
//! command providers that print bare source.
//!
//! Every round can be logged as one JSONL line (prompt, reply, verify
//! JSON, tokens, cost), so the loop doubles as a training-corpus
//! generator.
//!
//! Providers come from `twe-llm`: Anthropic and OpenAI-compatible APIs
//! (with the `llm-http` feature), any shell command, and fixtures for
//! tests.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub use twe_llm::{CommandProvider, FixtureProvider, Provider};
use twe_llm::edit::{self, Edit};
use twe_llm::{Message, Reply, Request, StopReason, Usage};

use crate::verify::verify_program_with_path;

/// Settings for one [`run_loop`] invocation.
#[derive(Clone, Debug)]
pub struct LoopOptions {
    /// Maximum rounds, including the first generation; `1` means no
    /// retries.
    pub max_rounds: u32,
    /// Where to log per-round JSONL traces. `None` disables tracing.
    pub trace_dir: Option<PathBuf>,
    /// Path used in verify diagnostics and traces (needn't exist).
    pub source_path: Option<String>,
    /// Log each round's request in the trace (a few KB per round).
    pub log_prompts: bool,
    /// The system prompt: grounding that stays the same across rounds
    /// (the Twe primer, say). The edit protocol's instructions are
    /// appended to it.
    pub system: String,
    /// The file the task starts from; empty for a new program.
    pub starter: String,
    /// Per-reply token ceiling.
    pub max_tokens: u32,
    /// Send verify's diagnostics back when it finds errors (on by
    /// default). Off, verify still runs and is recorded, but only the
    /// extra check of [`run_loop_checked`] can send a round back.
    pub verify_feedback: bool,
    /// The language's fence tag (`"twe"`; the benchmark's baseline uses
    /// `"python"`), used to read whole-file replies and to quote files.
    pub lang: String,
    /// Other fence tags accepted for whole files (`"py"`).
    pub lang_aliases: Vec<String>,
}

impl Default for LoopOptions {
    fn default() -> Self {
        Self {
            max_rounds: 5,
            trace_dir: None,
            source_path: None,
            log_prompts: true,
            system: String::new(),
            starter: String::new(),
            max_tokens: 16_000,
            verify_feedback: true,
            lang: "twe".into(),
            lang_aliases: Vec::new(),
        }
    }
}

/// One round of the loop.
#[derive(Clone, Debug)]
pub struct LoopRound {
    pub round: u32,
    /// The request's last user message (what this round asked).
    pub prompt: String,
    pub response: String,
    /// The file after this round's edit.
    pub source: String,
    /// Verify's JSON report; empty when the edit didn't apply.
    pub verify_json: String,
    /// Verify's error count (0 when the edit didn't apply).
    pub verify_errors: usize,
    /// What the extra check of [`run_loop_checked`] reported.
    pub check_error: Option<String>,
    /// Why the reply's edit couldn't be applied, if it couldn't.
    pub edit_error: Option<String>,
    pub passed: bool,
    pub stop: StopReason,
    pub usage: Usage,
    pub cost_usd: Option<f64>,
}

/// The loop's result: the last file, whether verify accepted it, and
/// the rounds that got there.
#[derive(Clone, Debug)]
pub struct LoopOutcome {
    pub final_source: String,
    pub passed: bool,
    pub rounds: Vec<LoopRound>,
    pub trace_path: Option<PathBuf>,
    /// Tokens over all rounds.
    pub usage: Usage,
    /// Dollars over all rounds; `None` when no round's cost was known.
    pub cost_usd: Option<f64>,
}

/// Run the loop on `task` until verify accepts the file or
/// `max_rounds` run out (see [`run_loop_checked`] for more checks). A provider error stops the loop and is
/// returned (an infrastructure failure, not the model's); a trace write
/// failure is reported on stderr and doesn't.
pub fn run_loop(
    provider: &mut dyn Provider,
    task: &str,
    options: &LoopOptions,
) -> Result<LoopOutcome, String> {
    run_loop_checked(provider, task, options, &|_| None)
}

/// [`run_loop`] with one more check after verify: `check(source)`
/// returns why the program isn't acceptable yet (the benchmark runs it
/// for two seconds and reports a crash), or `None`. Its message goes
/// back to the model like verify's diagnostics.
pub fn run_loop_checked(
    provider: &mut dyn Provider,
    task: &str,
    options: &LoopOptions,
    check: &dyn Fn(&str) -> Option<String>,
) -> Result<LoopOutcome, String> {
    let path = options.source_path.clone();
    let verify = move |source: &str| {
        let report = verify_program_with_path(source, path.as_deref());
        let json = report.to_json();
        StaticVerdict {
            errors: report.errors(),
            message: format!(
                "`twec verify` rejected the program. Its diagnostics (JSON v2) follow; each carries `fix.edits`, \
                 anchored replacements you can apply.\n\n{json}"
            ),
            report: json,
        }
    };
    run_loop_with(provider, task, options, &verify, check)
}

/// What a language's static check found (`twec verify` for Twe; the
/// benchmark's Python baseline uses `compile()` and pyflakes).
pub struct StaticVerdict {
    pub errors: usize,
    /// The full report, kept in the round's record and trace.
    pub report: String,
    /// What goes back to the model when there are errors.
    pub message: String,
}

/// The loop for any language: `static_check` plays verify's part
/// (sent back when it finds errors, if `verify_feedback` is on), then
/// `check` runs. Replies are read and written in `options.lang`'s
/// fenced blocks.
pub fn run_loop_with(
    provider: &mut dyn Provider,
    task: &str,
    options: &LoopOptions,
    static_check: &dyn Fn(&str) -> StaticVerdict,
    check: &dyn Fn(&str) -> Option<String>,
) -> Result<LoopOutcome, String> {
    let trace_path = options
        .trace_dir
        .as_ref()
        .map(|d| trace_filename(d, &provider.id()));
    if let Some(parent) = trace_path.as_ref().and_then(|p| p.parent()) {
        let _ = std::fs::create_dir_all(parent);
    }

    let lang = options.lang.as_str();
    let fences: Vec<&str> = std::iter::once(lang).chain(options.lang_aliases.iter().map(String::as_str)).collect();
    let protocol = edit::protocol_instructions(lang);
    let system = if options.system.is_empty() {
        protocol
    } else {
        format!("{}\n\n{protocol}", options.system)
    };
    let first = if options.starter.is_empty() {
        task.to_string()
    } else {
        format!("{task}\n\nThe current file:\n```{lang}\n{}\n```", options.starter)
    };
    let mut request = Request {
        system,
        messages: vec![Message::user(first)],
        max_tokens: options.max_tokens,
    };
    let mut source = options.starter.clone();
    let mut rounds: Vec<LoopRound> = Vec::new();
    let reply_how = format!(
        "Reply with SEARCH/REPLACE blocks against the current file, or the whole corrected file in one ```{lang} block."
    );

    for round in 1..=options.max_rounds {
        let reply = provider.complete(&request).map_err(|e| e.to_string())?;
        let applied = match edit::parse_reply(&reply.text, &fences) {
            Edit::Nothing => Ok(reply.text.trim().to_string()),
            e => edit::apply(&source, &e).map_err(|e| e.to_string()),
        };
        let truncated = if reply.stop == StopReason::MaxTokens {
            "Your reply was cut off at the token limit; keep the next one shorter (SEARCH/REPLACE blocks rather than the whole file).\n\n"
        } else {
            ""
        };
        let mut verify_errors = 0;
        let mut check_error = None;
        let (verify_json, passed, edit_error, feedback) = match applied {
            Ok(candidate) => {
                source = candidate;
                let verdict = static_check(&source);
                verify_errors = verdict.errors;
                if options.verify_feedback && verdict.errors > 0 {
                    let feedback = format!("{truncated}{}\n\n{reply_how}", verdict.message);
                    (verdict.report, false, None, feedback)
                } else if let Some(problem) = check(&source) {
                    check_error = Some(problem.clone());
                    (verdict.report, false, None, format!("{truncated}{problem}\n\n{reply_how}"))
                } else {
                    (verdict.report, true, None, String::new())
                }
            }
            Err(e) => {
                let feedback = format!(
                    "{truncated}Your edit could not be applied: {e}. The file is unchanged:\n```{lang}\n{source}\n```\n{reply_how}"
                );
                (String::new(), false, Some(e), feedback)
            }
        };
        let rec = LoopRound {
            round,
            prompt: if options.log_prompts {
                request.messages.last().map(|m| m.text.clone()).unwrap_or_default()
            } else {
                String::new()
            },
            response: reply.text.clone(),
            source: source.clone(),
            verify_json,
            verify_errors,
            check_error,
            edit_error,
            passed,
            stop: reply.stop,
            usage: reply.usage,
            cost_usd: reply.cost_usd,
        };
        if let Some(p) = trace_path.as_ref() {
            if let Err(e) = append_trace(p, &rec, &reply) {
                eprintln!("[twec llm-loop] trace write failed: {e}");
            }
        }
        rounds.push(rec);
        if passed || round == options.max_rounds {
            break;
        }
        request.messages.push(Message::assistant(reply.text));
        request.messages.push(Message::user(feedback));
    }

    let mut usage = Usage::default();
    for r in &rounds {
        usage.add(r.usage);
    }
    let costs: Vec<f64> = rounds.iter().filter_map(|r| r.cost_usd).collect();
    Ok(LoopOutcome {
        passed: rounds.last().is_some_and(|r| r.passed),
        final_source: source,
        rounds,
        trace_path,
        usage,
        cost_usd: (!costs.is_empty()).then(|| costs.iter().sum()),
    })
}

/// Pull a `.twe` source out of an LLM reply: the first ```` ```twe ````
/// (or untagged) fenced block, or the whole reply trimmed when there's
/// none.
pub fn extract_twe_source(reply: &str) -> String {
    edit::fenced_file(reply, &["twe"])
        .map(|s| s.trim_end().to_string())
        .unwrap_or_else(|| reply.trim().to_string())
}

// ---------------------------------------------------------------------------
// Trace logging
// ---------------------------------------------------------------------------

fn trace_filename(dir: &Path, provider_id: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let safe: String = provider_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
        .collect();
    dir.join(format!("llm_loop_{safe}_{ts}.jsonl"))
}

/// One JSONL line per round. Version 2 (web3d-M5) adds the model,
/// stop reason, token usage, cost and edit errors.
fn append_trace(path: &Path, rec: &LoopRound, reply: &Reply) -> std::io::Result<()> {
    use std::io::Write;
    let verify = if rec.verify_json.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&rec.verify_json).unwrap_or(serde_json::Value::Null)
    };
    let line = serde_json::json!({
        "tool": "twec-llm-loop",
        "version": 2,
        "model": reply.model,
        "round": rec.round,
        "passed": rec.passed,
        "stop": rec.stop.as_str(),
        "usage": {
            "input": rec.usage.input,
            "output": rec.usage.output,
            "cache_read": rec.usage.cache_read,
            "cache_write": rec.usage.cache_write,
        },
        "cost_usd": rec.cost_usd,
        "edit_error": rec.edit_error,
        "verify_errors": rec.verify_errors,
        "check_error": rec.check_error,
        "prompt": rec.prompt,
        "response": rec.response,
        "verify": verify,
    });
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_handles_fences_and_raw_text() {
        let reply = "Sure:\n\n```twe\nlet x = 1\nlet y = 2\n```\n\nHope that helps!";
        assert_eq!(extract_twe_source(reply), "let x = 1\nlet y = 2");
        assert_eq!(extract_twe_source("```\nlet x = 1\n```"), "let x = 1");
        assert_eq!(extract_twe_source("let x = 1\n"), "let x = 1");
    }

    #[test]
    fn loop_passes_on_first_round_when_program_clean() {
        let mut p = FixtureProvider::new(["```twe\nlet x = 1\n```".into()]);
        let outcome = run_loop(&mut p, "Bind x to 1.", &LoopOptions::default()).unwrap();
        assert!(outcome.passed);
        assert_eq!(outcome.rounds.len(), 1);
        assert_eq!(outcome.final_source, "let x = 1");
        // The edit protocol rides in the system prompt.
        assert!(p.requests[0].system.contains("<<<<<<< SEARCH"));
    }

    #[test]
    fn loop_continues_the_conversation_until_verify_is_clean() {
        let mut p = FixtureProvider::new([
            "```twe\n# verified\nlet apple = 1\nlet y = aple\n```".into(),
            "<<<<<<< SEARCH\nlet y = aple\n=======\nlet y = apple\n>>>>>>> REPLACE".into(),
        ]);
        let outcome = run_loop(&mut p, "task", &LoopOptions::default()).unwrap();
        assert!(outcome.passed, "{:?}", outcome.rounds);
        assert_eq!(outcome.final_source, "# verified\nlet apple = 1\nlet y = apple");
        // Round 2 sees round 1's reply and the verify JSON.
        let second = &p.requests[1].messages;
        assert_eq!(second.len(), 3);
        assert!(second[1].text.contains("aple"));
        assert!(second[2].text.contains("\"version\":2"));
    }

    #[test]
    fn an_edit_that_does_not_apply_is_sent_back() {
        let mut p = FixtureProvider::new([
            "<<<<<<< SEARCH\nlet z = 3\n=======\nlet z = 4\n>>>>>>> REPLACE".into(),
            "<<<<<<< SEARCH\nlet x = 1\n=======\nlet x = 2\n>>>>>>> REPLACE".into(),
        ]);
        let options = LoopOptions { starter: "let x = 1".into(), ..Default::default() };
        let outcome = run_loop(&mut p, "task", &options).unwrap();
        assert!(outcome.passed);
        assert!(outcome.rounds[0].edit_error.as_deref().unwrap().contains("did not match"));
        assert!(p.requests[0].messages[0].text.contains("The current file:\n```twe\nlet x = 1\n```"));
        assert!(p.requests[1].messages[2].text.contains("could not be applied"));
        assert_eq!(outcome.final_source, "let x = 2");
    }

    #[test]
    fn loop_gives_up_after_max_rounds_and_sums_usage() {
        let reply = || Reply {
            text: "```twe\n# verified\nlet x = oops\n```".into(),
            stop: StopReason::EndTurn,
            usage: Usage { input: 10, output: 5, cache_read: 100, cache_write: 0 },
            model: "claude-sonnet-5-5".into(),
            cost_usd: Some(0.5),
        };
        let mut p = FixtureProvider::with_replies([reply(), reply()]);
        let options = LoopOptions { max_rounds: 2, ..Default::default() };
        let outcome = run_loop(&mut p, "task", &options).unwrap();
        assert!(!outcome.passed);
        assert_eq!(outcome.rounds.len(), 2);
        assert_eq!(outcome.usage.cache_read, 200);
        assert_eq!(outcome.cost_usd, Some(1.0));
    }

    #[test]
    fn provider_error_propagates() {
        let mut p = FixtureProvider::new([]);
        assert!(run_loop(&mut p, "task", &LoopOptions::default()).is_err());
    }
}
