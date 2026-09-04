use crate::{
    context::ContextStrategy,
    inference::{ConversationItem, InferenceBackend, InferenceRequest, OutputEffect, ToolCall},
    model::ReasoningEffort,
    session::{SessionId, SessionStore},
    tool::{ToolDefinition, ToolRegistry},
};

use super::{CommittedModelTurn, CommittedToolRound, LoopError, ToolActivity};
use super::{PendingToolRound, ToolActivityStatus};

const STEP_LIMIT_OUTPUT: &str =
    "tool call was not executed because the run reached its model-turn limit";

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
            tool_calls: PendingToolRound { calls: tool_calls },
            text,
            usage,
        })
    }

    pub async fn invoke_and_commit(
        &mut self,
        pending: PendingToolRound,
    ) -> Result<CommittedToolRound, LoopError> {
        self.commit_tool_calls(pending.calls).await
    }

    pub fn skip_and_commit(
        &mut self,
        pending: PendingToolRound,
    ) -> Result<CommittedToolRound, LoopError> {
        let mut results = Vec::with_capacity(pending.calls.len());
        let mut activities = Vec::with_capacity(pending.calls.len());
        for call in pending.calls {
            results.push(ConversationItem::FunctionCallOutput {
                call_id: call.call_id,
                output: serde_json::json!({
                    "error": {
                        "kind": "step_limit",
                        "message": STEP_LIMIT_OUTPUT
                    }
                })
                .to_string(),
            });
            activities.push(ToolActivity {
                name: call.name,
                output: STEP_LIMIT_OUTPUT.to_owned(),
                status: ToolActivityStatus::Skipped,
            });
        }
        self.store.append(self.session_id, &results)?;
        self.input.extend(results);
        Ok(CommittedToolRound { activities })
    }

    async fn commit_tool_calls(
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
                status: if invocation.is_error {
                    ToolActivityStatus::Error
                } else {
                    ToolActivityStatus::Completed
                },
            });
        }
        self.store.append(self.session_id, &results)?;
        self.input.extend(results);
        Ok(CommittedToolRound { activities })
    }
}
