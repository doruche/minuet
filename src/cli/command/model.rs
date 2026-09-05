use clap::Subcommand;
use minuet::kernel::{KernelError, KernelHandle};

#[derive(Debug, Eq, PartialEq, Subcommand)]
pub enum Command {
    /// Show the active provider, model and effort
    Info,
    /// Set an opaque effort value; 'clear' uses the upstream default
    Effort { value: String },
}

impl Command {
    pub async fn execute(self, kernel: &KernelHandle) -> Result<String, KernelError> {
        match self {
            Self::Info => {
                let info = kernel.model_info().await?;
                Ok(format!(
                    "provider: {}\nmodel: {}\nreasoning effort: {}",
                    info.provider,
                    info.model,
                    info.reasoning_effort
                        .as_deref()
                        .unwrap_or("upstream default")
                ))
            },
            Self::Effort { value } => {
                let effort = (value != "clear").then_some(value);
                kernel.set_reasoning_effort(effort.clone()).await?;
                Ok(match effort {
                    Some(effort) => format!("reasoning effort set to `{effort}`"),
                    None => "reasoning effort cleared; the upstream default will be used".into(),
                })
            },
        }
    }
}
