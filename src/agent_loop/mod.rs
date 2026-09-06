use async_trait::async_trait;
use std::sync::Arc;
use thiserror::Error;

use crate::{
    inference::{InferenceError, TokenUsage},
    session::{SessionStoreError, TranscriptEntry, UsageSummary},
};

mod events;
mod react;
mod runtime;

pub use events::RunEvent;
pub(crate) use events::RunEvents;
pub use react::ReactLoop;
pub use runtime::LoopContext;

#[async_trait]
pub trait AgentLoop: Send + Sync {
    async fn run(&self, context: &mut LoopContext<'_>) -> Result<RunOutcome, LoopError>;
}

pub struct CommittedModelTurn {
    pub entries: Vec<Arc<TranscriptEntry>>,
    pub usage: Option<TokenUsage>,
}

impl CommittedModelTurn {
    /// Lossy final-text summary for consumers such as the one-shot CLI. This
    /// concatenation must never define presentation order or document boundaries.
    pub fn text_summary(&self) -> String {
        self.entries
            .iter()
            .filter_map(|entry| match entry.as_ref() {
                TranscriptEntry::ModelMessage { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

pub struct CommittedToolRound {
    pub activities: Vec<ToolActivity>,
}

#[derive(Clone, Debug)]
pub struct RunOutcome {
    /// Ordered concatenation of the final turn's messages for summary consumers;
    /// not an alternate presentation history or a Markdown document boundary.
    pub text: String,
    pub model_turns: usize,
    pub tool_activity: Vec<ToolActivity>,
    pub usage: UsageSummary,
    pub stop_reason: RunStopReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunStopReason {
    Completed,
    StepLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolActivityStatus {
    Completed,
    Error,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct ToolActivity {
    /// Original tool name for observation, not routing or execution authority.
    pub name: String,
    pub display: crate::session::ToolDisplay,
    pub status: ToolActivityStatus,
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error("prompt must not be empty")]
    EmptyPrompt,
    #[error("loop.max_steps must be greater than zero")]
    InvalidMaxSteps,
    #[error("loop protocol violation: {0}")]
    Protocol(&'static str),
    #[error("{source}; run also ended with unresolved session state: {pending}")]
    UnfinishedRun {
        #[source]
        source: Box<LoopError>,
        pending: Box<LoopError>,
    },
    #[error(transparent)]
    Inference(#[from] InferenceError),
    #[error(transparent)]
    Session(#[from] SessionStoreError),
}
