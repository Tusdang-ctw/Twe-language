//! Phase 33 session 8: error → fix corpus generator.
//!
//! Takes every `.twe` file under `--root` (default `tests/programs/`,
//! known-good by construction), applies a small set of mutation
//! rules that produce errors LLMs typically make, runs verify on
//! the broken sibling, and writes a `(original, mutated, verify_json,
//! fix_json)` triple to a JSONL file.
//!
//! ## Why this is a corpus, not a benchmark
//!
//! The eval harness (session 7) measures whether a model produces
//! a working program. The mutator measures whether a model can
//! *correct* a broken program — the more important loop in real
//! authoring, where the model spends most of its time fixing mistakes
//! it just made.
//!
//! Each output triple is a labelled training datum: `(broken_source,
//! verify_diagnostics) → fix_source`. The original is the supervised
//! target; the verify JSON v2 (Phase 33 session 2) is the input
//! signal the model learns to consume; the mutated source is the
//! premise. Auto-generated, zero human labelling.
//!
//! ## Rule set v1
//!
//! Two mutations land in this commit. They cover the highest-frequency
//! LLM authoring failure modes the verify harness has structured
//! fixes for, so the round-trip closes:
//!
//! - **`identifier_typo`** — pick a `let`-bound name from the file,
//!   inject a one-character typo at one of its use sites. Triggers
//!   the `name-error.unknown` diagnostic with a `did_you_mean`
//!   suggestion that the verify v2 `fix` field promotes to a
//!   structured rename. The triple round-trips perfectly.
//! - **`literal_type_mismatch`** — find an `int` literal on the
//!   right-hand side of an annotated `let` (`let x: int = 42`) and
//!   replace it with a string literal of the same value. Triggers
//!   `type-error.let-annotation`. (No structured fix yet — verify
//!   only emits a `help`. The triple still ships; the model learns
//!   the *kind* of error and the human-readable rationale.)
//!
//! Adding a rule: implement the [`MutationRule`] trait, register it
//! in [`RuleSet::all`]. Output schema doesn't change.

use std::path::{Path, PathBuf};

use crate::verify::{verify_program_with_path, VerifyReport};

// ---------------------------------------------------------------------------
// Rule trait + registry
// ---------------------------------------------------------------------------

/// One mutation rule. Given a source string, produces zero or more
/// `(name, mutated_source)` candidates. Each candidate becomes one
/// triple in the output corpus.
pub trait MutationRule {
    fn name(&self) -> &'static str;
    fn apply(&self, source: &str) -> Vec<MutationCandidate>;
}

/// One mutated variant of a source. `kind` is the rule that produced
/// it (carried into the trace); `mutated` is the broken source the
/// LLM is supposed to fix back to `original`.
#[derive(Debug, Clone)]
pub struct MutationCandidate {
    pub kind: &'static str,
    pub mutated: String,
    /// Free-text description for the trace. Useful when the model
    /// is being shown a triple as a few-shot example.
    pub note: String,
}

/// Filter + ordering for which rules to run. v1 has just two values:
/// `all` (default) and `identifier-typo` (single-rule for tighter
/// corpora). Future named rule sets land here.
#[derive(Clone, Copy, Debug)]
pub enum RuleSet {
    All,
    IdentifierTypoOnly,
    LiteralOnly,
}

impl RuleSet {
    pub fn parse(s: &str) -> Self {
        match s {
            "identifier-typo" | "typo" => RuleSet::IdentifierTypoOnly,
            "literal" | "literal-type" => RuleSet::LiteralOnly,
            _ => RuleSet::All,
        }
    }
    fn rules(self) -> Vec<Box<dyn MutationRule>> {
        match self {
            RuleSet::All => vec![Box::new(IdentifierTypoRule), Box::new(LiteralTypeRule)],
            RuleSet::IdentifierTypoOnly => vec![Box::new(IdentifierTypoRule)],
            RuleSet::LiteralOnly => vec![Box::new(LiteralTypeRule)],
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: identifier typo on a let-bound name
// ---------------------------------------------------------------------------

/// Find `let <name> = ...` declarations, then for each, find one use
/// site of `<name>` *after* the declaration, and inject a one-char
/// typo there. Skips identifiers shorter than 4 chars (typos on `n`
/// aren't recoverable by `did_you_mean`'s short-name distance limit).
///
/// The mutated source is prepended with `# strict` if the original
/// isn't already in strict / verified mode. Without strict on, the
/// inferer drops to Type::Unknown on unknown names without error,
/// so the resulting verify report would be empty and the triple
/// would teach the model nothing.
struct IdentifierTypoRule;

impl MutationRule for IdentifierTypoRule {
    fn name(&self) -> &'static str {
        "identifier_typo"
    }
    fn apply(&self, source: &str) -> Vec<MutationCandidate> {
        let mut out = Vec::new();
        let names = collect_let_names(source);
        let needs_strict_prefix = !crate::infer::detect_strict(source)
            && !crate::verify::detect_verified(source);
        for name in names {
            // Need at least 4 chars so did_you_mean's short-name
            // distance limit (1) accepts our 1-char typo as a
            // candidate suggestion.
            if name.chars().count() < 4 {
                continue;
            }
            // Find the first use site (after the declaring `let`).
            let Some(decl_idx) = find_let_decl(source, &name) else {
                continue;
            };
            let after_decl = &source[decl_idx..];
            let Some(use_offset) = find_use_after_decl(after_decl, &name) else {
                continue;
            };
            let absolute = decl_idx + use_offset;
            // Drop the second character. Yields a length-1 edit
            // distance from the original — what did_you_mean catches.
            let typo = make_typo(&name);
            if typo == name {
                continue;
            }
            let mut mutated = String::with_capacity(source.len() + 16);
            if needs_strict_prefix {
                mutated.push_str("# strict\n");
            }
            mutated.push_str(&source[..absolute]);
            mutated.push_str(&typo);
            mutated.push_str(&source[absolute + name.len()..]);
            out.push(MutationCandidate {
                kind: "identifier_typo",
                mutated,
                note: format!(
                    "renamed `{name}` to `{typo}` at one use site{}",
                    if needs_strict_prefix {
                        " (and prepended `# strict` so the unknown-name diagnostic fires)"
                    } else {
                        ""
                    }
                ),
            });
        }
        out
    }
}

/// Collect identifiers introduced by `let NAME = ...` at any
/// indentation. Lightweight regex-free scan — sufficient for the
/// `tests/programs/` corpus, which uses simple straight-line
/// declarations.
fn collect_let_names(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("let ").or_else(|| trimmed.strip_prefix("var ")) else {
            continue;
        };
        let rest = rest.trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

/// Byte offset of the `let NAME` declaring line, or `None` if not
/// found at the start of any line.
fn find_let_decl(source: &str, name: &str) -> Option<usize> {
    let needle1 = format!("let {name}");
    let needle2 = format!("var {name}");
    let mut start = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if (trimmed.starts_with(&needle1) || trimmed.starts_with(&needle2))
            && (trimmed.len() == needle1.len()
                || trimmed.len() == needle2.len()
                || matches!(
                    trimmed.as_bytes().get(needle1.len()),
                    Some(b' ') | Some(b'=') | Some(b':') | Some(b'\n')
                ))
        {
            return Some(start + line.len());
        }
        start += line.len();
    }
    None
}

/// Byte offset of the first `name`-as-identifier match in `slice`
/// (relative to `slice`). Skips occurrences inside string literals
/// and comments — the cheap way: scan and respect `#` (to end of
/// line) and `"` / `'` quote pairs.
fn find_use_after_decl(slice: &str, name: &str) -> Option<usize> {
    let bytes = slice.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    let mut in_comment = false;
    let mut quote: u8 = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_comment {
            if b == b'\n' {
                in_comment = false;
            }
            i += 1;
            continue;
        }
        if in_string {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == quote {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if b == b'#' {
            in_comment = true;
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' {
            in_string = true;
            quote = b;
            i += 1;
            continue;
        }
        // Boundary check: identifier matches are word-boundary aware.
        if matches_word_at(bytes, i, name) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn matches_word_at(bytes: &[u8], i: usize, name: &str) -> bool {
    let n = name.len();
    if i + n > bytes.len() {
        return false;
    }
    // Preceding byte (if any) must not be an ident continuation char.
    if i > 0 {
        let prev = bytes[i - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            return false;
        }
    }
    if &bytes[i..i + n] != name.as_bytes() {
        return false;
    }
    if i + n < bytes.len() {
        let next = bytes[i + n];
        if next.is_ascii_alphanumeric() || next == b'_' {
            return false;
        }
    }
    true
}

/// Drop the second character of `name`. `apple` → `aple`. Leaves
/// 4-char names recoverable by `did_you_mean`'s distance-1 cutoff.
fn make_typo(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() < 3 {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len() - 1);
    out.push(chars[0]);
    for c in &chars[2..] {
        out.push(*c);
    }
    out
}

// ---------------------------------------------------------------------------
// Rule: literal type mismatch
// ---------------------------------------------------------------------------

/// Find `let NAME: int = N` declarations and replace the integer
/// literal with a string literal of the same value (`"N"`). This
/// trips strict mode's let-annotation type check.
struct LiteralTypeRule;

impl MutationRule for LiteralTypeRule {
    fn name(&self) -> &'static str {
        "literal_type_mismatch"
    }
    fn apply(&self, source: &str) -> Vec<MutationCandidate> {
        let mut out = Vec::new();
        // Only fires on files that already opt into strict / verified
        // mode. Otherwise the strict checker doesn't run, no error
        // is produced, and the triple has nothing to teach the model.
        if !crate::infer::detect_strict(source) && !crate::verify::detect_verified(source) {
            return out;
        }
        // Walk lines looking for `let NAME: int = N`.
        for (line_idx, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("let ") {
                continue;
            }
            // Crude split — only annotated `: int = N` form fires.
            let Some(colon_pos) = trimmed.find(':') else {
                continue;
            };
            let after_colon = trimmed[colon_pos + 1..].trim_start();
            if !after_colon.starts_with("int") {
                continue;
            }
            let Some(eq_pos) = after_colon.find('=') else {
                continue;
            };
            let rhs = after_colon[eq_pos + 1..].trim();
            // Accept a bare positive integer literal on the rhs.
            let lit: String = rhs
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if lit.is_empty() {
                continue;
            }
            // Build the mutated source: replace the integer literal
            // text with `"<lit>"` on this line.
            let new_line = line.replacen(&lit, &format!("\"{lit}\""), 1);
            let mut mutated = String::with_capacity(source.len() + 2);
            for (i, l) in source.lines().enumerate() {
                if i > 0 {
                    mutated.push('\n');
                }
                if i == line_idx {
                    mutated.push_str(&new_line);
                } else {
                    mutated.push_str(l);
                }
            }
            // Preserve trailing newline if the source had one.
            if source.ends_with('\n') {
                mutated.push('\n');
            }
            out.push(MutationCandidate {
                kind: "literal_type_mismatch",
                mutated,
                note: format!(
                    "replaced int literal `{lit}` with string literal `\"{lit}\"` on line {}",
                    line_idx + 1
                ),
            });
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Summary of a `run` invocation.
#[derive(Debug, Clone)]
pub struct MutationReport {
    pub source_files: usize,
    pub triples_emitted: usize,
    pub rules_applied: Vec<&'static str>,
    pub output_path: PathBuf,
}

impl MutationReport {
    pub fn summary(&self) -> String {
        format!(
            "[twec mutate] scanned {} file(s), emitted {} triple(s) using rule(s): {} → {}",
            self.source_files,
            self.triples_emitted,
            self.rules_applied.join(", "),
            self.output_path.display()
        )
    }
}

/// Walk `root` (recursively), apply each enabled rule, write all
/// resulting triples as JSONL to `out_dir/error_fix.jsonl`.
pub fn run(root: &Path, out_dir: &Path, rule_set: RuleSet) -> MutationReport {
    let _ = std::fs::create_dir_all(out_dir);
    let out_path = out_dir.join("error_fix.jsonl");
    let mut writer = match std::fs::File::create(&out_path) {
        Ok(f) => std::io::BufWriter::new(f),
        Err(e) => {
            eprintln!(
                "[twec mutate] cannot create `{}`: {e}",
                out_path.display()
            );
            return MutationReport {
                source_files: 0,
                triples_emitted: 0,
                rules_applied: rule_set.rules().iter().map(|r| r.name()).collect(),
                output_path: out_path,
            };
        }
    };
    let rules = rule_set.rules();
    let rule_names: Vec<&'static str> = rules.iter().map(|r| r.name()).collect();
    let mut files = Vec::new();
    visit_files(root, &mut files);
    let mut triples = 0usize;
    for path in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for rule in &rules {
            for cand in rule.apply(&source) {
                let report = verify_program_with_path(
                    &cand.mutated,
                    Some(&path.display().to_string()),
                );
                if report.ok() {
                    // Mutation didn't actually break the program —
                    // skip; the corpus only carries (broken, fixed)
                    // pairs the verifier flags.
                    continue;
                }
                use std::io::Write;
                let line = build_triple_jsonl(&path.display().to_string(), &source, &cand, &report);
                if writer.write_all(line.as_bytes()).is_ok()
                    && writer.write_all(b"\n").is_ok()
                {
                    triples += 1;
                }
            }
        }
    }
    use std::io::Write;
    let _ = writer.flush();
    MutationReport {
        source_files: files.len(),
        triples_emitted: triples,
        rules_applied: rule_names,
        output_path: out_path,
    }
}

fn visit_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            visit_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "twe") {
            out.push(p);
        }
    }
}

fn build_triple_jsonl(
    path: &str,
    original: &str,
    cand: &MutationCandidate,
    report: &VerifyReport,
) -> String {
    let mut s = String::with_capacity(256 + original.len() + cand.mutated.len() * 2);
    s.push('{');
    s.push_str("\"tool\":\"twec-mutate\",\"version\":1");
    s.push_str(",\"source_path\":");
    json_string(&mut s, path);
    s.push_str(",\"rule\":");
    json_string(&mut s, cand.kind);
    s.push_str(",\"note\":");
    json_string(&mut s, &cand.note);
    s.push_str(",\"original\":");
    json_string(&mut s, original);
    s.push_str(",\"mutated\":");
    json_string(&mut s, &cand.mutated);
    s.push_str(",\"verify\":");
    s.push_str(&report.to_json());
    s.push('}');
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

    #[test]
    fn typo_rule_renames_use_site_only() {
        let src = "let apple = 1\nlet other = apple + 2\n";
        let cands = IdentifierTypoRule.apply(src);
        assert_eq!(cands.len(), 1);
        let mutated = &cands[0].mutated;
        // The declaring `let apple = 1` must remain intact;
        // only the use site is changed.
        assert!(mutated.contains("let apple = 1"));
        // The use site `apple + 2` must be the typo'd form.
        assert!(mutated.contains("aple + 2"));
    }

    #[test]
    fn typo_skips_short_names() {
        let src = "let n = 1\nlet m = n + 1\n";
        let cands = IdentifierTypoRule.apply(src);
        assert!(cands.is_empty(), "short-name typos shouldn't fire");
    }

    #[test]
    fn typo_ignores_occurrences_in_strings() {
        // `apple` appears in a string but not in code; rule should
        // emit no candidate (no use site found).
        let src = "let apple = 1\nprint(\"apple is great\")\n";
        let cands = IdentifierTypoRule.apply(src);
        assert!(cands.is_empty(), "string occurrences shouldn't count");
    }

    #[test]
    fn literal_rule_only_fires_in_strict_mode() {
        let strict = "# strict\nlet x: int = 42\n";
        let lax = "let x: int = 42\n";
        assert!(LiteralTypeRule.apply(lax).is_empty());
        assert!(!LiteralTypeRule.apply(strict).is_empty());
    }

    #[test]
    fn literal_rule_replaces_int_with_string() {
        let src = "# verified\nlet x: int = 42\n";
        let cands = LiteralTypeRule.apply(src);
        assert_eq!(cands.len(), 1);
        assert!(cands[0].mutated.contains("\"42\""));
    }

    #[test]
    fn run_emits_triples_and_returns_count() {
        let tmp = tempdir("mutate_run");
        let src_dir = tmp.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        // Need strict mode so verify actually flags the typo.
        std::fs::write(
            src_dir.join("a.twe"),
            "# verified\nlet apple = 1\nlet other = apple + 2\n",
        )
        .unwrap();
        let out = tmp.join("out");
        let report = run(&src_dir, &out, RuleSet::IdentifierTypoOnly);
        assert!(report.triples_emitted >= 1);
        let body = std::fs::read_to_string(out.join("error_fix.jsonl")).unwrap();
        assert!(body.contains("\"tool\":\"twec-mutate\""));
        assert!(body.contains("\"rule\":\"identifier_typo\""));
        assert!(body.contains("\"verify\":"));
        // Each triple is one JSONL line.
        let lines: Vec<&str> = body.lines().collect();
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.starts_with('{') && line.ends_with('}'));
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn tempdir(prefix: &str) -> PathBuf {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("twec_{prefix}_{ts}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
