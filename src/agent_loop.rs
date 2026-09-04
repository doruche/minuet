use async_trait::async_trait;
use thiserror::Error;

use crate::{
    inference::{
        ConversationItem, InferenceBackend, InferenceError, InferenceRequest, OutputEffect,
        TokenUsage, ToolCall,
    },
    model::ReasoningEffort,
    session::{SessionId, SessionStore, SessionStoreError, UsageSummary},
    tool::{ToolDefinition, ToolRegistry},
};

#[async_trait]
pub trait AgentLoop: Send + Sync {
    async fn run(
        &self,
        context: &mut LoopContext<'_>,
        prompt: String,
    ) -> Result<RunOutcome, LoopError>;
}

pub struct ReactLoop {
    max_steps: usize,
}

impl ReactLoop {
    pub fn new(max_steps: usize) -> Result<Self, LoopError> {
        if max_steps == 0 {
            return Err(LoopError::InvalidMaxSteps);
        }
        Ok(Self { max_steps })
    }
}

#[async_trait]
impl AgentLoop for ReactLoop {
    async fn run(
        &self,
        context: &mut LoopContext<'_>,
        prompt: String,
    ) -> Result<RunOutcome, LoopError> {
        let PreparedRun {
            mut input,
            reasoning_effort,
            definitions,
        } = context.prepare(prompt)?;
        let mut activities = Vec::new();
        let mut run_usage = UsageSummary::default();

        for model_turn in 1..=self.max_steps {
            let turn = context
                .infer_and_commit(&input, &definitions, reasoning_effort.as_ref())
                .await?;
            input.extend(turn.output_items);
            run_usage.observe(turn.usage);

            if turn.tool_calls.is_empty() {
                return Ok(RunOutcome {
                    text: turn.text,
                    model_turns: model_turn,
                    tool_activity: activities,
                    usage: run_usage,
                });
            }
            if model_turn == self.max_steps {
                return Err(LoopError::StepLimit(self.max_steps));
            }

            let tool_round = context.invoke_and_commit(turn.tool_calls).await?;
            input.extend(tool_round.results);
            activities.extend(tool_round.activities);
        }

        unreachable!("max_steps is non-zero and every loop path returns or continues")
    }
}

/// A narrow mechanism capability provided to a loop component. It preserves
/// session commit ordering and tool visibility while leaving the loop in
/// control of when inference and tool rounds occur.
pub struct LoopContext<'a> {
    backend: &'a dyn InferenceBackend,
    store: &'a mut dyn SessionStore,
    tools: &'a ToolRegistry,
    session_id: SessionId,
    model: &'a str,
}

impl<'a> LoopContext<'a> {
    pub(crate) fn new(
        backend: &'a dyn InferenceBackend,
        store: &'a mut dyn SessionStore,
        tools: &'a ToolRegistry,
        session_id: SessionId,
        model: &'a str,
    ) -> Self {
        Self {
            backend,
            store,
            tools,
            session_id,
            model,
        }
    }

    fn prepare(&mut self, prompt: String) -> Result<PreparedRun, LoopError> {
        if prompt.trim().is_empty() {
            return Err(LoopError::EmptyPrompt);
        }

        let snapshot = self.store.snapshot(self.session_id)?;
        let user_item = ConversationItem::UserText(prompt);
        self.store
            .append(self.session_id, std::slice::from_ref(&user_item))?;
        let mut input = snapshot.items;
        input.push(user_item);
        Ok(PreparedRun {
            input,
            reasoning_effort: snapshot.reasoning_effort,
            definitions: self.tools.definitions(),
        })
    }

    async fn infer_and_commit(
        &mut self,
        input: &[ConversationItem],
        definitions: &[ToolDefinition],
        reasoning_effort: Option<&ReasoningEffort>,
    ) -> Result<CommittedModelTurn, LoopError> {
        let response = self
            .backend
            .respond(InferenceRequest {
                model: self.model,
                input,
                tools: definitions,
                reasoning_effort,
            })
            .await?;

        let usage = response.usage;
        let mut output_items = Vec::with_capacity(response.output.len());
        let mut tool_calls = Vec::new();
        let mut text = String::new();
        for item in response.output {
            match item.effect() {
                OutputEffect::None => {},
                OutputEffect::Text(fragment) => text.push_str(fragment),
                OutputEffect::ToolCall(call) => tool_calls.push(call.clone()),
            }
            output_items.push(ConversationItem::Continuation(item.into_continuation()));
        }

        // Receipt of a model response is the commit point. From here on it
        // remains in history even if a later tool round or request fails.
        self.store
            .commit_inference(self.session_id, &output_items, usage)?;
        Ok(CommittedModelTurn {
            output_items,
            tool_calls,
            text,
            usage,
        })
    }

    async fn invoke_and_commit(
        &mut self,
        tool_calls: Vec<ToolCall>,
    ) -> Result<CommittedToolRound, LoopError> {
        let mut results = Vec::with_capacity(tool_calls.len());
        let mut activities = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            let invocation = self.tools.invoke(&call.name, &call.arguments).await;
            results.push(ConversationItem::FunctionCallOutput {
                call_id: call.call_id,
                output: invocation.output.clone(),
            });
            activities.push(ToolActivity {
                name: call.name,
                output: invocation.output,
                is_error: invocation.is_error,
            });
        }
        self.store.append(self.session_id, &results)?;
        Ok(CommittedToolRound {
            results,
            activities,
        })
    }
}

struct PreparedRun {
    input: Vec<ConversationItem>,
    reasoning_effort: Option<ReasoningEffort>,
    definitions: Vec<ToolDefinition>,
}

struct CommittedModelTurn {
    output_items: Vec<ConversationItem>,
    tool_calls: Vec<ToolCall>,
    text: String,
    usage: Option<TokenUsage>,
}

struct CommittedToolRound {
    results: Vec<ConversationItem>,
    activities: Vec<ToolActivity>,
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
