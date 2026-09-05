use clap::Subcommand;
use minuet::{
    kernel::{InputTokenCount, KernelError, KernelHandle},
    session::UsageSummary,
};

#[derive(Debug, Eq, PartialEq, Subcommand)]
pub enum Command {
    /// Count committed context and show usage
    Info,
}

impl Command {
    pub async fn execute(self, kernel: &KernelHandle) -> Result<String, KernelError> {
        match self {
            Self::Info => {
                let info = kernel.context_info().await?;
                let tokens = match info.committed_input_tokens {
                    InputTokenCount::Available(tokens) => format!("{tokens} (upstream count)"),
                    InputTokenCount::Unavailable(error) => format!("unavailable ({error})"),
                };
                Ok(format!(
                    "session: {}\ncommitted input tokens: {tokens}\n{}",
                    info.session_id,
                    usage(&info.usage)
                ))
            },
        }
    }
}

fn usage(usage: &UsageSummary) -> String {
    let latest = match usage.latest {
        Some(latest) => format!(
            "input {}, output {}, total {}",
            latest.input_tokens, latest.output_tokens, latest.total_tokens
        ),
        None if usage.reported_calls + usage.unreported_calls == 0 => "no model calls yet".into(),
        None => "unavailable for the latest model call".into(),
    };
    let qualifier = if usage.unreported_calls == 0 {
        ""
    } else {
        " (partial: at least one call omitted usage)"
    };
    format!(
        "last reported usage: {latest}\nsession reported usage: input {}, output {}, total {}{qualifier}",
        usage.input_tokens, usage.output_tokens, usage.total_tokens
    )
}
