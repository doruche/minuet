use clap::Subcommand;
use minuet::kernel::{KernelError, KernelHandle};

#[derive(Debug, Eq, PartialEq, Subcommand)]
pub enum Command {
    /// List compiled tools and their state
    List,
    /// Expose a compiled tool to subsequent model calls
    Enable { name: String },
    /// Hide a tool from subsequent model calls
    Disable { name: String },
}

impl Command {
    pub async fn execute(self, kernel: &KernelHandle) -> Result<String, KernelError> {
        match self {
            Self::List => {
                let tools = kernel.list_tools().await?;
                if tools.is_empty() {
                    return Ok("no compiled tools".into());
                }
                Ok(tools
                    .into_iter()
                    .map(|tool| {
                        format!(
                            "{} ({}) — {}",
                            tool.name,
                            if tool.enabled { "enabled" } else { "disabled" },
                            tool.description
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            },
            Self::Enable { name } => set_enabled(kernel, name, true).await,
            Self::Disable { name } => set_enabled(kernel, name, false).await,
        }
    }
}

async fn set_enabled(
    kernel: &KernelHandle,
    name: String,
    enabled: bool,
) -> Result<String, KernelError> {
    kernel.set_tool_enabled(&name, enabled).await?;
    Ok(format!(
        "tool `{name}` {}",
        if enabled { "enabled" } else { "disabled" }
    ))
}
