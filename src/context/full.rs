use super::{ContextStrategy, ConversationItem};

#[derive(Default)]
pub struct FullContext;

impl ContextStrategy for FullContext {
    fn prepare(&self, committed: &[ConversationItem]) -> Vec<ConversationItem> {
        committed.to_vec()
    }
}
