use std::sync::Arc;

use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{
    agent_loop::{AgentLoop, LoopContext, LoopError, RunOutcome},
    context::ContextStrategy,
    inference::{InferenceBackend, InferenceRequest},
    model::{IdentifierError, ModelSelection, ReasoningEffort},
    session::{SessionId, SessionStore, SessionStoreError, UsageSummary},
    tool::{ToolRegistry, ToolRegistryError, ToolStatus},
};

const COMMAND_BUFFER: usize = 16;

pub struct KernelOptions {
    pub model: ModelSelection,
    pub default_reasoning_effort: Option<ReasoningEffort>,
}

pub struct KernelComponents {
    pub backend: Arc<dyn InferenceBackend>,
    pub store: Box<dyn SessionStore>,
    pub tools: ToolRegistry,
    pub agent_loop: Arc<dyn AgentLoop>,
    pub context: Arc<dyn ContextStrategy>,
}

/// Starts the micro-kernel task. The task is the sole transition owner for the
/// active session and tool registry; handles can request transitions but never
/// receive the underlying store or registry.
pub fn start(
    mut components: KernelComponents,
    options: KernelOptions,
) -> Result<RunningKernel, KernelError> {
    let active_session = components
        .store
        .create(options.default_reasoning_effort.clone())?;
    let (sender, receiver) = mpsc::channel(COMMAND_BUFFER);
    let task = KernelTask {
        backend: components.backend,
        store: components.store,
        tools: components.tools,
        agent_loop: components.agent_loop,
        context: components.context,
        model: options.model,
        default_reasoning_effort: options.default_reasoning_effort,
        active_session,
        receiver,
    };
    let join = tokio::spawn(task.run());
    Ok(RunningKernel {
        handle: KernelHandle { sender },
        join,
    })
}

#[derive(Clone)]
pub struct KernelHandle {
    sender: mpsc::Sender<Command>,
}

impl KernelHandle {
    pub async fn run(&self, prompt: impl Into<String>) -> Result<RunOutcome, KernelError> {
        self.request(|reply| Command::Run {
            prompt: prompt.into(),
            reply,
        })
        .await
    }

    pub async fn new_session(&self) -> Result<SessionId, KernelError> {
        self.request(|reply| Command::NewSession { reply }).await
    }

    pub async fn model_info(&self) -> Result<ModelInfo, KernelError> {
        self.request(|reply| Command::ModelInfo { reply }).await
    }

    pub async fn set_reasoning_effort(&self, effort: Option<String>) -> Result<(), KernelError> {
        let effort = effort.map(ReasoningEffort::new).transpose()?;
        self.request(|reply| Command::SetReasoningEffort { effort, reply })
            .await
    }

    pub async fn list_tools(&self) -> Result<Vec<ToolStatus>, KernelError> {
        self.request(|reply| Command::ListTools { reply }).await
    }

    pub async fn set_tool_enabled(
        &self,
        name: impl Into<String>,
        enabled: bool,
    ) -> Result<(), KernelError> {
        self.request(|reply| Command::SetToolEnabled {
            name: name.into(),
            enabled,
            reply,
        })
        .await
    }

    pub async fn context_info(&self) -> Result<ContextInfo, KernelError> {
        self.request(|reply| Command::ContextInfo { reply }).await
    }

    async fn shutdown(&self) -> Result<(), KernelError> {
        self.request(|reply| Command::Shutdown { reply }).await
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, KernelError>>) -> Command,
    ) -> Result<T, KernelError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(command(reply))
            .await
            .map_err(|_| KernelError::Stopped)?;
        response.await.map_err(|_| KernelError::Stopped)?
    }
}

pub struct RunningKernel {
    handle: KernelHandle,
    join: JoinHandle<()>,
}

impl RunningKernel {
    pub fn handle(&self) -> KernelHandle {
        self.handle.clone()
    }

    pub async fn shutdown(self) -> Result<(), KernelError> {
        let shutdown_result = self.handle.shutdown().await;
        self.join
            .await
            .map_err(|error| KernelError::TaskJoin(error.to_string()))?;
        shutdown_result
    }
}

struct KernelTask {
    backend: Arc<dyn InferenceBackend>,
    store: Box<dyn SessionStore>,
    tools: ToolRegistry,
    agent_loop: Arc<dyn AgentLoop>,
    context: Arc<dyn ContextStrategy>,
    model: ModelSelection,
    default_reasoning_effort: Option<ReasoningEffort>,
    active_session: SessionId,
    receiver: mpsc::Receiver<Command>,
}

impl KernelTask {
    async fn run(mut self) {
        while let Some(command) = self.receiver.recv().await {
            match command {
                Command::Run { prompt, reply } => {
                    let result = self.run_loop(prompt).await;
                    let _ = reply.send(result);
                },
                Command::NewSession { reply } => {
                    let result = self.new_session();
                    let _ = reply.send(result);
                },
                Command::ModelInfo { reply } => {
                    let result = self.model_info();
                    let _ = reply.send(result);
                },
                Command::SetReasoningEffort { effort, reply } => {
                    let result = self
                        .store
                        .set_reasoning_effort(self.active_session, effort)
                        .map_err(Into::into);
                    let _ = reply.send(result);
                },
                Command::ListTools { reply } => {
                    let _ = reply.send(Ok(self.tools.list()));
                },
                Command::SetToolEnabled {
                    name,
                    enabled,
                    reply,
                } => {
                    let result = self.tools.set_enabled(&name, enabled).map_err(Into::into);
                    let _ = reply.send(result);
                },
                Command::ContextInfo { reply } => {
                    let result = self.context_info().await;
                    let _ = reply.send(result);
                },
                Command::Shutdown { reply } => {
                    let _ = reply.send(Ok(()));
                    break;
                },
            }
        }
    }

    fn new_session(&mut self) -> Result<SessionId, KernelError> {
        let id = self.store.create(self.default_reasoning_effort.clone())?;
        self.active_session = id;
        Ok(id)
    }

    fn model_info(&self) -> Result<ModelInfo, KernelError> {
        let snapshot = self.store.snapshot(self.active_session)?;
        Ok(ModelInfo {
            provider: self.model.provider.to_string(),
            model: self.model.model.to_string(),
            reasoning_effort: snapshot.reasoning_effort.map(|effort| effort.to_string()),
        })
    }

    async fn context_info(&mut self) -> Result<ContextInfo, KernelError> {
        let snapshot = self.store.snapshot(self.active_session)?;
        let definitions = self.tools.definitions();
        let prepared_input = self.context.prepare(&snapshot.items);
        let request = InferenceRequest {
            model: self.model.model.as_str(),
            input: &prepared_input,
            tools: &definitions,
            reasoning_effort: snapshot.reasoning_effort.as_ref(),
        };
        let input_tokens = match self.backend.count_input_tokens(request).await {
            Ok(tokens) => InputTokenCount::Available(tokens),
            Err(error) => InputTokenCount::Unavailable(error.to_string()),
        };
        Ok(ContextInfo {
            session_id: snapshot.id,
            committed_input_tokens: input_tokens,
            usage: snapshot.usage,
        })
    }

    async fn run_loop(&mut self, prompt: String) -> Result<RunOutcome, KernelError> {
        let agent_loop = Arc::clone(&self.agent_loop);
        let mut context = LoopContext::new(
            self.backend.as_ref(),
            self.context.as_ref(),
            self.store.as_mut(),
            &self.tools,
            self.active_session,
            self.model.model.as_str(),
            prompt,
        )?;
        agent_loop.run(&mut context).await.map_err(Into::into)
    }
}

enum Command {
    Run {
        prompt: String,
        reply: oneshot::Sender<Result<RunOutcome, KernelError>>,
    },
    NewSession {
        reply: oneshot::Sender<Result<SessionId, KernelError>>,
    },
    ModelInfo {
        reply: oneshot::Sender<Result<ModelInfo, KernelError>>,
    },
    SetReasoningEffort {
        effort: Option<ReasoningEffort>,
        reply: oneshot::Sender<Result<(), KernelError>>,
    },
    ListTools {
        reply: oneshot::Sender<Result<Vec<ToolStatus>, KernelError>>,
    },
    SetToolEnabled {
        name: String,
        enabled: bool,
        reply: oneshot::Sender<Result<(), KernelError>>,
    },
    ContextInfo {
        reply: oneshot::Sender<Result<ContextInfo, KernelError>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), KernelError>>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelInfo {
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ContextInfo {
    pub session_id: SessionId,
    pub committed_input_tokens: InputTokenCount,
    pub usage: UsageSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputTokenCount {
    Available(u64),
    Unavailable(String),
}

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("kernel task has stopped")]
    Stopped,
    #[error("kernel task failed: {0}")]
    TaskJoin(String),
    #[error(transparent)]
    AgentLoop(#[from] LoopError),
    #[error(transparent)]
    Identifier(#[from] IdentifierError),
    #[error(transparent)]
    Inference(#[from] crate::inference::InferenceError),
    #[error(transparent)]
    Session(#[from] SessionStoreError),
    #[error(transparent)]
    Tool(#[from] ToolRegistryError),
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::{
        agent_loop::ReactLoop,
        context::FullContext,
        inference::{
            ContinuationItem, ConversationItem, InferenceError, InferenceResponse, ModelOutputItem,
            OutputEffect, TokenUsage, ToolCall,
        },
        model::{ModelName, ProviderId},
        session::MemorySessionStore,
    };

    struct ScriptedBackend {
        responses: Mutex<VecDeque<InferenceResponse>>,
        seen_inputs: Mutex<Vec<Vec<ConversationItem>>>,
        input_tokens: u64,
    }

    #[async_trait]
    impl InferenceBackend for ScriptedBackend {
        async fn respond(
            &self,
            request: InferenceRequest<'_>,
        ) -> Result<InferenceResponse, InferenceError> {
            self.seen_inputs
                .lock()
                .unwrap()
                .push(request.input.to_vec());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| InferenceError::new("script exhausted"))
        }

        async fn count_input_tokens(
            &self,
            _request: InferenceRequest<'_>,
        ) -> Result<u64, InferenceError> {
            Ok(self.input_tokens)
        }
    }

    fn output(effect: OutputEffect) -> ModelOutputItem {
        ModelOutputItem::new(
            ContinuationItem::for_test(json!({"type":"test-continuation"})),
            effect,
        )
    }

    fn usage(input: u64, output: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
        }
    }

    fn options() -> KernelOptions {
        KernelOptions {
            model: ModelSelection {
                provider: ProviderId::new("test").unwrap(),
                model: ModelName::new("test-model").unwrap(),
            },
            default_reasoning_effort: None,
        }
    }

    #[tokio::test]
    async fn executes_tool_calls_and_commits_each_model_response() {
        let backend = Arc::new(ScriptedBackend {
            responses: Mutex::new(VecDeque::from([
                InferenceResponse {
                    output: vec![output(OutputEffect::ToolCall(ToolCall {
                        call_id: "call-1".to_owned(),
                        name: "echo".to_owned(),
                        arguments: r#"{"text":"hello"}"#.to_owned(),
                    }))],
                    usage: Some(usage(10, 3)),
                },
                InferenceResponse {
                    output: vec![output(OutputEffect::Text("done".to_owned()))],
                    usage: Some(usage(15, 2)),
                },
            ])),
            seen_inputs: Mutex::new(Vec::new()),
            input_tokens: 123,
        });
        let running = start(
            KernelComponents {
                backend: backend.clone(),
                store: Box::new(MemorySessionStore::default()),
                tools: ToolRegistry::with_builtins(&["echo".to_owned()]).unwrap(),
                agent_loop: Arc::new(ReactLoop::new(4).unwrap()),
                context: Arc::new(FullContext),
            },
            options(),
        )
        .unwrap();
        let handle = running.handle();

        let outcome = handle.run("use echo").await.unwrap();
        assert_eq!(outcome.text, "done");
        assert_eq!(outcome.model_turns, 2);
        assert_eq!(outcome.tool_activity.len(), 1);
        assert_eq!(outcome.usage.total_tokens, 30);

        {
            let seen = backend.seen_inputs.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert!(matches!(
                seen[1].last(),
                Some(ConversationItem::FunctionCallOutput { call_id, .. }) if call_id == "call-1"
            ));
        }

        let context = handle.context_info().await.unwrap();
        assert_eq!(
            context.committed_input_tokens,
            InputTokenCount::Available(123)
        );
        assert_eq!(context.usage.reported_calls, 2);
        assert_eq!(context.usage.total_tokens, 30);
        running.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn effort_is_session_state_and_is_not_validated() {
        let backend = Arc::new(ScriptedBackend {
            responses: Mutex::new(VecDeque::new()),
            seen_inputs: Mutex::new(Vec::new()),
            input_tokens: 0,
        });
        let running = start(
            KernelComponents {
                backend,
                store: Box::new(MemorySessionStore::default()),
                tools: ToolRegistry::with_builtins(&[]).unwrap(),
                agent_loop: Arc::new(ReactLoop::new(1).unwrap()),
                context: Arc::new(FullContext),
            },
            options(),
        )
        .unwrap();
        let handle = running.handle();

        handle
            .set_reasoning_effort(Some("vendor-depth-42".to_owned()))
            .await
            .unwrap();
        assert_eq!(
            handle.model_info().await.unwrap().reasoning_effort,
            Some("vendor-depth-42".to_owned())
        );
        let new_id = handle.new_session().await.unwrap();
        assert_eq!(new_id.to_string(), "1");
        assert_eq!(handle.model_info().await.unwrap().reasoning_effort, None);
        running.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn does_not_execute_tools_after_the_model_turn_limit() {
        let backend = Arc::new(ScriptedBackend {
            responses: Mutex::new(VecDeque::from([InferenceResponse {
                output: vec![output(OutputEffect::ToolCall(ToolCall {
                    call_id: "call-1".to_owned(),
                    name: "echo".to_owned(),
                    arguments: r#"{"text":"must-not-run"}"#.to_owned(),
                }))],
                usage: Some(usage(1, 1)),
            }])),
            seen_inputs: Mutex::new(Vec::new()),
            input_tokens: 0,
        });
        let running = start(
            KernelComponents {
                backend,
                store: Box::new(MemorySessionStore::default()),
                tools: ToolRegistry::with_builtins(&["echo".to_owned()]).unwrap(),
                agent_loop: Arc::new(ReactLoop::new(1).unwrap()),
                context: Arc::new(FullContext),
            },
            options(),
        )
        .unwrap();
        let handle = running.handle();

        let outcome = handle.run("loop forever").await.unwrap();
        assert_eq!(
            outcome.stop_reason,
            crate::agent_loop::RunStopReason::StepLimit
        );
        assert_eq!(outcome.tool_activity.len(), 1);
        assert_eq!(
            outcome.tool_activity[0].status,
            crate::agent_loop::ToolActivityStatus::Skipped
        );
        running.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_limited_run_closes_tool_calls_for_the_next_session_turn() {
        let backend = Arc::new(ScriptedBackend {
            responses: Mutex::new(VecDeque::from([
                InferenceResponse {
                    output: vec![output(OutputEffect::ToolCall(ToolCall {
                        call_id: "call-limited".to_owned(),
                        name: "echo".to_owned(),
                        arguments: r#"{"text":"must-not-run"}"#.to_owned(),
                    }))],
                    usage: Some(usage(1, 1)),
                },
                InferenceResponse {
                    output: vec![output(OutputEffect::Text("continued".to_owned()))],
                    usage: Some(usage(2, 1)),
                },
            ])),
            seen_inputs: Mutex::new(Vec::new()),
            input_tokens: 0,
        });
        let running = start(
            KernelComponents {
                backend: backend.clone(),
                store: Box::new(MemorySessionStore::default()),
                tools: ToolRegistry::with_builtins(&["echo".to_owned()]).unwrap(),
                agent_loop: Arc::new(ReactLoop::new(1).unwrap()),
                context: Arc::new(FullContext),
            },
            options(),
        )
        .unwrap();
        let handle = running.handle();

        let limited = handle.run("use echo").await.unwrap();
        assert_eq!(
            limited.stop_reason,
            crate::agent_loop::RunStopReason::StepLimit
        );
        assert_eq!(
            limited.tool_activity[0].status,
            crate::agent_loop::ToolActivityStatus::Skipped
        );

        let continued = handle.run("continue").await.unwrap();
        assert_eq!(
            continued.stop_reason,
            crate::agent_loop::RunStopReason::Completed
        );
        assert_eq!(continued.text, "continued");

        {
            let seen = backend.seen_inputs.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert!(seen[1].iter().any(|item| {
                matches!(
                    item,
                    ConversationItem::FunctionCallOutput { call_id, output }
                        if call_id == "call-limited" && output.contains("step_limit")
                )
            }));
        }
        running.shutdown().await.unwrap();
    }
}
