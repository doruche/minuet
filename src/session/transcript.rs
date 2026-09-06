//! Minuet-owned semantic history for replay and presentation.

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscriptEntry {
    UserMessage {
        text: String,
    },
    ModelMessage {
        text: String,
    },
    ToolInvocation {
        name: String,
        arguments: String,
        execution: ToolExecution,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolExecution {
    /// The model call is committed but no result batch has been committed.
    /// This does not assert whether external execution started or completed.
    Pending,
    Completed(String),
    Failed(String),
    Skipped(String),
}
