//! Phase 13 session 7: verified-mode diagnostics for LLM authorship.
//!
//! Surface and contract:
//!
//! * `# verified` (or `#! verified`, shebang-friendly) on one of
//!   the first ten non-blank lines of a file activates Tier 3
//!   reporting. The file is checked in strict mode (with the
//!   session 5 / session 6 record + lax-narrowing rescues), and the
//!   resulting diagnostics are formatted as a single JSON
//!   document — the LLM-self-correction contract documented in
//!   `docs/02-type-system.md` §"Tier 3: Verified".
//!
//! * `verify_program(source) -> VerifyReport` runs the pipeline
//!   end-to-end on already-loaded text. Callers who need a
//!   filename in the report (the CLI session 8 will) supply one
//!   via `verify_program_with_path`.
//!
//! * `VerifyReport::to_json()` emits the canonical document. The
//!   JSON shape is versioned through `tool` + `version` fields so
//!   downstream tools can reject unfamiliar payloads cleanly.
//!
//! Verified mode does not introduce new type-system rules; it is
//! a *reporting* layer over the same strict-lax inferer. An LLM
//! sitting in a self-correction loop can read the JSON, edit the
//! file, and re-run `twec verify` — the rules are stable across
//! the loop because the underlying inferer is.

use crate::infer;
use crate::lexer;
use crate::parser;

/// Detect a `# verified` opt-in directive in `source`. Mirrors
/// `infer::detect_strict` — same comment-form, same first-ten-lines
/// rule, no shadowing of the identifier `verified`.
pub fn detect_verified(source: &str) -> bool {
    let needles = ["# verified", "#! verified", "#verified", "#!verified"];
    for line in source.lines().take(10) {
        let trimmed = line.trim();
        if needles.contains(&trimmed) {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyDiagnostic {
    pub kind: String,
    pub severity: Severity,
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub help: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// One run of the verifier. Includes the diagnostics, a summary
/// count, and the metadata an LLM needs to anchor edits (the file
/// path and which mode produced the report).
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub file: Option<String>,
    pub strict: bool,
    pub verified: bool,
    pub diagnostics: Vec<VerifyDiagnostic>,
}

impl VerifyReport {
    pub fn errors(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }

    pub fn ok(&self) -> bool {
        self.errors() == 0
    }

    /// Emit the canonical JSON document. Format is hand-rolled (no
    /// serde dep): keys are stable, strings are JSON-escaped, the
    /// shape is documented in `docs/02-type-system.md` §"Tier 3".
    /// Versioned via `tool` + `version` so a future v2 shape can
    /// coexist; downstream tools should reject unknown versions.
    pub fn to_json(&self) -> String {
        let mut s = String::with_capacity(256 + self.diagnostics.len() * 128);
        s.push('{');
        s.push_str("\"tool\":\"twec-verify\",\"version\":1");
        s.push_str(",\"file\":");
        match &self.file {
            Some(f) => write_str_value(&mut s, f),
            None => s.push_str("null"),
        }
        s.push_str(",\"strict\":");
        s.push_str(if self.strict { "true" } else { "false" });
        s.push_str(",\"verified\":");
        s.push_str(if self.verified { "true" } else { "false" });
        s.push_str(",\"summary\":{\"errors\":");
        s.push_str(&self.errors().to_string());
        s.push_str(",\"warnings\":");
        s.push_str(&self.warnings().to_string());
        s.push_str("},\"diagnostics\":[");
        for (i, d) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('{');
            s.push_str("\"kind\":");
            write_str_value(&mut s, &d.kind);
            s.push_str(",\"severity\":");
            write_str_value(&mut s, d.severity.as_str());
            s.push_str(",\"line\":");
            s.push_str(&d.line.to_string());
            s.push_str(",\"col\":");
            s.push_str(&d.col.to_string());
            s.push_str(",\"message\":");
            write_str_value(&mut s, &d.message);
            s.push_str(",\"help\":");
            match &d.help {
                Some(h) => write_str_value(&mut s, h),
                None => s.push_str("null"),
            }
            s.push('}');
        }
        s.push_str("]}");
        s
    }
}

/// Run lex + parse + strict-lax inference on `source`. Errors at
/// any stage become diagnostics in the report. The `file` field is
/// populated from the optional `path` argument; pass `None` for
/// stdin / playground / tests.
pub fn verify_program_with_path(source: &str, path: Option<&str>) -> VerifyReport {
    let strict = infer::detect_strict(source) || detect_verified(source);
    let verified = detect_verified(source);
    let file = path.map(|p| p.to_string());

    let tokens = match lexer::lex(source) {
        Ok(t) => t,
        Err(e) => {
            return VerifyReport {
                file,
                strict,
                verified,
                diagnostics: vec![VerifyDiagnostic {
                    kind: "lex-error".to_string(),
                    severity: Severity::Error,
                    line: e.line,
                    col: e.col,
                    message: e.message,
                    help: e.help,
                }],
            };
        }
    };
    let program = match parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            return VerifyReport {
                file,
                strict,
                verified,
                diagnostics: vec![VerifyDiagnostic {
                    kind: "parse-error".to_string(),
                    severity: Severity::Error,
                    line: e.line,
                    col: e.col,
                    message: e.message,
                    help: e.help,
                }],
            };
        }
    };
    let (_bindings, errors) = infer::infer_program_strict(&program, strict);
    let diagnostics = errors
        .into_iter()
        .map(|e| VerifyDiagnostic {
            kind: classify_kind(&e.message),
            severity: Severity::Error,
            line: e.line,
            col: e.col,
            message: e.message,
            help: e.help,
        })
        .collect();
    VerifyReport {
        file,
        strict,
        verified,
        diagnostics,
    }
}

/// Convenience wrapper for callers that only have source text.
pub fn verify_program(source: &str) -> VerifyReport {
    verify_program_with_path(source, None)
}

/// Map a strict-mode error message to a stable, machine-readable
/// kind tag. The tags are public surface — LLMs will rule on them
/// — so changing one is a Tier-3 contract bump.
fn classify_kind(message: &str) -> String {
    if message.starts_with("call argument") {
        "type-error.call-argument".to_string()
    } else if message.starts_with("comparison") {
        "type-error.comparison".to_string()
    } else if message.starts_with("return") {
        "type-error.return".to_string()
    } else if message.starts_with("let annotation") {
        "type-error.let-annotation".to_string()
    } else if message.starts_with("param annotation") {
        "type-error.param-annotation".to_string()
    } else if message.starts_with("return annotation") {
        "type-error.return-annotation".to_string()
    } else if message.starts_with("field annotation") {
        "type-error.field-annotation".to_string()
    } else if message.starts_with("unknown name") {
        "name-error.unknown".to_string()
    } else {
        "type-error".to_string()
    }
}

/// JSON-escape a string the way `to_json` needs. Hand-rolled to
/// avoid pulling serde just for this — verify is a hot enough
/// path that a small fixed encoder is the right call.
fn write_str_value(s: &mut String, value: &str) {
    s.push('"');
    for ch in value.chars() {
        match ch {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                s.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => s.push(ch),
        }
    }
    s.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_verified_directive() {
        // First-line + within-first-ten-lines forms both activate.
        assert!(detect_verified("# verified\nlet x = 1\n"));
        assert!(detect_verified("#! verified\nlet x = 1\n"));
        assert!(detect_verified("#verified\nlet x = 1\n"));
        // Identifier `verified` is NOT a directive — it's a let
        // binding's name.
        assert!(!detect_verified("let verified = 1\n"));
    }

    #[test]
    fn directive_outside_first_ten_lines_does_not_activate() {
        // A `# verified` buried below ten lines (e.g. in a help
        // string formatted as a comment) doesn't accidentally flip
        // the mode. Mirrors `detect_strict`'s safety bound.
        let mut src = String::new();
        for _ in 0..12 {
            src.push_str("let x = 1\n");
        }
        src.push_str("# verified\n");
        assert!(!detect_verified(&src));
    }

    #[test]
    fn clean_program_returns_empty_diagnostics_in_verified_mode() {
        let report = verify_program("# verified\nlet x: int = 42\n");
        assert!(report.verified);
        assert!(report.strict);
        assert!(report.ok());
        assert_eq!(report.diagnostics.len(), 0);
    }

    #[test]
    fn type_mismatch_lands_as_one_diagnostic() {
        let report = verify_program("# verified\nlet x: int = \"hi\"\n");
        assert!(!report.ok());
        assert_eq!(report.errors(), 1);
        assert_eq!(report.diagnostics[0].kind, "type-error.let-annotation");
        assert_eq!(report.diagnostics[0].severity, Severity::Error);
    }

    #[test]
    fn unknown_name_lands_with_name_error_kind() {
        let report = verify_program("# verified\nlet x = totally_unknown + 1\n");
        assert_eq!(report.diagnostics[0].kind, "name-error.unknown");
    }

    #[test]
    fn parse_error_lands_with_parse_error_kind() {
        let report = verify_program("# verified\nlet = 1\n");
        assert_eq!(report.errors(), 1);
        assert_eq!(report.diagnostics[0].kind, "parse-error");
    }

    #[test]
    fn json_output_is_valid_for_clean_program() {
        let report = verify_program("# verified\nlet x: int = 42\n");
        let json = report.to_json();
        assert!(json.contains("\"tool\":\"twec-verify\""));
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"verified\":true"));
        assert!(json.contains("\"strict\":true"));
        assert!(json.contains("\"summary\":{\"errors\":0,\"warnings\":0}"));
        assert!(json.contains("\"diagnostics\":[]"));
    }

    #[test]
    fn json_output_includes_diagnostic_fields() {
        let report = verify_program("# verified\nlet x: int = \"hi\"\n");
        let json = report.to_json();
        assert!(json.contains("\"kind\":\"type-error.let-annotation\""));
        assert!(json.contains("\"severity\":\"error\""));
        assert!(json.contains("\"line\":2"));
        assert!(json.contains("\"col\":"));
    }

    #[test]
    fn json_output_escapes_special_chars_in_messages() {
        // Force a message containing a quote by lex-erroring on a
        // weird input. The escape path is used by every diagnostic;
        // any source that produces backslashes / quotes / newlines
        // in the message must round-trip through JSON cleanly.
        let report = verify_program("\"unterminated\n");
        let json = report.to_json();
        // The escape sequences should not be raw backslashes /
        // newlines in the JSON output — that would invalidate it.
        assert!(!json.contains("\n\""), "message bled raw newline: {json}");
    }

    #[test]
    fn file_field_carries_through_when_supplied() {
        let report =
            verify_program_with_path("let x = 1\n", Some("path/to/file.twe"));
        assert_eq!(report.file.as_deref(), Some("path/to/file.twe"));
        let json = report.to_json();
        assert!(json.contains("\"file\":\"path/to/file.twe\""));
    }

    #[test]
    fn file_field_is_null_when_unsupplied() {
        let report = verify_program("let x = 1\n");
        assert!(report.file.is_none());
        let json = report.to_json();
        assert!(json.contains("\"file\":null"));
    }

    #[test]
    fn strict_directive_alone_activates_strict_but_not_verified() {
        let report = verify_program("# strict\nlet x: int = \"hi\"\n");
        assert!(report.strict);
        assert!(!report.verified);
        // Strict-only files still produce diagnostics — verified is
        // a strict superset, not an orthogonal mode.
        assert!(!report.ok());
    }
}
