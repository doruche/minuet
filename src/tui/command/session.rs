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
    pub async fn execute(self, kernel: &KernelHandle) -> Result<String, KernelError> {
        match self {
            Self::New => Ok(format!("started session {}", kernel.new_session().await?)),
            Self::List => {
                let active = kernel.active_session().await?;
                let rows = kernel.list_sessions().await?;
                Ok(rows
                    .into_iter()
                    .map(|s| {
                        format!(
                            "{} {} {}",
                            if s.id == active { "*" } else { " " },
                            s.id,
                            s.brief
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
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
                Ok(format!(
                    "session {}: {} items, effort={:?}, usage_total={}",
                    id, row.item_count, row.config.reasoning_effort, row.usage.total_tokens
                ))
            },
            Self::Switch { id } => {
                let id = SessionId::parse(&id).map_err(KernelError::SessionId)?;
                Ok(format!(
                    "switched to session {}",
                    kernel.switch_session(id).await?
                ))
            },
            Self::Clear => {
                kernel.clear_session().await?;
                Ok("cleared conversation history".into())
            },
            Self::Delete { id } => {
                let id = SessionId::parse(&id).map_err(KernelError::SessionId)?;
                kernel.delete_session(id).await?;
                Ok(format!("deleted session {}", id))
            },
        }
    }
}
