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

use crate::ast::{Deprecation, Expr, Stmt};
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
    /// Phase 33 session 2: structured machine-applicable fix.
    /// When populated (high-confidence diagnostics like `did_you_mean`
    /// rename or annotation insertion), an LLM consuming the JSON v2
    /// output can apply the edits without re-parsing free-text help.
    /// `None` for diagnostics where no obvious single fix exists.
    pub fix: Option<Fix>,
}

/// Phase 33 session 2: a machine-applicable fix attached to a
/// diagnostic. One fix may apply multiple non-overlapping edits to
/// the same source file. `rationale` is a short human-readable
/// explanation — useful when the consumer wants to surface the fix
/// to a developer before applying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub edits: Vec<Edit>,
    pub rationale: String,
}

/// Phase 33 session 2: one byte-anchored replacement on a source
/// file. Coordinates are 1-based line + column (matching the rest
/// of the diagnostic surface). `len` is the byte length of the
/// span the edit replaces; an insertion uses `len: 0`. `replace` is
/// the new text — may contain newlines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub line: u32,
    pub col: u32,
    pub len: u32,
    pub replace: String,
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
    /// Versioned via `tool` + `version` so consumers can negotiate
    /// cleanly.
    ///
    /// **Schema v2 (Phase 33 session 2)** adds the `fix` field on
    /// each diagnostic. v1 consumers continue to work because every
    /// v1 field is preserved; v2-aware consumers pick up the new
    /// machine-applicable patches. Bump to v3 only when removing or
    /// reshaping an existing field.
    pub fn to_json(&self) -> String {
        let mut s = String::with_capacity(256 + self.diagnostics.len() * 192);
        s.push('{');
        s.push_str("\"tool\":\"twec-verify\",\"version\":2");
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
            s.push_str(",\"fix\":");
            match &d.fix {
                Some(f) => write_fix(&mut s, f),
                None => s.push_str("null"),
            }
            s.push('}');
        }
        s.push_str("]}");
        s
    }
}

/// Phase 33 session 2: serialize a `Fix` as JSON. Stable key order
/// (`rationale`, `edits`) so byte-for-byte snapshot tests don't
/// drift on hash-map iteration.
fn write_fix(s: &mut String, fix: &Fix) {
    s.push('{');
    s.push_str("\"rationale\":");
    write_str_value(s, &fix.rationale);
    s.push_str(",\"edits\":[");
    for (i, e) in fix.edits.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('{');
        s.push_str("\"line\":");
        s.push_str(&e.line.to_string());
        s.push_str(",\"col\":");
        s.push_str(&e.col.to_string());
        s.push_str(",\"len\":");
        s.push_str(&e.len.to_string());
        s.push_str(",\"replace\":");
        write_str_value(s, &e.replace);
        s.push('}');
    }
    s.push_str("]}");
}

/// Phase 13 session 10: caller-tunable verify options. `warn_deprecated`
/// scans the program for use sites of `@deprecated` symbols and
/// emits a `deprecation` warning per reference.
#[derive(Clone, Debug, Default)]
pub struct VerifyOptions {
    pub warn_deprecated: bool,
}

/// Run lex + parse + strict-lax inference on `source`. Errors at
/// any stage become diagnostics in the report. The `file` field is
/// populated from the optional `path` argument; pass `None` for
/// stdin / playground / tests.
pub fn verify_program_with_path(source: &str, path: Option<&str>) -> VerifyReport {
    verify_program_with_options(source, path, &VerifyOptions::default())
}

/// Variant of `verify_program_with_path` that consumes a
/// `VerifyOptions`. Phase 13 session 10.
pub fn verify_program_with_options(
    source: &str,
    path: Option<&str>,
    options: &VerifyOptions,
) -> VerifyReport {
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
                    fix: None,
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
                    fix: None,
                }],
            };
        }
    };
    let (_bindings, errors) = infer::infer_program_strict(&program, strict);
    let mut diagnostics: Vec<VerifyDiagnostic> = errors
        .into_iter()
        .map(|e| {
            let kind = classify_kind(&e.message);
            // Phase 33 session 2: synthesize a structured fix for
            // high-confidence diagnostic kinds. Currently:
            // - `did-you-mean` rename (replace bad ident with the
            //   suggested name at the diagnostic's line/col).
            //
            // Conservative by default: only emit a fix when the
            // suggestion is parseable from `help` and the bad
            // identifier is recoverable from `message` so we know
            // the byte-length to replace. Other kinds get `fix: None`
            // and rely on `help` until a follow-on session adds them.
            let fix = synthesize_fix(&kind, &e.message, e.help.as_deref(), e.line, e.col);
            VerifyDiagnostic {
                kind,
                severity: Severity::Error,
                line: e.line,
                col: e.col,
                message: e.message,
                help: e.help,
                fix,
            }
        })
        .collect();
    if options.warn_deprecated {
        let mut deprecation_warnings = collect_deprecated_uses(&program);
        diagnostics.append(&mut deprecation_warnings);
    }
    diagnostics.sort_by_key(|d| (d.line, d.col));
    VerifyReport {
        file,
        strict,
        verified,
        diagnostics,
    }
}

/// Phase 13 session 10: walk `program`, collect every top-level
/// `@deprecated` declaration's name + annotation info, then walk
/// again to find use sites of those names. Each use site becomes
/// a `Severity::Warning` diagnostic with a `deprecation` kind tag.
///
/// Use-site detection is *bare-name* matching today: an `Expr::Ident`
/// whose name is in the deprecated set produces a warning. Field
/// accesses on a deprecated module / class don't propagate — that's
/// a session-12-or-later refinement.
fn collect_deprecated_uses(program: &crate::ast::Program) -> Vec<VerifyDiagnostic> {
    let deprecated = collect_deprecated_decls(&program.stmts);
    if deprecated.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    walk_for_uses(&program.stmts, &deprecated, &mut out);
    out
}

fn collect_deprecated_decls(stmts: &[Stmt]) -> std::collections::HashMap<String, Deprecation> {
    let mut m = std::collections::HashMap::new();
    for stmt in stmts {
        match stmt {
            Stmt::FunctionDecl {
                name,
                deprecation: Some(d),
                ..
            }
            | Stmt::Decl {
                name,
                deprecation: Some(d),
                ..
            } => {
                m.insert(name.clone(), d.clone());
            }
            _ => {}
        }
    }
    m
}

fn walk_for_uses(
    stmts: &[Stmt],
    deprecated: &std::collections::HashMap<String, Deprecation>,
    out: &mut Vec<VerifyDiagnostic>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } => walk_expr(value, deprecated, out),
            Stmt::Assign { value, .. } => walk_expr(value, deprecated, out),
            Stmt::If {
                cond,
                then_body,
                elifs,
                else_body,
                ..
            } => {
                walk_expr(cond, deprecated, out);
                walk_for_uses(then_body, deprecated, out);
                for (c, body) in elifs {
                    walk_expr(c, deprecated, out);
                    walk_for_uses(body, deprecated, out);
                }
                if let Some(eb) = else_body {
                    walk_for_uses(eb, deprecated, out);
                }
            }
            Stmt::While { cond, body, .. } => {
                walk_expr(cond, deprecated, out);
                walk_for_uses(body, deprecated, out);
            }
            Stmt::For { iter, body, .. } => {
                walk_expr(iter, deprecated, out);
                walk_for_uses(body, deprecated, out);
            }
            Stmt::Return { value: Some(v), .. } => walk_expr(v, deprecated, out),
            Stmt::Expr(e) => walk_expr(e, deprecated, out),
            Stmt::FunctionDecl { body, .. } => {
                walk_for_uses(body, deprecated, out);
            }
            // `Spawn`, `Despawn`, `Wait`, `OnUpdate`, `OnRender`,
            // `Decl`, etc. carry expressions / blocks too. v0.7
            // session 10 ships the most-trafficked subset; the
            // long tail rides a follow-on if a real LLM-authored
            // codebase pressures it.
            _ => {}
        }
    }
}

fn walk_expr(
    expr: &Expr,
    deprecated: &std::collections::HashMap<String, Deprecation>,
    out: &mut Vec<VerifyDiagnostic>,
) {
    match expr {
        Expr::Ident { name, line, col } => {
            if let Some(dep) = deprecated.get(name) {
                let since = dep
                    .since
                    .as_deref()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default();
                out.push(VerifyDiagnostic {
                    kind: "deprecation".to_string(),
                    severity: Severity::Warning,
                    line: *line,
                    col: *col,
                    message: format!("`{name}` is deprecated{since}"),
                    help: Some(
                        "deprecated symbols still work in v0.7 but are scheduled for removal in v1.0; consult CHANGELOG for the replacement"
                            .to_string(),
                    ),
                    fix: None,
                });
            }
        }
        Expr::Call {
            callee,
            args,
            kwargs,
            ..
        } => {
            walk_expr(callee, deprecated, out);
            for a in args {
                walk_expr(a, deprecated, out);
            }
            for (_, e) in kwargs {
                walk_expr(e, deprecated, out);
            }
        }
        Expr::Field { object, .. } => walk_expr(object, deprecated, out),
        Expr::Index { object, index, .. } => {
            walk_expr(object, deprecated, out);
            walk_expr(index, deprecated, out);
        }
        Expr::Binary { left, right, .. } => {
            walk_expr(left, deprecated, out);
            walk_expr(right, deprecated, out);
        }
        Expr::Unary { operand, .. } => walk_expr(operand, deprecated, out),
        Expr::Tuple { elems, .. } => {
            for e in elems {
                walk_expr(e, deprecated, out);
            }
        }
        Expr::List { elems, .. } => {
            for e in elems {
                walk_expr(e, deprecated, out);
            }
        }
        Expr::Range { start, end, .. } => {
            walk_expr(start, deprecated, out);
            walk_expr(end, deprecated, out);
        }
        Expr::Interp { exprs, .. } => {
            // Interp's exprs are stored as raw source strings,
            // not parsed Expr nodes — they're evaluated lazily.
            // A deprecated name inside `"hi {old_thing}"` would
            // need re-parsing per chunk, which is more work than
            // session 10 should swallow. The follow-on lands
            // when interp authors press it.
            let _ = exprs;
        }
        Expr::IfExpr {
            cond,
            then_expr,
            elifs,
            else_expr,
            ..
        } => {
            walk_expr(cond, deprecated, out);
            walk_expr(then_expr, deprecated, out);
            for (c, body) in elifs {
                walk_expr(c, deprecated, out);
                walk_expr(body, deprecated, out);
            }
            walk_expr(else_expr, deprecated, out);
        }
        // Leaves that can't reference an identifier.
        Expr::Str { .. }
        | Expr::Int { .. }
        | Expr::Float { .. }
        | Expr::Percent { .. }
        | Expr::Quantity { .. }
        | Expr::Bool { .. }
        | Expr::SelfRef { .. } => {}
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

/// Phase 33 session 2: derive a structured `Fix` from a high-confidence
/// diagnostic, when one is available. Returns `None` for diagnostics
/// where no obvious single edit applies.
///
/// The strategy is conservative — only emit a fix when both the
/// original text and the replacement text are recoverable from the
/// `(message, help)` pair *without parsing source*. The only kind
/// satisfying that today is `name-error.unknown` with a `did_you_mean`
/// suggestion: the `message` carries the bad name in backticks
/// (`unknown name \`{name}\``), and the `help` carries the suggestion
/// in the same form (`did you mean \`{suggestion}\`?`).
///
/// Future kinds (literal-replaceable type mismatches, missing
/// `return`, missing annotation insertion) ride follow-on sessions
/// — each requires a dedicated synthesizer because the original
/// span and the replacement aren't recoverable from text alone.
fn synthesize_fix(
    kind: &str,
    message: &str,
    help: Option<&str>,
    line: u32,
    col: u32,
) -> Option<Fix> {
    if kind != "name-error.unknown" {
        return None;
    }
    let bad = extract_backticked(message)?;
    let suggestion = extract_did_you_mean(help?)?;
    if bad == suggestion {
        return None;
    }
    let len = bad.len() as u32;
    Some(Fix {
        rationale: format!("rename `{bad}` to `{suggestion}` (suggested by did_you_mean)"),
        edits: vec![Edit {
            line,
            col,
            len,
            replace: suggestion,
        }],
    })
}

/// Pull the first backticked token out of a string. Used to recover
/// the bad identifier from messages like ``unknown name `foo` ``.
fn extract_backticked(s: &str) -> Option<String> {
    let start = s.find('`')? + 1;
    let rest = &s[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

/// Pull the suggestion out of help text in the canonical
/// ``did you mean `bar`?`` shape. Returns `None` if `help` doesn't
/// match — defensive against future help re-wordings.
fn extract_did_you_mean(help: &str) -> Option<String> {
    let prefix = "did you mean `";
    let start = help.find(prefix)? + prefix.len();
    let rest = &help[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
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
        // Phase 33 session 2: schema bumped to v2 (adds machine-applicable
        // `fix` field on each diagnostic; v1 fields preserved).
        assert!(json.contains("\"version\":2"));
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
        let report = verify_program_with_path("let x = 1\n", Some("path/to/file.twe"));
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

    // ----- Phase 13 session 10: --warn-deprecated. -----

    #[test]
    fn warn_deprecated_off_by_default() {
        // Without the option, a deprecated-symbol use produces no
        // warning. Strict-mode errors still surface; deprecation is
        // its own opt-in surface so dev-loop runs aren't loud.
        let src = "@deprecated(\"since v0.7\")\nfunction old(): return 1\nlet x = old()\n";
        let report = verify_program(src);
        assert!(report.ok());
        assert_eq!(report.warnings(), 0);
    }

    #[test]
    fn warn_deprecated_emits_one_warning_per_use_site() {
        let src = concat!(
            "@deprecated(\"since v0.7\")\n",
            "function old(): return 1\n",
            "let x = old()\n",
            "let y = old() + old()\n",
        );
        let opts = VerifyOptions {
            warn_deprecated: true,
        };
        let report = verify_program_with_options(src, None, &opts);
        // Three uses of `old`: one in let x, two in `old() + old()`.
        assert_eq!(
            report.warnings(),
            3,
            "expected 3 warnings, got {:?}",
            report.diagnostics
        );
        // Errors stay zero — clean program apart from deprecation.
        assert_eq!(report.errors(), 0);
        for d in &report.diagnostics {
            assert_eq!(d.kind, "deprecation");
            assert_eq!(d.severity, Severity::Warning);
            assert!(d.message.contains("`old` is deprecated"));
            assert!(d.message.contains("since v0.7"));
        }
    }

    #[test]
    fn warn_deprecated_reports_zero_when_no_uses() {
        let src = concat!(
            "@deprecated(\"since v0.7\")\n",
            "function old(): return 1\n",
            "let x = 1\n",
        );
        let opts = VerifyOptions {
            warn_deprecated: true,
        };
        let report = verify_program_with_options(src, None, &opts);
        assert_eq!(report.warnings(), 0);
    }

    #[test]
    fn warn_deprecated_handles_bare_annotation_without_since() {
        // No `since` argument → message omits the version footnote.
        let src = concat!(
            "@deprecated\n",
            "function old(): return 1\n",
            "let x = old()\n",
        );
        let opts = VerifyOptions {
            warn_deprecated: true,
        };
        let report = verify_program_with_options(src, None, &opts);
        assert_eq!(report.warnings(), 1);
        let msg = &report.diagnostics[0].message;
        assert!(msg.contains("`old` is deprecated"));
        assert!(
            !msg.contains('('),
            "bare @deprecated should not parenthesise an empty since: {msg}"
        );
    }

    #[test]
    fn warn_deprecated_diagnostics_sorted_by_position() {
        // Multiple use sites should report in source order so an
        // LLM consumer can apply edits left-to-right without
        // tracking offset shifts.
        let src = concat!(
            "@deprecated(\"since v0.7\")\n",
            "function a(): return 1\n",
            "@deprecated(\"since v0.7\")\n",
            "function b(): return 2\n",
            "let x = b()\n",
            "let y = a()\n",
        );
        let opts = VerifyOptions {
            warn_deprecated: true,
        };
        let report = verify_program_with_options(src, None, &opts);
        assert_eq!(report.warnings(), 2);
        assert!(report.diagnostics[0].line < report.diagnostics[1].line);
        assert!(report.diagnostics[0].message.contains("`b`"));
        assert!(report.diagnostics[1].message.contains("`a`"));
    }
}
