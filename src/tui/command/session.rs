use super::Output;
use clap::Subcommand;
use minuet::{
    kernel::{KernelError, KernelHandle},
    session::SessionId,
};

#[derive(Debug, Eq, PartialEq, Subcommand)]
pub enum Command {
    New,
    List,
    Info,
    Switch { id: String },
    Clear,
    Delete { id: String },
}
impl Command {
    pub async fn execute(self, kernel: &KernelHandle) -> Result<Output, KernelError> {
        match self {
            Self::New => {
                let view = kernel.new_session().await?;
                Ok(Output::Session {
                    notice: format!("started session {}", view.id),
                    view,
                })
            },
            Self::List => {
                let active = kernel.active_session().await?;
                let rows = kernel.list_sessions().await?;
                Ok(Output::Notice(
                    rows.into_iter()
                        .map(|s| {
                            format!(
                                "{} {} {}",
                                if s.id == active { "*" } else { " " },
                                s.id,
                                s.brief
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ))
            },
            Self::Info => {
                let id = kernel.active_session().await?;
                let row = kernel
                    .list_sessions()
                    .await?
                    .into_iter()
                    .find(|s| s.id == id)
                    .ok_or(KernelError::Session(
                        minuet::session::SessionStoreError::NotFound(id),
                    ))?;
                Ok(Output::Notice(format!(
                    "session {}: {} items, effort={:?}, usage_total={}",
                    id, row.item_count, row.config.reasoning_effort, row.usage.total_tokens
                )))
            },
            Self::Switch { id } => {
                let id = SessionId::parse(&id).map_err(KernelError::SessionId)?;
                let view = kernel.switch_session(id).await?;
                Ok(Output::Session {
                    notice: format!("switched to session {}", view.id),
                    view,
                })
            },
            Self::Clear => {
                let view = kernel.clear_session().await?;
                Ok(Output::Session {
                    notice: format!("cleared conversation history · session {}", view.id),
                    view,
                })
            },
            Self::Delete { id } => {
                let id = SessionId::parse(&id).map_err(KernelError::SessionId)?;
                kernel.delete_session(id).await?;
                Ok(Output::Notice(format!("deleted session {}", id)))
            },
        }
    }
}
