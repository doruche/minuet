use async_trait::async_trait;
use thiserror::Error;

use crate::{
    inference::{InferenceError, TokenUsage, ToolCall},
    session::{SessionStoreError, UsageSummary},
};

mod react;
mod runtime;

pub use react::ReactLoop;
pub use runtime::LoopContext;

#[async_trait]
pub trait AgentLoop: Send + Sync {
    async fn run(&self, context: &mut LoopContext<'_>) -> Result<RunOutcome, LoopError>;
}

pub struct CommittedModelTurn {
    pub tool_calls: Vec<ToolCall>,
    pub text: String,
    pub usage: Option<TokenUsage>,
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
}

#[derive(Clone, Debug)]
pub struct ToolActivity {
    pub name: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error("prompt must not be empty")]
    EmptyPrompt,
    #[error("loop.max_steps must be greater than zero")]
    InvalidMaxSteps,
    #[error("ReAct loop reached its limit of {0} model turns")]
    StepLimit(usize),
    #[error(transparent)]
    Inference(#[from] InferenceError),
    #[error(transparent)]
    Session(#[from] SessionStoreError),
}
