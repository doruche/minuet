use async_trait::async_trait;
use thiserror::Error;

use crate::{
    context::ContextStrategy,
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
    async fn run(&self, context: &mut LoopContext<'_>) -> Result<RunOutcome, LoopError>;
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
    async fn run(&self, context: &mut LoopContext<'_>) -> Result<RunOutcome, LoopError> {
        let mut activities = Vec::new();
        let mut run_usage = UsageSummary::default();

        for model_turn in 1..=self.max_steps {
            let turn = context.infer_and_commit().await?;
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
    context: &'a dyn ContextStrategy,
    store: &'a mut dyn SessionStore,
    tools: &'a ToolRegistry,
    session_id: SessionId,
    model: &'a str,
    // This is a derived, run-scoped view of store-owned history. LoopContext is
    // its only mutator and refreshes it immediately after every store commit;
    // loop policy cannot access it directly or bypass ContextStrategy.
    input: Vec<ConversationItem>,
    // These immutable run snapshots come from the session and tool registry.
    // Kernel command serialization prevents either owner changing them until
    // the run completes; a later run always takes fresh snapshots.
    reasoning_effort: Option<ReasoningEffort>,
    definitions: Vec<ToolDefinition>,
}

impl<'a> LoopContext<'a> {
    pub(crate) fn new(
        backend: &'a dyn InferenceBackend,
        context: &'a dyn ContextStrategy,
        store: &'a mut dyn SessionStore,
        tools: &'a ToolRegistry,
        session_id: SessionId,
        model: &'a str,
        prompt: String,
    ) -> Result<Self, LoopError> {
        if prompt.trim().is_empty() {
            return Err(LoopError::EmptyPrompt);
        }

        let snapshot = store.snapshot(session_id)?;
        let user_item = ConversationItem::UserText(prompt);
        // Locally accepting user input commits it before network work. A failed
        // upstream call therefore leaves an observable unanswered user turn
        // instead of silently discarding input or guessing whether it ran.
        store.append(session_id, std::slice::from_ref(&user_item))?;
        let mut input = snapshot.items;
        input.push(user_item);
        let definitions = tools.definitions();

        Ok(Self {
            backend,
            context,
            store,
            tools,
            session_id,
            model,
            input,
            reasoning_effort: snapshot.reasoning_effort,
            definitions,
        })
    }

    pub async fn infer_and_commit(&mut self) -> Result<CommittedModelTurn, LoopError> {
        let prepared_input = self.context.prepare(&self.input);
        let response = self
            .backend
            .respond(InferenceRequest {
                model: self.model,
                input: &prepared_input,
                tools: &self.definitions,
                reasoning_effort: self.reasoning_effort.as_ref(),
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
        self.input.extend(output_items);
        Ok(CommittedModelTurn {
            tool_calls,
            text,
            usage,
        })
    }

    pub async fn invoke_and_commit(
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
        self.input.extend(results);
        Ok(CommittedToolRound { activities })
    }
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
