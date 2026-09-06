use std::sync::Arc;

use thiserror::Error;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    agent_loop::{AgentLoop, LoopContext, LoopError, RunEvent, RunOutcome},
    context::ContextStrategy,
    inference::{InferenceBackend, InferenceRequest},
    model::{IdentifierError, ModelSelection, ReasoningEffort},
    session::{
        SessionConfig, SessionId, SessionStore, SessionStoreError, TranscriptEntry, UsageSummary,
    },
    tool::{ToolRegistry, ToolRegistryError},
};

mod command;

use command::{Command, Envelope};
pub use command::{KernelHandle, RunRequest};

const COMMAND_BUFFER: usize = 16;

pub struct KernelOptions {
    pub model: ModelSelection,
    pub default_reasoning_effort: Option<ReasoningEffort>,
    pub default_enabled_tools: Vec<String>,
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
    let active_session = components.store.create(SessionConfig {
        reasoning_effort: options.default_reasoning_effort.clone(),
        enabled_tools: options.default_enabled_tools.clone(),
    })?;
    let (sender, receiver) = mpsc::channel(COMMAND_BUFFER);
    let task = KernelTask {
        backend: components.backend,
        store: components.store,
        tools: components.tools,
        agent_loop: components.agent_loop,
        context: components.context,
        model: options.model,
        default_reasoning_effort: options.default_reasoning_effort,
        default_enabled_tools: options.default_enabled_tools,
        active_session,
        receiver,
    };
    let join = tokio::spawn(task.run());
    Ok(RunningKernel {
        handle: KernelHandle::new(sender),
        join,
    })
}

/// Owns the task join. Call `shutdown` to wait for accepted work and resource
/// cleanup. Dropping this owner detaches the task; it continues until all handles
/// are dropped and queued work is drained (including any event backpressure).
#[must_use = "call shutdown to wait for kernel cleanup"]
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
    default_enabled_tools: Vec<String>,
    active_session: SessionId,
    receiver: mpsc::Receiver<Command>,
}

impl KernelTask {
    async fn run(mut self) {
        while let Some(command) = self.receiver.recv().await {
            // Successful enqueue hands execution to this task. Reply receivers
            // may detach, but that never cancels execution or session commits.
            match command {
                Command::Run(Envelope {
                    payload: run,
                    reply,
                }) => {
                    let _ = reply.send(self.run_loop(run.prompt, run.events).await);
                },
                Command::NewSession(Envelope { reply, .. }) => {
                    let _ = reply.send(self.new_session());
                },
                Command::ClearSession(Envelope { reply, .. }) => {
                    let _ = reply.send(self.clear_session());
                },
                Command::ActiveSession(Envelope { reply, .. }) => {
                    let _ = reply.send(Ok(self.active_session));
                },
                Command::ListSessions(Envelope { reply, .. }) => {
                    let _ = reply.send(Ok(self.store.list()));
                },
                Command::SwitchSession(Envelope { payload, reply }) => {
                    let _ = reply.send(self.switch_session(payload));
                },
                Command::DeleteSession(Envelope { payload, reply }) => {
                    let _ = reply.send(self.delete_session(payload));
                },
                Command::ModelInfo(Envelope { reply, .. }) => {
                    let _ = reply.send(self.model_info());
                },
                Command::SetReasoningEffort(Envelope {
                    payload: effort,
                    reply,
                }) => {
                    let result = self.set_reasoning_effort(effort);
                    let _ = reply.send(result);
                },
                Command::ListTools(Envelope { reply, .. }) => {
                    let result = self
                        .store
                        .snapshot(self.active_session)
                        .map(|s| self.tools.list_for(&s.config.enabled_tools))
                        .map_err(Into::into);
                    let _ = reply.send(result);
                },
                Command::SetToolEnabled(Envelope {
                    payload: change,
                    reply,
                }) => {
                    let result = self.set_tool_enabled(&change.name, change.enabled);
                    let _ = reply.send(result);
                },
                Command::ContextInfo(Envelope { reply, .. }) => {
                    let _ = reply.send(self.context_info().await);
                },
                Command::Shutdown(Envelope { reply, .. }) => {
                    // Reject further requests before acknowledging shutdown.
                    // Requests queued after this barrier are dropped, not executed.
                    self.receiver.close();
                    let _ = reply.send(Ok(()));
                    break;
                },
            }
        }
    }

    fn set_tool_enabled(&mut self, name: &str, enabled: bool) -> Result<(), KernelError> {
        let mut config = self.store.snapshot(self.active_session)?.config;
        if !self.tools.list().iter().any(|t| t.name == name) {
            return Err(ToolRegistryError::Unknown(name.to_owned()).into());
        }
        config.enabled_tools.retain(|n| n != name);
        if enabled {
            config.enabled_tools.push(name.to_owned());
        }
        self.store
            .set_config(self.active_session, config)
            .map_err(Into::into)
    }

    fn set_reasoning_effort(&mut self, effort: Option<ReasoningEffort>) -> Result<(), KernelError> {
        let mut config = self.store.snapshot(self.active_session)?.config;
        config.reasoning_effort = effort;
        self.store
            .set_config(self.active_session, config)
            .map_err(Into::into)
    }

    fn new_session(&mut self) -> Result<SessionView, KernelError> {
        let id = self.store.create(SessionConfig {
            reasoning_effort: self.default_reasoning_effort.clone(),
            enabled_tools: self.default_enabled_tools.clone(),
        })?;
        self.active_session = id;
        Ok(SessionView {
            id,
            entries: Vec::new(),
        })
    }

    fn switch_session(&mut self, id: SessionId) -> Result<SessionView, KernelError> {
        let entries = self.store.transcript(id)?;
        self.active_session = id;
        Ok(SessionView { id, entries })
    }
    fn delete_session(&mut self, id: SessionId) -> Result<(), KernelError> {
        if id == self.active_session {
            return Err(KernelError::ActiveSession);
        }
        self.store.delete(id).map_err(Into::into)
    }

    fn clear_session(&mut self) -> Result<SessionView, KernelError> {
        self.store.clear(self.active_session)?;
        Ok(SessionView {
            id: self.active_session,
            entries: Vec::new(),
        })
    }

    fn model_info(&self) -> Result<ModelInfo, KernelError> {
        let snapshot = self.store.snapshot(self.active_session)?;
        Ok(ModelInfo {
            provider: self.model.provider.to_string(),
            model: self.model.model.to_string(),
            reasoning_effort: snapshot
                .config
                .reasoning_effort
                .map(|effort| effort.to_string()),
        })
    }

    async fn context_info(&mut self) -> Result<ContextInfo, KernelError> {
        let snapshot = self.store.snapshot(self.active_session)?;
        let tool_snapshot = self.tools.snapshot(&snapshot.config.enabled_tools)?;
        let definitions = tool_snapshot.definitions();
        let prepared_input = self.context.prepare(&snapshot.items);
        let request = InferenceRequest {
            model: self.model.model.as_str(),
            input: &prepared_input,
            tools: &definitions,
            reasoning_effort: snapshot.config.reasoning_effort.as_ref(),
            observer: None,
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

    async fn run_loop(
        &mut self,
        prompt: String,
        events: Option<mpsc::Sender<RunEvent>>,
    ) -> Result<RunOutcome, KernelError> {
        let agent_loop = Arc::clone(&self.agent_loop);
        let session = self.store.snapshot(self.active_session)?;
        let tool_snapshot = self.tools.snapshot(&session.config.enabled_tools)?;
        let mut context = LoopContext::new(
            self.backend.as_ref(),
            self.context.as_ref(),
            self.store.as_mut(),
            &tool_snapshot,
            self.active_session,
            self.model.model.as_str(),
            prompt,
            crate::agent_loop::RunEvents(events),
        )?;
        let result = agent_loop.run(&mut context).await;
        context.finish(result).map_err(Into::into)
    }
}

/// A presentation snapshot taken at successful activation or clear. Entries
/// share immutable payloads with the repository; subsequent mutations may make
/// this view stale. It grants no authority over the active session or execution.
#[derive(Clone, Debug)]
pub struct SessionView {
    pub id: SessionId,
    pub entries: Vec<Arc<TranscriptEntry>>,
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
    #[error("the active session cannot be deleted")]
    ActiveSession,
    #[error(transparent)]
    SessionId(#[from] crate::session::SessionIdError),
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
            default_enabled_tools: vec!["echo".to_owned()],
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
                    output: vec![output(OutputEffect::Message("done".to_owned()))],
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
        let new_id = handle.new_session().await.unwrap().id;
        assert_ne!(new_id.to_string(), "");
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

        let (sender, mut receiver) = mpsc::channel(1);
        let run = tokio::spawn(async move {
            handle
                .run(RunRequest::with_events("loop forever", sender))
                .await
        });
        assert!(matches!(
            event(&mut receiver).await,
            RunEvent::InferenceStarted
        ));
        assert!(
            matches!(event(&mut receiver).await, RunEvent::ToolRoundCommitted { entries }
            if matches!(entries[0].as_ref(), TranscriptEntry::ToolInvocation {
                execution: crate::session::ToolExecution::Skipped(_), ..
            }))
        );
        assert!(
            receiver.recv().await.is_none(),
            "a skipped call must never start or emit output"
        );
        let outcome = run.await.unwrap().unwrap();
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
                    output: vec![output(OutputEffect::Message("continued".to_owned()))],
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

    struct StreamingTool {
        release: Arc<tokio::sync::Notify>,
        fail: bool,
    }

    #[async_trait]
    impl crate::tool::Tool for StreamingTool {
        fn definition(&self) -> crate::tool::ToolDefinition {
            crate::tool::ToolDefinition {
                name: "stream".into(),
                description: "test stream".into(),
                parameters: json!({"type": "object"}),
            }
        }

        async fn invoke(
            &self,
            _: serde_json::Value,
            output: &dyn crate::tool::ToolOutput,
        ) -> Result<serde_json::Value, crate::tool::ToolError> {
            output.write("开始").await;
            self.release.notified().await;
            output.write(&"中".repeat(5000)).await;
            output.write("完成\n").await;
            if self.fail {
                Err(crate::tool::ToolError::InvalidArguments(
                    "failed after output".into(),
                ))
            } else {
                Ok(json!({"result": "final result only"}))
            }
        }
    }

    fn streaming_kernel(
        fail: bool,
        followup: bool,
    ) -> (
        RunningKernel,
        Arc<ScriptedBackend>,
        Arc<tokio::sync::Notify>,
    ) {
        let mut responses = VecDeque::from([InferenceResponse {
            output: vec![output(OutputEffect::ToolCall(ToolCall {
                call_id: "stream-call".into(),
                name: "stream".into(),
                arguments: "{}".into(),
            }))],
            usage: None,
        }]);
        if followup {
            responses.push_back(InferenceResponse {
                output: vec![output(OutputEffect::Message("answer".into()))],
                usage: None,
            });
        }
        let backend = Arc::new(ScriptedBackend {
            responses: Mutex::new(responses),
            seen_inputs: Mutex::new(Vec::new()),
            input_tokens: 0,
        });
        let release = Arc::new(tokio::sync::Notify::new());
        let tools = ToolRegistry::new(
            [Arc::new(StreamingTool {
                release: release.clone(),
                fail,
            }) as Arc<dyn crate::tool::Tool>],
            &["stream".into()],
        )
        .unwrap();
        let running = start(
            KernelComponents {
                backend: backend.clone(),
                tools,
                store: Box::new(MemorySessionStore::default()),
                agent_loop: Arc::new(ReactLoop::new(4).unwrap()),
                context: Arc::new(FullContext),
            },
            {
                let mut opts = options();
                opts.default_enabled_tools = vec!["stream".into()];
                opts
            },
        )
        .unwrap();
        (running, backend, release)
    }

    async fn event(receiver: &mut mpsc::Receiver<RunEvent>) -> RunEvent {
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(3), receiver.recv())
                .await
                .expect("event stalled")
                .expect("event stream closed");
            if !matches!(
                event,
                RunEvent::ModelTextDelta { .. } | RunEvent::ModelTurnCommitted { .. }
            ) {
                return event;
            }
        }
    }

    #[tokio::test]
    async fn delivers_fragments_before_return_and_commits_only_the_final_result() {
        let (running, backend, release) = streaming_kernel(false, true);
        let handle = running.handle();
        let (sender, mut receiver) = mpsc::channel(1);
        let run =
            tokio::spawn(
                async move { handle.run(RunRequest::with_events("stream", sender)).await },
            );
        assert!(matches!(
            event(&mut receiver).await,
            RunEvent::InferenceStarted
        ));
        assert!(
            matches!(event(&mut receiver).await, RunEvent::ToolStarted { call_id, name }
            if call_id == "stream-call" && name == "stream")
        );
        assert!(
            matches!(event(&mut receiver).await, RunEvent::ToolOutput { call_id, text }
            if call_id == "stream-call" && text == "开始")
        );
        // This is a gate, not a timing assumption: the tool cannot return until
        // the observer has actually received its first unterminated fragment.
        assert!(!run.is_finished());
        release.notify_one();
        let mut streamed = String::new();
        loop {
            match event(&mut receiver).await {
                RunEvent::ToolOutput { call_id, text } => {
                    assert_eq!(call_id, "stream-call");
                    assert!(text.len() <= 4096);
                    streamed.push_str(&text);
                },
                RunEvent::ToolExecutionFinished { activity, .. } => {
                    assert_eq!(
                        activity.status,
                        crate::agent_loop::ToolActivityStatus::Completed
                    );
                    assert_eq!(activity.output, r#"{"result":"final result only"}"#);
                    break;
                },
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(streamed, format!("{}完成\n", "中".repeat(5000)));
        assert!(
            matches!(event(&mut receiver).await, RunEvent::ToolRoundCommitted { entries }
            if matches!(entries[0].as_ref(), TranscriptEntry::ToolInvocation {
                execution: crate::session::ToolExecution::Completed(_), ..
            }))
        );
        assert!(matches!(
            event(&mut receiver).await,
            RunEvent::InferenceStarted
        ));
        assert_eq!(run.await.unwrap().unwrap().text, "answer");
        assert!(matches!(
            receiver.recv().await,
            Some(RunEvent::ModelTurnCommitted { entries })
                if matches!(entries[0].as_ref(), TranscriptEntry::ModelMessage { text } if text == "answer")
        ));
        assert!(receiver.recv().await.is_none());
        {
            let seen = backend.seen_inputs.lock().unwrap();
            assert!(
                matches!(seen[1].last(), Some(ConversationItem::FunctionCallOutput { output, .. })
                if output == r#"{"result":"final result only"}"#)
            );
        }
        running.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn partial_output_and_tool_error_remain_observable_if_later_inference_fails() {
        let (running, _, release) = streaming_kernel(true, false);
        let handle = running.handle();
        let (sender, mut receiver) = mpsc::channel(1);
        let run =
            tokio::spawn(
                async move { handle.run(RunRequest::with_events("stream", sender)).await },
            );
        event(&mut receiver).await;
        event(&mut receiver).await;
        assert!(matches!(
            event(&mut receiver).await,
            RunEvent::ToolOutput { .. }
        ));
        release.notify_one();
        let mut saw_failure = false;
        while let Some(event) = receiver.recv().await {
            if let RunEvent::ToolExecutionFinished { activity, .. } = event {
                assert_eq!(
                    activity.status,
                    crate::agent_loop::ToolActivityStatus::Error
                );
                assert!(activity.output.contains("failed after output"));
                saw_failure = true;
            }
        }
        assert!(saw_failure);
        assert!(run.await.unwrap().is_err());
        running.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn detaching_observer_and_result_allows_execution_and_shutdown_to_finish() {
        let (running, backend, release) = streaming_kernel(false, true);
        let handle = running.handle();
        let (sender, mut receiver) = mpsc::channel(1);
        let run =
            tokio::spawn(
                async move { handle.run(RunRequest::with_events("stream", sender)).await },
            );
        event(&mut receiver).await;
        event(&mut receiver).await;
        event(&mut receiver).await;
        // Drop the caller's wait, not the kernel-owned execution. The next write
        // spans several queue slots, proving a closed observer cannot block it.
        run.abort();
        assert!(run.await.unwrap_err().is_cancelled());
        drop(receiver);
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(3), running.shutdown())
            .await
            .expect("shutdown blocked on detached observer")
            .unwrap();
        assert_eq!(backend.seen_inputs.lock().unwrap().len(), 2);
    }
    #[tokio::test]
    async fn shutdown_finishes_prior_commands_and_rejects_later_ones() {
        let (running, _, release) = streaming_kernel(false, true);
        let handle = running.handle();
        let (sender, mut receiver) = mpsc::channel(1);
        let run_handle = handle.clone();
        let run = tokio::spawn(async move {
            run_handle
                .run(RunRequest::with_events("stream", sender))
                .await
        });
        event(&mut receiver).await;
        event(&mut receiver).await;
        event(&mut receiver).await;
        // The tool is gated, so each poll below can only enqueue its request.
        let before = handle.new_session();
        let shutdown = running.shutdown();
        let after = handle.new_session();
        tokio::pin!(before, shutdown, after);
        tokio::select! {
            biased;
            _ = &mut before => panic!("run must retain the sequencer"),
            () = std::future::ready(()) => {},
        }
        tokio::select! {
            biased;
            _ = &mut shutdown => panic!("shutdown must wait for prior work"),
            () = std::future::ready(()) => {},
        }
        tokio::select! {
            biased;
            _ = &mut after => panic!("shutdown has not yet reached the sequencer"),
            () = std::future::ready(()) => {},
        }
        drop(receiver);
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            assert!(run.await.unwrap().is_ok());
            assert!(before.await.is_ok());
            shutdown.await.unwrap();
            assert!(matches!(after.await, Err(KernelError::Stopped)));
            assert!(matches!(
                handle.model_info().await,
                Err(KernelError::Stopped)
            ));
        })
        .await
        .expect("shutdown stalled");
    }
    #[tokio::test]
    async fn activation_views_isolate_history_and_failed_switch_keeps_active_session() {
        let backend = Arc::new(ScriptedBackend {
            responses: Mutex::new(VecDeque::from([
                InferenceResponse {
                    output: vec![output(OutputEffect::Message("answer A".into()))],
                    usage: Some(usage(2, 3)),
                },
                InferenceResponse {
                    output: vec![output(OutputEffect::Message("answer B".into()))],
                    usage: None,
                },
            ])),
            seen_inputs: Mutex::new(Vec::new()),
            input_tokens: 0,
        });
        let running = start(
            KernelComponents {
                backend: backend.clone(),
                store: Box::new(MemorySessionStore::default()),
                tools: ToolRegistry::with_builtins(&[]).unwrap(),
                agent_loop: Arc::new(ReactLoop::new(2).unwrap()),
                context: Arc::new(FullContext),
            },
            options(),
        )
        .unwrap();
        let h = running.handle();
        let a = h.active_session().await.unwrap();
        h.run("question A").await.unwrap();
        let b = h.new_session().await.unwrap();
        assert!(b.entries.is_empty());
        h.run("question B").await.unwrap();
        let a_view = h.switch_session(a).await.unwrap();
        assert_eq!(a_view.id, a);
        assert_eq!(a_view.entries.len(), 2);
        assert!(
            matches!(&*a_view.entries[1], TranscriptEntry::ModelMessage { text } if text == "answer A")
        );
        assert_eq!(backend.seen_inputs.lock().unwrap()[1].len(), 1);
        let missing = SessionId::new();
        assert!(h.switch_session(missing).await.is_err());
        assert_eq!(h.active_session().await.unwrap(), a);
        assert!(h.delete_session(a).await.is_err());
        // Inference failure retains accepted input, never fabricates model text.
        assert!(h.run("unanswered").await.is_err());
        let failed = h.switch_session(a).await.unwrap();
        assert_eq!(failed.entries.len(), 3);
        assert!(
            matches!(&*failed.entries[2], TranscriptEntry::UserMessage { text } if text == "unanswered")
        );
        assert_eq!(a_view.entries.len(), 2);
        let cleared = h.clear_session().await.unwrap();
        assert_eq!(cleared.id, a);
        assert!(cleared.entries.is_empty());
        assert_eq!(
            h.context_info().await.unwrap().usage,
            UsageSummary::default()
        );
        let b_view = h.switch_session(b.id).await.unwrap();
        assert_eq!(b_view.entries.len(), 2);
        h.delete_session(a).await.unwrap();
        assert!(h.switch_session(a).await.is_err());
        assert_eq!(h.active_session().await.unwrap(), b.id);
        assert_eq!(backend.seen_inputs.lock().unwrap().len(), 3);
        running.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn switch_waits_for_run_and_captures_the_committed_transcript() {
        let (running, backend, release) = streaming_kernel(false, true);
        let h = running.handle();
        let id = h.active_session().await.unwrap();
        let run_handle = h.clone();
        let (sender, mut receiver) = mpsc::channel(1);
        let run = tokio::spawn(async move {
            run_handle
                .run(RunRequest::with_events("stream", sender))
                .await
        });
        event(&mut receiver).await;
        event(&mut receiver).await;
        event(&mut receiver).await;
        let switch = h.switch_session(id);
        tokio::pin!(switch);
        tokio::select! {
            biased;
            _ = &mut switch => panic!("switch must wait for run"),
            () = std::future::ready(()) => {},
        }
        drop(receiver);
        release.notify_one();
        let view = tokio::time::timeout(std::time::Duration::from_secs(3), switch)
            .await
            .unwrap()
            .unwrap();
        assert!(run.await.unwrap().is_ok());
        assert_eq!(view.id, id);
        assert!(view.entries.iter().any(|entry| matches!(
            entry.as_ref(),
            TranscriptEntry::ToolInvocation {
                execution: crate::session::ToolExecution::Completed(_),
                ..
            }
        )));
        assert_eq!(backend.seen_inputs.lock().unwrap().len(), 2);
        running.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn kernel_rejects_policy_completion_with_unresolved_calls() {
        struct UnfinishedPolicy;
        #[async_trait]
        impl AgentLoop for UnfinishedPolicy {
            async fn run(&self, context: &mut LoopContext<'_>) -> Result<RunOutcome, LoopError> {
                let turn = context.infer_and_commit().await?;
                Ok(RunOutcome {
                    text: turn.text_summary(),
                    model_turns: 1,
                    tool_activity: vec![],
                    usage: UsageSummary::default(),
                    stop_reason: crate::agent_loop::RunStopReason::Completed,
                })
            }
        }
        let backend = Arc::new(ScriptedBackend {
            responses: Mutex::new(VecDeque::from([InferenceResponse {
                output: vec![output(OutputEffect::ToolCall(ToolCall {
                    call_id: "x".into(),
                    name: "echo".into(),
                    arguments: "{}".into(),
                }))],
                usage: None,
            }])),
            seen_inputs: Mutex::new(Vec::new()),
            input_tokens: 0,
        });
        let running = start(
            KernelComponents {
                backend: backend.clone(),
                store: Box::new(MemorySessionStore::default()),
                tools: ToolRegistry::with_builtins(&["echo".into()]).unwrap(),
                agent_loop: Arc::new(UnfinishedPolicy),
                context: Arc::new(FullContext),
            },
            options(),
        )
        .unwrap();
        let h = running.handle();
        let id = h.active_session().await.unwrap();
        assert!(h.run("first").await.is_err());
        let before = h.switch_session(id).await.unwrap();
        assert_eq!(before.entries.len(), 2);
        assert!(matches!(
            before.entries[1].as_ref(),
            TranscriptEntry::ToolInvocation {
                execution: crate::session::ToolExecution::Pending,
                ..
            }
        ));
        assert!(h.run("must not append").await.is_err());
        assert_eq!(h.switch_session(id).await.unwrap().entries, before.entries);
        assert_eq!(backend.seen_inputs.lock().unwrap().len(), 1);
        running.shutdown().await.unwrap();
    }
}
