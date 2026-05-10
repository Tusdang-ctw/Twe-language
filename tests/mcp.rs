//! Phase 33 session 5: integration tests for the MCP stdio server.
//!
//! Exercises every tool through the public `handle_message` entry
//! point. Drives canned JSON-RPC requests, asserts well-formed
//! replies. The unit tests in `src/mcp.rs` cover the basics; this
//! suite locks down the cross-tool round-trip an LLM agent uses:
//!
//!   1. `verify(broken)` → diagnostics with structured fix
//!   2. `apply_patch(source, fix.edits)` → corrected source
//!   3. `verify(corrected)` → no diagnostics
//!
//! That sequence is the reason MCP exists — exposing every Twe
//! tool to any client closes the LLM loop with zero bespoke wiring.

use twec::json;
use twec::mcp::handle_message;

fn rpc(method: &str, params: serde_args::J) -> String {
    let body = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{method}\",\"params\":{}}}",
        params.0
    );
    handle_message(&body).expect("expected reply")
}

/// Tiny "JSON literal" type so the test code reads like JSON without
/// pulling serde. Built ad-hoc since the test driver is the only
/// caller. Use `j("{ ... }")` to wrap a literal.
mod serde_args {
    pub struct J(pub String);
    pub fn j(s: &str) -> J {
        J(s.to_string())
    }
}

use serde_args::j;

#[test]
fn initialize_handshake_returns_protocol_metadata() {
    let reply = rpc("initialize", j("{}"));
    let v = json::parse(&reply).unwrap();
    let result = v.get("result").expect("must have result");
    assert!(result.get("protocolVersion").is_some());
    assert!(result.get("serverInfo").is_some());
    assert!(result.get("capabilities").is_some());
}

#[test]
fn tools_list_describes_every_canonical_tool() {
    let reply = rpc("tools/list", j("{}"));
    let v = json::parse(&reply).unwrap();
    let tools = v
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .expect("tools/list must return result.tools[]");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
        .collect();
    for expected in &[
        "parse",
        "verify",
        "format",
        "grammar",
        "stdlib_list",
        "stdlib_lookup",
        "apply_patch",
    ] {
        assert!(names.contains(expected), "tools/list missing `{expected}`");
    }
    // Every tool descriptor must carry a non-empty description and
    // a JSON-Schema-shaped inputSchema.
    for tool in tools {
        let desc = tool.get("description").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!desc.is_empty(), "tool missing description: {tool:?}");
        let schema = tool.get("inputSchema").expect("tool missing inputSchema");
        assert_eq!(
            schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "inputSchema type must be object"
        );
    }
}

#[test]
fn parse_returns_ast_for_valid_program() {
    let body = "{\"name\":\"parse\",\"arguments\":{\"source\":\"let x = 1\\n\"}}";
    let reply = rpc("tools/call", j(body));
    // The reply payload is wrapped: result.content[0].text is the
    // stringified inner JSON.
    let inner = unwrap_text_payload(&reply);
    assert!(inner.contains("\"ok\":true"), "got: {inner}");
    assert!(inner.contains("\"ast\""));
    assert!(inner.contains("\"Program\""));
}

#[test]
fn parse_reports_lex_error_with_line_col() {
    // An unterminated string literal triggers the lex stage; the
    // reply must surface the failure structurally, not as a
    // protocol error.
    let body = "{\"name\":\"parse\",\"arguments\":{\"source\":\"let x = \\\"unclosed\\n\"}}";
    let reply = rpc("tools/call", j(body));
    let inner = unwrap_text_payload(&reply);
    assert!(inner.contains("\"ok\":false"));
    assert!(inner.contains("\"line\":"));
}

#[test]
fn verify_apply_patch_round_trip_closes_the_loop() {
    // The contract this test guards: an MCP-aware agent gets a
    // structured fix from `verify`, hands it back to `apply_patch`,
    // and re-runs `verify` to confirm. Three tool calls, no source
    // parsing in client-side code.

    // Step 1: verify the broken program. Note: regular (non-raw)
    // string so `\n` is interpreted by the JSON parser, not Rust;
    // raw strings interact badly with Rust 2021's reserved-prefix
    // rules around `\n` even though we're not actually escaping.
    let verify_body = "{\"name\":\"verify\",\"arguments\":{\"source\":\"# verified\\nlet apple = 1\\nlet y = aple\\n\"}}";
    let reply1 = rpc("tools/call", j(verify_body));
    let report = unwrap_text_payload(&reply1);
    assert!(report.contains("\"version\":2"));

    // Pull the first edit out of the verify reply. The shape is
    // `diagnostics[0].fix.edits[0]`. Hand-parse to keep the test
    // free of serde.
    let report_v = json::parse(&report).unwrap();
    let diags = report_v
        .get("diagnostics")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(!diags.is_empty(), "expected at least one diagnostic");
    let fix = diags[0].get("fix").expect("first diagnostic must carry fix");
    let edits = fix.get("edits").and_then(|v| v.as_array()).unwrap();
    assert!(!edits.is_empty());
    let edits_text = json::to_string(&twec::json::Value::Array(edits.clone()));

    // Step 2: apply the patch via MCP.
    let apply_body = format!(
        "{{\"name\":\"apply_patch\",\"arguments\":{{\"source\":\"# verified\\nlet apple = 1\\nlet y = aple\\n\",\"edits\":{edits_text}}}}}"
    );
    let reply2 = rpc("tools/call", j(&apply_body));
    let patched_payload = unwrap_text_payload(&reply2);
    let patched_v = json::parse(&patched_payload).unwrap();
    let patched = patched_v.get("source").and_then(|v| v.as_str()).unwrap();
    assert!(patched.contains("let y = apple"));

    // Step 3: re-verify the patched source.
    let reverify_body = format!(
        r#"{{"name":"verify","arguments":{{"source":{}}}}}"#,
        json::to_string(&twec::json::Value::Str(patched.to_string()))
    );
    let reply3 = rpc("tools/call", j(&reverify_body));
    let report2 = unwrap_text_payload(&reply3);
    let r2 = json::parse(&report2).unwrap();
    let errors = r2
        .get("summary")
        .and_then(|v| v.get("errors"))
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    assert_eq!(errors, 0, "after patch, verify must be clean: {report2}");
}

#[test]
fn stdlib_lookup_returns_null_for_unknown_name() {
    let body = "{\"name\":\"stdlib_lookup\",\"arguments\":{\"name\":\"definitely_not_a_real_builtin\"}}";
    let reply = rpc("tools/call", j(body));
    let inner = unwrap_text_payload(&reply);
    // Wrapped null payload comes through as the string `null`.
    assert_eq!(inner.trim(), "null");
}

#[test]
fn grammar_supports_all_three_formats() {
    for format in &["gbnf", "json-schema", "ebnf"] {
        let body = format!(r#"{{"name":"grammar","arguments":{{"format":"{format}"}}}}"#);
        let reply = rpc("tools/call", j(&body));
        let inner = unwrap_text_payload(&reply);
        let v = json::parse(&inner).unwrap();
        let g = v.get("grammar").and_then(|x| x.as_str()).unwrap_or("");
        assert!(!g.is_empty(), "grammar payload empty for {format}");
        assert_eq!(
            v.get("format").and_then(|x| x.as_str()),
            Some(*format),
            "grammar should echo requested format"
        );
    }
}

/// Unwrap MCP's `result.content[0].text` envelope. Tools/call wraps
/// every payload as a single text part containing the JSON-stringified
/// result; clients then re-parse for structured access.
fn unwrap_text_payload(reply: &str) -> String {
    let v = json::parse(reply).expect("reply must be valid JSON");
    let content = v
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .expect("reply must have result.content[]");
    assert_eq!(content.len(), 1, "expected exactly one content part");
    content[0]
        .get("text")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .expect("content[0].text must be a string")
}
