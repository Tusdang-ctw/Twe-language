//! web3d-M5: OpenAI-compatible chat completions
//! (`POST {base}/chat/completions`), the API local servers speak:
//! Ollama (`http://localhost:11434/v1`, the default) and llama.cpp's
//! `llama-server`. `OPENAI_API_KEY`, when set, is sent as a bearer
//! token. Local models have no list price, so replies carry no cost.

use serde_json::{json, Value};

use crate::{pricing, Error, Provider, Reply, Request, StopReason, Usage};

pub struct OpenAiCompatible {
    pub base_url: String,
    pub model: String,
    /// A GBNF grammar the server constrains decoding to (llama.cpp's
    /// `grammar` extension; the benchmark's constrained-decoding arm
    /// passes Twe's, from `twec grammar --format gbnf`). The model can
    /// then only produce a program, so it answers with bare source.
    pub grammar: Option<String>,
}

impl OpenAiCompatible {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        OpenAiCompatible { base_url: base_url.into(), model: model.into(), grammar: None }
    }

    pub fn ollama(model: impl Into<String>) -> Self {
        Self::new("http://localhost:11434/v1", model)
    }
}

pub fn request_body(model: &str, grammar: Option<&str>, request: &Request) -> Value {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system }));
    }
    for m in &request.messages {
        messages.push(json!({ "role": m.role.as_str(), "content": m.text }));
    }
    let mut body = json!({ "model": model, "messages": messages, "max_tokens": request.max_tokens });
    if let Some(g) = grammar {
        body["grammar"] = json!(g);
    }
    body
}

pub fn parse_response(body: &str) -> Result<Reply, Error> {
    let v: Value = serde_json::from_str(body).map_err(|e| Error::Malformed(e.to_string()))?;
    let choice = &v["choices"][0];
    let text = choice["message"]["content"]
        .as_str()
        .ok_or_else(|| Error::Malformed(format!("no choices[0].message.content in {}", body.chars().take(200).collect::<String>())))?;
    let u = &v["usage"];
    let prompt = u["prompt_tokens"].as_u64().unwrap_or(0);
    let cached = u["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
    let usage = Usage {
        input: prompt.saturating_sub(cached),
        output: u["completion_tokens"].as_u64().unwrap_or(0),
        cache_read: cached,
        cache_write: 0,
    };
    let model = v["model"].as_str().unwrap_or_default().to_string();
    Ok(Reply {
        text: text.to_string(),
        stop: StopReason::parse(choice["finish_reason"].as_str().unwrap_or_default()),
        usage,
        cost_usd: pricing::cost(&model, usage),
        model,
    })
}

impl Provider for OpenAiCompatible {
    #[cfg(feature = "http")]
    fn complete(&mut self, request: &Request) -> Result<Reply, Error> {
        let mut headers = Vec::new();
        if let Some(key) = crate::anthropic::env_nonempty("OPENAI_API_KEY") {
            headers.push(("authorization", format!("Bearer {key}")));
        }
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = request_body(&self.model, self.grammar.as_deref(), request).to_string();
        parse_response(&crate::http::post_json(&url, &headers, &body)?)
    }

    #[cfg(not(feature = "http"))]
    fn complete(&mut self, _: &Request) -> Result<Reply, Error> {
        Err(Error::Config(crate::NO_HTTP.into()))
    }

    fn id(&self) -> String {
        if self.grammar.is_some() {
            format!("openai:{}+gbnf", self.model)
        } else {
            format!("openai:{}", self.model)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_becomes_the_first_message() {
        let body = request_body("llama3.1", None, &Request::new("primer", "task"));
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "task");
        assert!(body.get("grammar").is_none());
        let constrained = request_body("llama3.1", Some("root ::= \"x\""), &Request::new("", "task"));
        assert_eq!(constrained["grammar"], "root ::= \"x\"");
    }

    #[test]
    fn cached_prompt_tokens_are_split_out() {
        let body = r#"{"model": "llama3.1", "choices": [{"message": {"content": "hi"}, "finish_reason": "length"}],
                       "usage": {"prompt_tokens": 1000, "completion_tokens": 7,
                                 "prompt_tokens_details": {"cached_tokens": 800}}}"#;
        let reply = parse_response(body).unwrap();
        assert_eq!((reply.usage.input, reply.usage.cache_read), (200, 800));
        assert_eq!(reply.stop, StopReason::MaxTokens);
        assert_eq!(reply.cost_usd, None);
    }
}
