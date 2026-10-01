//! web3d-M5: canned replies for tests. Never touches a network.

use std::collections::VecDeque;

use crate::{Error, Provider, Reply, Request};

/// Returns queued replies in order and records every request it was
/// sent. An empty queue is an error, not a panic, so a loop under test
/// reports it cleanly.
pub struct FixtureProvider {
    pub replies: VecDeque<Reply>,
    pub requests: Vec<Request>,
}

impl FixtureProvider {
    /// Text replies with no token accounting.
    pub fn new(replies: impl IntoIterator<Item = String>) -> Self {
        Self::with_replies(replies.into_iter().map(|t| Reply::text_only(t, "fixture")))
    }

    pub fn with_replies(replies: impl IntoIterator<Item = Reply>) -> Self {
        FixtureProvider {
            replies: replies.into_iter().collect(),
            requests: Vec::new(),
        }
    }
}

impl Provider for FixtureProvider {
    fn complete(&mut self, request: &Request) -> Result<Reply, Error> {
        self.requests.push(request.clone());
        self.replies
            .pop_front()
            .ok_or_else(|| Error::Config("FixtureProvider has no replies left".into()))
    }

    fn id(&self) -> String {
        "fixture".into()
    }
}
