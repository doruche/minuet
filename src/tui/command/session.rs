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
                let rows = kernel.list_sessions().await?;
                Ok(rows
                    .into_iter()
                    .map(|s| format!("{} {}", s.id, s.brief))
                    .collect::<Vec<_>>()
                    .join("\n"))
            },
            Self::Info => {
                let id = kernel.active_session().await?;
                Ok(format!("active session {id}"))
            },
            Self::Switch { id } => {
                let id = SessionId::parse(&id).map_err(|e| KernelError::SessionId(e))?;
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
                let id = SessionId::parse(&id).map_err(|e| KernelError::SessionId(e))?;
                kernel.delete_session(id).await?;
                Ok(format!("deleted session {}", id))
            },
        }
    }
}
