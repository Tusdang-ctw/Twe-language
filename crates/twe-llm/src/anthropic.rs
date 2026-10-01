//! web3d-M5: the Anthropic Messages API (`POST /v1/messages`).
//!
//! - **Caching:** the system prompt (Twe's primer, the task rules) is
//!   marked for prompt caching, so the rounds of a verify loop and the
//!   samples of a benchmark read it from the cache.
//! - **Thinking:** left to the model's default (adaptive on current
//!   models). Depth is set with `effort`, the only knob current models
//!   accept; they reject `temperature`.
//! - **Fallbacks are off:** a refused request comes back as
//!   [`StopReason::Refusal`](crate::StopReason) rather than being re-run
//!   on another model, since a benchmark must know which model
//!   answered.
//! - **Credentials:** `ANTHROPIC_API_KEY`, or an OAuth token in
//!   `ANTHROPIC_AUTH_TOKEN`. `ANTHROPIC_BASE_URL` overrides the endpoint.

use serde_json::{json, Value};

use crate::{pricing, Error, Provider, Reply, Request, StopReason, Usage};

pub struct Anthropic {
    pub model: String,
    /// `low` … `max`; `None` leaves the model's default.
    pub effort: Option<String>,
    /// Overrides `ANTHROPIC_BASE_URL` (tests point it at a local server).
    pub base_url: Option<String>,
    /// Overrides `ANTHROPIC_API_KEY`.
    pub api_key: Option<String>,
}

impl Anthropic {
    pub fn new(model: impl Into<String>) -> Self {
        Anthropic { model: model.into(), effort: None, base_url: None, api_key: None }
    }
}

/// The request body.
pub fn request_body(model: &str, effort: Option<&str>, request: &Request) -> Value {
    let messages: Vec<Value> = request
        .messages
        .iter()
        .map(|m| json!({ "role": m.role.as_str(), "content": m.text }))
        .collect();
    let mut body = json!({
        "model": model,
        "max_tokens": request.max_tokens,
        "messages": messages,
    });
    if !request.system.is_empty() {
        body["system"] = json!([{
            "type": "text",
            "text": request.system,
            "cache_control": { "type": "ephemeral" },
        }]);
    }
    if let Some(effort) = effort {
        body["output_config"] = json!({ "effort": effort });
    }
    body
}

/// Read a response body into a [`Reply`]. Text blocks are joined;
/// thinking blocks are left out.
pub fn parse_response(body: &str) -> Result<Reply, Error> {
    let v: Value = serde_json::from_str(body).map_err(|e| Error::Malformed(e.to_string()))?;
    let content = v["content"]
        .as_array()
        .ok_or_else(|| Error::Malformed(format!("no `content` in {}", head(body))))?;
    let text: Vec<&str> = content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let u = &v["usage"];
    let n = |key: &str| u[key].as_u64().unwrap_or(0);
    let usage = Usage {
        input: n("input_tokens"),
        output: n("output_tokens"),
        cache_read: n("cache_read_input_tokens"),
        cache_write: n("cache_creation_input_tokens"),
    };
    let model = v["model"].as_str().unwrap_or_default().to_string();
    Ok(Reply {
        text: text.join(""),
        stop: StopReason::parse(v["stop_reason"].as_str().unwrap_or_default()),
        usage,
        cost_usd: pricing::cost(&model, usage),
        model,
    })
}

fn head(s: &str) -> String {
    s.chars().take(200).collect()
}

impl Provider for Anthropic {
    #[cfg(feature = "http")]
    fn complete(&mut self, request: &Request) -> Result<Reply, Error> {
        let mut headers = vec![("anthropic-version", "2023-06-01".to_string())];
        if let Some(key) = self.api_key.clone().or_else(|| env_nonempty("ANTHROPIC_API_KEY")) {
            headers.push(("x-api-key", key));
        } else if let Some(token) = env_nonempty("ANTHROPIC_AUTH_TOKEN") {
            headers.push(("authorization", format!("Bearer {token}")));
            headers.push(("anthropic-beta", "oauth-2025-04-20".to_string()));
        } else {
            return Err(Error::Config(
                "no Anthropic credentials: set ANTHROPIC_API_KEY (or ANTHROPIC_AUTH_TOKEN)".into(),
            ));
        }
        let base = self
            .base_url
            .clone()
            .or_else(|| env_nonempty("ANTHROPIC_BASE_URL"))
            .unwrap_or_else(|| "https://api.anthropic.com".into());
        let url = format!("{}/v1/messages", base.trim_end_matches('/'));
        let body = request_body(&self.model, self.effort.as_deref(), request).to_string();
        parse_response(&crate::http::post_json(&url, &headers, &body)?)
    }

    #[cfg(not(feature = "http"))]
    fn complete(&mut self, _: &Request) -> Result<Reply, Error> {
        Err(Error::Config(crate::NO_HTTP.into()))
    }

    fn id(&self) -> String {
        match &self.effort {
            Some(e) => format!("anthropic:{}@{e}", self.model),
            None => format!("anthropic:{}", self.model),
        }
    }
}

#[cfg(feature = "http")]
pub(crate) fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    #[test]
    fn body_caches_the_system_prompt_and_sets_effort() {
        let mut request = Request::new("primer", "task");
        request.messages.push(Message::assistant("draft"));
        request.messages.push(Message::user("fix"));
        let body = request_body("claude-sonnet-5-5", Some("high"), &request);
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["max_tokens"], 16000);
        assert!(body.get("temperature").is_none());
        let bare = request_body("m", None, &Request::new("", "task"));
        assert!(bare.get("system").is_none() && bare.get("output_config").is_none());
    }

    #[test]
    fn response_joins_text_and_counts_every_kind_of_token() {
        let body = r#"{
            "model": "claude-sonnet-5-5",
            "stop_reason": "end_turn",
            "content": [
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": "```twe\n"},
                {"type": "text", "text": "let x = 1\n```"}
            ],
            "usage": {"input_tokens": 100, "output_tokens": 50,
                      "cache_read_input_tokens": 4000, "cache_creation_input_tokens": 0}
        }"#;
        let reply = parse_response(body).unwrap();
        assert_eq!(reply.text, "```twe\nlet x = 1\n```");
        assert_eq!(reply.stop, StopReason::EndTurn);
        assert_eq!(reply.usage.cache_read, 4000);
        // 100 × 2 + 50 × 10 + 4000 × 0.20, per million.
        assert!((reply.cost_usd.unwrap() - 0.0015).abs() < 1e-12);
    }

    /// The real HTTP path against a local server: headers, the body,
    /// a retry after an overloaded (529) answer, then a reply.
    #[cfg(feature = "http")]
    #[test]
    fn http_call_retries_an_overload_then_reads_the_reply() {
        use std::io::{BufRead, BufReader, Read, Write};
        const OVERLOADED: &str = "HTTP/1.1 529 Overloaded\r\nretry-after: 0\r\ncontent-length: 52\r\n\r\n{\"error\": {\"type\": \"overloaded\", \"message\": \"busy\"}}";
        const OK_BODY: &str = r#"{"model": "claude-haiku-4-5", "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hello"}],
            "usage": {"input_tokens": 3, "output_tokens": 2}}"#;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let ok = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{OK_BODY}",
                OK_BODY.len()
            );
            let mut seen: Vec<(String, String)> = Vec::new();
            for answer in [OVERLOADED.to_string(), ok] {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                seen.push((head, String::from_utf8(body).unwrap()));
                reader.get_mut().write_all(answer.as_bytes()).unwrap();
            }
            seen
        });
        let mut p = Anthropic::new("claude-haiku-4-5");
        p.base_url = Some(format!("http://127.0.0.1:{port}"));
        p.api_key = Some("test-key".into());
        let reply = p.complete(&Request::new("primer", "say hello")).unwrap();
        assert_eq!(reply.text, "hello");
        assert_eq!(reply.usage.output, 2);
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2, "one retry after the 529");
        let (head, body) = &seen[1];
        let head = head.to_ascii_lowercase();
        assert!(head.starts_with("post /v1/messages "), "{head}");
        assert!(head.contains("x-api-key: test-key"));
        assert!(head.contains("anthropic-version: 2023-06-01"));
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["messages"][0]["content"], "say hello");
    }

    #[test]
    fn refusals_are_replies_not_errors() {
        let body = r#"{"model": "m", "stop_reason": "refusal", "content": [], "usage": {}}"#;
        assert_eq!(parse_response(body).unwrap().stop, StopReason::Refusal);
        assert!(matches!(parse_response("{}"), Err(Error::Malformed(_))));
    }
}
