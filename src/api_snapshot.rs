//! Phase 35 session 1: API stability snapshot tooling.
//!
//! Closes Phase 16's last open exit criterion — "six months of API
//! stability since the v0.7 freeze." The mechanical part of that
//! criterion is "no public-surface change between snapshot N and
//! snapshot N+30days." The judgement part is "if there *was* a change,
//! was it expected and `@deprecated`-cycled?"
//!
//! This module builds the input to that audit: a canonical, hashable
//! JSON document of every public-API surface a Twe author / LLM /
//! tool consumer could call. Two surfaces feed in:
//!
//! 1. **Stdlib manifest** — every callable from
//!    [`crate::stdlib::manifest`]. 235 builtins as of Phase 33.
//! 2. **Keyword list** — the closed set of reserved words from
//!    `src/lexer.rs`. Locked per `CLAUDE.md` "What is locked" but
//!    listed here so a snapshot diff catches any drift.
//! 3. **Tool versions** — the schema version of every JSON-emitting
//!    `twec` subcommand (verify v2, grammar v1, stdlib v1, corpus v1,
//!    eval v1, api-snapshot v1).
//!
//! The output gets a deterministic FNV-1a hash so two snapshots are
//! byte-identical iff every public surface matched.
//!
//! ## CLI
//!
//! - `twec api-snapshot [-o PATH]` — write the current snapshot to
//!   PATH (or stdout). Suggested checkin location:
//!   `docs/api-snapshots/<YYYY-MM-DD>.json`.
//! - `twec api-diff <old> <new>` — read two snapshots and report
//!   builtins added, builtins removed, builtins with changed
//!   signatures, keyword list deltas. Exit 0 if identical, exit 3
//!   if any drift was found (so CI can gate releases on this).
//!
//! ## What this does *not* do
//!
//! It does not classify a diff as "breaking" vs. "additive." That's
//! the maintainer's call (additive-but-deprecated is fine; removal
//! without a deprecation cycle is not). The tool reports facts; the
//! LTS branch policy in `CONTRIBUTING.md` codifies the response.

use crate::stdlib::manifest;

/// Canonical keyword list — must match the match arm in
/// `src/lexer.rs::Lexer::ident_or_keyword`. Listed here separately
/// rather than re-introspecting the lexer because the lexer's match
/// is private and adding a public iterator just for the snapshot
/// would inflate the lexer's surface area. If a keyword is added in
/// the lexer without being added here, the next `api-diff` will
/// flag the drift.
const KEYWORDS: &[&str] = &[
    "actor", "and", "break", "choice", "continue", "despawn",
    "dialogue", "elif", "else", "entity", "every", "extends", "for",
    "function", "if", "import", "in", "inventory", "item", "let",
    "modifier", "not", "on", "or", "particles", "return", "say",
    "scene", "self", "spawn", "state", "var", "visual", "wait",
    "while",
];

/// Schema versions for every JSON-emitting tool. A bump here is a
/// breaking change to the LLM/tool contract and must come with a
/// migration note + at minimum one release of dual-version emit.
const TOOL_VERSIONS: &[(&str, u32)] = &[
    ("api-snapshot", 1),
    ("corpus", 1),
    ("eval", 1),
    ("grammar", 1),
    ("stdlib", 1),
    ("verify", 2),
];

/// FNV-1a, mirroring `src/net.rs::fnv1a`. Inlined rather than
/// re-exported so the snapshot module stays free of net-only
/// dependencies (`#[cfg(not(target_arch = "wasm32"))]` guards).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Build the snapshot's canonical JSON form. Output format:
///
/// ```json
/// {
///   "tool": "twec-api-snapshot",
///   "version": 1,
///   "hash": "0xfedcba98...",
///   "builtins_count": 235,
///   "keywords_count": 35,
///   "builtins": [{"name":"...","category":"...","params":[...]}],
///   "keywords": ["actor","and",...],
///   "tool_versions": {"api-snapshot":1,"corpus":1,...}
/// }
/// ```
///
/// The hash is FNV-1a over the body that *follows* the hash field —
/// equivalently, the snapshot string with `"hash":"0x..."` replaced
/// by `"hash":""`. This means hashing is order-stable and a snapshot
/// is reproducible byte-for-byte from the stdlib + keyword inputs.
pub fn snapshot_json() -> String {
    let body = render_body();
    let h = fnv1a(body.as_bytes());
    let mut out = String::with_capacity(body.len() + 96);
    out.push('{');
    out.push_str("\"tool\":\"twec-api-snapshot\",\"version\":1,");
    out.push_str(&format!("\"hash\":\"0x{h:016x}\","));
    out.push_str(&body[1..body.len() - 1]);
    out.push('}');
    out.push('\n');
    out
}

fn render_body() -> String {
    let manifest = manifest();
    let mut s = String::with_capacity(8192);
    s.push('{');
    s.push_str(&format!("\"builtins_count\":{},", manifest.len()));
    s.push_str(&format!("\"keywords_count\":{},", KEYWORDS.len()));
    s.push_str("\"builtins\":[");
    for (i, spec) in manifest.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":\"{}\",\"category\":\"{}\",\"params\":[",
            json_escape(&spec.name),
            json_escape(&spec.category),
        ));
        for (j, p) in spec.params.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            s.push('"');
            s.push_str(&json_escape(p));
            s.push('"');
        }
        s.push_str("]}");
    }
    s.push_str("],\"keywords\":[");
    for (i, k) in KEYWORDS.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        s.push_str(k);
        s.push('"');
    }
    s.push_str("],\"tool_versions\":{");
    for (i, (name, v)) in TOOL_VERSIONS.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("\"{name}\":{v}"));
    }
    s.push_str("}}");
    s
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Result of comparing two snapshots. Empty `is_clean` if and only
/// if every public surface matched.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApiDiff {
    pub builtins_added: Vec<String>,
    pub builtins_removed: Vec<String>,
    pub builtins_changed: Vec<String>,
    pub keywords_added: Vec<String>,
    pub keywords_removed: Vec<String>,
    pub tool_version_changes: Vec<(String, u32, u32)>,
}

impl ApiDiff {
    pub fn is_clean(&self) -> bool {
        self.builtins_added.is_empty()
            && self.builtins_removed.is_empty()
            && self.builtins_changed.is_empty()
            && self.keywords_added.is_empty()
            && self.keywords_removed.is_empty()
            && self.tool_version_changes.is_empty()
    }
}

/// Diff two snapshot strings. Reads each through a deliberately tiny
/// JSON-extraction shim (we don't need a real parser — we wrote the
/// emitter ourselves and the field order is fixed). If either input
/// is malformed, returns an `ApiDiff` with everything from the
/// "valid" side reported as removed/added.
pub fn diff(old_json: &str, new_json: &str) -> ApiDiff {
    let old = parse_snapshot(old_json);
    let new = parse_snapshot(new_json);
    let mut d = ApiDiff::default();

    // Builtins
    for (name, params) in &new.builtins {
        match old.builtins.iter().find(|(n, _)| n == name) {
            None => d.builtins_added.push(name.clone()),
            Some((_, old_params)) if old_params != params => {
                d.builtins_changed.push(name.clone());
            }
            _ => {}
        }
    }
    for (name, _) in &old.builtins {
        if !new.builtins.iter().any(|(n, _)| n == name) {
            d.builtins_removed.push(name.clone());
        }
    }

    // Keywords
    for k in &new.keywords {
        if !old.keywords.contains(k) {
            d.keywords_added.push(k.clone());
        }
    }
    for k in &old.keywords {
        if !new.keywords.contains(k) {
            d.keywords_removed.push(k.clone());
        }
    }

    // Tool versions
    for (name, new_v) in &new.tool_versions {
        match old.tool_versions.iter().find(|(n, _)| n == name) {
            None => d
                .tool_version_changes
                .push((name.clone(), 0, *new_v)),
            Some((_, old_v)) if old_v != new_v => d
                .tool_version_changes
                .push((name.clone(), *old_v, *new_v)),
            _ => {}
        }
    }
    for (name, old_v) in &old.tool_versions {
        if !new.tool_versions.iter().any(|(n, _)| n == name) {
            d.tool_version_changes
                .push((name.clone(), *old_v, 0));
        }
    }

    d
}

#[derive(Debug, Default)]
struct ParsedSnapshot {
    builtins: Vec<(String, Vec<String>)>,
    keywords: Vec<String>,
    tool_versions: Vec<(String, u32)>,
}

fn parse_snapshot(json: &str) -> ParsedSnapshot {
    let mut p = ParsedSnapshot::default();
    // Pull the `"builtins":[...]` block. Flat-text scan; the emitter
    // is the only producer so we trust the layout.
    if let Some(start) = json.find("\"builtins\":[") {
        let after = &json[start + "\"builtins\":[".len()..];
        if let Some(end) = match_array_end(after) {
            let body = &after[..end];
            for entry in split_top_objects(body) {
                let name = extract_str_field(entry, "\"name\":\"");
                let params = extract_str_array_field(entry, "\"params\":[");
                if !name.is_empty() {
                    p.builtins.push((name, params));
                }
            }
        }
    }
    if let Some(kw_start) = json.find("\"keywords\":[") {
        let after = &json[kw_start + "\"keywords\":[".len()..];
        if let Some(end) = match_array_end(after) {
            let body = &after[..end];
            p.keywords = split_string_array(body);
        }
    }
    if let Some(tv_start) = json.find("\"tool_versions\":{") {
        let after = &json[tv_start + "\"tool_versions\":{".len()..];
        if let Some(end) = match_object_end(after) {
            let body = &after[..end];
            for entry in body.split(',') {
                if let Some((k, v)) = entry.split_once(':') {
                    let key = k.trim().trim_matches('"').to_string();
                    let val: u32 = v.trim().parse().unwrap_or(0);
                    p.tool_versions.push((key, val));
                }
            }
        }
    }
    p
}

fn match_array_end(s: &str) -> Option<usize> {
    let mut depth: i32 = 1;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn match_object_end(s: &str) -> Option<usize> {
    let mut depth: i32 = 1;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_objects(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut depth: i32 = 0;
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    out.push(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    out
}

fn extract_str_field(entry: &str, key_prefix: &str) -> String {
    if let Some(start) = entry.find(key_prefix) {
        let rest = &entry[start + key_prefix.len()..];
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    String::new()
}

fn extract_str_array_field(entry: &str, key_prefix: &str) -> Vec<String> {
    if let Some(start) = entry.find(key_prefix) {
        let rest = &entry[start + key_prefix.len()..];
        if let Some(end) = match_array_end(rest) {
            return split_string_array(&rest[..end]);
        }
    }
    Vec::new()
}

fn split_string_array(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, c) in body.char_indices() {
        if c == '"' {
            // find matching close quote, accounting for backslash
            // escapes (the emitter only escapes the standard set).
            let after = &body[i + 1..];
            let mut esc = false;
            let mut close: Option<usize> = None;
            for (j, c2) in after.char_indices() {
                if esc {
                    esc = false;
                    continue;
                }
                if c2 == '\\' {
                    esc = true;
                    continue;
                }
                if c2 == '"' {
                    close = Some(j);
                    break;
                }
            }
            if let Some(end) = close {
                out.push(after[..end].to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_deterministic() {
        let a = snapshot_json();
        let b = snapshot_json();
        assert_eq!(a, b);
    }

    #[test]
    fn snapshot_includes_known_builtins_and_keywords() {
        let s = snapshot_json();
        assert!(s.contains("\"name\":\"print\""));
        assert!(s.contains("\"name\":\"math.sqrt\""));
        assert!(s.contains("\"entity\""));
        assert!(s.contains("\"function\""));
    }

    #[test]
    fn snapshot_emits_a_hash() {
        let s = snapshot_json();
        assert!(s.contains("\"hash\":\"0x"));
        // The hash is FNV-1a → 16 hex digits, prefixed `0x`.
        assert!(s.contains("\"hash\":\"0x") && s.matches('"').count() >= 4);
    }

    #[test]
    fn diff_is_clean_against_self() {
        let s = snapshot_json();
        let d = diff(&s, &s);
        assert!(d.is_clean(), "got: {:?}", d);
    }

    #[test]
    fn diff_detects_removed_builtin() {
        let new_s = snapshot_json();
        // Construct an "old" snapshot with one extra builtin in the
        // builtins array. The emitter output will then report it as
        // removed when diffed against current.
        let needle = "\"builtins\":[";
        let injected = "{\"name\":\"sentinel.removed\",\"category\":\"core\",\"params\":[]},";
        let pos = new_s.find(needle).unwrap() + needle.len();
        let mut old_s = String::new();
        old_s.push_str(&new_s[..pos]);
        old_s.push_str(injected);
        old_s.push_str(&new_s[pos..]);
        let d = diff(&old_s, &new_s);
        assert!(
            d.builtins_removed.iter().any(|n| n == "sentinel.removed"),
            "got: {:?}",
            d
        );
    }

    #[test]
    fn diff_detects_added_keyword() {
        let new_s = snapshot_json();
        let needle = "\"keywords\":[";
        let pos = new_s.find(needle).unwrap() + needle.len();
        // Old snapshot is missing the "actor" keyword.
        let stripped = new_s.replace("\"actor\",", "");
        let d = diff(&stripped, &new_s);
        let _ = pos;
        assert!(
            d.keywords_added.iter().any(|k| k == "actor"),
            "got: {:?}",
            d
        );
    }
}
