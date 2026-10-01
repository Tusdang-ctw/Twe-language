//! web3d-M5: Twe's LLM layer, shared by `twec` and Studio.
//!
//! - [`Provider`]: one blocking request/reply call. Implementations:
//!   [`anthropic::Anthropic`] and [`openai::OpenAiCompatible`] (with the
//!   `http` feature), [`command::CommandProvider`] (pipes the prompt
//!   through any program), [`fixture::FixtureProvider`] (tests).
//! - [`Reply`]: the text plus what the call cost: stop reason, token
//!   usage including prompt-cache reads and writes, and dollars when the
//!   model's prices are known ([`pricing`]).
//! - [`edit`]: the one edit protocol — SEARCH/REPLACE blocks or a whole
//!   fenced file — parsed and applied in one place.
//!
//! The crate knows nothing about Twe syntax: verifying, running and
//! grading programs is the caller's job.

pub mod anthropic;
pub mod command;
pub mod edit;
pub mod fixture;
pub mod openai;
pub mod pricing;

// Request bodies and response parsing are plain functions, always
// compiled (and tested without a network); only the call itself needs
// the `http` feature.
#[cfg(feature = "http")]
mod http;

/// What an API provider says when it was built without HTTP.
#[cfg(not(feature = "http"))]
const NO_HTTP: &str = "this build has no HTTP client: rebuild with `--features llm-http` (twec) or `--features http` (twe-llm)";

pub use command::CommandProvider;
pub use fixture::FixtureProvider;

/// Who said a message in a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Message { role: Role::User, text: text.into() }
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Message { role: Role::Assistant, text: text.into() }
    }
}

/// One call to a model. `system` holds what stays the same across a
/// conversation (the Twe primer, the task's rules), so providers that
/// cache prompts can cache it; `messages` alternate user / assistant
/// and end with a user message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub system: String,
    pub messages: Vec<Message>,
    /// The reply's token ceiling (thinking included, for models that
    /// think).
    pub max_tokens: u32,
}

impl Request {
    pub fn new(system: impl Into<String>, user: impl Into<String>) -> Self {
        Request {
            system: system.into(),
            messages: vec![Message::user(user)],
            max_tokens: 16_000,
        }
    }

    /// The whole request as plain text, for providers that take one
    /// prompt (a shell command) and for hashing.
    pub fn flatten(&self) -> String {
        let mut out = String::new();
        if !self.system.is_empty() {
            out.push_str(&self.system);
            out.push_str("\n\n");
        }
        let multi = self.messages.len() > 1;
        for (i, m) in self.messages.iter().enumerate() {
            if i > 0 {
                out.push_str("\n\n");
            }
            if multi {
                out.push_str(match m.role {
                    Role::User => "## User\n\n",
                    Role::Assistant => "## Assistant\n\n",
                });
            }
            out.push_str(&m.text);
        }
        out
    }
}

/// Why the model stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// Finished its reply.
    EndTurn,
    /// Hit `max_tokens`: the reply is truncated.
    MaxTokens,
    /// Declined the request (a safety classifier, for instance).
    Refusal,
    Other,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::Refusal => "refusal",
            StopReason::Other => "other",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "end_turn" | "stop" | "stop_sequence" => StopReason::EndTurn,
            "max_tokens" | "length" => StopReason::MaxTokens,
            "refusal" | "content_filter" => StopReason::Refusal,
            _ => StopReason::Other,
        }
    }
}

/// Tokens a call used. `input` counts only uncached input; cached
/// input is split into what was read from the cache and what was
/// written to it (each billed at its own rate).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// The reply's text (text blocks joined; thinking left out).
    pub text: String,
    pub stop: StopReason,
    pub usage: Usage,
    /// The model that answered, as the provider reported it.
    pub model: String,
    /// What the call cost in US dollars, when the model's prices are
    /// known ([`pricing::cost`]).
    pub cost_usd: Option<f64>,
}

impl Reply {
    /// A reply with no accounting (shell commands, fixtures).
    pub fn text_only(text: impl Into<String>, model: impl Into<String>) -> Self {
        Reply {
            text: text.into(),
            stop: StopReason::EndTurn,
            usage: Usage::default(),
            model: model.into(),
            cost_usd: None,
        }
    }
}

/// Why a call produced no reply. These are infrastructure failures,
/// not model mistakes: callers stop rather than count them against
/// the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Missing key, unknown model, bad configuration.
    Config(String),
    /// The service answered with an error status (after retries for
    /// the retryable ones).
    Http { status: u16, message: String },
    /// No answer at all: connection, timeout, a program that failed.
    Transport(String),
    /// An answer that couldn't be read.
    Malformed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Config(m) => write!(f, "{m}"),
            Error::Http { status, message } => write!(f, "HTTP {status}: {message}"),
            Error::Transport(m) => write!(f, "{m}"),
            Error::Malformed(m) => write!(f, "unreadable reply: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// One blocking round trip with a model.
pub trait Provider {
    fn complete(&mut self, request: &Request) -> Result<Reply, Error>;

    /// What answers, for logs and result files: `"anthropic:claude-sonnet-5-5"`,
    /// `"command:claude"`, `"fixture"`.
    fn id(&self) -> String;
}

impl<P: Provider + ?Sized> Provider for Box<P> {
    fn complete(&mut self, request: &Request) -> Result<Reply, Error> {
        (**self).complete(request)
    }
    fn id(&self) -> String {
        (**self).id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_labels_turns_only_in_a_conversation() {
        let single = Request::new("rules", "task");
        assert_eq!(single.flatten(), "rules\n\ntask");
        let mut multi = single.clone();
        multi.messages.push(Message::assistant("draft"));
        multi.messages.push(Message::user("fix it"));
        assert_eq!(
            multi.flatten(),
            "rules\n\n## User\n\ntask\n\n## Assistant\n\ndraft\n\n## User\n\nfix it"
        );
    }

    #[test]
    fn stop_reasons_from_both_apis() {
        assert_eq!(StopReason::parse("end_turn"), StopReason::EndTurn);
        assert_eq!(StopReason::parse("stop"), StopReason::EndTurn);
        assert_eq!(StopReason::parse("length"), StopReason::MaxTokens);
        assert_eq!(StopReason::parse("refusal"), StopReason::Refusal);
        assert_eq!(StopReason::parse("pause_turn"), StopReason::Other);
    }
}
