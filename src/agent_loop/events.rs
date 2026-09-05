use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::tool::ToolOutput;

use super::ToolActivity;

/// Ordered observations of one run. Tool completion describes execution, not
/// session commit or run success; the run's returned Result remains authoritative
/// for its overall outcome. Call IDs correlate output with the current call,
/// not with a globally unique task or a separately writable execution registry.
#[derive(Clone, Debug)]
pub enum RunEvent {
    InferenceStarted,
    ToolStarted {
        call_id: String,
        name: String,
    },
    ToolOutput {
        call_id: String,
        text: String,
    },
    ToolFinished {
        call_id: String,
        activity: ToolActivity,
        elapsed: Duration,
    },
}

pub(crate) struct RunEvents(pub(crate) Option<mpsc::Sender<RunEvent>>);

impl RunEvents {
    pub(crate) async fn send(&self, event: RunEvent) {
        if let Some(sender) = &self.0 {
            // Observation is not execution authority. Closing the receiver
            // releases pending writes and lets execution/commit/shutdown proceed.
            let _ = sender.send(event).await;
        }
    }

    pub(crate) fn tool_output<'a>(&'a self, call_id: &'a str) -> impl ToolOutput + 'a {
        CallOutput {
            events: self,
            call_id,
        }
    }
}

struct CallOutput<'a> {
    events: &'a RunEvents,
    call_id: &'a str,
}

#[async_trait]
impl ToolOutput for CallOutput<'_> {
    async fn write(&self, mut text: &str) {
        if self.events.0.is_none() {
            return;
        }
        // Bound queued fragment sizes as well as the caller-selected queue
        // length. Splitting preserves UTF-8 and does not insert line endings.
        while !text.is_empty() {
            let end = text.floor_char_boundary(4096.min(text.len()));
            let (fragment, rest) = text.split_at(end);
            self.events
                .send(RunEvent::ToolOutput {
                    call_id: self.call_id.to_owned(),
                    text: fragment.to_owned(),
                })
                .await;
            text = rest;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn closing_a_full_queue_releases_a_pending_output_write() {
        let (sender, receiver) = mpsc::channel(1);
        let events = RunEvents(Some(sender));
        events.send(RunEvent::InferenceStarted).await;
        let output = events.tool_output("call");
        let text = "中文".repeat(5000);
        let write = output.write(&text);
        tokio::pin!(write);
        tokio::select! {
            biased;
            () = &mut write => panic!("a full queue must apply backpressure"),
            () = std::future::ready(()) => {},
        }
        drop(receiver);
        tokio::time::timeout(Duration::from_secs(3), write)
            .await
            .expect("detaching must release pending and subsequent writes");
    }
}
