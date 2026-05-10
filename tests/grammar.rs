//! Phase 33 session 1: integration tests for `twec grammar`.
//!
//! Two responsibilities:
//!
//! 1. **Format soundness** — each emitted format is non-empty,
//!    well-formed, and exposes the canonical metadata an external
//!    tool needs (root rule, version, keyword list).
//!
//! 2. **Drift catch** — the keyword list in `grammar::KEYWORDS`
//!    matches the keyword set the lexer recognises. Missing the
//!    test would let a new keyword land without showing up in the
//!    constrained-decoding alphabet, which would silently break
//!    LLM authoring of programs using that keyword.
//!
//! The "round-trip every example" leg of the plan is approximated
//! here by lex-tokenising every example and asserting that every
//! keyword token actually present in real Twe code appears in
//! `grammar::KEYWORDS`. This catches the most common drift mode
//! (real example uses a keyword the grammar export forgot).

use twec::grammar::{self, Format, KEYWORDS};
use twec::lexer::{lex, TokenKind};

#[test]
fn gbnf_format_is_well_formed() {
    let s = grammar::export(Format::Gbnf);
    assert!(s.contains("root ::= program"), "GBNF must declare root rule");
    assert!(s.contains("ident       ::="), "GBNF must define ident token class");
    assert!(s.contains("string      ::="), "GBNF must define string token class");
    // Every keyword must appear as a quoted terminal.
    for kw in KEYWORDS {
        assert!(
            s.contains(&format!("\"{kw}\"")),
            "GBNF missing keyword terminal `{kw}`"
        );
    }
}

#[test]
fn json_schema_format_is_balanced_and_versioned() {
    let s = grammar::export(Format::JsonSchema);
    assert!(s.starts_with('{') && s.ends_with('}'));
    // Brace-balance check (the export is hand-rolled JSON; a simple
    // counter catches dropped braces during edits).
    let opens = s.matches('{').count();
    let closes = s.matches('}').count();
    assert_eq!(opens, closes, "unbalanced braces in JSON schema export");
    let opens = s.matches('[').count();
    let closes = s.matches(']').count();
    assert_eq!(opens, closes, "unbalanced brackets in JSON schema export");

    assert!(s.contains("\"tool\":\"twec-grammar\""));
    assert!(s.contains("\"version\":1"));
    assert!(s.contains("\"start\":\"program\""));
    assert!(s.contains("\"productions\":["));
    assert!(s.contains("\"keywords\":["));
    assert!(s.contains("\"token_classes\":"));
}

#[test]
fn ebnf_format_has_program_and_productions() {
    let s = grammar::export(Format::Ebnf);
    assert!(s.contains("program"), "EBNF missing program rule");
    // Pick a sample of productions that should always be present.
    for production in &["stmt", "expr", "if_stmt", "let_stmt", "function_stmt"] {
        assert!(
            s.contains(production),
            "EBNF missing expected production `{production}`"
        );
    }
}

#[test]
fn keyword_list_is_unique() {
    let mut seen = std::collections::HashSet::new();
    for kw in KEYWORDS {
        assert!(seen.insert(*kw), "duplicate keyword in KEYWORDS: `{kw}`");
    }
}

#[test]
fn examples_use_only_known_keywords() {
    // For every example file, lex it and assert every keyword
    // (TokenKind variant that maps to a reserved word) corresponds
    // to a string in `grammar::KEYWORDS`. If a new keyword lands in
    // the lexer without being added to the grammar export, this
    // test fails — that's the drift mode we're guarding against.
    let example_dir = std::path::Path::new("examples");
    assert!(example_dir.is_dir(), "examples/ directory not found");

    let known: std::collections::HashSet<&'static str> = KEYWORDS.iter().copied().collect();
    let mut total_files = 0;

    visit_twe_files(example_dir, &mut |path: &std::path::Path| {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return,
        };
        // Skip files that aren't lexable in isolation. Some demos
        // depend on engine ambient state that the lexer doesn't
        // care about — but if the lexer fails, that's a real
        // problem the test should report rather than swallow.
        let tokens = match lex(&src) {
            Ok(t) => t,
            Err(e) => panic!("lex failed on {}: {}", path.display(), e.message),
        };
        total_files += 1;
        for tok in &tokens {
            if let Some(kw) = keyword_text(&tok.kind) {
                assert!(
                    known.contains(kw),
                    "lexer recognises keyword `{kw}` (in {}) but \
                     grammar::KEYWORDS does not list it. Add it to \
                     `src/grammar.rs::KEYWORDS` so the constrained-\
                     decoding alphabet stays in sync.",
                    path.display()
                );
            }
        }
    });

    assert!(
        total_files > 0,
        "no .twe examples found — test cannot meaningfully run"
    );
}

fn visit_twe_files(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            visit_twe_files(&p, f);
        } else if p.extension().is_some_and(|e| e == "twe") {
            f(&p);
        }
    }
}

/// Map a TokenKind back to its keyword spelling. Returns None for
/// non-keyword tokens (identifiers, literals, punctuation).
fn keyword_text(kind: &TokenKind) -> Option<&'static str> {
    Some(match kind {
        TokenKind::Let => "let",
        TokenKind::Var => "var",
        TokenKind::On => "on",
        TokenKind::If => "if",
        TokenKind::Elif => "elif",
        TokenKind::Else => "else",
        TokenKind::And => "and",
        TokenKind::Or => "or",
        TokenKind::Not => "not",
        TokenKind::Entity => "entity",
        TokenKind::Item => "item",
        TokenKind::Modifier => "modifier",
        TokenKind::Inventory => "inventory",
        TokenKind::Scene => "scene",
        TokenKind::Particles => "particles",
        TokenKind::Visual => "visual",
        TokenKind::State => "state",
        TokenKind::Every => "every",
        TokenKind::Extends => "extends",
        TokenKind::KwSelf => "self",
        TokenKind::Function => "function",
        TokenKind::Return => "return",
        TokenKind::While => "while",
        TokenKind::For => "for",
        TokenKind::In => "in",
        TokenKind::Break => "break",
        TokenKind::Continue => "continue",
        TokenKind::Spawn => "spawn",
        TokenKind::Despawn => "despawn",
        TokenKind::Wait => "wait",
        TokenKind::Dialogue => "dialogue",
        TokenKind::Say => "say",
        TokenKind::Choice => "choice",
        TokenKind::Actor => "actor",
        TokenKind::Import => "import",
        _ => return None,
    })
}
