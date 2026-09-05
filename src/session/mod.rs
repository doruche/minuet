use std::fmt;

use thiserror::Error;

use crate::{
    inference::{ConversationItem, TokenUsage},
    model::ReasoningEffort,
};

mod memory;

pub use memory::MemorySessionStore;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionId(u64);

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub items: Vec<ConversationItem>,
    pub reasoning_effort: Option<ReasoningEffort>,
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

/// The store is the sole owner of committed conversation history, per-session
/// settings, and observed usage. Snapshots are immutable and may become stale;
/// the kernel uses them only while it serializes all commands for one run.
pub trait SessionStore: Send {
    fn create(
        &mut self,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<SessionId, SessionStoreError>;

    fn snapshot(&self, id: SessionId) -> Result<SessionSnapshot, SessionStoreError>;

    fn append(
        &mut self,
        id: SessionId,
        items: &[ConversationItem],
    ) -> Result<(), SessionStoreError>;

    fn clear(&mut self, id: SessionId) -> Result<(), SessionStoreError>;

    fn commit_inference(
        &mut self,
        id: SessionId,
        output: &[ConversationItem],
        usage: Option<TokenUsage>,
    ) -> Result<(), SessionStoreError>;

    fn set_reasoning_effort(
        &mut self,
        id: SessionId,
        effort: Option<ReasoningEffort>,
    ) -> Result<(), SessionStoreError>;
}

struct Session {
    items: Vec<ConversationItem>,
    reasoning_effort: Option<ReasoningEffort>,
    usage: UsageSummary,
}

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("session {0} was not found")]
    NotFound(SessionId),
    #[error("session identifier space is exhausted")]
    IdExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inference_commit_updates_history_and_usage_together() {
        let mut store = MemorySessionStore::default();
        let id = store.create(None).unwrap();
        let output = [ConversationItem::UserText("test".to_owned())];
        let usage = TokenUsage {
            input_tokens: 4,
            output_tokens: 2,
            total_tokens: 6,
        };

        store.commit_inference(id, &output, Some(usage)).unwrap();
        let snapshot = store.snapshot(id).unwrap();
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.usage.reported_calls, 1);
        assert_eq!(snapshot.usage.total_tokens, 6);
        assert_eq!(snapshot.usage.latest, Some(usage));
    }

    #[test]
    fn tracks_calls_without_upstream_usage_as_unknown() {
        let mut summary = UsageSummary::default();
        summary.observe(None);
        assert_eq!(summary.reported_calls, 0);
        assert_eq!(summary.unreported_calls, 1);
        assert_eq!(summary.total_tokens, 0);
        assert_eq!(summary.latest, None);
    }

    #[test]
    fn clearing_a_session_removes_history_and_usage_but_keeps_settings() {
        let mut store = MemorySessionStore::default();
        let id = store
            .create(Some(ReasoningEffort::new("careful").unwrap()))
            .unwrap();
        store
            .append(id, &[ConversationItem::UserText("old".into())])
            .unwrap();
        store
            .commit_inference(
                id,
                &[ConversationItem::FunctionCallOutput {
                    call_id: "call".into(),
                    output: "answer".into(),
                }],
                Some(TokenUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: 3,
                }),
            )
            .unwrap();

        store.clear(id).unwrap();
        let snapshot = store.snapshot(id).unwrap();
        assert!(snapshot.items.is_empty());
        assert_eq!(snapshot.usage, UsageSummary::default());
        assert_eq!(
            snapshot.reasoning_effort,
            Some(ReasoningEffort::new("careful").unwrap())
        );
    }
}
