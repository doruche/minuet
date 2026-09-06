use super::{
    ConversationItem, ModelCommit, ModelOutputItem, SessionConfig, SessionId, SessionRepository,
    SessionRepositoryError, SessionSnapshot, SessionSummary, TokenUsage, ToolExecution,
    ToolOutcome, TranscriptEntry, UsageSummary,
};
use crate::inference::OutputEffect;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Default)]
pub struct MemorySessionRepository {
    sessions: BTreeMap<SessionId, Record>,
}
struct Record {
    items: Vec<ConversationItem>,
    // Owner-private association for the one unresolved result batch. The slot
    // selects its transcript invocation; the call ID encodes context output.
    // clear discards both histories and this association together.
    pending: Vec<(usize, String)>,
    transcript: Vec<Arc<TranscriptEntry>>,
    config: SessionConfig,
    usage: UsageSummary,
}
impl SessionRepository for MemorySessionRepository {
    fn create(&mut self, config: SessionConfig) -> Result<SessionId, SessionRepositoryError> {
        let id = loop {
            let id = SessionId::new();
            if !self.sessions.contains_key(&id) {
                break id;
            }
        };
        self.sessions.insert(
            id,
            Record {
                items: vec![],
                pending: vec![],
                transcript: vec![],
                config,
                usage: UsageSummary::default(),
            },
        );
        Ok(id)
    }
    fn list(&self) -> Vec<SessionSummary> {
        self.sessions
            .iter()
            .map(|(id, s)| SessionSummary {
                id: *id,
                brief: brief(&s.items),
                item_count: s.items.len(),
                config: s.config.clone(),
                usage: s.usage.clone(),
            })
            .collect()
    }
    fn snapshot(&self, id: SessionId) -> Result<SessionSnapshot, SessionRepositoryError> {
        let s = self.get(id)?;
        Ok(SessionSnapshot {
            id,
            items: s.items.clone(),
            config: s.config.clone(),
            usage: s.usage.clone(),
        })
    }
    fn ensure_ready(&self, id: SessionId) -> Result<(), SessionRepositoryError> {
        if !self.get(id)?.pending.is_empty() {
            return Err(SessionRepositoryError::InvalidToolRound {
                id,
                reason: "previous tool round has no committed results",
            });
        }
        Ok(())
    }
    fn transcript(
        &self,
        id: SessionId,
    ) -> Result<Vec<Arc<TranscriptEntry>>, SessionRepositoryError> {
        Ok(self.get(id)?.transcript.clone())
    }
    fn commit_user(&mut self, id: SessionId, text: &str) -> Result<(), SessionRepositoryError> {
        self.ensure_ready(id)?;
        let s = self.get_mut(id)?;
        s.items.push(ConversationItem::UserText(text.to_owned()));
        s.transcript.push(Arc::new(TranscriptEntry::UserMessage {
            text: text.to_owned(),
        }));
        Ok(())
    }
    fn commit_inference(
        &mut self,
        id: SessionId,
        output: &[ModelOutputItem],
        usage: Option<TokenUsage>,
    ) -> Result<ModelCommit, SessionRepositoryError> {
        self.ensure_ready(id)?;
        let s = self.get_mut(id)?;
        let start = s.transcript.len();
        let mut calls = Vec::new();
        for item in output {
            match item.effect() {
                OutputEffect::None => {},
                OutputEffect::Message(text) if text.is_empty() => {},
                OutputEffect::Message(text) => {
                    s.transcript.push(Arc::new(TranscriptEntry::ModelMessage {
                        text: text.clone(),
                    }))
                },
                OutputEffect::ToolCall(call) => {
                    s.pending.push((s.transcript.len(), call.call_id.clone()));
                    calls.push(call.clone());
                    s.transcript.push(Arc::new(TranscriptEntry::ToolInvocation {
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        execution: ToolExecution::Pending,
                    }));
                },
            }
            s.items.push(ConversationItem::Continuation(
                item.clone().into_continuation(),
            ));
        }
        s.usage.observe(usage);
        Ok(ModelCommit {
            entries: s.transcript[start..].to_vec(),
            calls,
        })
    }
    fn commit_tool_round(
        &mut self,
        id: SessionId,
        results: Vec<ToolOutcome>,
    ) -> Result<Vec<Arc<TranscriptEntry>>, SessionRepositoryError> {
        let s = self.get_mut(id)?;
        let invalid = |reason| SessionRepositoryError::InvalidToolRound { id, reason };
        if s.pending.is_empty() {
            return Err(invalid("no pending tool round"));
        }
        if s.pending.len() != results.len() {
            return Err(invalid("result count differs from committed calls"));
        }
        for (index, _) in &s.pending {
            if !matches!(
                s.transcript.get(*index).map(AsRef::as_ref),
                Some(TranscriptEntry::ToolInvocation {
                    execution: ToolExecution::Pending,
                    ..
                })
            ) {
                return Err(invalid("invocation is not pending"));
            }
        }
        // All fallible validation precedes publication of either side. Retained
        // views keep their entries; only a changed invocation uses copy-on-write.
        let mut committed = Vec::with_capacity(results.len());
        for ((index, call_id), result) in std::mem::take(&mut s.pending).into_iter().zip(results) {
            let (output, execution) = match result {
                ToolOutcome::Completed(output) => {
                    (output.clone(), ToolExecution::Completed(output))
                },
                ToolOutcome::Failed(output) => (output.clone(), ToolExecution::Failed(output)),
                ToolOutcome::Skipped {
                    reason,
                    context_output,
                } => (context_output, ToolExecution::Skipped(reason)),
            };
            let TranscriptEntry::ToolInvocation {
                execution: current, ..
            } = Arc::make_mut(&mut s.transcript[index])
            else {
                unreachable!("validated above")
            };
            *current = execution;
            committed.push(Arc::clone(&s.transcript[index]));
            s.items
                .push(ConversationItem::FunctionCallOutput { call_id, output });
        }
        Ok(committed)
    }
    fn clear(&mut self, id: SessionId) -> Result<(), SessionRepositoryError> {
        let s = self.get_mut(id)?;
        s.items.clear();
        s.pending.clear();
        s.transcript.clear();
        s.usage = UsageSummary::default();
        Ok(())
    }
    fn delete(&mut self, id: SessionId) -> Result<(), SessionRepositoryError> {
        self.sessions
            .remove(&id)
            .map(|_| ())
            .ok_or(SessionRepositoryError::NotFound(id))
    }
    fn set_config(
        &mut self,
        id: SessionId,
        c: SessionConfig,
    ) -> Result<(), SessionRepositoryError> {
        self.get_mut(id)?.config = c;
        Ok(())
    }
}
impl MemorySessionRepository {
    fn get(&self, id: SessionId) -> Result<&Record, SessionRepositoryError> {
        self.sessions
            .get(&id)
            .ok_or(SessionRepositoryError::NotFound(id))
    }
    fn get_mut(&mut self, id: SessionId) -> Result<&mut Record, SessionRepositoryError> {
        self.sessions
            .get_mut(&id)
            .ok_or(SessionRepositoryError::NotFound(id))
    }
}
fn brief(items: &[ConversationItem]) -> String {
    items
        .iter()
        .find_map(|i| match i {
            ConversationItem::UserText(t) => Some(t.chars().take(80).collect()),
            _ => None,
        })
        .unwrap_or_else(|| "empty session".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{ContinuationItem, ToolCall};
    use serde_json::json;

    fn model(effect: OutputEffect) -> ModelOutputItem {
        ModelOutputItem::new(ContinuationItem::for_test(json!({"opaque": true})), effect)
    }

    fn call(name: &str) -> ModelOutputItem {
        model(OutputEffect::ToolCall(ToolCall {
            call_id: "provider-id".into(),
            name: name.into(),
            arguments: "{}".into(),
        }))
    }

    #[test]
    fn semantic_order_and_results_share_reads_without_copying_payloads() {
        let mut store = MemorySessionRepository::default();
        let id = store.create(SessionConfig::default()).unwrap();
        store.commit_user(id, "question").unwrap();
        let committed = store
            .commit_inference(
                id,
                &[
                    model(OutputEffect::None),
                    model(OutputEffect::Message("before".into())),
                    call("first"),
                    model(OutputEffect::Message("after".into())),
                    call("second"),
                ],
                Some(TokenUsage {
                    input_tokens: 2,
                    output_tokens: 3,
                    total_tokens: 5,
                }),
            )
            .unwrap();
        let before = store.transcript(id).unwrap();
        assert_eq!(committed.entries, before[1..]);
        assert!(
            committed
                .entries
                .iter()
                .zip(&before[1..])
                .all(|(a, b)| Arc::ptr_eq(a, b))
        );
        assert_eq!(
            committed
                .calls
                .iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        let other_read = store.transcript(id).unwrap();
        assert!(
            before
                .iter()
                .zip(&other_read)
                .all(|(a, b)| Arc::ptr_eq(a, b))
        );
        assert_eq!(before.len(), 5);
        assert!(matches!(&*before[0], TranscriptEntry::UserMessage { text } if text == "question"));
        assert!(matches!(&*before[1], TranscriptEntry::ModelMessage { text } if text == "before"));
        assert!(matches!(&*before[3], TranscriptEntry::ModelMessage { text } if text == "after"));
        let results = store
            .commit_tool_round(
                id,
                vec![
                    ToolOutcome::Completed("success".into()),
                    ToolOutcome::Failed("error with output".into()),
                ],
            )
            .unwrap();
        let after = store.transcript(id).unwrap();
        assert!(Arc::ptr_eq(&results[0], &after[2]));
        assert!(Arc::ptr_eq(&results[1], &after[4]));
        assert!(Arc::ptr_eq(&before[1], &after[1]));
        assert!(matches!(
            &*before[2],
            TranscriptEntry::ToolInvocation {
                execution: ToolExecution::Pending,
                ..
            }
        ));
        assert!(
            matches!(&*after[2], TranscriptEntry::ToolInvocation { execution: ToolExecution::Completed(text), .. } if text == "success")
        );
        assert!(
            matches!(&*after[4], TranscriptEntry::ToolInvocation { execution: ToolExecution::Failed(text), .. } if text == "error with output")
        );
        let context = store.snapshot(id).unwrap();
        assert_eq!(context.items.len(), 8);
        assert!(
            matches!(&context.items[6], ConversationItem::FunctionCallOutput {call_id, output} if call_id == "provider-id" && output == "success")
        );
        assert_eq!(context.usage.total_tokens, 5);
    }

    #[test]
    fn invalid_rounds_cannot_partially_change_either_history() {
        let mut store = MemorySessionRepository::default();
        let a = store.create(SessionConfig::default()).unwrap();
        let b = store.create(SessionConfig::default()).unwrap();
        store.commit_inference(a, &[call("first")], None).unwrap();
        let before = store.transcript(a).unwrap();
        let before_context = store.snapshot(a).unwrap();
        assert!(store.commit_user(a, "must not append").is_err());
        assert!(
            store
                .commit_inference(a, &[], Some(TokenUsage::default()))
                .is_err()
        );
        assert_eq!(
            store.snapshot(a).unwrap().items.len(),
            before_context.items.len()
        );
        assert_eq!(store.snapshot(a).unwrap().usage, before_context.usage);
        assert!(store.commit_tool_round(b, vec![]).is_err());
        assert!(
            store
                .commit_tool_round(b, vec![ToolOutcome::Completed("wrong".into())])
                .is_err()
        );
        assert!(store.snapshot(b).unwrap().items.is_empty());
        assert_eq!(store.transcript(a).unwrap(), before);
        store
            .commit_tool_round(a, vec![ToolOutcome::Completed("first result".into())])
            .unwrap();
        store
            .commit_inference(a, &[call("second"), call("third")], None)
            .unwrap();
        let before = store.transcript(a).unwrap();
        let count = store.snapshot(a).unwrap().items.len();
        assert!(
            store
                .commit_tool_round(a, vec![ToolOutcome::Failed("only one".into())])
                .is_err()
        );
        assert_eq!(store.transcript(a).unwrap(), before);
        assert_eq!(store.snapshot(a).unwrap().items.len(), count);
    }

    #[test]
    fn clear_discards_pending_round_and_preserves_session_identity() {
        let mut store = MemorySessionRepository::default();
        let config = SessionConfig {
            enabled_tools: vec!["echo".into()],
            ..SessionConfig::default()
        };
        let a = store.create(config.clone()).unwrap();
        let b = store.create(SessionConfig::default()).unwrap();
        store.commit_user(b, "retained").unwrap();
        store
            .commit_inference(
                a,
                &[call("old")],
                Some(TokenUsage {
                    total_tokens: 8,
                    ..TokenUsage::default()
                }),
            )
            .unwrap();
        let read = store.transcript(a).unwrap();
        store.clear(a).unwrap();
        assert!(store.transcript(a).unwrap().is_empty());
        let cleared = store.snapshot(a).unwrap();
        assert!(cleared.items.is_empty());
        assert_eq!(cleared.config, config);
        assert_eq!(cleared.usage, UsageSummary::default());
        store.commit_inference(a, &[call("new")], None).unwrap();
        store
            .commit_tool_round(
                a,
                vec![ToolOutcome::Skipped {
                    reason: "limit".into(),
                    context_output: "encoded error".into(),
                }],
            )
            .unwrap();
        assert!(
            matches!(&*store.transcript(a).unwrap()[0], TranscriptEntry::ToolInvocation {execution: ToolExecution::Skipped(reason), ..} if reason == "limit")
        );
        assert!(
            matches!(&store.snapshot(a).unwrap().items[1], ConversationItem::FunctionCallOutput {output, ..} if output == "encoded error")
        );
        store.delete(a).unwrap();
        assert!(store.transcript(a).is_err());
        assert!(store.snapshot(a).is_err());
        assert!(matches!(&*read[0], TranscriptEntry::ToolInvocation {name, ..} if name == "old"));
        assert_eq!(store.transcript(b).unwrap().len(), 1);
    }
}
