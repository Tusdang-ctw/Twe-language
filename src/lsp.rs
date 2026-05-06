//! Minimal Language Server Protocol implementation for Twe.
//!
//! Speaks JSON-RPC 2.0 over stdio. Supported requests: diagnostics
//! (republished on every open / change), hover with inferred type,
//! go-to-definition, and completion. Phase 5 entry shipped completion;
//! prior phases shipped the rest.
//!
//! Lifecycle:
//!   1. Client sends `initialize` request → we reply with our
//!      `ServerCapabilities` (`textDocumentSync = Full`, no
//!      hover/definition yet).
//!   2. Client sends `initialized` notification → we're live.
//!   3. For each open file, client sends `textDocument/didOpen` /
//!      `didChange` / `didClose` notifications. We track the
//!      latest text per URI in a HashMap and publish diagnostics
//!      on every update.
//!   4. Client sends `shutdown` request → we reply `null` and
//!      stop processing further notifications.
//!   5. Client sends `exit` → we return from the loop.
//!
//! Wire format: each message is `Content-Length: N\r\n\r\n` plus
//! N bytes of JSON (LSP §3 Base Protocol).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};

use crate::ast::{DeclMember, Program, StateMember, Stmt};
use crate::json::{self, obj, Value};
use crate::{lexer, parser};

/// Run the LSP loop reading from `input` and writing to `output`.
/// Returns `Ok(())` on graceful shutdown (`exit` after `shutdown`)
/// or any IO error encountered while reading.
pub fn run<R: Read, W: Write>(input: R, mut output: W) -> std::io::Result<()> {
    let mut input = BufReader::new(input);
    let mut server = Server::new();
    loop {
        let body = match read_message(&mut input)? {
            Some(b) => b,
            None => return Ok(()),
        };
        let request = match json::parse(&body) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("lsp: dropping malformed message: {e}");
                continue;
            }
        };
        if !server.handle(&request, &mut output)? {
            return Ok(());
        }
    }
}

/// Read one LSP message from `input`. Returns `Ok(None)` on EOF.
fn read_message<R: BufRead>(input: &mut R) -> std::io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    let mut header = String::new();
    loop {
        header.clear();
        let n = input.read_line(&mut header)?;
        if n == 0 {
            return Ok(None);
        }
        let line = header.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse().map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("bad Content-Length: {e}"),
                )
            })?);
        }
        // Other headers (Content-Type) are ignored — LSP spec only
        // requires Content-Length to be honoured.
    }
    let len = content_length.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "LSP message missing Content-Length",
        )
    })?;
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    String::from_utf8(body).map(Some).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("body not UTF-8: {e}"),
        )
    })
}

fn write_message<W: Write>(output: &mut W, body: &str) -> std::io::Result<()> {
    write!(output, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    output.flush()
}

/// Per-session state: open documents + shutdown flag.
struct Server {
    /// `uri` -> latest text. We re-parse from scratch on every
    /// change since Twe files are tiny and the parser is fast.
    documents: HashMap<String, String>,
    /// Set once the client has sent `shutdown`. After this, all
    /// requests get InvalidRequest replies and notifications are
    /// dropped (per LSP spec §3.13 / §3.14).
    shutdown_received: bool,
}

impl Server {
    fn new() -> Self {
        Self {
            documents: HashMap::new(),
            shutdown_received: false,
        }
    }

    /// Returns `Ok(true)` to keep the loop running, `Ok(false)`
    /// when an `exit` notification was received and the loop
    /// should terminate.
    fn handle<W: Write>(&mut self, msg: &Value, output: &mut W) -> std::io::Result<bool> {
        let method = msg.get("method").and_then(|m| m.as_str());
        let id = msg.get("id");
        match method {
            Some("initialize") => {
                self.send_response(output, id, initialize_result())?;
                Ok(true)
            }
            Some("initialized") => Ok(true),
            Some("shutdown") => {
                self.shutdown_received = true;
                self.send_response(output, id, Value::Null)?;
                Ok(true)
            }
            Some("exit") => Ok(false),
            Some("textDocument/didOpen") => {
                if let Some((uri, text)) = parse_did_open(msg) {
                    self.documents.insert(uri.clone(), text.clone());
                    self.publish_diagnostics(output, &uri, &text)?;
                }
                Ok(true)
            }
            Some("textDocument/didChange") => {
                if let Some((uri, text)) = parse_did_change(msg) {
                    self.documents.insert(uri.clone(), text.clone());
                    self.publish_diagnostics(output, &uri, &text)?;
                }
                Ok(true)
            }
            Some("textDocument/didClose") => {
                if let Some(uri) = parse_did_close(msg) {
                    self.documents.remove(&uri);
                    // Clear diagnostics on close so the client
                    // doesn't keep stale squiggles around.
                    self.publish_diagnostics_raw(output, &uri, Vec::new())?;
                }
                Ok(true)
            }
            Some("textDocument/definition") => {
                let result = self
                    .resolve_position(msg)
                    .map(|(uri, sym)| {
                        let text = self.documents.get(&uri).cloned().unwrap_or_default();
                        symbol_to_location(&uri, &sym, &text)
                    })
                    .unwrap_or(Value::Null);
                self.send_response(output, id, result)?;
                Ok(true)
            }
            Some("textDocument/completion") => {
                let result = self
                    .completions_for(msg)
                    .map(|items| {
                        obj([
                            ("isIncomplete", Value::Bool(false)),
                            ("items", Value::Array(items)),
                        ])
                    })
                    .unwrap_or(Value::Null);
                self.send_response(output, id, result)?;
                Ok(true)
            }
            Some("textDocument/hover") => {
                let result = self
                    .resolve_position(msg)
                    .map(|(uri, sym)| {
                        // Run type inference on the current
                        // document and look up `sym.name` in the
                        // top-level bindings. Inference is a
                        // single-pass best-effort; it returns
                        // Type::Unknown when it can't prove
                        // anything (per non-strict's no-false-
                        // positives guarantee).
                        let inferred = self.documents.get(&uri).and_then(|text| {
                            let tokens = lexer::lex(text).ok()?;
                            let program = parser::parse(&tokens).ok()?;
                            let bindings = crate::infer::infer_program(&program);
                            bindings.get(&sym.name).cloned()
                        });
                        symbol_to_hover(&sym, inferred.as_ref())
                    })
                    .unwrap_or(Value::Null);
                self.send_response(output, id, result)?;
                Ok(true)
            }
            Some(other) => {
                // Unknown method. If it's a request (has `id`),
                // reply MethodNotFound so the client doesn't hang.
                if id.is_some() {
                    self.send_error(
                        output,
                        id,
                        -32601,
                        format!("method '{other}' not implemented"),
                    )?;
                }
                Ok(true)
            }
            None => Ok(true),
        }
    }

    fn send_response<W: Write>(
        &self,
        output: &mut W,
        id: Option<&Value>,
        result: Value,
    ) -> std::io::Result<()> {
        let mut o = std::collections::BTreeMap::new();
        o.insert("jsonrpc".to_string(), Value::Str("2.0".into()));
        if let Some(id) = id {
            o.insert("id".to_string(), id.clone());
        }
        o.insert("result".to_string(), result);
        write_message(output, &json::to_string(&Value::Object(o)))
    }

    fn send_error<W: Write>(
        &self,
        output: &mut W,
        id: Option<&Value>,
        code: i64,
        message: String,
    ) -> std::io::Result<()> {
        let mut o = std::collections::BTreeMap::new();
        o.insert("jsonrpc".to_string(), Value::Str("2.0".into()));
        if let Some(id) = id {
            o.insert("id".to_string(), id.clone());
        }
        o.insert(
            "error".to_string(),
            obj([("code", Value::Int(code)), ("message", Value::Str(message))]),
        );
        write_message(output, &json::to_string(&Value::Object(o)))
    }

    fn send_notification<W: Write>(
        &self,
        output: &mut W,
        method: &str,
        params: Value,
    ) -> std::io::Result<()> {
        let msg = obj([
            ("jsonrpc", Value::Str("2.0".into())),
            ("method", Value::Str(method.into())),
            ("params", params),
        ]);
        write_message(output, &json::to_string(&msg))
    }

    /// Re-lex + re-parse `text` and emit a `publishDiagnostics`
    /// notification for any errors (or an empty array to clear).
    fn publish_diagnostics<W: Write>(
        &self,
        output: &mut W,
        uri: &str,
        text: &str,
    ) -> std::io::Result<()> {
        let diags = collect_diagnostics(text);
        self.publish_diagnostics_raw(output, uri, diags)
    }

    fn publish_diagnostics_raw<W: Write>(
        &self,
        output: &mut W,
        uri: &str,
        diags: Vec<Value>,
    ) -> std::io::Result<()> {
        let params = obj([
            ("uri", Value::Str(uri.into())),
            ("diagnostics", Value::Array(diags)),
        ]);
        self.send_notification(output, "textDocument/publishDiagnostics", params)
    }
}

impl Server {
    /// Common path for `textDocument/definition` and `hover`:
    /// extract (uri, line, col) from the request, look up the
    /// document text, find the identifier under the cursor,
    /// resolve it against the file's symbol index. Returns the
    /// uri (echoed back into the Location response) and the
    /// matching `Symbol`.
    ///
    /// Scoping limitations (MVP, documented in the LSP README):
    /// only top-level declarations + nested method/state names
    /// are indexed. Local lets and parameters fall through to
    /// the global lookup. A full lexical-scope pass arrives in
    /// a follow-up session.
    fn resolve_position(&self, msg: &Value) -> Option<(String, Symbol)> {
        let params = msg.get("params")?;
        let uri = params
            .get("textDocument")?
            .get("uri")?
            .as_str()?
            .to_string();
        let pos = params.get("position")?;
        let line = pos.get("line")?.as_i64()? as u32;
        let character = pos.get("character")?.as_i64()? as u32;
        let text = self.documents.get(&uri)?;
        let name = identifier_at(text, line, character)?;
        let tokens = lexer::lex(text).ok()?;
        let program = parser::parse(&tokens).ok()?;
        let symbols = collect_symbols(&program);
        let sym = symbols.into_iter().find(|s| s.name == name)?;
        Some((uri, sym))
    }

    /// Build the completion list for the document referenced by
    /// `msg`. Returns `None` only if the request is malformed or
    /// the URI isn't tracked; a parse error in the document still
    /// yields keywords + stdlib (the user is typing — they need
    /// completions *especially* when the file is broken).
    fn completions_for(&self, msg: &Value) -> Option<Vec<Value>> {
        let uri = msg
            .get("params")?
            .get("textDocument")?
            .get("uri")?
            .as_str()?;
        let text = self.documents.get(uri)?;
        Some(compute_completions(text))
    }
}

/// One declaration site found in the AST. Used by hover +
/// go-to-definition to resolve identifiers back to source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-indexed line, matching the lexer / parser convention.
    pub line: u32,
    /// 1-indexed column.
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Let,
    Var,
    Function,
    Entity,
    Item,
    Modifier,
    Inventory,
    Scene,
    Particles,
    Visual,
    Method,
    State,
    InitialState,
    Field,
}

impl SymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Let => "let",
            SymbolKind::Var => "var",
            SymbolKind::Function => "function",
            SymbolKind::Entity => "entity",
            SymbolKind::Item => "item",
            SymbolKind::Modifier => "modifier",
            SymbolKind::Inventory => "inventory",
            SymbolKind::Scene => "scene",
            SymbolKind::Particles => "particles",
            SymbolKind::Visual => "visual",
            SymbolKind::Method => "method",
            SymbolKind::State => "state",
            SymbolKind::InitialState => "initial state",
            SymbolKind::Field => "field",
        }
    }
}

/// Walk the program collecting every declaration site we can
/// resolve a name reference to. Top-level decls + every method,
/// state, initial-state, and field name inside a class body.
/// Local lets and function parameters are intentionally skipped
/// (no scope tracking yet — see `resolve_position`'s comment).
pub fn collect_symbols(program: &Program) -> Vec<Symbol> {
    let mut out = Vec::new();
    for stmt in &program.stmts {
        collect_from_stmt(stmt, &mut out);
    }
    out
}

fn collect_from_stmt(stmt: &Stmt, out: &mut Vec<Symbol>) {
    match stmt {
        Stmt::Let {
            name, line, col, ..
        } => out.push(Symbol {
            name: name.clone(),
            kind: SymbolKind::Let,
            line: *line,
            col: *col,
        }),
        Stmt::FunctionDecl {
            name, line, col, ..
        } => out.push(Symbol {
            name: name.clone(),
            kind: SymbolKind::Function,
            line: *line,
            col: *col,
        }),
        Stmt::Decl {
            kind,
            name,
            members,
            line,
            col,
            ..
        } => {
            out.push(Symbol {
                name: name.clone(),
                kind: decl_kind_to_symbol(*kind),
                line: *line,
                col: *col,
            });
            for m in members {
                collect_from_member(m, out);
            }
        }
        // Block-bearing statements: walk into bodies so nested
        // function declarations land in the index too. Skip
        // local lets — those are scope-sensitive and a future
        // pass will handle them properly.
        Stmt::If {
            then_body,
            elifs,
            else_body,
            ..
        } => {
            for s in then_body {
                collect_from_stmt(s, out);
            }
            for (_, body) in elifs {
                for s in body {
                    collect_from_stmt(s, out);
                }
            }
            if let Some(eb) = else_body {
                for s in eb {
                    collect_from_stmt(s, out);
                }
            }
        }
        Stmt::While { body, .. } | Stmt::For { body, .. } | Stmt::OnUpdate { body, .. } => {
            for s in body {
                collect_from_stmt(s, out);
            }
        }
        // Everything else (Expr, Assign, Break, Continue, Return,
        // Spawn, Despawn, Transition, Var) doesn't introduce a
        // resolvable name into the global / class namespace.
        _ => {}
    }
}

fn collect_from_member(member: &DeclMember, out: &mut Vec<Symbol>) {
    match member {
        DeclMember::Field {
            name, line, col, ..
        } => out.push(Symbol {
            name: name.clone(),
            kind: SymbolKind::Field,
            line: *line,
            col: *col,
        }),
        DeclMember::Method {
            name,
            body,
            line,
            col,
            ..
        } => {
            out.push(Symbol {
                name: name.clone(),
                kind: SymbolKind::Method,
                line: *line,
                col: *col,
            });
            for s in body {
                collect_from_stmt(s, out);
            }
        }
        DeclMember::InitialState { name, line, col } => out.push(Symbol {
            name: name.clone(),
            kind: SymbolKind::InitialState,
            line: *line,
            col: *col,
        }),
        DeclMember::State {
            name,
            members,
            line,
            col,
        } => {
            out.push(Symbol {
                name: name.clone(),
                kind: SymbolKind::State,
                line: *line,
                col: *col,
            });
            for sm in members {
                collect_from_state_member(sm, out);
            }
        }
    }
}

fn collect_from_state_member(m: &StateMember, out: &mut Vec<Symbol>) {
    match m {
        StateMember::Stmt(s) => collect_from_stmt(s, out),
        StateMember::Every { body, .. }
        | StateMember::OnRender { body, .. }
        | StateMember::OnKeyPress { body, .. }
        | StateMember::OnUpdate { body, .. }
        | StateMember::OnPredicate { body, .. } => {
            for s in body {
                collect_from_stmt(s, out);
            }
        }
    }
}

fn decl_kind_to_symbol(k: crate::ast::DeclKind) -> SymbolKind {
    use crate::ast::DeclKind;
    match k {
        DeclKind::Entity => SymbolKind::Entity,
        DeclKind::Item => SymbolKind::Item,
        DeclKind::Modifier => SymbolKind::Modifier,
        DeclKind::Inventory => SymbolKind::Inventory,
        DeclKind::Scene => SymbolKind::Scene,
        DeclKind::Particles => SymbolKind::Particles,
        DeclKind::Visual => SymbolKind::Visual,
    }
}

/// Extract the identifier under the LSP-cursor position. Walks
/// the line `line` (zero-indexed) of `text`, scans backwards
/// from `character` to the start of the identifier, then forwards
/// to its end. Returns `None` if the cursor isn't on an identifier
/// character (matches Twe's identifier rule: `[A-Za-z_][A-Za-z_0-9]*`).
pub fn identifier_at(text: &str, line: u32, character: u32) -> Option<String> {
    let line_text = text.lines().nth(line as usize)?;
    let chars: Vec<char> = line_text.chars().collect();
    let mut idx = character as usize;
    if idx > chars.len() {
        return None;
    }
    // The cursor sits BETWEEN characters in LSP. If we're at the
    // right edge of an identifier (either past end-of-line or on
    // a non-id char with an id-char to the left), step left so
    // we're inside the word the user clicked at the end of.
    let at_end_or_break = idx == chars.len() || (idx < chars.len() && !is_id_continue(chars[idx]));
    if at_end_or_break && idx > 0 && is_id_continue(chars[idx - 1]) {
        idx -= 1;
    }
    if idx >= chars.len() || !is_id_continue(chars[idx]) {
        return None;
    }
    // Scan backwards to start.
    let mut start = idx;
    while start > 0 && is_id_continue(chars[start - 1]) {
        start -= 1;
    }
    // The first char must be a valid id-start (not a digit).
    if !is_id_start(chars[start]) {
        return None;
    }
    // Scan forwards to end.
    let mut end = idx + 1;
    while end < chars.len() && is_id_continue(chars[end]) {
        end += 1;
    }
    Some(chars[start..end].iter().collect())
}

fn is_id_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_id_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Build an LSP `Location` from a `Symbol`. Range covers the
/// declaration's name. The parser stores the line:col of the
/// statement's leading keyword (`let`, `function`, `entity`),
/// not of the name itself, so we scan the source line for the
/// name to get the precise column. Positions converted to
/// 0-indexed per LSP §3.6.
fn symbol_to_location(uri: &str, sym: &Symbol, text: &str) -> Value {
    let (line, col) = name_position_in_source(text, sym);
    let name_len = sym.name.chars().count() as i64;
    let start = obj([
        ("line", Value::Int(line as i64)),
        ("character", Value::Int(col as i64)),
    ]);
    let end = obj([
        ("line", Value::Int(line as i64)),
        ("character", Value::Int(col as i64 + name_len)),
    ]);
    obj([
        ("uri", Value::Str(uri.to_string())),
        ("range", obj([("start", start), ("end", end)])),
    ])
}

/// Find the actual name column on the symbol's source line.
/// Falls back to the parser's recorded column if the search
/// misses (shouldn't happen on well-formed input). Returns
/// 0-indexed line + character per LSP convention.
fn name_position_in_source(text: &str, sym: &Symbol) -> (u32, u32) {
    let line_idx = sym.line.saturating_sub(1) as usize;
    if let Some(line_text) = text.lines().nth(line_idx) {
        // Match on word boundary so `n` doesn't match the `n` inside
        // a longer identifier like `name`. Walk byte-by-byte.
        let bytes = line_text.as_bytes();
        let needle = sym.name.as_bytes();
        let mut i = 0;
        while i + needle.len() <= bytes.len() {
            let prev_ok = i == 0 || !is_id_continue_byte(bytes[i - 1]);
            let next_ok =
                i + needle.len() == bytes.len() || !is_id_continue_byte(bytes[i + needle.len()]);
            if prev_ok && next_ok && &bytes[i..i + needle.len()] == needle {
                return (line_idx as u32, i as u32);
            }
            i += 1;
        }
    }
    (sym.line.saturating_sub(1), sym.col.saturating_sub(1))
}

fn is_id_continue_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Build an LSP `Hover` for a `Symbol` with its inferred type
/// (when known). Markdown content rendered as a fenced `twe`
/// code block. When inference produced a useful type
/// (anything but `Type::Unknown`), it's appended as `: type`
/// so the user sees a complete declaration line.
fn symbol_to_hover(sym: &Symbol, inferred: Option<&crate::types::Type>) -> Value {
    let body = match inferred {
        Some(t) if !t.is_unknown() => match sym.kind {
            // Functions display the signature inline rather than
            // `function name: function(...)`.
            crate::lsp::SymbolKind::Function => {
                format!("```twe\nfunction {} : {}\n```", sym.name, t)
            }
            crate::lsp::SymbolKind::Method => {
                format!("```twe\nmethod {} : {}\n```", sym.name, t)
            }
            _ => format!("```twe\n{} {} : {}\n```", sym.kind.label(), sym.name, t),
        },
        _ => format!("```twe\n{} {}\n```", sym.kind.label(), sym.name),
    };
    obj([(
        "contents",
        obj([
            ("kind", Value::Str("markdown".into())),
            ("value", Value::Str(body)),
        ]),
    )])
}

fn initialize_result() -> Value {
    obj([
        (
            "capabilities",
            obj([
                // Full-document sync — client sends the whole text on
                // every change. Twe files are small enough that
                // incremental sync isn't worth the complexity in the
                // MVP.
                ("textDocumentSync", Value::Int(1)),
                ("definitionProvider", Value::Bool(true)),
                ("hoverProvider", Value::Bool(true)),
                (
                    "completionProvider",
                    obj([
                        // No trigger characters — VS Code asks for
                        // completion on every identifier keystroke
                        // by default. `.` would be useful for dotted
                        // builtins (math.abs, random.int) but the
                        // current stdlib registers them as flat
                        // names; revisit when namespaces become
                        // first-class.
                        ("resolveProvider", Value::Bool(false)),
                    ]),
                ),
            ]),
        ),
        (
            "serverInfo",
            obj([
                ("name", Value::Str("twec lsp".into())),
                ("version", Value::Str(env!("CARGO_PKG_VERSION").into())),
            ]),
        ),
    ])
}

// LSP `CompletionItemKind` constants (LSP §3.18). Inlined as a
// `mod` of `i64`s rather than an enum to keep the JSON surface
// unambiguous — these go straight into the wire format.
mod ck {
    pub const FUNCTION: i64 = 3;
    pub const FIELD: i64 = 5;
    pub const VARIABLE: i64 = 6;
    pub const CLASS: i64 = 7;
    pub const METHOD: i64 = 2;
    pub const ENUM: i64 = 13;
    pub const KEYWORD: i64 = 14;
    pub const CONSTANT: i64 = 21;
}

/// Twe keyword set, mirrored from the lexer's identifier-to-keyword
/// match in `src/lexer.rs:938`. Plus `true` / `false`, which the
/// parser at `src/parser.rs:1170` recognises as bool literals
/// (they are not lexer keywords but they look and feel like ones
/// to the user). If the lexer grows a keyword, add it here too.
const TWE_KEYWORDS: &[&str] = &[
    "and",
    "break",
    "continue",
    "despawn",
    "elif",
    "else",
    "entity",
    "every",
    "extends",
    "false",
    "for",
    "function",
    "if",
    "import",
    "in",
    "initial",
    "inventory",
    "item",
    "let",
    "modifier",
    "not",
    "on",
    "or",
    "particles",
    "render",
    "return",
    "scene",
    "self",
    "spawn",
    "state",
    "true",
    "update",
    "var",
    "while",
];

/// Build the completion list for `text`. Layered: user-declared
/// names (with inferred type when known), language keywords,
/// stdlib globals. Order doesn't affect the editor's filter — VS
/// Code re-sorts by match score — but a stable order makes
/// snapshot-style tests easier.
pub fn compute_completions(text: &str) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();

    // User symbols. Even a partial parse is useful — when the
    // file is broken the parser bails, but on a clean file we
    // also get inferred types to annotate each item.
    if let Ok(tokens) = lexer::lex(text) {
        if let Ok(program) = parser::parse(&tokens) {
            let bindings = crate::infer::infer_program(&program);
            for sym in collect_symbols(&program) {
                let detail = bindings
                    .get(&sym.name)
                    .filter(|t| !t.is_unknown())
                    .map(|t| t.to_string());
                items.push(completion_item(
                    &sym.name,
                    symbol_kind_to_completion_kind(sym.kind),
                    detail.as_deref(),
                ));
            }
        }
    }

    for kw in TWE_KEYWORDS {
        items.push(completion_item(kw, ck::KEYWORD, None));
    }

    // Stdlib builtins. We bootstrap a fresh `Env`, run
    // `stdlib::install`, and read the resulting binding names.
    // This keeps completion in sync with the runtime: every name
    // a program can call shows up automatically. The cost is one
    // env construction per request; for editor traffic it's
    // negligible and the alternative (a parallel hardcoded list)
    // would drift.
    //
    // Namespaces (`math`, `random`, `key`, `screen`, `time`, …)
    // ship as `Value::Object` whose fields are the namespace
    // members. We surface both forms — the bare namespace
    // (`math`) so the user gets it as soon as they type `m`, and
    // every dotted member (`math.abs`) so completion still works
    // after the dot. VS Code's filtering takes care of the rest.
    let mut env = crate::value::Env::new();
    crate::stdlib::install(&mut env);
    for (name, value) in env.iter_bindings() {
        if value.is_object() {
            let obj_rc = value.as_object();
            items.push(completion_item(&name, ck::VARIABLE, None));
            let obj = obj_rc.borrow();
            for field in obj.fields.keys() {
                items.push(completion_item(
                    &format!("{name}.{field}"),
                    ck::FUNCTION,
                    None,
                ));
            }
        } else {
            items.push(completion_item(&name, ck::FUNCTION, None));
        }
    }

    items
}

fn symbol_kind_to_completion_kind(kind: SymbolKind) -> i64 {
    match kind {
        SymbolKind::Let => ck::CONSTANT,
        SymbolKind::Var => ck::VARIABLE,
        SymbolKind::Function => ck::FUNCTION,
        SymbolKind::Method => ck::METHOD,
        SymbolKind::Field => ck::FIELD,
        SymbolKind::Entity
        | SymbolKind::Item
        | SymbolKind::Modifier
        | SymbolKind::Inventory
        | SymbolKind::Scene
        | SymbolKind::Particles
        | SymbolKind::Visual => ck::CLASS,
        SymbolKind::State | SymbolKind::InitialState => ck::ENUM,
    }
}

fn completion_item(label: &str, kind: i64, detail: Option<&str>) -> Value {
    let mut fields = vec![
        ("label", Value::Str(label.into())),
        ("kind", Value::Int(kind)),
    ];
    if let Some(d) = detail {
        fields.push(("detail", Value::Str(d.into())));
    }
    obj(fields)
}

fn parse_did_open(msg: &Value) -> Option<(String, String)> {
    let td = msg.get("params")?.get("textDocument")?;
    let uri = td.get("uri")?.as_str()?.to_string();
    let text = td.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

fn parse_did_change(msg: &Value) -> Option<(String, String)> {
    let params = msg.get("params")?;
    let uri = params
        .get("textDocument")?
        .get("uri")?
        .as_str()?
        .to_string();
    // Full-sync mode means contentChanges has exactly one entry
    // with a `text` field carrying the entire new document.
    let changes = params.get("contentChanges")?.as_array()?;
    let last = changes.last()?;
    let text = last.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

fn parse_did_close(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("textDocument")?
        .get("uri")?
        .as_str()
        .map(|s| s.to_string())
}

/// Lex + parse `text` and produce LSP `Diagnostic` objects for any
/// errors. Returns an empty Vec when the file parses cleanly. Phase
/// 6 session 1: when the file opts into strict mode (`# strict`
/// directive), strict-mode type errors flow through here too so
/// VS Code shows them inline alongside parse errors.
fn collect_diagnostics(text: &str) -> Vec<Value> {
    let tokens = match lexer::lex(text) {
        Err(e) => return vec![diagnostic_at(e.line, e.col, &e.message)],
        Ok(t) => t,
    };
    let program = match parser::parse(&tokens) {
        Err(e) => return vec![diagnostic_at(e.line, e.col, &e.message)],
        Ok(p) => p,
    };
    if !crate::infer::detect_strict(text) {
        return Vec::new();
    }
    let (_bindings, errors) = crate::infer::infer_program_strict(&program, true);
    errors
        .iter()
        .map(|e| diagnostic_at(e.line, e.col, &format!("type error: {}", e.message)))
        .collect()
}

fn diagnostic_at(line: u32, col: u32, message: &str) -> Value {
    // LSP positions are zero-indexed; Twe's lex/parse errors are
    // one-indexed (matching what `twec run` prints). Subtract 1
    // and saturate at 0 so a position of 0,0 stays 0,0.
    let l = line.saturating_sub(1);
    let c = col.saturating_sub(1);
    let pos = obj([
        ("line", Value::Int(l as i64)),
        ("character", Value::Int(c as i64)),
    ]);
    let range = obj([("start", pos.clone()), ("end", pos)]);
    obj([
        ("range", range),
        ("severity", Value::Int(1)), // 1 = Error per LSP spec.
        ("source", Value::Str("twec".into())),
        ("message", Value::Str(message.to_string())),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Send a single LSP message (with framing) to the server and
    /// collect its byte stream of replies. Convenience for the
    /// table-driven tests below.
    fn lsp_exchange(messages: &[&str]) -> String {
        let mut input = Vec::new();
        for body in messages {
            input.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
            input.extend_from_slice(body.as_bytes());
        }
        let mut output = Vec::new();
        run(Cursor::new(input), &mut output).expect("lsp loop");
        String::from_utf8(output).expect("utf8 output")
    }

    /// Split a raw LSP byte stream into individual message bodies.
    fn split_messages(stream: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = stream.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            // Find the header end (\r\n\r\n).
            let header_end = bytes[pos..]
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|i| pos + i)
                .expect("header end");
            let header = std::str::from_utf8(&bytes[pos..header_end]).unwrap();
            let len: usize = header
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length:").map(|s| s.trim()))
                .and_then(|s| s.parse().ok())
                .expect("content length");
            let body_start = header_end + 4;
            let body_end = body_start + len;
            out.push(
                std::str::from_utf8(&bytes[body_start..body_end])
                    .unwrap()
                    .to_string(),
            );
            pos = body_end;
        }
        out
    }

    #[test]
    fn initialize_then_shutdown_exits_cleanly() {
        let out = lsp_exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
        ]);
        let msgs = split_messages(&out);
        assert_eq!(msgs.len(), 2, "want 2 replies (initialize + shutdown)");
        let init = json::parse(&msgs[0]).expect("init reply");
        assert_eq!(init.get("id").and_then(|v| v.as_i64()), Some(1));
        assert!(init.get("result").is_some());
        let shutdown = json::parse(&msgs[1]).expect("shutdown reply");
        assert_eq!(shutdown.get("id").and_then(|v| v.as_i64()), Some(2));
    }

    #[test]
    fn did_open_with_clean_program_publishes_no_diagnostics() {
        let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///hello.twe","languageId":"twe","version":1,"text":"print(\"hi\")\n"}}}"#;
        let out = lsp_exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            did_open,
            r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
        ]);
        let msgs = split_messages(&out);
        // initialize reply, didOpen → publishDiagnostics, shutdown reply.
        assert_eq!(msgs.len(), 3, "got {} messages", msgs.len());
        let pub_diag = json::parse(&msgs[1]).expect("publishDiagnostics");
        assert_eq!(
            pub_diag.get("method").and_then(|v| v.as_str()),
            Some("textDocument/publishDiagnostics"),
        );
        let diags = pub_diag.get("params").unwrap().get("diagnostics").unwrap();
        assert_eq!(diags.as_array().unwrap().len(), 0);
    }

    #[test]
    fn did_open_with_parse_error_publishes_diagnostic() {
        // Missing closing ')' is a parse error.
        let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///bad.twe","languageId":"twe","version":1,"text":"print(\"oops\n"}}}"#;
        let out = lsp_exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            did_open,
            r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
        ]);
        let msgs = split_messages(&out);
        let pub_diag = json::parse(&msgs[1]).expect("publishDiagnostics");
        let diags = pub_diag
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(diags.len(), 1, "expected one error diagnostic");
        let d = &diags[0];
        assert_eq!(d.get("severity").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(d.get("source").and_then(|v| v.as_str()), Some("twec"));
        assert!(d.get("message").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn did_change_re_parses_and_clears_old_diagnostics() {
        // Open with a parse error, then change to a clean program;
        // second publishDiagnostics should be empty.
        let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///x.twe","languageId":"twe","version":1,"text":"let "}}}"#;
        let did_change = r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///x.twe","version":2},"contentChanges":[{"text":"let x = 5\n"}]}}"#;
        let out = lsp_exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            did_open,
            did_change,
            r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
        ]);
        let msgs = split_messages(&out);
        // initialize reply, two publishDiagnostics, shutdown reply.
        assert_eq!(msgs.len(), 4, "got {} messages", msgs.len());
        let first_diags = json::parse(&msgs[1])
            .unwrap()
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap()
            .len();
        let second_diags = json::parse(&msgs[2])
            .unwrap()
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap()
            .len();
        assert!(first_diags >= 1, "first should have errors");
        assert_eq!(second_diags, 0, "second should be clean");
    }

    // --- Symbol collection + identifier-at ---

    fn parse_to_program(src: &str) -> crate::ast::Program {
        let tokens = lexer::lex(src).expect("lex");
        parser::parse(&tokens).expect("parse")
    }

    #[test]
    fn collect_symbols_picks_up_top_level_decls() {
        let src = "let x = 5\nfunction add(a, b):\n    return a + b\n";
        let syms = collect_symbols(&parse_to_program(src));
        let names: Vec<_> = syms.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(
            names,
            vec![("x", SymbolKind::Let), ("add", SymbolKind::Function)],
        );
    }

    #[test]
    fn collect_symbols_walks_into_classes() {
        let src = "item Counter:\n    value: 0\n\n    bump(amount):\n        self.value = self.value + amount\n";
        let syms = collect_symbols(&parse_to_program(src));
        let kinds: Vec<_> = syms.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("Counter", SymbolKind::Item),
                ("value", SymbolKind::Field),
                ("bump", SymbolKind::Method),
            ],
        );
    }

    #[test]
    fn collect_symbols_walks_into_scenes_and_states() {
        let src = "scene S:\n    var n = 0\n    initial: a\n    state a:\n        every 100ms:\n            n += 1\n    state done:\n";
        let syms = collect_symbols(&parse_to_program(src));
        let kinds: Vec<_> = syms.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert!(kinds.contains(&("S", SymbolKind::Scene)));
        assert!(kinds.contains(&("n", SymbolKind::Field)));
        assert!(kinds.contains(&("a", SymbolKind::InitialState)));
        assert!(kinds.contains(&("a", SymbolKind::State)));
        assert!(kinds.contains(&("done", SymbolKind::State)));
    }

    #[test]
    fn identifier_at_finds_word_under_cursor() {
        let text = "let foo = 5\nprint(foo)\n";
        // Cursor on `foo` of let:
        assert_eq!(identifier_at(text, 0, 4).as_deref(), Some("foo"));
        assert_eq!(identifier_at(text, 0, 5).as_deref(), Some("foo"));
        assert_eq!(identifier_at(text, 0, 6).as_deref(), Some("foo"));
        // Cursor immediately after `foo` (boundary): should still match.
        assert_eq!(identifier_at(text, 0, 7).as_deref(), Some("foo"));
        // Cursor on `foo` of print arg:
        assert_eq!(identifier_at(text, 1, 7).as_deref(), Some("foo"));
        // Cursor on `print`:
        assert_eq!(identifier_at(text, 1, 0).as_deref(), Some("print"));
        // Cursor on `=` (well past the previous word, no id at idx):
        assert_eq!(identifier_at(text, 0, 8).as_deref(), None);
    }

    #[test]
    fn identifier_at_handles_underscore_and_digits() {
        let text = "let _hello_2 = 1\n";
        assert_eq!(identifier_at(text, 0, 4).as_deref(), Some("_hello_2"));
        assert_eq!(identifier_at(text, 0, 11).as_deref(), Some("_hello_2"));
    }

    #[test]
    fn identifier_at_returns_none_off_line() {
        let text = "let x = 5\n";
        assert_eq!(identifier_at(text, 99, 0), None);
    }

    // --- LSP definition + hover round-trip ---

    fn lsp_with_doc(uri: &str, text: &str, extra: &[&str]) -> Vec<String> {
        let did_open = format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"twe","version":1,"text":{}}}}}}}"#,
            json::to_string(&Value::Str(text.to_string()))
        );
        let mut messages: Vec<String> = vec![
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_string(),
            did_open,
        ];
        for m in extra {
            messages.push((*m).to_string());
        }
        messages.push(r#"{"jsonrpc":"2.0","id":99,"method":"shutdown"}"#.to_string());
        messages.push(r#"{"jsonrpc":"2.0","method":"exit"}"#.to_string());
        let refs: Vec<&str> = messages.iter().map(|s| s.as_str()).collect();
        let stream = lsp_exchange(&refs);
        split_messages(&stream)
    }

    #[test]
    fn definition_resolves_top_level_let() {
        let text = "let foo = 5\nprint(foo)\n";
        // Cursor on `foo` in print(foo) → line 1, col 6 (zero-indexed).
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///t.twe"},"position":{"line":1,"character":6}}}"#;
        let msgs = lsp_with_doc("file:///t.twe", text, &[req]);
        // Order: initialize reply, didOpen → publishDiagnostics,
        // definition reply, shutdown reply.
        let def_reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("definition reply");
        let v = json::parse(def_reply).expect("parse def reply");
        let result = v.get("result").expect("result field");
        assert_eq!(
            result.get("uri").and_then(|x| x.as_str()),
            Some("file:///t.twe")
        );
        let range = result.get("range").expect("range");
        let start = range.get("start").unwrap();
        // `let foo = 5` — `foo` starts at col 4 (0-indexed) on line 0.
        assert_eq!(start.get("line").and_then(|v| v.as_i64()), Some(0));
        assert_eq!(start.get("character").and_then(|v| v.as_i64()), Some(4));
    }

    #[test]
    fn definition_returns_null_when_identifier_not_found() {
        let text = "let foo = 5\n";
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///t.twe"},"position":{"line":0,"character":2}}}"#;
        let msgs = lsp_with_doc("file:///t.twe", text, &[req]);
        let def_reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("definition reply");
        let v = json::parse(def_reply).expect("parse");
        // Cursor is on `let` (a keyword, not a user-declared name)
        // → should resolve to None and reply with `result: null`.
        assert_eq!(v.get("result"), Some(&Value::Null));
    }

    #[test]
    fn hover_returns_markdown_for_function() {
        let text = "function greet():\n    print(\"hi\")\n\ngreet()\n";
        // Cursor on `greet` of the call site: line 3 col 2.
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.twe"},"position":{"line":3,"character":2}}}"#;
        let msgs = lsp_with_doc("file:///t.twe", text, &[req]);
        let hover_reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("hover reply");
        let v = json::parse(hover_reply).expect("parse");
        let contents = v.get("result").unwrap().get("contents").unwrap();
        assert_eq!(
            contents.get("kind").and_then(|v| v.as_str()),
            Some("markdown")
        );
        let body = contents
            .get("value")
            .and_then(|v| v.as_str())
            .expect("value");
        assert!(body.contains("function greet"), "got: {body}");
    }

    // --- 4g: hover shows inferred type ---

    #[test]
    fn hover_includes_inferred_let_type() {
        // `let n = 42` — hover on `n` (or any reference to it)
        // should show its inferred type (`int`).
        let text = "let n = 42\nprint(n)\n";
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.twe"},"position":{"line":1,"character":6}}}"#;
        let msgs = lsp_with_doc("file:///t.twe", text, &[req]);
        let hover_reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("hover reply");
        let v = json::parse(hover_reply).expect("parse");
        let body = v
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .get("value")
            .and_then(|s| s.as_str())
            .expect("value");
        // The body is markdown; it should contain the type.
        assert!(body.contains("int"), "got: {body}");
        assert!(body.contains("n"), "got: {body}");
    }

    #[test]
    fn hover_includes_function_signature() {
        // `function add(a, b): return a + b` — hover on `add`
        // should show `function(int, int) -> int` from inference.
        let text = "function add(a, b):\n    return a + b\n\nlet r = add(1, 2)\n";
        // Cursor on the call-site `add` at line 3, col 8.
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.twe"},"position":{"line":3,"character":9}}}"#;
        let msgs = lsp_with_doc("file:///t.twe", text, &[req]);
        let hover_reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("hover reply");
        let v = json::parse(hover_reply).expect("parse");
        let body = v
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .get("value")
            .and_then(|s| s.as_str())
            .expect("value");
        assert!(body.contains("function(int, int) -> int"), "got: {body}");
    }

    #[test]
    fn hover_falls_back_when_type_unknown() {
        // A name with an unresolvable type should still produce
        // a hover (just without the `: type` suffix). Use an
        // ident referencing a non-existent name so inference
        // returns Unknown.
        let text = "let mystery = unresolved_thing\nprint(mystery)\n";
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.twe"},"position":{"line":1,"character":7}}}"#;
        let msgs = lsp_with_doc("file:///t.twe", text, &[req]);
        let hover_reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("hover reply");
        let v = json::parse(hover_reply).expect("parse");
        let body = v
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .get("value")
            .and_then(|s| s.as_str())
            .expect("value");
        assert!(body.contains("mystery"), "got: {body}");
        // Unknown shouldn't render as `: ?` — when the type is
        // useless, we drop the type clause entirely.
        assert!(!body.contains(": ?"), "got: {body}");
    }

    #[test]
    fn initialize_advertises_definition_and_hover() {
        let out = lsp_exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
        ]);
        let init = json::parse(&split_messages(&out)[0]).expect("init");
        let caps = init.get("result").unwrap().get("capabilities").unwrap();
        assert_eq!(caps.get("definitionProvider"), Some(&Value::Bool(true)));
        assert_eq!(caps.get("hoverProvider"), Some(&Value::Bool(true)));
    }

    // --- Phase 5 entry: completion ---

    fn completion_labels(items: &Value) -> Vec<String> {
        items
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.get("label").and_then(|s| s.as_str()).map(str::to_string))
            .collect()
    }

    #[test]
    fn initialize_advertises_completion_provider() {
        let out = lsp_exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
        ]);
        let init = json::parse(&split_messages(&out)[0]).expect("init");
        let caps = init.get("result").unwrap().get("capabilities").unwrap();
        assert!(caps.get("completionProvider").is_some());
    }

    #[test]
    fn completion_includes_user_let_with_inferred_type() {
        // `let n = 42` — completion should offer `n` with detail `int`.
        let text = "let n = 42\n";
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///c.twe"},"position":{"line":1,"character":0}}}"#;
        let msgs = lsp_with_doc("file:///c.twe", text, &[req]);
        let reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("completion reply");
        let v = json::parse(reply).expect("parse");
        let items = v.get("result").unwrap().get("items").unwrap();
        let labels = completion_labels(items);
        assert!(labels.contains(&"n".to_string()), "labels: {labels:?}");
        // Find the `n` item and confirm the detail carries the type.
        let n_item = items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i.get("label").and_then(|s| s.as_str()) == Some("n"))
            .expect("n item");
        let detail = n_item
            .get("detail")
            .and_then(|s| s.as_str())
            .expect("detail");
        assert!(detail.contains("int"), "detail: {detail}");
    }

    #[test]
    fn completion_includes_keywords_and_stdlib() {
        let text = "let x = 1\n";
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///c.twe"},"position":{"line":1,"character":0}}}"#;
        let msgs = lsp_with_doc("file:///c.twe", text, &[req]);
        let reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("completion reply");
        let v = json::parse(reply).expect("parse");
        let labels = completion_labels(v.get("result").unwrap().get("items").unwrap());
        // Spot-check a keyword and a few stdlib names.
        assert!(
            labels.contains(&"function".to_string()),
            "no `function` keyword"
        );
        assert!(labels.contains(&"for".to_string()), "no `for` keyword");
        assert!(labels.contains(&"print".to_string()), "no `print` builtin");
        assert!(labels.contains(&"load".to_string()), "no `load` builtin");
        assert!(
            labels.contains(&"math.abs".to_string()),
            "no `math.abs` builtin"
        );
    }

    #[test]
    fn completion_survives_unparseable_file() {
        // User mid-typing — file doesn't parse — completion still
        // emits keywords + stdlib so they have something to pick.
        let text = "let "; // partial input
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///broken.twe"},"position":{"line":0,"character":4}}}"#;
        let msgs = lsp_with_doc("file:///broken.twe", text, &[req]);
        let reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("completion reply");
        let v = json::parse(reply).expect("parse");
        let labels = completion_labels(v.get("result").unwrap().get("items").unwrap());
        assert!(labels.contains(&"function".to_string()));
        assert!(labels.contains(&"print".to_string()));
    }

    #[test]
    fn completion_returns_null_for_unknown_uri() {
        // Completion requested for a URI we never opened — reply
        // should be `null` (graceful) rather than a server error.
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///nope.twe"},"position":{"line":0,"character":0}}}"#;
        let out = lsp_exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            req,
            r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
        ]);
        let msgs = split_messages(&out);
        let reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("completion reply");
        let v = json::parse(reply).expect("parse");
        assert_eq!(v.get("result"), Some(&Value::Null));
    }

    #[test]
    fn completion_marks_user_function_with_function_kind() {
        // Different SymbolKinds map to different LSP CompletionItemKinds —
        // sanity-check the function path.
        let text = "function greet():\n    return 0\n";
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///c.twe"},"position":{"line":2,"character":0}}}"#;
        let msgs = lsp_with_doc("file:///c.twe", text, &[req]);
        let reply = msgs
            .iter()
            .find(|m| m.contains(r#""id":2"#))
            .expect("completion reply");
        let v = json::parse(reply).expect("parse");
        let items = v
            .get("result")
            .unwrap()
            .get("items")
            .unwrap()
            .as_array()
            .unwrap();
        let greet = items
            .iter()
            .find(|i| i.get("label").and_then(|s| s.as_str()) == Some("greet"))
            .expect("greet item");
        // ck::FUNCTION = 3
        assert_eq!(greet.get("kind").and_then(|v| v.as_i64()), Some(3));
    }

    #[test]
    fn unknown_method_request_returns_method_not_found() {
        // Pick a method we haven't implemented (rename is way beyond
        // current scope); confirm we reply MethodNotFound rather
        // than letting the client hang.
        let unknown = r#"{"jsonrpc":"2.0","id":42,"method":"textDocument/rename","params":{}}"#;
        let out = lsp_exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            unknown,
            r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
        ]);
        let msgs = split_messages(&out);
        let err_reply = json::parse(&msgs[1]).expect("err reply");
        assert_eq!(err_reply.get("id").and_then(|v| v.as_i64()), Some(42));
        let err = err_reply.get("error").expect("error field");
        assert_eq!(err.get("code").and_then(|v| v.as_i64()), Some(-32601));
    }
}
