use async_trait::async_trait;
use thiserror::Error;

use crate::{
    inference::{InferenceError, TokenUsage, ToolCall},
    session::{SessionStoreError, UsageSummary},
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
    pub tool_calls: PendingToolRound,
    pub text: String,
    pub usage: Option<TokenUsage>,
}

/// Tool calls returned by one committed model turn. The call list is created
/// only by `LoopContext::infer_and_commit` and is consumed by exactly one tool
/// round operation, so a loop cannot fabricate or reuse calls against a
/// different model response.
pub struct PendingToolRound {
    calls: Vec<ToolCall>,
}

impl PendingToolRound {
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub fn calls(&self) -> &[ToolCall] {
        &self.calls
    }
}

pub struct CommittedToolRound {
    pub activities: Vec<ToolActivity>,
}

#[derive(Clone, Debug)]
pub struct RunOutcome {
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
    pub name: String,
    pub output: String,
    pub status: ToolActivityStatus,
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error("prompt must not be empty")]
    EmptyPrompt,
    #[error("loop.max_steps must be greater than zero")]
    InvalidMaxSteps,
    #[error(transparent)]
    Inference(#[from] InferenceError),
    #[error(transparent)]
    Session(#[from] SessionStoreError),
}
