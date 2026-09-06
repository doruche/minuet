use tokio::sync::{mpsc, oneshot};

use super::{ContextInfo, KernelError, ModelInfo, SessionView};
use crate::{
    agent_loop::{RunEvent, RunOutcome},
    model::ReasoningEffort,
    session::{SessionId, SessionSummary},
    tool::ToolStatus,
};

/// A capability to submit commands to the kernel's single sequencer.
/// Commands before shutdown finish in order; commands queued after it fail with
/// `KernelError::Stopped`, even if enqueue initially succeeded.
#[derive(Clone)]
pub struct KernelHandle {
    sender: mpsc::Sender<Command>,
}

impl KernelHandle {
    pub(super) fn new(sender: mpsc::Sender<Command>) -> Self {
        Self { sender }
    }

    /// The optional event channel must be drained concurrently: a full queue
    /// pauses execution. Successful enqueue transfers execution to the kernel;
    /// dropping the result future or event receiver does not cancel that work.
    pub async fn run(&self, request: impl Into<RunRequest>) -> Result<RunOutcome, KernelError> {
        self.request(request.into(), Command::Run).await
    }

    pub async fn new_session(&self) -> Result<SessionView, KernelError> {
        self.request((), Command::NewSession).await
    }

    pub async fn clear_session(&self) -> Result<SessionView, KernelError> {
        self.request((), Command::ClearSession).await
    }
    pub async fn active_session(&self) -> Result<SessionId, KernelError> {
        self.request((), Command::ActiveSession).await
    }
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, KernelError> {
        self.request((), Command::ListSessions).await
    }
    pub async fn switch_session(&self, id: SessionId) -> Result<SessionView, KernelError> {
        self.request(id, Command::SwitchSession).await
    }
    pub async fn delete_session(&self, id: SessionId) -> Result<(), KernelError> {
        self.request(id, Command::DeleteSession).await
    }

    pub async fn model_info(&self) -> Result<ModelInfo, KernelError> {
        self.request((), Command::ModelInfo).await
    }

    pub async fn set_reasoning_effort(&self, effort: Option<String>) -> Result<(), KernelError> {
        let effort = effort.map(ReasoningEffort::new).transpose()?;
        self.request(effort, Command::SetReasoningEffort).await
    }

    pub async fn list_tools(&self) -> Result<Vec<ToolStatus>, KernelError> {
        self.request((), Command::ListTools).await
    }

    pub async fn set_tool_enabled(
        &self,
        name: impl Into<String>,
        enabled: bool,
    ) -> Result<(), KernelError> {
        self.request(
            SetToolEnabled {
                name: name.into(),
                enabled,
            },
            Command::SetToolEnabled,
        )
        .await
    }

    pub async fn context_info(&self) -> Result<ContextInfo, KernelError> {
        self.request((), Command::ContextInfo).await
    }

    pub(super) async fn shutdown(&self) -> Result<(), KernelError> {
        self.request((), Command::Shutdown).await
    }

    async fn request<P, R>(
        &self,
        payload: P,
        command: impl FnOnce(Envelope<P, R>) -> Command,
    ) -> Result<R, KernelError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(command(Envelope { payload, reply }))
            .await
            .map_err(|_| KernelError::Stopped)?;
        response.await.map_err(|_| KernelError::Stopped)?
    }
}

/// One run's input and optional observation capability. The returned result is
/// authoritative for success; events describe progress and may precede failure.
pub struct RunRequest {
    pub prompt: String,
    pub events: Option<mpsc::Sender<RunEvent>>,
}

impl From<String> for RunRequest {
    fn from(prompt: String) -> Self {
        Self {
            prompt,
            events: None,
        }
    }
}

impl From<&str> for RunRequest {
    fn from(prompt: &str) -> Self {
        prompt.to_owned().into()
    }
}

impl RunRequest {
    pub fn with_events(prompt: impl Into<String>, events: mpsc::Sender<RunEvent>) -> Self {
        Self {
            prompt: prompt.into(),
            events: Some(events),
        }
    }
}

pub(super) struct SetToolEnabled {
    pub name: String,
    pub enabled: bool,
}

// Each variant fixes the payload/result pairing at compile time. Only the
// envelope owns reply transport; payloads carry no response channels.
pub(super) enum Command {
    Run(Envelope<RunRequest, RunOutcome>),
    NewSession(Envelope<(), SessionView>),
    ClearSession(Envelope<(), SessionView>),
    ActiveSession(Envelope<(), SessionId>),
    ListSessions(Envelope<(), Vec<SessionSummary>>),
    SwitchSession(Envelope<SessionId, SessionView>),
    DeleteSession(Envelope<SessionId, ()>),
    ModelInfo(Envelope<(), ModelInfo>),
    SetReasoningEffort(Envelope<Option<ReasoningEffort>, ()>),
    ListTools(Envelope<(), Vec<ToolStatus>>),
    SetToolEnabled(Envelope<SetToolEnabled, ()>),
    ContextInfo(Envelope<(), ContextInfo>),
    Shutdown(Envelope<(), ()>),
}

pub(super) struct Envelope<P, R> {
    pub(super) payload: P,
    pub(super) reply: oneshot::Sender<Result<R, KernelError>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replies_remain_paired_when_commands_finish_out_of_order() {
        let (sender, mut receiver) = mpsc::channel(2);
        let handle = KernelHandle::new(sender);
        let requests = async {
            let (first, second) = tokio::join!(
                handle.set_tool_enabled("first", true),
                handle.set_tool_enabled("second", false),
            );
            assert!(first.is_ok());
            assert!(matches!(second, Err(KernelError::Stopped)));
        };
        let dispatch = async {
            let Command::SetToolEnabled(first) = receiver.recv().await.unwrap() else {
                panic!("wrong command")
            };
            let Command::SetToolEnabled(second) = receiver.recv().await.unwrap() else {
                panic!("wrong command")
            };
            assert_eq!(first.payload.name, "first");
            assert!(first.payload.enabled);
            assert_eq!(second.payload.name, "second");
            assert!(!second.payload.enabled);
            second.reply.send(Err(KernelError::Stopped)).unwrap();
            first.reply.send(Ok(())).unwrap();
        };
        tokio::join!(requests, dispatch);
    }

    #[tokio::test]
    async fn stopped_is_reported_for_both_enqueue_and_reply_failure() {
        let (sender, mut receiver) = mpsc::channel(1);
        let handle = KernelHandle::new(sender);
        let dispatch = async {
            // Losing the accepted command also closes its one-shot reply.
            drop(receiver.recv().await.unwrap());
            drop(receiver);
        };
        let (result, ()) = tokio::join!(handle.new_session(), dispatch);
        assert!(matches!(result, Err(KernelError::Stopped)));
        assert!(matches!(
            handle.list_tools().await,
            Err(KernelError::Stopped)
        ));
    }

    #[tokio::test]
    async fn cancelling_before_enqueue_does_not_submit_a_command() {
        let (sender, mut receiver) = mpsc::channel(1);
        let (reply, _response) = oneshot::channel();
        sender
            .send(Command::ListTools(Envelope { payload: (), reply }))
            .await
            .unwrap_or_else(|_| panic!("queue closed"));
        let handle = KernelHandle::new(sender);
        {
            let request = handle.new_session();
            tokio::pin!(request);
            tokio::select! {
                biased;
                _ = &mut request => panic!("full queue must block enqueue"),
                () = std::future::ready(()) => {},
            }
        }
        assert!(matches!(
            receiver.recv().await.unwrap(),
            Command::ListTools(_)
        ));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}
