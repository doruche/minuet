use async_trait::async_trait;

/// Execution-time text for an observer, separate from the tool's final result.
/// The invocation lends this capability only until the tool returns. Tools
/// must await writes (and join any scoped producers) before returning so that
/// output cannot arrive after their completion event.
#[async_trait]
pub trait ToolOutput: Send + Sync {
    /// Applies backpressure while an observer is attached. Detaching the
    /// observer makes writes no-ops; it never cancels or fails the tool.
    async fn write(&self, text: &str);
}

/// An intentionally unobserved invocation, e.g. a direct registry caller.
#[async_trait]
impl ToolOutput for () {
    async fn write(&self, _text: &str) {}
}
