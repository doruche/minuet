use std::time::{Duration, Instant};

use tokio::time::timeout;

use crate::{
    context::ContextStrategy,
    inference::{InferenceBackend, InferenceError, InferenceRequest, ToolCall},
    model::ReasoningEffort,
    session::{SessionId, SessionStore, ToolOutcome},
    tool::{ToolDefinition, ToolSnapshot, encode_error},
};

use super::{
    CommittedModelTurn, CommittedToolRound, LoopError, RunEvent, RunOutcome, ToolActivity,
    ToolActivityStatus, events::RunEvents,
};

const STEP_LIMIT_OUTPUT: &str =
    "tool call was not executed because the run reached its model-turn limit";

/// Mechanism capability for loop policy. The repository owns committed facts;
/// this context owns the one opportunity to execute each committed call round.
pub struct LoopContext<'a> {
    events: RunEvents,
    backend: &'a dyn InferenceBackend,
    context: &'a dyn ContextStrategy,
    store: &'a mut dyn SessionStore,
    tools: &'a ToolSnapshot,
    session_id: SessionId,
    model: &'a str,
    // Taken before execution or skipping. Absence is NOT proof of commitment:
    // cancellation or commit failure leaves repository-owned results unresolved.
    // Neither path restores these requests or permits implicit re-execution.
    pending: Option<Vec<ToolCall>>,
    // Immutable run snapshots. Kernel serialization prevents these owners
    // changing policy until the run completes; subsequent runs refresh them.
    reasoning_effort: Option<ReasoningEffort>,
    definitions: Vec<ToolDefinition>,
}

impl<'a> LoopContext<'a> {
    #[expect(
        clippy::too_many_arguments,
        reason = "explicit borrowed owner capabilities and run input"
    )]
    pub(crate) fn new(
        backend: &'a dyn InferenceBackend,
        context: &'a dyn ContextStrategy,
        store: &'a mut dyn SessionStore,
        tools: &'a ToolSnapshot,
        session_id: SessionId,
        model: &'a str,
        prompt: String,
        events: RunEvents,
    ) -> Result<Self, LoopError> {
        if prompt.trim().is_empty() {
            return Err(LoopError::EmptyPrompt);
        }
        store.ensure_ready(session_id)?;
        let snapshot = store.snapshot(session_id)?;
        // Accept input before network work, but never append to an unresolved
        // round left by an earlier internal protocol failure.
        store.commit_user(session_id, &prompt)?;
        Ok(Self {
            events,
            backend,
            context,
            store,
            tools,
            session_id,
            model,
            pending: None,
            reasoning_effort: snapshot.config.reasoning_effort,
            definitions: tools.definitions(),
        })
    }

    fn ensure_ready(&self) -> Result<(), LoopError> {
        self.store.ensure_ready(self.session_id)?;
        if self.pending.is_some() {
            return Err(LoopError::Protocol("unconsumed execution requests"));
        }
        Ok(())
    }

    /// Checks both execution and commit obligations even when policy returns
    /// early. Preserve the originating failure when reporting an open round.
    pub(crate) fn finish(
        &self,
        result: Result<RunOutcome, LoopError>,
    ) -> Result<RunOutcome, LoopError> {
        match (result, self.ensure_ready()) {
            (result, Ok(())) => result,
            (Ok(_), Err(pending)) => Err(pending),
            (Err(source), Err(pending)) => Err(LoopError::UnfinishedRun {
                source: Box::new(source),
                pending: Box::new(pending),
            }),
        }
    }

    /// Whether policy can choose execution or skipping of the current round.
    /// This is not an observation of whether its results have committed.
    pub fn has_pending_tools(&self) -> bool {
        self.pending.is_some()
    }

    pub async fn infer_and_commit(&mut self) -> Result<CommittedModelTurn, LoopError> {
        self.ensure_ready()?;
        let snapshot = self.store.snapshot(self.session_id)?;
        let prepared_input = self.context.prepare(&snapshot.items);
        self.events.send(RunEvent::InferenceStarted).await;
        let response = timeout(
            Duration::from_secs(120),
            self.backend.respond(InferenceRequest {
                model: self.model,
                input: &prepared_input,
                tools: &self.definitions,
                reasoning_effort: self.reasoning_effort.as_ref(),
                observer: Some(&self.events),
            }),
        )
        .await
        .map_err(|_| LoopError::Inference(InferenceError::new("inference request timed out")))??;

        let committed =
            self.store
                .commit_inference(self.session_id, &response.output, response.usage)?;
        // Install the execution opportunity before awaiting publication. If a
        // policy drops that future, the completion check still sees the round.
        self.pending = (!committed.calls.is_empty()).then_some(committed.calls);
        self.events
            .send(RunEvent::ModelTurnCommitted {
                entries: committed.entries.clone(),
            })
            .await;
        Ok(CommittedModelTurn {
            entries: committed.entries,
            usage: response.usage,
        })
    }

    fn take_pending(&mut self) -> Result<Vec<ToolCall>, LoopError> {
        self.pending.take().ok_or(LoopError::Protocol(
            "no unconsumed tool round; invocation and skipping are single-use",
        ))
    }

    /// Records the step-limit policy's explicit decision not to execute work.
    pub async fn skip_pending(&mut self) -> Result<CommittedToolRound, LoopError> {
        let calls = self.take_pending()?;
        let outcomes: Vec<_> = calls
            .iter()
            .map(|_| ToolOutcome::Skipped {
                reason: STEP_LIMIT_OUTPUT.to_owned(),
                context_output: encode_error("step_limit", STEP_LIMIT_OUTPUT),
            })
            .collect();
        let activities = calls
            .iter()
            .zip(&outcomes)
            .map(|(call, outcome)| activity(&call.name, outcome))
            .collect();
        let entries = self.store.commit_tool_round(self.session_id, outcomes)?;
        // Skipped work has no execution-finished observation. Its only outcome
        // publication is this committed result, also used by replay.
        self.events
            .send(RunEvent::ToolRoundCommitted { entries })
            .await;
        Ok(CommittedToolRound { activities })
    }

    pub async fn invoke_pending(&mut self) -> Result<CommittedToolRound, LoopError> {
        let calls = self.take_pending()?;
        let mut outcomes = Vec::with_capacity(calls.len());
        let mut activities = Vec::with_capacity(calls.len());
        for call in calls {
            self.events
                .send(RunEvent::ToolStarted {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                })
                .await;
            let started = Instant::now();
            let invocation = {
                let output = self.events.tool_output(&call.call_id);
                self.tools
                    .invoke(&call.name, &call.arguments, &output)
                    .await
            };
            let elapsed = started.elapsed();
            let outcome = if invocation.is_error {
                ToolOutcome::Failed(invocation.output)
            } else {
                ToolOutcome::Completed(invocation.output)
            };
            let activity = activity(&call.name, &outcome);
            self.events
                .send(RunEvent::ToolExecutionFinished {
                    call_id: call.call_id,
                    activity: activity.clone(),
                    elapsed,
                })
                .await;
            outcomes.push(outcome);
            activities.push(activity);
        }
        let entries = self.store.commit_tool_round(self.session_id, outcomes)?;
        self.events
            .send(RunEvent::ToolRoundCommitted { entries })
            .await;
        Ok(CommittedToolRound { activities })
    }
}

fn activity(name: &str, outcome: &ToolOutcome) -> ToolActivity {
    let (output, status) = match outcome {
        ToolOutcome::Completed(output) => (output, ToolActivityStatus::Completed),
        ToolOutcome::Failed(output) => (output, ToolActivityStatus::Error),
        ToolOutcome::Skipped { reason, .. } => (reason, ToolActivityStatus::Skipped),
    };
    ToolActivity {
        name: name.to_owned(),
        output: output.clone(),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent_loop::RunStopReason,
        context::FullContext,
        inference::{
            ContinuationItem, ConversationItem, InferenceResponse, ModelOutputItem, OutputEffect,
            TokenUsage,
        },
        session::{
            MemorySessionRepository, ModelCommit, SessionConfig, SessionRepositoryError,
            SessionSnapshot, SessionSummary, ToolExecution, TranscriptEntry, UsageSummary,
        },
        tool::{Tool, ToolError, ToolOutput, ToolRegistry},
    };
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };
    use tokio::sync::{Notify, mpsc};

    struct Backend {
        responses: Mutex<VecDeque<InferenceResponse>>,
        inputs: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl InferenceBackend for Backend {
        async fn respond(
            &self,
            request: InferenceRequest<'_>,
        ) -> Result<InferenceResponse, InferenceError> {
            // Compare complete opaque/context values without teaching this test
            // backend a provider's wire representation.
            self.inputs
                .lock()
                .unwrap()
                .push(format!("{:?}", request.input));
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| InferenceError::new("script exhausted"))
        }
        async fn count_input_tokens(&self, _: InferenceRequest<'_>) -> Result<u64, InferenceError> {
            Ok(0)
        }
    }

    struct ProbeTool {
        executed: Arc<Mutex<Vec<Value>>>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl Tool for ProbeTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "probe".into(),
                description: "test".into(),
                parameters: json!({"type":"object"}),
            }
        }
        async fn invoke(&self, args: Value, _: &dyn ToolOutput) -> Result<Value, ToolError> {
            self.executed.lock().unwrap().push(args.clone());
            if args["wait"] == true {
                self.release.notified().await;
            }
            if args["fail"] == true {
                return Err(ToolError::Execution("probe failed".into()));
            }
            Ok(args)
        }
    }

    // Fault injection is confined to the repository boundary under test. Every
    // successful operation retains the production repository's semantics.
    #[derive(Default)]
    struct Store {
        memory: MemorySessionRepository,
        reject_results: bool,
    }
    impl SessionStore for Store {
        fn create(&mut self, config: SessionConfig) -> Result<SessionId, SessionRepositoryError> {
            self.memory.create(config)
        }
        fn list(&self) -> Vec<SessionSummary> {
            self.memory.list()
        }
        fn snapshot(&self, id: SessionId) -> Result<SessionSnapshot, SessionRepositoryError> {
            self.memory.snapshot(id)
        }
        fn ensure_ready(&self, id: SessionId) -> Result<(), SessionRepositoryError> {
            self.memory.ensure_ready(id)
        }
        fn transcript(
            &self,
            id: SessionId,
        ) -> Result<Vec<Arc<TranscriptEntry>>, SessionRepositoryError> {
            self.memory.transcript(id)
        }
        fn commit_user(&mut self, id: SessionId, text: &str) -> Result<(), SessionRepositoryError> {
            self.memory.commit_user(id, text)
        }
        fn commit_inference(
            &mut self,
            id: SessionId,
            output: &[ModelOutputItem],
            usage: Option<TokenUsage>,
        ) -> Result<ModelCommit, SessionRepositoryError> {
            self.memory.commit_inference(id, output, usage)
        }
        fn commit_tool_round(
            &mut self,
            id: SessionId,
            results: Vec<ToolOutcome>,
        ) -> Result<Vec<Arc<TranscriptEntry>>, SessionRepositoryError> {
            if self.reject_results {
                return Err(SessionRepositoryError::InvalidToolRound {
                    id,
                    reason: "injected rejection",
                });
            }
            self.memory.commit_tool_round(id, results)
        }
        fn clear(&mut self, id: SessionId) -> Result<(), SessionRepositoryError> {
            self.memory.clear(id)
        }
        fn delete(&mut self, id: SessionId) -> Result<(), SessionRepositoryError> {
            self.memory.delete(id)
        }
        fn set_config(
            &mut self,
            id: SessionId,
            config: SessionConfig,
        ) -> Result<(), SessionRepositoryError> {
            self.memory.set_config(id, config)
        }
    }

    struct Fixture {
        store: Store,
        id: SessionId,
        backend: Arc<Backend>,
        tools: ToolSnapshot,
        executed: Arc<Mutex<Vec<Value>>>,
        release: Arc<Notify>,
    }

    impl Fixture {
        fn new(turns: Vec<Vec<ModelOutputItem>>) -> Self {
            let mut store = Store::default();
            let id = store.create(SessionConfig::default()).unwrap();
            let executed = Arc::new(Mutex::new(Vec::new()));
            let release = Arc::new(Notify::new());
            let tools = ToolRegistry::new(
                [Arc::new(ProbeTool {
                    executed: executed.clone(),
                    release: release.clone(),
                }) as Arc<dyn Tool>],
                &["probe".into()],
            )
            .unwrap()
            .snapshot(&["probe".into()])
            .unwrap();
            Self {
                store,
                id,
                backend: Arc::new(Backend {
                    responses: Mutex::new(
                        turns
                            .into_iter()
                            .map(|output| InferenceResponse {
                                output,
                                usage: None,
                            })
                            .collect(),
                    ),
                    inputs: Mutex::new(Vec::new()),
                }),
                tools,
                executed,
                release,
            }
        }
        fn runtime(
            &mut self,
            events: Option<mpsc::Sender<RunEvent>>,
        ) -> Result<LoopContext<'_>, LoopError> {
            self.runtime_with(events, &FullContext)
        }

        fn runtime_with<'a>(
            &'a mut self,
            events: Option<mpsc::Sender<RunEvent>>,
            strategy: &'a dyn ContextStrategy,
        ) -> Result<LoopContext<'a>, LoopError> {
            LoopContext::new(
                self.backend.as_ref(),
                strategy,
                &mut self.store,
                &self.tools,
                self.id,
                "model",
                "question".into(),
                RunEvents(events),
            )
        }
    }

    fn item(effect: OutputEffect) -> ModelOutputItem {
        ModelOutputItem::new(
            ContinuationItem::for_test(json!({"opaque": format!("{effect:?}")})),
            effect,
        )
    }
    fn call(id: &str, args: Value) -> ModelOutputItem {
        item(OutputEffect::ToolCall(ToolCall {
            call_id: id.into(),
            name: "probe".into(),
            arguments: args.to_string(),
        }))
    }
    fn success() -> RunOutcome {
        RunOutcome {
            text: String::new(),
            model_turns: 1,
            tool_activity: Vec::new(),
            usage: UsageSummary::default(),
            stop_reason: RunStopReason::Completed,
        }
    }

    #[tokio::test]
    async fn round_operations_are_single_use_and_inference_checks_before_network() {
        for skip in [false, true] {
            let mut f = Fixture::new(vec![vec![call("x", json!({"value":1}))]]);
            let backend = f.backend.clone();
            let executed = f.executed.clone();
            let mut context = f.runtime(None).unwrap();
            assert!(context.invoke_pending().await.is_err());
            assert!(context.skip_pending().await.is_err());
            context.infer_and_commit().await.unwrap();
            assert!(context.infer_and_commit().await.is_err());
            assert_eq!(backend.inputs.lock().unwrap().len(), 1);
            if skip {
                context.skip_pending().await.unwrap();
            } else {
                context.invoke_pending().await.unwrap();
            }
            assert!(context.invoke_pending().await.is_err());
            assert!(context.skip_pending().await.is_err());
            assert!(context.finish(Ok(success())).is_ok());
            assert_eq!(executed.lock().unwrap().len(), usize::from(!skip));
        }
    }

    #[tokio::test]
    async fn unfinished_run_retains_the_original_failure_and_rejects_new_input() {
        let mut f = Fixture::new(vec![vec![call("x", json!({}))]]);
        {
            let mut context = f.runtime(None).unwrap();
            context.infer_and_commit().await.unwrap();
            assert!(context.finish(Ok(success())).is_err());
            let error = context
                .finish(Err(LoopError::Protocol("policy failed")))
                .unwrap_err();
            assert!(error.to_string().contains("policy failed"));
            assert!(error.to_string().contains("unresolved session state"));
        }
        let before = f.store.transcript(f.id).unwrap();
        assert!(f.runtime(None).is_err());
        assert_eq!(f.store.transcript(f.id).unwrap(), before);
        assert_eq!(f.backend.inputs.lock().unwrap().len(), 1);
        assert!(f.executed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejected_commit_cannot_publish_terminal_states_or_restore_execution() {
        for skip in [false, true] {
            let mut f = Fixture::new(vec![vec![call("x", json!({"effect":"done"}))]]);
            f.store.reject_results = true;
            let executed = f.executed.clone();
            let backend = f.backend.clone();
            let (sender, mut events) = mpsc::channel(32);
            {
                let mut context = f.runtime(Some(sender)).unwrap();
                context.infer_and_commit().await.unwrap();
                let error = if skip {
                    context.skip_pending().await.err().unwrap()
                } else {
                    context.invoke_pending().await.err().unwrap()
                };
                assert!(error.to_string().contains("injected rejection"));
                assert!(!context.has_pending_tools());
                assert!(context.invoke_pending().await.is_err());
                assert!(context.skip_pending().await.is_err());
                assert!(context.infer_and_commit().await.is_err());
                let error = context.finish(Err(error)).unwrap_err();
                assert!(error.to_string().contains("injected rejection"));
                assert!(error.to_string().contains("unresolved session state"));
            }
            let mut returned = 0;
            while let Some(event) = events.recv().await {
                match event {
                    RunEvent::ToolExecutionFinished { .. } => returned += 1,
                    RunEvent::ToolRoundCommitted { .. } => panic!("rejected commit was published"),
                    _ => {},
                }
            }
            assert_eq!(returned, usize::from(!skip));
            assert_eq!(executed.lock().unwrap().len(), usize::from(!skip));
            assert_eq!(backend.inputs.lock().unwrap().len(), 1);
            let entries = f.store.transcript(f.id).unwrap();
            assert!(matches!(
                entries[1].as_ref(),
                TranscriptEntry::ToolInvocation {
                    execution: ToolExecution::Pending,
                    ..
                }
            ));
            assert_eq!(f.store.snapshot(f.id).unwrap().items.len(), 2);
            assert!(f.runtime(None).is_err());
            assert_eq!(f.store.transcript(f.id).unwrap(), entries);
        }
    }

    #[tokio::test]
    async fn dropping_a_policy_operation_does_not_restore_consumed_calls() {
        let mut f = Fixture::new(vec![vec![call("x", json!({"wait":true}))]]);
        let executed = f.executed.clone();
        let mut context = f.runtime(None).unwrap();
        context.infer_and_commit().await.unwrap();
        {
            let invoke = context.invoke_pending();
            tokio::pin!(invoke);
            tokio::select! {
                biased;
                _ = &mut invoke => panic!("tool is gated"),
                () = std::future::ready(()) => {},
            }
        }
        assert_eq!(executed.lock().unwrap().len(), 1);
        assert!(context.invoke_pending().await.is_err());
        assert!(context.infer_and_commit().await.is_err());
        assert!(context.finish(Ok(success())).is_err());
    }

    #[tokio::test]
    async fn ordered_commit_and_execution_observations_have_distinct_handoffs() {
        let mut f = Fixture::new(vec![vec![
            item(OutputEffect::Message("A".into())),
            call("x", json!({"value":1})),
            item(OutputEffect::Message("B".into())),
            call("y", json!({"wait":true})),
        ]]);
        let release = f.release.clone();
        let (sender, mut events) = mpsc::channel(1);
        let mut context = f.runtime(Some(sender)).unwrap();
        let run = async {
            let turn = context.infer_and_commit().await.unwrap();
            assert_eq!(turn.text_summary(), "AB");
            context.invoke_pending().await.unwrap();
            context.finish(Ok(success())).unwrap();
        };
        let observe = async {
            assert!(matches!(
                events.recv().await,
                Some(RunEvent::InferenceStarted)
            ));
            let Some(RunEvent::ModelTurnCommitted { entries }) = events.recv().await else {
                panic!("missing model commit")
            };
            assert_eq!(entries.len(), 4);
            assert!(
                matches!(entries[0].as_ref(), TranscriptEntry::ModelMessage { text } if text == "A")
            );
            assert!(matches!(
                entries[1].as_ref(),
                TranscriptEntry::ToolInvocation {
                    execution: ToolExecution::Pending,
                    ..
                }
            ));
            assert!(
                matches!(entries[2].as_ref(), TranscriptEntry::ModelMessage { text } if text == "B")
            );
            assert!(
                matches!(events.recv().await, Some(RunEvent::ToolStarted { call_id, .. }) if call_id == "x")
            );
            assert!(
                matches!(events.recv().await, Some(RunEvent::ToolExecutionFinished { call_id, .. }) if call_id == "x")
            );
            assert!(
                matches!(events.recv().await, Some(RunEvent::ToolStarted { call_id, .. }) if call_id == "y")
            );
            assert!(
                events.try_recv().is_err(),
                "Y is gated; no round can commit yet"
            );
            release.notify_one();
            assert!(
                matches!(events.recv().await, Some(RunEvent::ToolExecutionFinished { call_id, .. }) if call_id == "y")
            );
            let Some(RunEvent::ToolRoundCommitted { entries: results }) = events.recv().await
            else {
                panic!("missing tool commit")
            };
            assert_eq!(results.len(), 2);
            assert!(
                matches!(results[0].as_ref(), TranscriptEntry::ToolInvocation { execution: ToolExecution::Completed(output), .. } if output.contains("value"))
            );
            assert!(
                matches!(results[1].as_ref(), TranscriptEntry::ToolInvocation { execution: ToolExecution::Completed(output), .. } if output.contains("wait"))
            );
            assert!(
                matches!(
                    entries[1].as_ref(),
                    TranscriptEntry::ToolInvocation {
                        execution: ToolExecution::Pending,
                        ..
                    }
                ),
                "retained commit is an immutable snapshot"
            );
        };
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(run, observe);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn every_inference_reads_repository_context_across_rounds_and_runs() {
        struct TailContext;
        impl ContextStrategy for TailContext {
            fn prepare(&self, committed: &[ConversationItem]) -> Vec<ConversationItem> {
                committed.iter().skip(1).cloned().collect()
            }
        }
        for strategy in [&FullContext as &dyn ContextStrategy, &TailContext] {
            for skip in [false, true] {
                let mut f = Fixture::new(vec![
                    vec![
                        call("reused", json!({"value":1})),
                        call("other", json!({"fail":true})),
                    ],
                    vec![call("reused", json!({"value":2}))],
                    vec![item(OutputEffect::Message("final".into()))],
                ]);
                let backend = f.backend.clone();
                {
                    let mut context = f.runtime_with(None, strategy).unwrap();
                    for _ in 0..2 {
                        let expected = format!(
                            "{:?}",
                            strategy.prepare(
                                &context.store.snapshot(context.session_id).unwrap().items
                            )
                        );
                        context.infer_and_commit().await.unwrap();
                        assert_eq!(backend.inputs.lock().unwrap().last().unwrap(), &expected);
                        if skip {
                            context.skip_pending().await.unwrap();
                        } else {
                            context.invoke_pending().await.unwrap();
                        }
                    }
                    context.finish(Ok(success())).unwrap();
                }
                let committed = f.store.snapshot(f.id).unwrap();
                let outputs: Vec<_> = committed
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        ConversationItem::FunctionCallOutput { call_id, output } => Some((
                            call_id.as_str(),
                            serde_json::from_str::<Value>(output).unwrap(),
                        )),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    outputs.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                    ["reused", "other", "reused"]
                );
                if skip {
                    assert!(
                        outputs
                            .iter()
                            .all(|(_, value)| value["error"]["kind"] == "step_limit")
                    );
                } else {
                    assert_eq!(outputs[0].1, json!({"value":1}));
                    assert_eq!(outputs[1].1["error"]["kind"], "execution_error");
                    assert_eq!(outputs[2].1, json!({"value":2}));
                }
                let mut context = f.runtime_with(None, strategy).unwrap();
                let expected = format!(
                    "{:?}",
                    strategy.prepare(&context.store.snapshot(context.session_id).unwrap().items)
                );
                context.infer_and_commit().await.unwrap();
                assert_eq!(backend.inputs.lock().unwrap().last().unwrap(), &expected);
                let before = context.store.transcript(context.session_id).unwrap();
                let usage = context.store.snapshot(context.session_id).unwrap().usage;
                assert!(context.infer_and_commit().await.is_err());
                assert_eq!(
                    context.store.transcript(context.session_id).unwrap(),
                    before
                );
                assert_eq!(
                    context.store.snapshot(context.session_id).unwrap().usage,
                    usage
                );
            }
        }
    }
}
