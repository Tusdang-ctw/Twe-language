//! Phase 33 session 5: stdio Model Context Protocol server.
//!
//! Exposes Twe's existing tools — parse, verify, format, grammar,
//! stdlib lookup, patch application — over JSON-RPC 2.0 on stdin /
//! stdout. Once running, any MCP client (Claude Desktop, Cursor,
//! the future Twe Studio) can drive Twe with one config paste:
//!
//! ```json
//! { "mcpServers": { "twe": { "command": "twec", "args": ["mcp"] } } }
//! ```
//!
//! ## Why stdio + newline framing
//!
//! The MCP spec defines two transports. Stdio is the right shape for
//! a CLI binary like `twec`: the client spawns us, pipes JSON-RPC
//! messages through the pipe, and reads replies the same way. Each
//! message is a single JSON object on its own line — simpler than
//! LSP's `Content-Length` framing and, for the message sizes Twe
//! deals in, equally fast. HTTP transport is a follow-on if a
//! networked client needs it.
//!
//! ## Tool surface
//!
//! Every tool reuses an existing entry point — no new logic lives
//! in this module, only adapters. That keeps the MCP boundary cheap
//! to maintain: a new feature in `verify` shows up in MCP for free
//! by editing the adapter signature. Current tools:
//!
//! - `parse(source)` — lex + parse, return AST as JSON
//! - `verify(source, file?, warn_deprecated?)` — same JSON v2 shape
//!   as `twec verify`, including the new `fix` field
//! - `format(source)` — canonical pretty-print
//! - `grammar(format)` — export grammar in gbnf / json-schema / ebnf
//! - `stdlib_list(category?)` — full or filtered manifest
//! - `stdlib_lookup(name)` — one BuiltinSpec by name (or null)
//! - `apply_patch(source, edits)` — apply structured edits to text
//!   (the round-trip primitive an LLM uses to consume a verify fix)

use std::io::{BufRead, BufWriter, Write};

use crate::json::{self, Value};

/// Server protocol + implementation versions reported in the
/// `initialize` reply. Bump when the tool surface or schemas
/// change in a way clients should be aware of.
const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "twec-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Public entry — used by `cli::handle_mcp`.
// ---------------------------------------------------------------------------

/// Run the MCP server reading from `stdin` and writing to `stdout`.
/// Blocks until EOF on stdin or the client sends a `shutdown`. The
/// return value is the process exit code.
pub fn serve_stdio() -> i32 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    let mut handler = Handler::new();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[twec mcp] stdin read failed: {e}");
                return 1;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = handler.handle(&line);
        if let Some(text) = response {
            if writeln!(writer, "{text}").is_err() {
                return 1;
            }
            if writer.flush().is_err() {
                return 1;
            }
        }
        if handler.should_exit() {
            break;
        }
    }
    0
}

/// Single-message dispatch entry, exposed for tests so the server
/// can be exercised without spawning a subprocess. Returns the
/// reply line (or `None` for notifications, which JSON-RPC does
/// not reply to).
pub fn handle_message(msg: &str) -> Option<String> {
    let mut h = Handler::new();
    h.handle(msg)
}

// ---------------------------------------------------------------------------
// Handler — owns server-side state (initialised flag, exit signal).
// ---------------------------------------------------------------------------

struct Handler {
    initialised: bool,
    shutdown_received: bool,
    should_exit: bool,
}

impl Handler {
    fn new() -> Self {
        Self {
            initialised: false,
            shutdown_received: false,
            should_exit: false,
        }
    }

    fn should_exit(&self) -> bool {
        self.should_exit
    }

    fn handle(&mut self, line: &str) -> Option<String> {
        let parsed = match json::parse(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_response(
                    Value::Null,
                    -32700,
                    &format!("parse error: {e}"),
                ));
            }
        };
        let method = parsed.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = parsed.get("id").cloned().unwrap_or(Value::Null);
        let is_notification = matches!(id, Value::Null);
        let params = parsed.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => Some(self.handle_initialize(id, &params)),
            "initialized" | "notifications/initialized" => {
                // Per MCP spec, this is a notification — no reply.
                self.initialised = true;
                None
            }
            "shutdown" => {
                self.shutdown_received = true;
                Some(success_response(id, Value::Null))
            }
            "exit" => {
                self.should_exit = true;
                None
            }
            "tools/list" => Some(success_response(id, tools_list())),
            "tools/call" => Some(self.handle_tools_call(id, &params)),
            // Grounding surface: the language guide, grammar, and curated
            // examples as read-only context the model can pull on demand.
            "resources/list" => Some(success_response(id, resources_list())),
            "resources/read" => Some(self.handle_resources_read(id, &params)),
            // User-selectable prompt templates that preload the right grounding
            // for the common authoring tasks (scaffold, add feature, fix errors).
            "prompts/list" => Some(success_response(id, prompts_list())),
            "prompts/get" => Some(self.handle_prompts_get(id, &params)),
            other => {
                if is_notification {
                    None
                } else {
                    Some(error_response(
                        id,
                        -32601,
                        &format!("method not found: {other}"),
                    ))
                }
            }
        }
    }

    fn handle_initialize(&mut self, id: Value, _params: &Value) -> String {
        // We don't enforce the client-side `initialize` happens before
        // tool calls — being lenient lets `tests/mcp.rs` drive raw
        // requests without a handshake. Production clients always do
        // the handshake first.
        self.initialised = true;
        let result = json::obj([
            ("protocolVersion", Value::Str(PROTOCOL_VERSION.into())),
            (
                "capabilities",
                json::obj([
                    ("tools", json::obj([("listChanged", Value::Bool(false))])),
                    ("resources", json::obj([("listChanged", Value::Bool(false))])),
                    ("prompts", json::obj([("listChanged", Value::Bool(false))])),
                ]),
            ),
            (
                "serverInfo",
                json::obj([
                    ("name", Value::Str(SERVER_NAME.into())),
                    ("version", Value::Str(SERVER_VERSION.into())),
                ]),
            ),
            // The single most important field for a custom language: hosts
            // inject this into the model's system prompt at connect time, so a
            // client that knows nothing about Twe is grounded before its first
            // turn. Without it the model writes Python-shaped guesses. The full
            // guide + examples are available as `twe://` resources.
            ("instructions", Value::Str(crate::primer::INSTRUCTIONS.into())),
        ]);
        success_response(id, result)
    }

    fn handle_tools_call(&self, id: Value, params: &Value) -> String {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);
        match dispatch_tool(name, &args) {
            Ok(content) => success_response(id, wrap_content(content)),
            Err((code, msg)) => error_response(id, code, &msg),
        }
    }

    fn handle_resources_read(&self, id: Value, params: &Value) -> String {
        let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        match resource_read(uri) {
            Ok(contents) => success_response(id, json::obj([("contents", Value::Array(vec![contents]))])),
            Err((code, msg)) => error_response(id, code, &msg),
        }
    }

    fn handle_prompts_get(&self, id: Value, params: &Value) -> String {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);
        match prompt_get(name, &args) {
            Ok(result) => success_response(id, result),
            Err((code, msg)) => error_response(id, code, &msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool dispatch — pure adapters around existing Twe APIs.
// ---------------------------------------------------------------------------

/// Dispatch one tool call. Returns the raw result Value to be wrapped
/// by `wrap_content`, or `(code, message)` for an error.
fn dispatch_tool(name: &str, args: &Value) -> Result<Value, (i64, String)> {
    match name {
        "parse" => tool_parse(args),
        "verify" => tool_verify(args),
        "format" => tool_format(args),
        "grammar" => tool_grammar(args),
        "stdlib_list" => Ok(tool_stdlib_list(args)),
        "stdlib_lookup" => Ok(tool_stdlib_lookup(args)),
        "apply_patch" => tool_apply_patch(args),
        other => Err((-32602, format!("unknown tool: {other}"))),
    }
}

fn tool_parse(args: &Value) -> Result<Value, (i64, String)> {
    let source = required_string(args, "source")?;
    let tokens = match crate::lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => {
            return Ok(json::obj([
                ("ok", Value::Bool(false)),
                ("stage", Value::Str("lex".into())),
                ("line", Value::Int(e.line as i64)),
                ("col", Value::Int(e.col as i64)),
                ("message", Value::Str(e.message)),
            ]));
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            return Ok(json::obj([
                ("ok", Value::Bool(false)),
                ("stage", Value::Str("parse".into())),
                ("line", Value::Int(e.line as i64)),
                ("col", Value::Int(e.col as i64)),
                ("message", Value::Str(e.message)),
            ]));
        }
    };
    // Wrap the existing AST JSON emitter — its output is already a
    // JSON document; round-trip through our parser to get a typed
    // Value so we can nest it.
    let ast_text = crate::ast_json::to_json(&program);
    let ast = json::parse(&ast_text)
        .map_err(|e| (-32603, format!("ast emitter produced invalid json: {e}")))?;
    Ok(json::obj([("ok", Value::Bool(true)), ("ast", ast)]))
}

fn tool_verify(args: &Value) -> Result<Value, (i64, String)> {
    let source = required_string(args, "source")?;
    let path = optional_string(args, "file");
    let warn_deprecated = args
        .get("warn_deprecated")
        .and_then(|v| match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        })
        .unwrap_or(false);
    let options = crate::verify::VerifyOptions { warn_deprecated };
    let report =
        crate::verify::verify_program_with_options(&source, path.as_deref(), &options);
    let report_text = report.to_json();
    json::parse(&report_text)
        .map_err(|e| (-32603, format!("verify report invalid json: {e}")))
}

fn tool_format(args: &Value) -> Result<Value, (i64, String)> {
    let source = required_string(args, "source")?;
    let tokens = crate::lexer::lex(&source).map_err(|e| (-32602, format!("lex: {}", e.message)))?;
    let program =
        crate::parser::parse(&tokens).map_err(|e| (-32602, format!("parse: {}", e.message)))?;
    let formatted = crate::printer::print_program_with_trivia(&program, &source);
    Ok(json::obj([("formatted", Value::Str(formatted))]))
}

fn tool_grammar(args: &Value) -> Result<Value, (i64, String)> {
    let format_arg = optional_string(args, "format").unwrap_or_else(|| "gbnf".to_string());
    let format = crate::grammar::Format::parse(&format_arg)
        .ok_or_else(|| (-32602, format!("unknown grammar format: {format_arg}")))?;
    let body = crate::grammar::export(format);
    Ok(json::obj([
        ("format", Value::Str(format_arg)),
        ("grammar", Value::Str(body)),
    ]))
}

fn tool_stdlib_list(args: &Value) -> Value {
    let category = optional_string(args, "category");
    let manifest = crate::stdlib::manifest();
    let filtered: Vec<&crate::stdlib::BuiltinSpec> = match category.as_deref() {
        Some(c) => manifest.iter().filter(|s| s.category == c).collect(),
        None => manifest.iter().collect(),
    };
    let body = crate::stdlib::manifest_to_json(&filtered);
    // Round-trip through our parser to nest as a Value.
    json::parse(&body).unwrap_or(Value::Null)
}

fn tool_stdlib_lookup(args: &Value) -> Value {
    let Some(name) = optional_string(args, "name") else {
        return Value::Null;
    };
    let manifest = crate::stdlib::manifest();
    match manifest.iter().find(|s| s.name == name) {
        Some(spec) => spec_to_json(spec),
        None => Value::Null,
    }
}

fn tool_apply_patch(args: &Value) -> Result<Value, (i64, String)> {
    let source = required_string(args, "source")?;
    let edits_v = args
        .get("edits")
        .and_then(|v| v.as_array())
        .ok_or((-32602, "missing `edits` array".to_string()))?;
    let mut edits: Vec<crate::verify::Edit> = Vec::new();
    for e in edits_v {
        let line = e.get("line").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
        let col = e.get("col").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
        let len = e.get("len").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
        let replace = e
            .get("replace")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if line == 0 || col == 0 {
            return Err((-32602, format!("edit anchor must be 1-based: {e:?}")));
        }
        edits.push(crate::verify::Edit {
            line,
            col,
            len,
            replace,
        });
    }
    let patched = apply_edits(&source, &edits);
    Ok(json::obj([("source", Value::Str(patched))]))
}

/// Apply edits right-to-left so earlier offsets aren't perturbed by
/// later applications. Mirrors the helper in `tests/verify_v2.rs`.
fn apply_edits(src: &str, edits: &[crate::verify::Edit]) -> String {
    let mut byte_edits: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let start = line_col_to_byte(src, e.line, e.col);
            (start, start + e.len as usize, e.replace.as_str())
        })
        .collect();
    byte_edits.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
    let mut out = src.to_string();
    for (start, end, repl) in byte_edits {
        let end = end.min(out.len());
        let start = start.min(end);
        out.replace_range(start..end, repl);
    }
    out
}

fn line_col_to_byte(src: &str, line: u32, col: u32) -> usize {
    let mut current_line = 1u32;
    let mut line_start = 0usize;
    for (i, b) in src.bytes().enumerate() {
        if current_line == line {
            return line_start + (col as usize - 1);
        }
        if b == b'\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }
    if current_line == line {
        line_start + (col as usize - 1)
    } else {
        src.len()
    }
}

fn spec_to_json(spec: &crate::stdlib::BuiltinSpec) -> Value {
    let params: Vec<Value> = spec.params.iter().map(|p| Value::Str(p.clone())).collect();
    json::obj([
        ("name", Value::Str(spec.name.clone())),
        ("category", Value::Str(spec.category.clone())),
        ("params", Value::Array(params)),
        (
            "doc",
            spec.doc
                .as_ref()
                .map(|s| Value::Str(s.clone()))
                .unwrap_or(Value::Null),
        ),
        (
            "since",
            spec.since
                .as_ref()
                .map(|s| Value::Str(s.clone()))
                .unwrap_or(Value::Null),
        ),
        ("deprecated", Value::Bool(spec.deprecated)),
    ])
}

// ---------------------------------------------------------------------------
// Tools/list — describes the surface for clients.
// ---------------------------------------------------------------------------

fn tools_list() -> Value {
    let tools: Vec<Value> = vec![
        tool_descriptor(
            "parse",
            "Lex + parse a Twe source. Returns {ok, ast} on success or {ok: false, stage, line, col, message} on error.",
            &[("source", "Twe source code", true)],
        ),
        tool_descriptor(
            "verify",
            "Run the strict / verified inferer on Twe source. Returns the canonical verify JSON v2 (with structured `fix` patches on high-confidence diagnostics).",
            &[
                ("source", "Twe source code", true),
                ("file", "Optional file path for diagnostic anchoring", false),
                ("warn_deprecated", "Boolean — emit deprecation warnings", false),
            ],
        ),
        tool_descriptor(
            "format",
            "Pretty-print Twe source in canonical form. Returns {formatted}.",
            &[("source", "Twe source code", true)],
        ),
        tool_descriptor(
            "grammar",
            "Export the Twe grammar in `gbnf` (default), `json-schema`, or `ebnf` format. Returns {format, grammar}.",
            &[("format", "gbnf | json-schema | ebnf", false)],
        ),
        tool_descriptor(
            "stdlib_list",
            "Enumerate the stdlib manifest, optionally filtered by category. Returns the same payload as `twec stdlib --json`.",
            &[("category", "Filter by category (math, draw, ui, ...)", false)],
        ),
        tool_descriptor(
            "stdlib_lookup",
            "Look up one stdlib builtin by canonical name. Returns BuiltinSpec or null.",
            &[("name", "Canonical builtin name (e.g. math.sqrt)", true)],
        ),
        tool_descriptor(
            "apply_patch",
            "Apply a list of structured edits to a source string. Returns {source}. Edits are 1-based line/col anchors with byte length to replace.",
            &[
                ("source", "Source text", true),
                ("edits", "Array of {line, col, len, replace}", true),
            ],
        ),
    ];
    json::obj([("tools", Value::Array(tools))])
}

fn tool_descriptor(name: &str, description: &str, params: &[(&str, &str, bool)]) -> Value {
    let mut props_pairs: Vec<(String, Value)> = Vec::new();
    let mut required: Vec<Value> = Vec::new();
    for (param, desc, is_required) in params {
        let mut kind: Value = Value::Str("string".into());
        // Heuristic: if the description starts with "Boolean" mark the
        // schema type as boolean, "Array" as array. Keeps the descriptor
        // honest without a heavier type system.
        if desc.starts_with("Boolean") {
            kind = Value::Str("boolean".into());
        } else if desc.starts_with("Array") {
            kind = Value::Str("array".into());
        }
        props_pairs.push((
            (*param).to_string(),
            Value::Object(
                [
                    ("type".to_string(), kind),
                    ("description".to_string(), Value::Str((*desc).to_string())),
                ]
                .into_iter()
                .collect(),
            ),
        ));
        if *is_required {
            required.push(Value::Str((*param).to_string()));
        }
    }
    let schema = Value::Object(
        [
            ("type".to_string(), Value::Str("object".into())),
            (
                "properties".to_string(),
                Value::Object(props_pairs.into_iter().collect()),
            ),
            ("required".to_string(), Value::Array(required)),
        ]
        .into_iter()
        .collect(),
    );
    json::obj([
        ("name", Value::Str(name.to_string())),
        ("description", Value::Str(description.to_string())),
        ("inputSchema", schema),
    ])
}

// ---------------------------------------------------------------------------
// Resources — read-only grounding context the model can pull on demand.
// ---------------------------------------------------------------------------

const GUIDE_URI: &str = "twe://guide";
const GRAMMAR_EBNF_URI: &str = "twe://grammar";
const GRAMMAR_GBNF_URI: &str = "twe://grammar.gbnf";

fn resource_descriptor(uri: &str, name: &str, description: &str, mime: &str) -> Value {
    json::obj([
        ("uri", Value::Str(uri.to_string())),
        ("name", Value::Str(name.to_string())),
        ("description", Value::Str(description.to_string())),
        ("mimeType", Value::Str(mime.to_string())),
    ])
}

fn resources_list() -> Value {
    let mut resources = vec![
        resource_descriptor(
            GUIDE_URI,
            "Twe language guide",
            "The canonical Twe authoring cheatsheet: syntax, semantics, the six block keywords, events, state machines, and a full worked example. Read this before writing Twe.",
            "text/markdown",
        ),
        resource_descriptor(
            GRAMMAR_EBNF_URI,
            "Twe grammar (EBNF)",
            "The exact Twe grammar in EBNF — the parser contract.",
            "text/plain",
        ),
        resource_descriptor(
            GRAMMAR_GBNF_URI,
            "Twe grammar (GBNF)",
            "The Twe grammar in GBNF, for grammar-constrained decoding on local models.",
            "text/plain",
        ),
    ];
    for ex in crate::primer::EXAMPLES {
        resources.push(resource_descriptor(
            &format!("twe://examples/{}", ex.name),
            &format!("Example: {}", ex.name),
            ex.description,
            "text/plain",
        ));
    }
    json::obj([("resources", Value::Array(resources))])
}

/// Resolve a `twe://` URI to its `{uri, mimeType, text}` content part.
fn resource_read(uri: &str) -> Result<Value, (i64, String)> {
    let (mime, text) = match uri {
        GUIDE_URI => ("text/markdown", crate::primer::guide().to_string()),
        GRAMMAR_EBNF_URI => {
            let fmt = crate::grammar::Format::parse("ebnf").unwrap();
            ("text/plain", crate::grammar::export(fmt))
        }
        GRAMMAR_GBNF_URI => {
            let fmt = crate::grammar::Format::parse("gbnf").unwrap();
            ("text/plain", crate::grammar::export(fmt))
        }
        other => {
            let name = other.strip_prefix("twe://examples/").ok_or((
                -32602,
                format!("unknown resource uri: {other}"),
            ))?;
            let ex = crate::primer::example(name)
                .ok_or((-32602, format!("unknown example: {name}")))?;
            ("text/plain", ex.source.to_string())
        }
    };
    Ok(json::obj([
        ("uri", Value::Str(uri.to_string())),
        ("mimeType", Value::Str(mime.into())),
        ("text", Value::Str(text)),
    ]))
}

// ---------------------------------------------------------------------------
// Prompts — user-selectable templates that preload the right grounding.
// ---------------------------------------------------------------------------

fn prompt_argument(name: &str, description: &str, required: bool) -> Value {
    json::obj([
        ("name", Value::Str(name.to_string())),
        ("description", Value::Str(description.to_string())),
        ("required", Value::Bool(required)),
    ])
}

fn prompt_descriptor(name: &str, description: &str, arguments: Vec<Value>) -> Value {
    json::obj([
        ("name", Value::Str(name.to_string())),
        ("description", Value::Str(description.to_string())),
        ("arguments", Value::Array(arguments)),
    ])
}

fn prompts_list() -> Value {
    let prompts = vec![
        prompt_descriptor(
            "scaffold_game",
            "Scaffold a complete, verified Twe program from a description.",
            vec![prompt_argument("description", "What the game should be (genre, core mechanic, controls).", true)],
        ),
        prompt_descriptor(
            "add_feature",
            "Add or change a feature in an existing Twe file, returning a verified result.",
            vec![
                prompt_argument("request", "The change to make, in plain language.", true),
                prompt_argument("source", "The current .twe source to modify.", false),
            ],
        ),
        prompt_descriptor(
            "fix_errors",
            "Fix the verify errors in a Twe file until it passes.",
            vec![prompt_argument("source", "The .twe source that currently fails `verify`.", true)],
        ),
    ];
    json::obj([("prompts", Value::Array(prompts))])
}

/// A `prompts/get` result: a description plus the user-role messages the client
/// drops into the conversation. Every template leads with the primer so the
/// model is grounded even if the host did not surface the `instructions` field.
fn prompt_get(name: &str, args: &Value) -> Result<Value, (i64, String)> {
    let arg = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();

    let (description, body) = match name {
        "scaffold_game" => {
            let desc = arg("description");
            if desc.is_empty() {
                return Err((-32602, "scaffold_game requires a `description` argument".into()));
            }
            (
                "Scaffold a complete, verified Twe program.".to_string(),
                format!(
                    "Write a complete Twe program for this game:\n\n{desc}\n\n\
                     Confirm every stdlib call with `stdlib_lookup`, then run `verify` and \
                     apply its `fix` patches until it reports zero errors. Return only the \
                     final verified `.twe` source in a ```twe code block. Consult the \
                     `twe://guide` resource and `twe://examples/<name>` as needed."
                ),
            )
        }
        "add_feature" => {
            let request = arg("request");
            if request.is_empty() {
                return Err((-32602, "add_feature requires a `request` argument".into()));
            }
            let source = arg("source");
            let src_block = if source.is_empty() {
                "Open the file the user is working on first.".to_string()
            } else {
                format!("Current source:\n```twe\n{source}\n```")
            };
            (
                "Add a feature to an existing Twe file.".to_string(),
                format!(
                    "{src_block}\n\nChange requested: {request}\n\n\
                     Make the smallest correct edit. Confirm any new stdlib call with \
                     `stdlib_lookup`, then `verify` and fix until clean. Return the verified \
                     full source in a ```twe code block."
                ),
            )
        }
        "fix_errors" => {
            let source = arg("source");
            if source.is_empty() {
                return Err((-32602, "fix_errors requires a `source` argument".into()));
            }
            (
                "Fix verify errors in a Twe file.".to_string(),
                format!(
                    "This Twe source fails `verify`:\n```twe\n{source}\n```\n\n\
                     Run `verify`, apply its structured `fix` patches (or correct the syntax \
                     against the `twe://guide`), and repeat until `verify` reports zero errors. \
                     Return the verified full source in a ```twe code block."
                ),
            )
        }
        other => return Err((-32602, format!("unknown prompt: {other}"))),
    };

    let text = format!("{}\n\n---\n\n{body}", crate::primer::INSTRUCTIONS);
    let message = json::obj([
        ("role", Value::Str("user".into())),
        (
            "content",
            json::obj([
                ("type", Value::Str("text".into())),
                ("text", Value::Str(text)),
            ]),
        ),
    ]);
    Ok(json::obj([
        ("description", Value::Str(description)),
        ("messages", Value::Array(vec![message])),
    ]))
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

fn success_response(id: Value, result: Value) -> String {
    let env = json::obj([
        ("jsonrpc", Value::Str("2.0".into())),
        ("id", id),
        ("result", result),
    ]);
    json::to_string(&env)
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    let err = json::obj([
        ("code", Value::Int(code)),
        ("message", Value::Str(message.to_string())),
    ]);
    let env = json::obj([
        ("jsonrpc", Value::Str("2.0".into())),
        ("id", id),
        ("error", err),
    ]);
    json::to_string(&env)
}

/// MCP `tools/call` results are wrapped in a `content` array of
/// typed parts (text, image, etc.). We always emit one text part
/// containing the JSON-stringified payload — clients that want
/// structured access parse it back.
fn wrap_content(payload: Value) -> Value {
    let part = json::obj([
        ("type", Value::Str("text".into())),
        ("text", Value::Str(json::to_string(&payload))),
    ]);
    json::obj([
        ("content", Value::Array(vec![part])),
        ("isError", Value::Bool(false)),
    ])
}

fn required_string(args: &Value, name: &str) -> Result<String, (i64, String)> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| (-32602, format!("missing required string `{name}`")))
}

fn optional_string(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(|v| v.as_str()).map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, params: &str) -> String {
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{method}\",\"params\":{params}}}")
    }

    #[test]
    fn initialize_returns_server_info() {
        let reply = handle_message(&req("initialize", "{}")).unwrap();
        assert!(reply.contains("\"result\""));
        assert!(reply.contains("\"protocolVersion\""));
        assert!(reply.contains("\"twec-mcp\""));
    }

    #[test]
    fn tools_list_includes_canonical_tools() {
        let reply = handle_message(&req("tools/list", "{}")).unwrap();
        for name in &[
            "parse",
            "verify",
            "format",
            "grammar",
            "stdlib_list",
            "stdlib_lookup",
            "apply_patch",
        ] {
            assert!(
                reply.contains(&format!("\"name\":\"{name}\"")),
                "tools/list missing {name}"
            );
        }
    }

    #[test]
    fn unknown_method_returns_jsonrpc_error() {
        let reply = handle_message(&req("totally/unknown", "{}")).unwrap();
        assert!(reply.contains("\"error\""));
        assert!(reply.contains("-32601"));
    }

    #[test]
    fn malformed_input_returns_parse_error() {
        let reply = handle_message("not json at all").unwrap();
        assert!(reply.contains("\"error\""));
        assert!(reply.contains("-32700"));
    }

    #[test]
    fn tools_call_verify_round_trips() {
        let body = "{\"name\":\"verify\",\"arguments\":{\"source\":\"# verified\\nlet x: int = 42\\n\"}}";
        let reply = handle_message(&req("tools/call", body)).unwrap();
        // The verify JSON v2 is wrapped in the tools/call content
        // envelope as a stringified text part — every quote inside
        // the inner JSON is backslash-escaped in the outer envelope.
        assert!(reply.contains("\\\"twec-verify\\\""));
        assert!(reply.contains("\\\"version\\\":2"));
    }

    #[test]
    fn tools_call_grammar_returns_gbnf_by_default() {
        let body = "{\"name\":\"grammar\",\"arguments\":{}}";
        let reply = handle_message(&req("tools/call", body)).unwrap();
        assert!(reply.contains("root ::= program"));
    }

    #[test]
    fn tools_call_stdlib_lookup_finds_known_builtin() {
        let body = "{\"name\":\"stdlib_lookup\",\"arguments\":{\"name\":\"math.sqrt\"}}";
        let reply = handle_message(&req("tools/call", body)).unwrap();
        assert!(reply.contains("\\\"name\\\":\\\"math.sqrt\\\""));
        assert!(reply.contains("\\\"category\\\":\\\"math\\\""));
    }

    #[test]
    fn tools_call_apply_patch_replaces_text() {
        let body = r#"{"name":"apply_patch","arguments":{"source":"let y = aple\n","edits":[{"line":1,"col":9,"len":4,"replace":"apple"}]}}"#;
        let reply = handle_message(&req("tools/call", body)).unwrap();
        assert!(reply.contains("apple"));
    }

    #[test]
    fn notification_initialized_does_not_reply() {
        let msg = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}";
        assert!(handle_message(msg).is_none());
    }

    #[test]
    fn initialize_advertises_instructions_and_capabilities() {
        let reply = handle_message(&req("initialize", "{}")).unwrap();
        assert!(reply.contains("\"instructions\""), "initialize must carry grounding instructions");
        assert!(reply.contains("GOLDEN RULES"), "instructions should be the Twe primer");
        assert!(reply.contains("\"resources\""), "must advertise resources capability");
        assert!(reply.contains("\"prompts\""), "must advertise prompts capability");
    }

    #[test]
    fn resources_list_includes_guide_grammar_and_examples() {
        let reply = handle_message(&req("resources/list", "{}")).unwrap();
        assert!(reply.contains("twe://guide"));
        assert!(reply.contains("twe://grammar"));
        assert!(reply.contains("twe://examples/snake"));
    }

    #[test]
    fn resources_read_guide_returns_markdown_body() {
        let body = "{\"uri\":\"twe://guide\"}";
        let reply = handle_message(&req("resources/read", body)).unwrap();
        assert!(reply.contains("\\\"contents\\\"") || reply.contains("\"contents\""));
        assert!(reply.contains("Twe"), "guide body should mention Twe");
    }

    #[test]
    fn resources_read_example_returns_source() {
        let body = "{\"uri\":\"twe://examples/snake\"}";
        let reply = handle_message(&req("resources/read", body)).unwrap();
        assert!(reply.contains("scene Snake"), "snake example source expected");
    }

    #[test]
    fn resources_read_unknown_uri_errors() {
        let body = "{\"uri\":\"twe://nope\"}";
        let reply = handle_message(&req("resources/read", body)).unwrap();
        assert!(reply.contains("\"error\""));
    }

    #[test]
    fn prompts_list_includes_templates() {
        let reply = handle_message(&req("prompts/list", "{}")).unwrap();
        for name in &["scaffold_game", "add_feature", "fix_errors"] {
            assert!(reply.contains(name), "prompts/list missing {name}");
        }
    }

    #[test]
    fn prompts_get_scaffold_embeds_primer_and_task() {
        let body = "{\"name\":\"scaffold_game\",\"arguments\":{\"description\":\"a pong clone\"}}";
        let reply = handle_message(&req("prompts/get", body)).unwrap();
        assert!(reply.contains("GOLDEN RULES"), "prompt should lead with the primer");
        assert!(reply.contains("pong clone"), "prompt should embed the task");
        assert!(reply.contains("\"messages\""));
    }

    #[test]
    fn prompts_get_missing_required_arg_errors() {
        let body = "{\"name\":\"scaffold_game\",\"arguments\":{}}";
        let reply = handle_message(&req("prompts/get", body)).unwrap();
        assert!(reply.contains("\"error\""));
    }
}
