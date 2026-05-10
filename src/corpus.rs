//! Phase 33 session 6: examples-as-corpus.
//!
//! Every example in `examples/` carries a structured comment header
//! that turns it into a labelled training / evaluation datum:
//!
//! ```twe
//! # @task: A vampire-survivors-style auto-attack with XP and level-up.
//! # @inputs: arrow keys / WASD
//! # @expected: enemies chase, XP gems collect on overlap, level-up modal at 5 XP
//! # @category: 2d
//! # @difficulty: large
//! ```
//!
//! `twec corpus --json` walks the directory, parses each header, and
//! emits the labeled set as a single JSON document. Three downstream
//! consumers already need this:
//!
//! 1. **Few-shot pool** for `twec llm-loop` prompts — pick the 3
//!    most similar `@task`s by semantic distance, paste the pairs
//!    into the model's context.
//! 2. **Eval seed** for `twec eval` (Phase 33 Tier 3): each labelled
//!    example becomes a target the LLM must reproduce.
//! 3. **Fine-tune training data**: `(task, source)` pairs, filtered
//!    by `@difficulty` and `@category`.
//!
//! Header parsing is line-based and lenient — missing fields land as
//! `None` rather than refusing the whole entry. The companion test
//! enforces that *shipped* examples have full headers (drift catch).

use std::path::{Path, PathBuf};

/// One labelled example. Fields mirror the `@key:` lines exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusEntry {
    /// Path relative to the scan root. Stable identifier across
    /// runs; suitable as an LLM-loop / eval suite key.
    pub path: PathBuf,
    pub task: Option<String>,
    pub inputs: Option<String>,
    pub expected: Option<String>,
    pub category: Option<String>,
    pub difficulty: Option<String>,
    /// Total source lines (after parsing the header). Cheap proxy
    /// for "size of program the model has to author."
    pub line_count: usize,
}

impl CorpusEntry {
    /// True if every required header field is populated.
    /// `tests/corpus.rs` asserts this is true for the shipped set.
    pub fn is_complete(&self) -> bool {
        self.task.is_some()
            && self.inputs.is_some()
            && self.expected.is_some()
            && self.category.is_some()
            && self.difficulty.is_some()
    }

    /// Names of header fields that are still missing. Used by the
    /// completeness test to produce actionable error messages.
    pub fn missing_fields(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.task.is_none() {
            out.push("@task");
        }
        if self.inputs.is_none() {
            out.push("@inputs");
        }
        if self.expected.is_none() {
            out.push("@expected");
        }
        if self.category.is_none() {
            out.push("@category");
        }
        if self.difficulty.is_none() {
            out.push("@difficulty");
        }
        out
    }
}

/// Parse the header out of a Twe source string. Tolerates blank
/// lines and `#`-only lines before the header. Stops scanning at
/// the first non-comment, non-blank line — headers must live in
/// the file's leading comment block.
pub fn parse_header(source: &str) -> CorpusEntry {
    let mut entry = CorpusEntry {
        path: PathBuf::new(),
        task: None,
        inputs: None,
        expected: None,
        category: None,
        difficulty: None,
        line_count: source.lines().count(),
    };
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Some(rest) = line.strip_prefix('#') else {
            // First non-comment line ends the header window.
            break;
        };
        let rest = rest.trim_start();
        let Some(after_at) = rest.strip_prefix('@') else {
            // Plain `# something` comment — keep scanning, the
            // user's prose can sit anywhere in the leading block.
            continue;
        };
        let Some((key, value)) = after_at.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "task" => entry.task = Some(value),
            "inputs" => entry.inputs = Some(value),
            "expected" => entry.expected = Some(value),
            "category" => entry.category = Some(value),
            "difficulty" => entry.difficulty = Some(value),
            _ => {} // Unknown @key — ignored, future-compatible.
        }
    }
    entry
}

/// Walk `root` recursively, returning one `CorpusEntry` per `.twe`
/// file found. Sorted by path for stable output.
pub fn scan_corpus(root: &Path) -> Vec<CorpusEntry> {
    let mut out = Vec::new();
    visit(root, root, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn visit(root: &Path, dir: &Path, out: &mut Vec<CorpusEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "twe") {
            if let Ok(source) = std::fs::read_to_string(&path) {
                let mut e = parse_header(&source);
                e.path = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_path_buf();
                out.push(e);
            }
        }
    }
}

/// Render a corpus list as JSON. Hand-rolled to match the no-serde
/// pattern. Versioned via tool + version.
pub fn to_json(entries: &[CorpusEntry]) -> String {
    let mut s = String::with_capacity(256 + entries.len() * 256);
    s.push('{');
    s.push_str("\"tool\":\"twec-corpus\",\"version\":1");
    s.push_str(",\"count\":");
    s.push_str(&entries.len().to_string());
    let complete = entries.iter().filter(|e| e.is_complete()).count();
    s.push_str(",\"complete\":");
    s.push_str(&complete.to_string());
    s.push_str(",\"entries\":[");
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');
        s.push_str("\"path\":");
        json_string(&mut s, &e.path.display().to_string().replace('\\', "/"));
        s.push_str(",\"task\":");
        json_optional(&mut s, e.task.as_deref());
        s.push_str(",\"inputs\":");
        json_optional(&mut s, e.inputs.as_deref());
        s.push_str(",\"expected\":");
        json_optional(&mut s, e.expected.as_deref());
        s.push_str(",\"category\":");
        json_optional(&mut s, e.category.as_deref());
        s.push_str(",\"difficulty\":");
        json_optional(&mut s, e.difficulty.as_deref());
        s.push_str(",\"line_count\":");
        s.push_str(&e.line_count.to_string());
        s.push_str(",\"complete\":");
        s.push_str(if e.is_complete() { "true" } else { "false" });
        s.push('}');
    }
    s.push_str("]}");
    s
}

fn json_optional(out: &mut String, v: Option<&str>) {
    match v {
        Some(s) => json_string(out, s),
        None => out.push_str("null"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_header() {
        let src = "# @task: orbit\n# @inputs: keyboard\n# @expected: square circles around point\n# @category: 2d\n# @difficulty: trivial\n\nlet x = 1\n";
        let e = parse_header(src);
        assert_eq!(e.task.as_deref(), Some("orbit"));
        assert_eq!(e.inputs.as_deref(), Some("keyboard"));
        assert_eq!(e.expected.as_deref(), Some("square circles around point"));
        assert_eq!(e.category.as_deref(), Some("2d"));
        assert_eq!(e.difficulty.as_deref(), Some("trivial"));
        assert!(e.is_complete());
    }

    #[test]
    fn missing_fields_reported() {
        let src = "# @task: orbit\n\nlet x = 1\n";
        let e = parse_header(src);
        assert!(!e.is_complete());
        assert_eq!(
            e.missing_fields(),
            vec!["@inputs", "@expected", "@category", "@difficulty"]
        );
    }

    #[test]
    fn header_stops_at_first_code_line() {
        let src = "# @task: short\nlet x = 1\n# @inputs: too_late\n";
        let e = parse_header(src);
        assert_eq!(e.task.as_deref(), Some("short"));
        // The @inputs line below code shouldn't be picked up.
        assert!(e.inputs.is_none());
    }

    #[test]
    fn unknown_keys_ignored_not_errored() {
        let src = "# @task: a\n# @future_field: b\nlet x = 1\n";
        let e = parse_header(src);
        assert_eq!(e.task.as_deref(), Some("a"));
    }

    #[test]
    fn json_output_is_balanced() {
        let entries = vec![CorpusEntry {
            path: PathBuf::from("foo.twe"),
            task: Some("t".into()),
            inputs: Some("i".into()),
            expected: Some("e".into()),
            category: Some("2d".into()),
            difficulty: Some("trivial".into()),
            line_count: 10,
        }];
        let s = to_json(&entries);
        assert_eq!(s.matches('{').count(), s.matches('}').count());
        assert_eq!(s.matches('[').count(), s.matches(']').count());
        assert!(s.contains("\"tool\":\"twec-corpus\""));
        assert!(s.contains("\"complete\":true"));
        assert!(s.contains("\"count\":1"));
    }
}
