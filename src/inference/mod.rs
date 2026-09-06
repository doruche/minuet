mod openai_responses;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::{model::ReasoningEffort, tool::ToolDefinition};

pub use openai_responses::{OpenAiResponsesBackend, OpenAiResponsesBackendError};

#[derive(Clone, Debug)]
pub enum ConversationItem {
    UserText(String),
    Continuation(ContinuationItem),
    FunctionCallOutput { call_id: String, output: String },
}

/// A backend-owned context item which the kernel may retain and replay but
/// must not interpret. Keeping it opaque preserves protocol-specific reasoning
/// and tool-call state without making the session store another wire owner.
#[derive(Clone, Debug)]
pub struct ContinuationItem {
    value: Value,
}

impl ContinuationItem {
    /// Creates an intentionally opaque protocol item. Backend adapters use this
    /// at their boundary; kernel and session code must only retain and replay it.
    fn from_protocol_value(value: Value) -> Self {
        Self { value }
    }

    fn protocol_value(&self) -> &Value {
        &self.value
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: Value) -> Self {
        Self::from_protocol_value(value)
    }
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug)]
pub struct ModelOutputItem {
    continuation: ContinuationItem,
    effect: OutputEffect,
}

impl ModelOutputItem {
    /// Couples an immutable protocol item with the one behavioral projection
    /// which a backend permits the loop to observe.
    pub fn new(continuation: ContinuationItem, effect: OutputEffect) -> Self {
        Self {
            continuation,
            effect,
        }
    }

    pub fn effect(&self) -> &OutputEffect {
        &self.effect
    }

    pub fn into_continuation(self) -> ContinuationItem {
        self.continuation
    }
}

/// This projection is derived once from the immutable continuation item. It is
/// the only part of a backend output on which kernel behavior may depend.
#[derive(Clone, Debug)]
pub enum OutputEffect {
    None,
    /// One complete model message. Its document boundary is independent of
    /// stream fragments and of other messages returned in the same response.
    Message(String),
    ToolCall(ToolCall),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Clone, Debug)]
pub struct InferenceResponse {
    pub output: Vec<ModelOutputItem>,
    pub usage: Option<TokenUsage>,
}

pub struct InferenceRequest<'a> {
    pub model: &'a str,
    pub input: &'a [ConversationItem],
    pub tools: &'a [ToolDefinition],
    pub reasoning_effort: Option<&'a ReasoningEffort>,
    pub observer: Option<&'a dyn InferenceObserver>,
}

#[async_trait]
pub trait InferenceObserver: Send + Sync {
    async fn text_delta(&self, text: &str);
}

#[async_trait]
pub trait InferenceBackend: Send + Sync {
    async fn respond(
        &self,
        request: InferenceRequest<'_>,
    ) -> Result<InferenceResponse, InferenceError>;

    async fn count_input_tokens(
        &self,
        request: InferenceRequest<'_>,
    ) -> Result<u64, InferenceError>;
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct InferenceError {
    message: String,
}

impl InferenceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<OpenAiResponsesBackendError> for InferenceError {
    fn from(error: OpenAiResponsesBackendError) -> Self {
        Self::new(error.to_string())
    }
}
