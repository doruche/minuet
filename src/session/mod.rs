use std::fmt;

use thiserror::Error;
use uuid::Uuid;

use crate::{
    inference::{ConversationItem, TokenUsage},
    model::ReasoningEffort,
};

mod memory;
pub use memory::MemorySessionRepository;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(Uuid);

impl SessionId {
    pub fn parse(value: &str) -> Result<Self, SessionIdError> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| SessionIdError::Invalid(value.to_owned()))
    }
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionConfig {
    pub reasoning_effort: Option<ReasoningEffort>,
    pub enabled_tools: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub items: Vec<ConversationItem>,
    pub config: SessionConfig,
    pub usage: UsageSummary,
}

#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub id: SessionId,
    pub brief: String,
    pub item_count: usize,
    pub config: SessionConfig,
    pub usage: UsageSummary,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UsageSummary {
    pub reported_calls: u64,
    pub unreported_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub latest: Option<TokenUsage>,
}

impl UsageSummary {
    pub fn observe(&mut self, usage: Option<TokenUsage>) {
        let Some(usage) = usage else {
            self.unreported_calls = self.unreported_calls.saturating_add(1);
            self.latest = None;
            return;
        };
        self.reported_calls = self.reported_calls.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(usage.total_tokens);
        self.latest = Some(usage);
    }
}

pub trait SessionRepository: Send {
    fn create(&mut self, config: SessionConfig) -> Result<SessionId, SessionRepositoryError>;
    fn list(&self) -> Vec<SessionSummary>;
    fn snapshot(&self, id: SessionId) -> Result<SessionSnapshot, SessionRepositoryError>;
    fn append(
        &mut self,
        id: SessionId,
        items: &[ConversationItem],
    ) -> Result<(), SessionRepositoryError>;
    fn clear(&mut self, id: SessionId) -> Result<(), SessionRepositoryError>;
    fn delete(&mut self, id: SessionId) -> Result<(), SessionRepositoryError>;
    fn commit_inference(
        &mut self,
        id: SessionId,
        output: &[ConversationItem],
        usage: Option<TokenUsage>,
    ) -> Result<(), SessionRepositoryError>;
    fn set_config(
        &mut self,
        id: SessionId,
        config: SessionConfig,
    ) -> Result<(), SessionRepositoryError>;
}

#[derive(Debug, Error)]
pub enum SessionIdError {
    #[error("invalid session ID `{0}`")]
    Invalid(String),
}

#[derive(Debug, Error)]
pub enum SessionRepositoryError {
    #[error("session {0} was not found")]
    NotFound(SessionId),
    #[error("session {0} already exists")]
    AlreadyExists(SessionId),
}

pub use SessionRepository as SessionStore;
pub use SessionRepositoryError as SessionStoreError;
pub use memory::MemorySessionRepository as MemorySessionStore;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_ids_round_trip() {
        let id = SessionId::new();
        assert_eq!(SessionId::parse(&id.to_string()).unwrap(), id);
    }
    #[test]
    fn unknown_usage_is_counted() {
        let mut usage = UsageSummary::default();
        usage.observe(None);
        assert_eq!(usage.unreported_calls, 1);
    }
}
