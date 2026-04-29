//! Minimal Language Server Protocol implementation for Twe.
//!
//! Speaks JSON-RPC 2.0 over stdio. The MVP scope is **diagnostics
//! only** — the editor opens a `.twe` file, we re-lex + re-parse
//! on every `didChange`, and any lex/parse error becomes an
//! `Error` diagnostic at its line:col. Hover, go-to-def, and
//! completion stay deferred for a follow-up session.
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
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("body not UTF-8: {e}"))
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
            Some(other) => {
                // Unknown method. If it's a request (has `id`),
                // reply MethodNotFound so the client doesn't hang.
                if id.is_some() {
                    self.send_error(output, id, -32601, format!("method '{other}' not implemented"))?;
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

fn initialize_result() -> Value {
    obj([(
        "capabilities",
        obj([
            // Full-document sync — client sends the whole text on
            // every change. Twe files are small enough that
            // incremental sync isn't worth the complexity in the
            // MVP.
            ("textDocumentSync", Value::Int(1)),
        ]),
    ),
        (
            "serverInfo",
            obj([
                ("name", Value::Str("twec lsp".into())),
                ("version", Value::Str(env!("CARGO_PKG_VERSION").into())),
            ]),
        )])
}

fn parse_did_open(msg: &Value) -> Option<(String, String)> {
    let td = msg.get("params")?.get("textDocument")?;
    let uri = td.get("uri")?.as_str()?.to_string();
    let text = td.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

fn parse_did_change(msg: &Value) -> Option<(String, String)> {
    let params = msg.get("params")?;
    let uri = params.get("textDocument")?.get("uri")?.as_str()?.to_string();
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
/// errors. Returns an empty Vec when the file parses cleanly.
fn collect_diagnostics(text: &str) -> Vec<Value> {
    match lexer::lex(text) {
        Err(e) => vec![diagnostic_at(e.line, e.col, &e.message)],
        Ok(tokens) => match parser::parse(&tokens) {
            Err(e) => vec![diagnostic_at(e.line, e.col, &e.message)],
            Ok(_) => Vec::new(),
        },
    }
}

fn diagnostic_at(line: u32, col: u32, message: &str) -> Value {
    // LSP positions are zero-indexed; Twe's lex/parse errors are
    // one-indexed (matching what `twec run` prints). Subtract 1
    // and saturate at 0 so a position of 0,0 stays 0,0.
    let l = line.saturating_sub(1);
    let c = col.saturating_sub(1);
    let pos = obj([("line", Value::Int(l as i64)), ("character", Value::Int(c as i64))]);
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
            out.push(std::str::from_utf8(&bytes[body_start..body_end]).unwrap().to_string());
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
        let diags = pub_diag.get("params").unwrap().get("diagnostics").unwrap().as_array().unwrap();
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

    #[test]
    fn unknown_method_request_returns_method_not_found() {
        let unknown = r#"{"jsonrpc":"2.0","id":42,"method":"textDocument/hover","params":{}}"#;
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
