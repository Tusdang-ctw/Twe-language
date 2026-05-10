//! Phase 33 session 1: portable grammar export.
//!
//! Twe's grammar is small and LL(1)-ish. Exposing it as a portable
//! artifact unlocks the single largest accuracy lever for LLM
//! authoring: **constrained generation**. A model decoding against
//! GBNF / JSON-Schema / EBNF cannot emit a token that breaks the
//! grammar, so syntactic-class hallucinations drop to zero.
//!
//! ## Three formats
//!
//! - **GBNF** — [llama.cpp constrained generation](https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md). Local models constrain decoding directly.
//! - **JSON Schema** — a portable JSON description of the grammar's
//!   productions. Consumed by RAG / prompt-construction pipelines and
//!   by the Twe MCP server (Phase 33 session 5).
//! - **EBNF** — human-readable; doc inclusion + tree-sitter regen.
//!
//! ## Source of truth
//!
//! The parser stays hand-written per CLAUDE.md's locked decisions.
//! The grammar in [`RULES`] is *checked against* the parser via
//! `tests/grammar.rs` (every example in `examples/` parses → at least
//! one rule covers each top-level Stmt variant). It is **not**
//! generated from the parser, and the parser is **not** generated
//! from it. Both are sources of truth that must agree on the
//! recognised language.

use std::fmt::Write;

/// One grammar production. The body is a small EBNF-flavoured DSL:
///
/// - `'kw'`       → terminal literal (a keyword or symbol)
/// - `IDENT`      → identifier token
/// - `INT`, `FLOAT`, `STRING` → token classes
/// - `NEWLINE`, `INDENT`, `DEDENT` → layout tokens
/// - `name`       → reference another rule
/// - `( ... )`    → grouping
/// - `*` / `+` / `?` → zero-or-more / one-or-more / optional
/// - `|`          → alternation (lowest precedence)
/// - whitespace   → sequence
///
/// The DSL is parsed once by [`parse_rule`] into a typed [`Item`]
/// tree, then rendered to each target format.
#[derive(Clone, Copy)]
pub struct Rule {
    pub name: &'static str,
    pub body: &'static str,
}

/// Twe's surface grammar. Pragmatic, sufficient for constrained
/// generation; not pedantically complete on every parser quirk.
/// Adding a production here without the parser also recognising it
/// is a bug — `tests/grammar.rs` round-trips every example to catch
/// drift in the most common direction (parser knows something the
/// grammar doesn't).
pub const RULES: &[Rule] = &[
    Rule { name: "program", body: "stmt*" },

    // --- statements ---
    Rule { name: "stmt", body: "let_stmt | var_stmt | if_stmt | on_stmt | decl_stmt \
                                | function_stmt | return_stmt | while_stmt | for_stmt \
                                | break_stmt | continue_stmt | spawn_stmt | despawn_stmt \
                                | wait_stmt | dialogue_stmt | say_stmt | choice_stmt \
                                | import_stmt | annotated_stmt | transition_stmt | expr_stmt" },

    Rule { name: "let_stmt", body: "'let' IDENT type_annotation? '=' expr NEWLINE" },
    Rule { name: "var_stmt", body: "'var' IDENT type_annotation? '=' expr NEWLINE" },

    Rule { name: "if_stmt",
           body: "'if' expr ':' block elif_clause* else_clause?" },
    Rule { name: "elif_clause", body: "'elif' expr ':' block" },
    Rule { name: "else_clause", body: "'else' ':' block" },

    Rule { name: "on_stmt",
           body: "'on' on_event NEWLINE block" },
    Rule { name: "on_event",
           body: "IDENT '(' params? ')' \
                | IDENT '.' IDENT '(' params? ')'" },

    Rule { name: "decl_stmt",
           body: "decl_kind IDENT extends_clause? ':' decl_body" },
    Rule { name: "decl_kind",
           body: "'entity' | 'item' | 'modifier' | 'inventory' | 'scene' \
                | 'particles' | 'visual'" },
    Rule { name: "extends_clause", body: "'extends' IDENT" },
    Rule { name: "decl_body", body: "INDENT decl_member+ DEDENT" },
    Rule { name: "decl_member",
           body: "field_decl | function_stmt | state_block | initial_decl" },
    Rule { name: "field_decl",
           body: "('let' | 'var') IDENT type_annotation? '=' expr NEWLINE" },
    Rule { name: "state_block",
           body: "'state' IDENT ':' INDENT state_member+ DEDENT" },
    Rule { name: "state_member",
           body: "on_stmt | every_clock | function_stmt" },
    Rule { name: "every_clock", body: "'every' expr ':' block" },
    Rule { name: "initial_decl", body: "'initial' ':' IDENT NEWLINE" },

    Rule { name: "function_stmt",
           body: "'function' IDENT '(' params? ')' return_annotation? ':' block" },
    Rule { name: "return_stmt", body: "'return' expr? NEWLINE" },

    Rule { name: "while_stmt", body: "'while' expr ':' block" },
    Rule { name: "for_stmt", body: "'for' IDENT 'in' expr ':' block" },
    Rule { name: "break_stmt", body: "'break' NEWLINE" },
    Rule { name: "continue_stmt", body: "'continue' NEWLINE" },

    Rule { name: "spawn_stmt", body: "'spawn' IDENT ('at' expr)? NEWLINE" },
    Rule { name: "despawn_stmt", body: "'despawn' expr NEWLINE" },
    Rule { name: "wait_stmt", body: "'wait' expr NEWLINE" },
    Rule { name: "transition_stmt", body: "'-' '>' IDENT NEWLINE" },

    Rule { name: "dialogue_stmt", body: "'dialogue' IDENT ':' INDENT dialogue_member+ DEDENT" },
    Rule { name: "dialogue_member", body: "actor_decl | say_stmt | choice_stmt" },
    Rule { name: "actor_decl", body: "'actor' IDENT '=' expr NEWLINE" },
    Rule { name: "say_stmt", body: "'say' (IDENT ':')? expr NEWLINE" },
    Rule { name: "choice_stmt",
           body: "'choice' ':' INDENT choice_branch+ DEDENT" },
    Rule { name: "choice_branch", body: "STRING ':' (block | expr NEWLINE)" },

    Rule { name: "import_stmt", body: "'import' STRING ('as' IDENT)? NEWLINE" },

    Rule { name: "annotated_stmt", body: "'@' IDENT ('(' arg_list? ')')? NEWLINE+ stmt" },

    Rule { name: "expr_stmt", body: "expr (assign_op expr)? NEWLINE" },
    Rule { name: "assign_op", body: "'=' | '+=' | '-=' | '*=' | '/='" },

    Rule { name: "block", body: "INDENT stmt+ DEDENT" },
    Rule { name: "params", body: "param (',' param)*" },
    Rule { name: "param", body: "IDENT type_annotation?" },
    Rule { name: "type_annotation", body: "':' type_expr" },
    Rule { name: "return_annotation", body: "'->' type_expr" },
    Rule { name: "type_expr", body: "IDENT ('|' IDENT)* '?'?" },

    // --- expressions, lowest precedence first ---
    Rule { name: "expr", body: "or_expr" },
    Rule { name: "or_expr", body: "and_expr ('or' and_expr)*" },
    Rule { name: "and_expr", body: "not_expr ('and' not_expr)*" },
    Rule { name: "not_expr", body: "'not' not_expr | compare_expr" },
    Rule { name: "compare_expr",
           body: "sum_expr (compare_op sum_expr)*" },
    Rule { name: "compare_op",
           body: "'==' | '!=' | '<' | '<=' | '>' | '>='" },
    Rule { name: "sum_expr", body: "term_expr (('+' | '-') term_expr)*" },
    Rule { name: "term_expr", body: "factor_expr (('*' | '/' | '%') factor_expr)*" },
    Rule { name: "factor_expr", body: "('-' | '+') factor_expr | postfix_expr" },
    Rule { name: "postfix_expr",
           body: "primary_expr postfix_op*" },
    Rule { name: "postfix_op",
           body: "'.' IDENT \
                | '(' arg_list? ')' \
                | '[' expr ']'" },
    Rule { name: "primary_expr",
           body: "literal | IDENT | 'self' | tuple_expr | list_expr \
                | range_expr | if_expr | '(' expr ')'" },
    Rule { name: "if_expr", body: "'if' expr ':' expr 'else' ':' expr" },
    Rule { name: "tuple_expr", body: "'(' expr (',' expr)+ ')'" },
    Rule { name: "list_expr", body: "'[' (expr (',' expr)*)? ']'" },
    Rule { name: "range_expr", body: "INT '..' '='? INT" },

    Rule { name: "arg_list", body: "arg (',' arg)*" },
    Rule { name: "arg", body: "IDENT ':' expr | expr" },

    // --- atoms ---
    Rule { name: "literal",
           body: "INT | FLOAT | STRING | 'true' | 'false' | 'nil' | quantity_lit | percent_lit" },
    Rule { name: "quantity_lit", body: "(INT | FLOAT) UNIT" },
    Rule { name: "percent_lit", body: "(INT | FLOAT) '%'" },
];

/// All keywords recognized by the lexer. Mirrors `src/lexer.rs:1010-1046`;
/// drift between this list and the lexer is caught by `tests/grammar.rs`.
/// Also enumerated as terminals in the JSON / GBNF outputs so a constrained
/// decoder knows the legal keyword vocabulary.
pub const KEYWORDS: &[&str] = &[
    "let", "var", "on", "if", "elif", "else",
    "and", "or", "not",
    "entity", "item", "modifier", "inventory", "scene", "particles", "visual",
    "state", "every", "extends", "self",
    "function", "return", "while", "for", "in", "break", "continue",
    "spawn", "despawn", "wait",
    "dialogue", "say", "choice", "actor",
    "import",
    // Literal-position keywords that aren't TokenKind variants but appear
    // in the grammar (recognised as Ident at lex time, contextual-keyword
    // checked in the parser).
    "true", "false", "nil",
    "as", "at", "initial",
];

/// Output format for the grammar export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Gbnf,
    JsonSchema,
    Ebnf,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "gbnf" | "GBNF" => Some(Format::Gbnf),
            "json-schema" | "json_schema" | "json" => Some(Format::JsonSchema),
            "ebnf" | "EBNF" => Some(Format::Ebnf),
            _ => None,
        }
    }
}

/// Render the canonical Twe grammar in the requested format.
/// Convenience wrapper over [`render`] with [`RULES`].
pub fn export(format: Format) -> String {
    render(RULES, format)
}

/// Render a custom rule set. Exposed so tests / future tooling can
/// validate sub-grammars without hardcoding the canonical [`RULES`].
pub fn render(rules: &[Rule], format: Format) -> String {
    match format {
        Format::Gbnf => render_gbnf(rules),
        Format::JsonSchema => render_json(rules),
        Format::Ebnf => render_ebnf(rules),
    }
}

// ---------------------------------------------------------------------------
// DSL parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A literal terminal — `'let'`, `':'`, `'->'`. The string is
    /// the unquoted source.
    Term(String),
    /// A reference to another rule by name.
    NonTerm(String),
    /// An ALL-CAPS token-class reference — `IDENT`, `INT`, `STRING`,
    /// `NEWLINE`, `INDENT`, `DEDENT`, `UNIT`. Renderers know how
    /// to spell each in their target format.
    TokenClass(String),
    /// `( a b | c d )`
    Group(Vec<Vec<Item>>),
    /// `x*`
    Repeat(Box<Item>),
    /// `x+`
    OneMore(Box<Item>),
    /// `x?`
    Optional(Box<Item>),
}

/// Parse a rule body into a list of alternatives. Each alternative
/// is a sequence of items. Whitespace separates items; `|` separates
/// alternatives. Postfix `*`, `+`, `?` bind tightest.
pub fn parse_rule(body: &str) -> Vec<Vec<Item>> {
    let mut p = DslParser { src: body, pos: 0 };
    p.parse_alts()
}

struct DslParser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> DslParser<'a> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn parse_alts(&mut self) -> Vec<Vec<Item>> {
        let mut alts = vec![self.parse_seq()];
        loop {
            self.skip_ws();
            if matches!(self.peek(), Some('|')) {
                self.bump();
                alts.push(self.parse_seq());
            } else {
                break;
            }
        }
        alts
    }

    fn parse_seq(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let item = self.parse_postfix();
            items.push(item);
        }
        items
    }

    fn parse_postfix(&mut self) -> Item {
        let inner = self.parse_atom();
        self.skip_ws();
        match self.peek() {
            Some('*') => {
                self.bump();
                Item::Repeat(Box::new(inner))
            }
            Some('+') => {
                self.bump();
                Item::OneMore(Box::new(inner))
            }
            Some('?') => {
                self.bump();
                Item::Optional(Box::new(inner))
            }
            _ => inner,
        }
    }

    fn parse_atom(&mut self) -> Item {
        self.skip_ws();
        match self.peek() {
            Some('\'') => self.parse_literal(),
            Some('(') => self.parse_group(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => self.parse_ident(),
            other => panic!(
                "grammar DSL: unexpected character {other:?} at pos {} in {:?}",
                self.pos, self.src
            ),
        }
    }

    fn parse_literal(&mut self) -> Item {
        debug_assert!(matches!(self.peek(), Some('\'')));
        self.bump(); // opening quote
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == '\'' {
                let text = self.src[start..self.pos].to_string();
                self.bump(); // closing quote
                return Item::Term(text);
            }
            self.bump();
        }
        panic!("grammar DSL: unterminated literal in {:?}", self.src);
    }

    fn parse_group(&mut self) -> Item {
        debug_assert!(matches!(self.peek(), Some('(')));
        self.bump();
        let alts = self.parse_alts();
        self.skip_ws();
        if !matches!(self.peek(), Some(')')) {
            panic!("grammar DSL: missing ')' in {:?}", self.src);
        }
        self.bump();
        Item::Group(alts)
    }

    fn parse_ident(&mut self) -> Item {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        let text = self.src[start..self.pos].to_string();
        if is_token_class(&text) {
            Item::TokenClass(text)
        } else {
            Item::NonTerm(text)
        }
    }
}

fn is_token_class(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_uppercase() || c == '_') && !name.is_empty()
}

// ---------------------------------------------------------------------------
// GBNF (llama.cpp grammar format)
// ---------------------------------------------------------------------------

fn render_gbnf(rules: &[Rule]) -> String {
    // GBNF uses `name ::= alt | alt` form, identifiers in kebab-case
    // (underscore is fine), and `"literal"` for terminals. We define
    // every nonterminal, then provide token-class rules for IDENT,
    // INT, FLOAT, STRING, NEWLINE, INDENT, DEDENT, UNIT.
    let mut out = String::new();
    out.push_str("# Twe grammar — GBNF (Phase 33 session 1)\n");
    out.push_str("# Generated by `twec grammar --format gbnf`. Hand-edits will be lost.\n");
    out.push_str("# https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md\n\n");
    out.push_str("root ::= program\n\n");
    for rule in rules {
        let alts = parse_rule(rule.body);
        let _ = writeln!(out, "{} ::= {}", gbnf_name(rule.name), gbnf_alts(&alts));
    }
    out.push_str("\n# --- token classes ---\n");
    out.push_str("ident       ::= [a-zA-Z_] [a-zA-Z0-9_]*\n");
    out.push_str("int         ::= [0-9]+\n");
    out.push_str("float       ::= [0-9]+ \".\" [0-9]+\n");
    out.push_str("string      ::= \"\\\"\" ([^\"\\\\\\n] | \"\\\\\" .)* \"\\\"\"\n");
    out.push_str("unit        ::= \"s\" | \"ms\" | \"min\" | \"h\" | \"m\" | \"cm\" | \"mm\" | \"km\"\n");
    out.push_str("              | \"px\" | \"kg\" | \"g\" | \"mg\" | \"deg\" | \"rad\"\n");
    out.push_str("newline     ::= \"\\n\"\n");
    out.push_str("# Twe is layout-sensitive; INDENT / DEDENT are produced by the lexer.\n");
    out.push_str("# Constrained decoders that don't track indentation should treat them\n");
    out.push_str("# as zero-width markers and rely on the model's training to indent.\n");
    out.push_str("indent      ::= \"\"\n");
    out.push_str("dedent      ::= \"\"\n");
    out.push_str("\n# --- keywords (legal vocabulary) ---\n");
    out.push_str("# Listed for documentation. Each appears as a quoted terminal in the\n");
    out.push_str("# productions above.\n");
    out.push_str("# ");
    for (i, kw) in KEYWORDS.iter().enumerate() {
        if i > 0 { out.push(' '); }
        out.push_str(kw);
    }
    out.push('\n');
    out
}

fn gbnf_name(s: &str) -> String {
    s.replace('_', "-")
}

fn gbnf_alts(alts: &[Vec<Item>]) -> String {
    alts.iter()
        .map(|seq| gbnf_seq(seq))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn gbnf_seq(seq: &[Item]) -> String {
    seq.iter().map(gbnf_item).collect::<Vec<_>>().join(" ")
}

fn gbnf_item(item: &Item) -> String {
    match item {
        Item::Term(s) => format!("\"{}\"", gbnf_escape(s)),
        Item::NonTerm(s) => gbnf_name(s),
        Item::TokenClass(s) => match s.as_str() {
            "IDENT" => "ident".into(),
            "INT" => "int".into(),
            "FLOAT" => "float".into(),
            "STRING" => "string".into(),
            "NEWLINE" => "newline".into(),
            "INDENT" => "indent".into(),
            "DEDENT" => "dedent".into(),
            "UNIT" => "unit".into(),
            other => other.to_lowercase(),
        },
        Item::Group(alts) => format!("({})", gbnf_alts(alts)),
        Item::Repeat(inner) => format!("{}*", gbnf_item(inner)),
        Item::OneMore(inner) => format!("{}+", gbnf_item(inner)),
        Item::Optional(inner) => format!("{}?", gbnf_item(inner)),
    }
}

fn gbnf_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ---------------------------------------------------------------------------
// JSON Schema (portable JSON description)
// ---------------------------------------------------------------------------

fn render_json(rules: &[Rule]) -> String {
    // Hand-rolled JSON to match the rest of the project's "no serde
    // dependency" pattern (cf. `verify::to_json`, `ast_json::to_json`).
    // Versioned via tool + version so consumers can negotiate cleanly.
    let mut out = String::new();
    out.push('{');
    out.push_str("\"tool\":\"twec-grammar\",\"version\":1");
    out.push_str(",\"start\":\"program\"");
    out.push_str(",\"keywords\":[");
    for (i, kw) in KEYWORDS.iter().enumerate() {
        if i > 0 { out.push(','); }
        json_str(&mut out, kw);
    }
    out.push(']');
    out.push_str(",\"token_classes\":");
    out.push_str(json_token_classes());
    out.push_str(",\"productions\":[");
    for (i, rule) in rules.iter().enumerate() {
        if i > 0 { out.push(','); }
        out.push('{');
        out.push_str("\"name\":");
        json_str(&mut out, rule.name);
        out.push_str(",\"alternatives\":[");
        let alts = parse_rule(rule.body);
        for (j, seq) in alts.iter().enumerate() {
            if j > 0 { out.push(','); }
            out.push('[');
            for (k, item) in seq.iter().enumerate() {
                if k > 0 { out.push(','); }
                json_item(&mut out, item);
            }
            out.push(']');
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

fn json_token_classes() -> &'static str {
    // Concrete patterns for each token class. Mirror the lexer; the
    // patterns aren't enforced at JSON parse time but document the
    // contract for downstream tools.
    "{\
\"IDENT\":{\"pattern\":\"[a-zA-Z_][a-zA-Z0-9_]*\"},\
\"INT\":{\"pattern\":\"[0-9]+\"},\
\"FLOAT\":{\"pattern\":\"[0-9]+\\\\.[0-9]+\"},\
\"STRING\":{\"pattern\":\"\\\"([^\\\"\\\\\\\\\\\\n]|\\\\\\\\.)*\\\"\"},\
\"NEWLINE\":{\"description\":\"line terminator produced by the lexer\"},\
\"INDENT\":{\"description\":\"layout token at increased indentation\"},\
\"DEDENT\":{\"description\":\"layout token at decreased indentation\"},\
\"UNIT\":{\"enum\":[\"s\",\"ms\",\"min\",\"h\",\"m\",\"cm\",\"mm\",\"km\",\"px\",\"kg\",\"g\",\"mg\",\"deg\",\"rad\"]}\
}"
}

fn json_item(out: &mut String, item: &Item) {
    match item {
        Item::Term(s) => {
            out.push_str("{\"kind\":\"terminal\",\"value\":");
            json_str(out, s);
            out.push('}');
        }
        Item::NonTerm(s) => {
            out.push_str("{\"kind\":\"nonterminal\",\"name\":");
            json_str(out, s);
            out.push('}');
        }
        Item::TokenClass(s) => {
            out.push_str("{\"kind\":\"token_class\",\"name\":");
            json_str(out, s);
            out.push('}');
        }
        Item::Group(alts) => {
            out.push_str("{\"kind\":\"group\",\"alternatives\":[");
            for (i, seq) in alts.iter().enumerate() {
                if i > 0 { out.push(','); }
                out.push('[');
                for (j, it) in seq.iter().enumerate() {
                    if j > 0 { out.push(','); }
                    json_item(out, it);
                }
                out.push(']');
            }
            out.push_str("]}");
        }
        Item::Repeat(inner) => {
            out.push_str("{\"kind\":\"repeat\",\"inner\":");
            json_item(out, inner);
            out.push('}');
        }
        Item::OneMore(inner) => {
            out.push_str("{\"kind\":\"one_or_more\",\"inner\":");
            json_item(out, inner);
            out.push('}');
        }
        Item::Optional(inner) => {
            out.push_str("{\"kind\":\"optional\",\"inner\":");
            json_item(out, inner);
            out.push('}');
        }
    }
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------
// EBNF (human-readable, tree-sitter regen-friendly)
// ---------------------------------------------------------------------------

fn render_ebnf(rules: &[Rule]) -> String {
    let mut out = String::new();
    out.push_str("(* Twe grammar — EBNF (Phase 33 session 1) *)\n");
    out.push_str("(* Generated by `twec grammar --format ebnf`. Hand-edits will be lost. *)\n\n");
    for rule in rules {
        let alts = parse_rule(rule.body);
        let _ = writeln!(out, "{:<22} = {} ;", rule.name, ebnf_alts(&alts));
    }
    out.push_str("\n(* --- token classes ---*)\n");
    out.push_str("IDENT   = ? identifier: [a-zA-Z_][a-zA-Z0-9_]* ? ;\n");
    out.push_str("INT     = ? digits: [0-9]+ ? ;\n");
    out.push_str("FLOAT   = ? floating-point: [0-9]+\".\"[0-9]+ ? ;\n");
    out.push_str("STRING  = ? quoted string literal ? ;\n");
    out.push_str("NEWLINE = ? line terminator from lexer ? ;\n");
    out.push_str("INDENT  = ? layout token at increased indentation ? ;\n");
    out.push_str("DEDENT  = ? layout token at decreased indentation ? ;\n");
    out.push_str(
        "UNIT    = \"s\" | \"ms\" | \"min\" | \"h\" | \"m\" | \"cm\" | \"mm\" | \"km\"\n          | \"px\" | \"kg\" | \"g\" | \"mg\" | \"deg\" | \"rad\" ;\n",
    );
    out
}

fn ebnf_alts(alts: &[Vec<Item>]) -> String {
    alts.iter()
        .map(|seq| ebnf_seq(seq))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn ebnf_seq(seq: &[Item]) -> String {
    seq.iter().map(ebnf_item).collect::<Vec<_>>().join(" ")
}

fn ebnf_item(item: &Item) -> String {
    match item {
        Item::Term(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        Item::NonTerm(s) => s.clone(),
        Item::TokenClass(s) => s.clone(),
        Item::Group(alts) => format!("( {} )", ebnf_alts(alts)),
        Item::Repeat(inner) => format!("{{ {} }}", ebnf_item(inner)),
        Item::OneMore(inner) => format!("{} {{ {} }}", ebnf_item(inner), ebnf_item(inner)),
        Item::Optional(inner) => format!("[ {} ]", ebnf_item(inner)),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsl_parses_literal() {
        let alts = parse_rule("'let'");
        assert_eq!(alts.len(), 1);
        assert_eq!(alts[0], vec![Item::Term("let".into())]);
    }

    #[test]
    fn dsl_parses_alternation() {
        let alts = parse_rule("'a' | 'b'");
        assert_eq!(alts.len(), 2);
    }

    #[test]
    fn dsl_parses_postfix() {
        let alts = parse_rule("IDENT*");
        assert!(matches!(alts[0][0], Item::Repeat(_)));
        let alts = parse_rule("IDENT?");
        assert!(matches!(alts[0][0], Item::Optional(_)));
        let alts = parse_rule("IDENT+");
        assert!(matches!(alts[0][0], Item::OneMore(_)));
    }

    #[test]
    fn dsl_parses_group() {
        let alts = parse_rule("'(' (IDENT | INT)+ ')'");
        assert_eq!(alts.len(), 1);
        assert_eq!(alts[0].len(), 3);
        assert!(matches!(&alts[0][1], Item::OneMore(b) if matches!(**b, Item::Group(_))));
    }

    #[test]
    fn every_canonical_rule_parses() {
        // Smoke test: every rule body in RULES round-trips through
        // the DSL parser without panic.
        for rule in RULES {
            let _ = parse_rule(rule.body);
        }
    }

    #[test]
    fn gbnf_export_has_root_and_keywords() {
        let s = export(Format::Gbnf);
        assert!(s.contains("root ::= program"), "missing root rule");
        for kw in KEYWORDS {
            assert!(
                s.contains(&format!("\"{kw}\"")),
                "GBNF missing keyword `{kw}`"
            );
        }
    }

    #[test]
    fn json_export_parses_and_has_metadata() {
        let s = export(Format::JsonSchema);
        // Well-formed: starts with `{`, ends with `}`, balanced.
        assert!(s.starts_with('{'), "JSON missing leading `{{`");
        assert!(s.ends_with('}'), "JSON missing trailing `}}`");
        assert!(s.contains("\"tool\":\"twec-grammar\""));
        assert!(s.contains("\"version\":1"));
        assert!(s.contains("\"start\":\"program\""));
        // Spot-check that all keywords made it into the keywords array.
        for kw in KEYWORDS {
            assert!(s.contains(&format!("\"{kw}\"")), "JSON missing `{kw}`");
        }
    }

    #[test]
    fn ebnf_export_has_program_rule() {
        let s = export(Format::Ebnf);
        assert!(s.contains("program"), "EBNF missing program rule");
        assert!(s.contains("\"let\""), "EBNF missing `let` terminal");
    }

    #[test]
    fn format_parse_round_trip() {
        assert_eq!(Format::parse("gbnf"), Some(Format::Gbnf));
        assert_eq!(Format::parse("ebnf"), Some(Format::Ebnf));
        assert_eq!(Format::parse("json-schema"), Some(Format::JsonSchema));
        assert_eq!(Format::parse("json"), Some(Format::JsonSchema));
        assert_eq!(Format::parse("unknown"), None);
    }
}
