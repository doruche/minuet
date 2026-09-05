use std::collections::BTreeMap;

use super::{
    ConversationItem, ReasoningEffort, Session, SessionId, SessionSnapshot, SessionStore,
    SessionStoreError, TokenUsage,
};

#[derive(Default)]
pub struct MemorySessionStore {
    next_id: u64,
    sessions: BTreeMap<SessionId, Session>,
}

impl SessionStore for MemorySessionStore {
    fn create(
        &mut self,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<SessionId, SessionStoreError> {
        let id = SessionId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(SessionStoreError::IdExhausted)?;
        self.sessions.insert(
            id,
            Session {
                items: Vec::new(),
                reasoning_effort,
                usage: super::UsageSummary::default(),
            },
        );
        Ok(id)
    }

    fn snapshot(&self, id: SessionId) -> Result<SessionSnapshot, SessionStoreError> {
        let session = self.session(id)?;
        Ok(SessionSnapshot {
            id,
            items: session.items.clone(),
            reasoning_effort: session.reasoning_effort.clone(),
            usage: session.usage.clone(),
        })
    }

    fn append(
        &mut self,
        id: SessionId,
        items: &[ConversationItem],
    ) -> Result<(), SessionStoreError> {
        self.session_mut(id)?.items.extend_from_slice(items);
        Ok(())
    }

    fn clear(&mut self, id: SessionId) -> Result<(), SessionStoreError> {
        let session = self.session_mut(id)?;
        session.items.clear();
        session.usage = super::UsageSummary::default();
        Ok(())
    }

    fn commit_inference(
        &mut self,
        id: SessionId,
        output: &[ConversationItem],
        usage: Option<TokenUsage>,
    ) -> Result<(), SessionStoreError> {
        let session = self.session_mut(id)?;
        session.items.extend_from_slice(output);
        session.usage.observe(usage);
        Ok(())
    }

    fn set_reasoning_effort(
        &mut self,
        id: SessionId,
        effort: Option<ReasoningEffort>,
    ) -> Result<(), SessionStoreError> {
        self.session_mut(id)?.reasoning_effort = effort;
        Ok(())
    }
}

impl MemorySessionStore {
    fn session(&self, id: SessionId) -> Result<&Session, SessionStoreError> {
        self.sessions
            .get(&id)
            .ok_or(SessionStoreError::NotFound(id))
    }

    fn session_mut(&mut self, id: SessionId) -> Result<&mut Session, SessionStoreError> {
        self.sessions
            .get_mut(&id)
            .ok_or(SessionStoreError::NotFound(id))
    }
}
