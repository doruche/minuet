use crate::inference::ConversationItem;

/// Selects the model-visible view of canonical session history. A strategy
/// returns an owned snapshot; it never mutates or becomes a second owner of the
/// committed conversation.
pub trait ContextStrategy: Send + Sync {
    fn prepare(&self, committed: &[ConversationItem]) -> Vec<ConversationItem>;
}

#[derive(Default)]
pub struct FullContext;

impl ContextStrategy for FullContext {
    fn prepare(&self, committed: &[ConversationItem]) -> Vec<ConversationItem> {
        committed.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_context_preserves_every_item_in_order() {
        let committed = [
            ConversationItem::UserText("first".to_owned()),
            ConversationItem::FunctionCallOutput {
                call_id: "call".to_owned(),
                output: "result".to_owned(),
            },
        ];

        let prepared = FullContext.prepare(&committed);
        assert_eq!(prepared.len(), 2);
        assert!(matches!(
            &prepared[0],
            ConversationItem::UserText(text) if text == "first"
        ));
    }
}
