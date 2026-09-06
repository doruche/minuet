use super::{
    ConversationItem, SessionConfig, SessionId, SessionRepository, SessionRepositoryError,
    SessionSnapshot, SessionSummary, TokenUsage, UsageSummary,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct MemorySessionRepository {
    sessions: BTreeMap<SessionId, Record>,
}
struct Record {
    items: Vec<ConversationItem>,
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
    fn append(
        &mut self,
        id: SessionId,
        x: &[ConversationItem],
    ) -> Result<(), SessionRepositoryError> {
        self.get_mut(id)?.items.extend_from_slice(x);
        Ok(())
    }
    fn clear(&mut self, id: SessionId) -> Result<(), SessionRepositoryError> {
        let s = self.get_mut(id)?;
        s.items.clear();
        s.usage = UsageSummary::default();
        Ok(())
    }
    fn delete(&mut self, id: SessionId) -> Result<(), SessionRepositoryError> {
        self.sessions
            .remove(&id)
            .map(|_| ())
            .ok_or(SessionRepositoryError::NotFound(id))
    }
    fn commit_inference(
        &mut self,
        id: SessionId,
        x: &[ConversationItem],
        u: Option<TokenUsage>,
    ) -> Result<(), SessionRepositoryError> {
        let s = self.get_mut(id)?;
        s.items.extend_from_slice(x);
        s.usage.observe(u);
        Ok(())
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
