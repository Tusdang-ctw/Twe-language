//! Phase 33 session 7: replay-based LLM evaluation harness.
//!
//! Standardised, automated benchmark for "did the LLM produce a
//! working game?" Each *suite* under `eval/` packages a prompt with
//! the configuration to grade a generated `.twe` source against:
//!
//! ```text
//! eval/
//!   <suite_name>/
//!     prompt.md          ← the LLM authoring task
//!     expected.txt       ← canonical stdout after running for N frames
//!     config.toml        ← frames + dt + scoring weights
//! ```
//!
//! `twec eval <suite> --source <file>` runs the file through
//! [`eval::run_with_frames`] and grades the result. Without
//! `--source`, the harness can call out to an LLM provider via the
//! Phase 33 session 4 [`llm_loop`](crate::llm_loop) — the loop and
//! the scorecard share types, so a single run produces both a
//! generated program and its grade.
//!
//! ## Why this is the right shape
//!
//! - **Reuses the deterministic execution path** the Phase 29 replay
//!   subsystem already proved out — same source, same output, every
//!   run. No flaky benchmarks.
//! - **Score is a JSON document**, not a number. Downstream tools
//!   (the future fine-tune's reward model, leaderboards) consume
//!   the structured score; humans skim `pass: true / false`.
//! - **Suites live as plain files** in `eval/`. Adding one is `mkdir
//!   eval/new_suite && touch prompt.md expected.txt config.toml` —
//!   no Rust changes, no rebuild, no test fixture wiring.

use std::path::{Path, PathBuf};

use crate::eval;
use crate::lexer;
use crate::parser;

// ---------------------------------------------------------------------------
// Suite — packaged authoring task + grading config.
// ---------------------------------------------------------------------------

/// One eval suite parsed from disk.
#[derive(Debug, Clone)]
pub struct Suite {
    pub name: String,
    pub root: PathBuf,
    pub prompt: String,
    pub expected: String,
    pub config: SuiteConfig,
}

/// Per-suite scoring + execution configuration.
///
/// Parsed from `config.toml` with sensible defaults if the file is
/// absent (lets a contributor add a suite by writing only `prompt.md`
/// and `expected.txt` and trust the defaults).
#[derive(Debug, Clone)]
pub struct SuiteConfig {
    /// Number of frames `eval::run_with_frames` advances. The LLM's
    /// program is graded on its captured stdout after this many
    /// `on update(dt)` ticks.
    pub frames: u32,
    /// Frame delta in seconds. Default 1/60.
    pub dt: f64,
    /// Match strategy applied to (expected, actual) stdout.
    pub match_mode: MatchMode,
}

impl Default for SuiteConfig {
    fn default() -> Self {
        Self {
            frames: 60,
            dt: 1.0 / 60.0,
            match_mode: MatchMode::Substring,
        }
    }
}

/// How to compare actual stdout against `expected.txt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// Expected text appears anywhere in actual stdout (whitespace
    /// trimmed). Tolerates extra logging and prologue / epilogue
    /// banners. Default — most LLM-authored programs add chatty
    /// prints the spec doesn't enumerate.
    Substring,
    /// Actual stdout equals expected exactly after trim. Use for
    /// tight regression suites where a stdout diff is the spec.
    Exact,
    /// Each non-blank expected line appears in actual (in order).
    /// Looser than Exact, tighter than Substring — picks up the
    /// "right shape" without exact whitespace fidelity.
    Lines,
}

impl MatchMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "substring" | "contains" => Some(MatchMode::Substring),
            "exact" => Some(MatchMode::Exact),
            "lines" => Some(MatchMode::Lines),
            _ => None,
        }
    }
}

/// Read a suite from `eval/<name>/`. Missing `expected.txt` is an
/// error (a suite with no expected output isn't a graded test);
/// missing `prompt.md` and `config.toml` use defaults.
pub fn load_suite(suite_dir: &Path) -> Result<Suite, String> {
    let name = suite_dir
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("eval suite path has no name: {}", suite_dir.display()))?;
    if !suite_dir.is_dir() {
        return Err(format!("eval suite directory missing: {}", suite_dir.display()));
    }
    let expected_path = suite_dir.join("expected.txt");
    let expected = std::fs::read_to_string(&expected_path).map_err(|e| {
        format!(
            "missing expected.txt for suite `{name}`: {} ({e})",
            expected_path.display()
        )
    })?;
    let prompt_path = suite_dir.join("prompt.md");
    let prompt = std::fs::read_to_string(&prompt_path).unwrap_or_default();
    let config_path = suite_dir.join("config.toml");
    let config = if config_path.exists() {
        let text = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("reading {}: {e}", config_path.display()))?;
        parse_config(&text)?
    } else {
        SuiteConfig::default()
    };
    Ok(Suite {
        name,
        root: suite_dir.to_path_buf(),
        prompt,
        expected,
        config,
    })
}

/// Tiny TOML parser specialised to the four keys we accept. Keeps
/// the no-extra-deps contract — `toml` crate is in Cargo.toml but
/// only for `twe.toml` parsing in `build.rs`, and pulling it in
/// here would tie the eval module to the build-pipeline crate.
fn parse_config(text: &str) -> Result<SuiteConfig, String> {
    let mut cfg = SuiteConfig::default();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        match key {
            "frames" => {
                cfg.frames = value
                    .parse()
                    .map_err(|e| format!("config `frames` not an int: {e}"))?;
            }
            "dt" => {
                cfg.dt = value
                    .parse()
                    .map_err(|e| format!("config `dt` not a float: {e}"))?;
            }
            "match_mode" | "match" => {
                cfg.match_mode = MatchMode::parse(value)
                    .ok_or_else(|| format!("config `match_mode`: unknown value `{value}`"))?;
            }
            _ => {
                // Unknown key — ignored, future-compatible.
            }
        }
    }
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// Result of grading one (suite, source) pair.
#[derive(Debug, Clone)]
pub struct Score {
    pub suite: String,
    pub passed: bool,
    pub stage: Stage,
    pub actual_output: String,
    pub message: Option<String>,
    pub source_lines: usize,
    pub source_bytes: usize,
}

/// Pipeline stage reached. A failure stops the pipeline at the first
/// red light; the score reports which stage tripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Lex,
    Parse,
    Run,
    Match,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Stage::Lex => "lex",
            Stage::Parse => "parse",
            Stage::Run => "run",
            Stage::Match => "match",
        }
    }
}

/// Grade `source` against `suite`. Single-shot, deterministic — no
/// LLM provider involved. The `twec eval` CLI calls this when the
/// caller passes `--source <file>`; the LLM-driven flow generates
/// source first via [`crate::llm_loop`] and then calls in here.
pub fn grade_source(suite: &Suite, source: &str) -> Score {
    let lines = source.lines().count();
    let bytes = source.len();
    let mut score = Score {
        suite: suite.name.clone(),
        passed: false,
        stage: Stage::Lex,
        actual_output: String::new(),
        message: None,
        source_lines: lines,
        source_bytes: bytes,
    };
    let tokens = match lexer::lex(source) {
        Ok(t) => t,
        Err(e) => {
            score.stage = Stage::Lex;
            score.message = Some(e.message);
            return score;
        }
    };
    let program = match parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            score.stage = Stage::Parse;
            score.message = Some(e.message);
            return score;
        }
    };
    let output = match eval::run_with_frames(&program, suite.config.frames, suite.config.dt) {
        Ok(s) => s,
        Err(e) => {
            score.stage = Stage::Run;
            score.message = Some(e.message);
            return score;
        }
    };
    score.actual_output = output.clone();
    score.stage = Stage::Match;
    score.passed = output_matches(&suite.expected, &output, suite.config.match_mode);
    if !score.passed {
        score.message = Some(format!(
            "stdout did not match (mode: {:?}). expected first 80 chars: {:?}; actual first 80 chars: {:?}",
            suite.config.match_mode,
            head(&suite.expected, 80),
            head(&output, 80),
        ));
    }
    score
}

fn output_matches(expected: &str, actual: &str, mode: MatchMode) -> bool {
    match mode {
        MatchMode::Substring => actual.contains(expected.trim()),
        MatchMode::Exact => actual.trim() == expected.trim(),
        MatchMode::Lines => {
            let mut act_iter = actual.lines();
            for needle in expected.lines() {
                let needle = needle.trim();
                if needle.is_empty() {
                    continue;
                }
                let mut found = false;
                for line in act_iter.by_ref() {
                    if line.contains(needle) {
                        found = true;
                        break;
                    }
                }
                if !found {
                    return false;
                }
            }
            true
        }
    }
}

fn head(s: &str, n: usize) -> &str {
    let mut end = 0;
    for (count, (i, _)) in s.char_indices().enumerate() {
        if count >= n {
            break;
        }
        end = i + s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
    }
    &s[..end]
}

// ---------------------------------------------------------------------------
// JSON scorecard
// ---------------------------------------------------------------------------

/// Render one or more scores as a single canonical JSON document.
/// Matches the no-serde pattern used by the rest of the Phase 33
/// surface (verify, grammar, stdlib, corpus).
pub fn scorecard_json(scores: &[Score]) -> String {
    let mut s = String::with_capacity(256 + scores.len() * 256);
    s.push('{');
    s.push_str("\"tool\":\"twec-eval\",\"version\":1");
    let total = scores.len();
    let passed = scores.iter().filter(|sc| sc.passed).count();
    s.push_str(",\"summary\":{\"total\":");
    s.push_str(&total.to_string());
    s.push_str(",\"passed\":");
    s.push_str(&passed.to_string());
    s.push_str(",\"failed\":");
    s.push_str(&(total - passed).to_string());
    s.push_str("},\"scores\":[");
    for (i, sc) in scores.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');
        s.push_str("\"suite\":");
        json_string(&mut s, &sc.suite);
        s.push_str(",\"passed\":");
        s.push_str(if sc.passed { "true" } else { "false" });
        s.push_str(",\"stage\":");
        json_string(&mut s, sc.stage.as_str());
        s.push_str(",\"source_lines\":");
        s.push_str(&sc.source_lines.to_string());
        s.push_str(",\"source_bytes\":");
        s.push_str(&sc.source_bytes.to_string());
        s.push_str(",\"actual_output\":");
        json_string(&mut s, &sc.actual_output);
        s.push_str(",\"message\":");
        match &sc.message {
            Some(m) => json_string(&mut s, m),
            None => s.push_str("null"),
        }
        s.push('}');
    }
    s.push_str("]}");
    s
}

fn json_string(out: &mut String, value: &str) {
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

    fn fixture_suite(expected: &str, frames: u32) -> Suite {
        Suite {
            name: "fixture".to_string(),
            root: PathBuf::new(),
            prompt: "ignored".into(),
            expected: expected.to_string(),
            config: SuiteConfig {
                frames,
                dt: 1.0 / 60.0,
                match_mode: MatchMode::Substring,
            },
        }
    }

    #[test]
    fn grade_source_passes_when_output_matches_substring() {
        let suite = fixture_suite("hello", 1);
        let score = grade_source(&suite, "print(\"hello world\")\n");
        assert!(score.passed);
        assert_eq!(score.stage, Stage::Match);
    }

    #[test]
    fn grade_source_reports_lex_failure_with_message() {
        let suite = fixture_suite("anything", 1);
        let score = grade_source(&suite, "let x = \"unclosed\n");
        assert!(!score.passed);
        assert_eq!(score.stage, Stage::Lex);
        assert!(score.message.is_some());
    }

    #[test]
    fn grade_source_reports_parse_failure() {
        let suite = fixture_suite("anything", 1);
        let score = grade_source(&suite, "let = 1\n");
        assert!(!score.passed);
        assert_eq!(score.stage, Stage::Parse);
    }

    #[test]
    fn grade_source_reports_run_failure() {
        let suite = fixture_suite("anything", 1);
        // Reference an unbound name — runtime error at lookup.
        let score = grade_source(&suite, "print(undefined_thing)\n");
        assert!(!score.passed);
        assert_eq!(score.stage, Stage::Run);
    }

    #[test]
    fn grade_source_advances_frames_for_on_update_emission() {
        // The program prints once per frame; with frames = 3 we expect
        // three lines in the output.
        let suite = fixture_suite("tick", 3);
        let score = grade_source(&suite, "on update(dt):\n    print(\"tick\")\n");
        assert!(score.passed, "got: {:?}", score);
        // Three "tick" lines (with their newlines) should appear.
        assert!(score.actual_output.matches("tick").count() >= 3);
    }

    #[test]
    fn match_mode_exact_rejects_extra_output() {
        let mut suite = fixture_suite("hello\n", 1);
        suite.config.match_mode = MatchMode::Exact;
        let score = grade_source(&suite, "print(\"hello\")\nprint(\"extra\")\n");
        assert!(!score.passed, "extra output should fail Exact match");
    }

    #[test]
    fn match_mode_lines_skips_blank_expected_lines() {
        let mut suite = fixture_suite("first\n\nsecond\n", 1);
        suite.config.match_mode = MatchMode::Lines;
        let score = grade_source(&suite, "print(\"first\")\nprint(\"second\")\n");
        assert!(score.passed);
    }

    #[test]
    fn parse_config_handles_known_keys_and_ignores_unknown() {
        let text = "frames = 30\n# comment\ndt = 0.05\nmatch_mode = \"exact\"\nfuture_key = 42\n";
        let cfg = parse_config(text).unwrap();
        assert_eq!(cfg.frames, 30);
        assert!((cfg.dt - 0.05).abs() < 1e-9);
        assert_eq!(cfg.match_mode, MatchMode::Exact);
    }

    #[test]
    fn scorecard_json_is_balanced_and_versioned() {
        let scores = vec![
            Score {
                suite: "a".into(),
                passed: true,
                stage: Stage::Match,
                actual_output: "hi".into(),
                message: None,
                source_lines: 1,
                source_bytes: 4,
            },
            Score {
                suite: "b".into(),
                passed: false,
                stage: Stage::Run,
                actual_output: String::new(),
                message: Some("boom".into()),
                source_lines: 5,
                source_bytes: 80,
            },
        ];
        let s = scorecard_json(&scores);
        assert!(s.contains("\"tool\":\"twec-eval\""));
        assert!(s.contains("\"version\":1"));
        assert!(s.contains("\"total\":2"));
        assert!(s.contains("\"passed\":1"));
        assert!(s.contains("\"failed\":1"));
        assert_eq!(s.matches('{').count(), s.matches('}').count());
        assert_eq!(s.matches('[').count(), s.matches(']').count());
    }
}
