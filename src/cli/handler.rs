use minuet::kernel::{ContextInfo, KernelError, KernelHandle, ModelInfo};
use minuet::session::SessionId;
use minuet::tool::ToolStatus;

use super::command::Command;

pub enum Effect {
    Exit,
    Help,
    NewSession(SessionId),
    Model(ModelInfo),
    ReasoningEffortSet(Option<String>),
    Tools(Vec<ToolStatus>),
    ToolEnabled { name: String, enabled: bool },
    Context(ContextInfo),
}

pub async fn handle(kernel: &KernelHandle, command: Command) -> Result<Effect, KernelError> {
    match command {
        Command::Exit => Ok(Effect::Exit),
        Command::Help => Ok(Effect::Help),
        Command::NewSession => Ok(Effect::NewSession(kernel.new_session().await?)),
        Command::ModelInfo => Ok(Effect::Model(kernel.model_info().await?)),
        Command::SetReasoningEffort(effort) => {
            kernel.set_reasoning_effort(Some(effort.clone())).await?;
            Ok(Effect::ReasoningEffortSet(Some(effort)))
        },
        Command::ClearReasoningEffort => {
            kernel.set_reasoning_effort(None).await?;
            Ok(Effect::ReasoningEffortSet(None))
        },
        Command::ListTools => Ok(Effect::Tools(kernel.list_tools().await?)),
        Command::SetToolEnabled { name, enabled } => {
            kernel.set_tool_enabled(&name, enabled).await?;
            Ok(Effect::ToolEnabled { name, enabled })
        },
        Command::ContextInfo => Ok(Effect::Context(kernel.context_info().await?)),
    }
}
