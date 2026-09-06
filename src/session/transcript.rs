//! Minuet-owned semantic history for replay and presentation.

/// Historical plain text captured by the loop from the tool boundary (or skip
/// policy), committed by the repository. Never refreshed from current tools;
/// only presentation may depend on it, not execution or model input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDisplay {
    pub call: String,
    pub output: String,
}

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
        /// Original model request for inspection, never re-execution. Completed
        /// presentation uses the captured display, not these raw arguments.
        arguments: String,
        execution: ToolExecution,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolExecution {
    /// The model call is committed but no result batch has been committed.
    /// This does not assert whether external execution started or completed.
    Pending,
    Completed(ToolDisplay),
    Failed(ToolDisplay),
    Skipped(ToolDisplay),
}
