use std::{
    future::Future,
    io,
    pin::Pin,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event};
use minuet::{
    agent_loop::{RunEvent, RunOutcome, RunStopReason},
    kernel::{KernelError, KernelHandle, RunRequest},
};
use tokio::sync::mpsc;

use super::{
    command,
    input::{Action, Input},
    render::{self, Tone},
    terminal::Screen,
};

const PROGRESS_BUFFER: usize = 32;

enum Reply {
    Run(Result<RunOutcome, KernelError>),
    Command(Result<String, KernelError>),
}

struct Request {
    reply: Pin<Box<dyn Future<Output = Reply>>>,
    // Local waiting time, from submission until the reply is observed. This is
    // presentation data, not provider execution time or a timeout deadline.
    started: Instant,
    // A projection of received events that may lag execution. Only the pending
    // request, never this label, controls input admission and completion.
    label: String,
}

impl Request {
    fn new(reply: impl Future<Output = Reply> + 'static, label: &str) -> Self {
        Self {
            reply: Box::pin(reply),
            started: Instant::now(),
            label: label.into(),
        }
    }

    fn status(&self) -> render::Status<'_> {
        render::Status {
            label: &self.label,
            elapsed: self.started.elapsed(),
        }
    }
}

async fn next_event() -> io::Result<Event> {
    loop {
        // Inline rendering also reads cursor-position replies. Keep all reads
        // on this interaction task so another reader cannot steal those replies.
        // Crossterm 0.29's use-dev-tty source skips reads for a zero timeout.
        // A bounded poll keeps one input reader while covering cursor replies
        // and SIGWINCH without the default Mio source's lost readiness edge.
        // Return to ZERO when that source supports nonblocking polls.
        if event::poll(std::time::Duration::from_millis(1))? {
            return event::read();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

pub async fn run(kernel: KernelHandle) -> io::Result<()> {
    let mut screen = Screen::open()?;
    let result = interact(kernel, &mut screen).await;
    // An I/O failure can leave the viewport anchor unknown (including a failed
    // cursor query after Markdown publication). Restore modes without clearing
    // through a stale anchor and erasing already published output.
    let restore = if result.is_ok() {
        screen.finish()
    } else {
        screen.restore_modes()
    };
    result.and(restore)
}

async fn interact(kernel: KernelHandle, screen: &mut Screen) -> io::Result<()> {
    let mut input = Input::default();
    let mut request: Option<Request> = None;
    // An observer exists only for the active run. Completion drains and drops
    // it before accepting another request; it never determines run completion.
    let mut events: Option<mpsc::Receiver<RunEvent>> = None;
    let mut last_draw = Instant::now() - Duration::from_millis(100);
    let mut redraw = tokio::time::interval(Duration::from_millis(40));
    redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    screen.line("Minuet — /help for commands", Tone::Meta)?;

    loop {
        let preview_changed = screen.advance_model_preview();
        if request.is_none() || preview_changed || last_draw.elapsed() >= Duration::from_millis(100)
        {
            screen.draw(&input, request.as_ref().map(Request::status))?;
            last_draw = Instant::now();
        }
        tokio::select! {
            biased;
            signal = &mut interrupt => {
                signal?;
                screen.line("Exiting; waiting for shutdown.", Tone::Meta)?;
                return Ok(());
            },
            incoming = next_event() => {
                let action = input.handle(incoming?, request.is_some());
                match action {
                    Action::Exit => {
                        screen.line("Exiting; waiting for shutdown.", Tone::Meta)?;
                        return Ok(());
                    },
                    Action::Edit => {},
                    Action::Submit(line) => {
                        if !line.trim().is_empty() {
                            screen.line(&format!("> {line}"), Tone::User)?;
                        }
                        match command::parse(&line) {
                            command::Input::Empty => {},
                            command::Input::Notice { text, is_error } => {
                                screen.line(
                                    &text,
                                    if is_error { Tone::Error } else { Tone::Command },
                                )?;
                            },
                            command::Input::Command(command::Command::Exit) => return Ok(()),
                            command::Input::Command(command) => {
                                let kernel = kernel.clone();
                                request = Some(Request::new(
                                    async move { Reply::Command(command.execute(&kernel).await) },
                                    "Processing command…",
                                ));
                            },
                            command::Input::Prompt(prompt) => {
                                let (sender, receiver) = mpsc::channel(PROGRESS_BUFFER);
                                events = Some(receiver);
                                let kernel = kernel.clone();
                                request = Some(Request::new(
                                    async move { Reply::Run(kernel.run(RunRequest::with_events(prompt, sender)).await) },
                                    "Starting…",
                                ));
                            },
                        }
                    },
                }
            },
            event = async { events.as_mut().unwrap().recv().await }, if events.is_some() => {
                match event {
                    Some(event) => {
                        let label = &mut request.as_mut().unwrap().label;
                        progress(screen, label, event)?;
                        // Combine a bounded batch of ready updates per redraw;
                        // input and SIGINT remain serviced between batches.
                        for _ in 1..PROGRESS_BUFFER {
                            match events.as_mut().unwrap().try_recv() {
                                Ok(event) => progress(screen, label, event)?,
                                Err(_) => break,
                            }
                        }
                    },
                    None => events = None,
                }
            },
            reply = async { request.as_mut().unwrap().reply.as_mut().await }, if request.is_some() => {
                let mut completed = request.take().unwrap();
                let elapsed = render::elapsed(completed.started.elapsed());
                // A result can be ready alongside the last events. Drain those
                // before displaying completion; never replay RunOutcome's tool
                // summary after its live events have already been presented.
                if let Some(mut remaining) = events.take() {
                    while let Ok(event) = remaining.try_recv() { progress(screen, &mut completed.label, event)?; }
                }
                match reply {
                    Reply::Run(Ok(outcome)) => {
                        if outcome.text.is_empty() {
                            screen.line("(no text output)", Tone::Meta)?;
                        } else {
                            screen.markdown(&outcome.text)?;
                        }
                        if outcome.stop_reason == RunStopReason::StepLimit {
                            screen.line("run stopped: model-turn limit reached; pending tool calls were not executed", Tone::Error)?;
                            screen.line(&format!("Stopped · {elapsed}"), Tone::Meta)?;
                        } else {
                            screen.line(&format!("Completed · {elapsed}"), Tone::Meta)?;
                        }
                    },
                    Reply::Run(Err(error)) => {
                        screen.model_failed()?;
                        screen.line(&format!("error: {error}"), Tone::Error)?;
                        screen.line(&format!("Failed · {elapsed}"), Tone::Error)?;
                    },
                    Reply::Command(Ok(text)) => screen.line(&text, Tone::Command)?,
                    Reply::Command(Err(error)) => screen.line(&format!("error: {error}"), Tone::Error)?,
                }
            },
            _ = redraw.tick(), if request.is_some() => {},
        }
    }
}

fn progress(screen: &mut Screen, status: &mut String, event: RunEvent) -> io::Result<()> {
    match event {
        RunEvent::InferenceStarted => *status = "Waiting for model…".into(),
        RunEvent::ModelTextDelta { text } => screen.model_delta(&text),
        RunEvent::ModelTurnCommitted {
            text,
            has_tool_calls,
        } => {
            screen.model_commit(&text, has_tool_calls)?;
            *status = "Continuing…".into();
        },
        RunEvent::ToolStarted { name, .. } => {
            *status = format!("Running {name}…");
            screen.line(&format!("Running {name}"), Tone::Meta)?;
        },
        RunEvent::ToolOutput { text, .. } => screen.fragment(&text, Tone::Text)?,
        RunEvent::ToolFinished {
            activity, elapsed, ..
        } => {
            let (label, tone) = render::tool_result(&activity, elapsed);
            screen.line(&label, tone)?;
            screen.line("Result:", Tone::Meta)?;
            screen.line(&activity.output, Tone::Text)?;
            *status = "Continuing…".into();
        },
    }
    Ok(())
}
