use std::{future::Future, io, pin::Pin};

use crossterm::event::{self, Event};
use minuet::{
    agent_loop::{RunEvent, RunOutcome},
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
    Run(RunOutcome),
    Command(String),
}
type Request = Pin<Box<dyn Future<Output = Result<Reply, KernelError>>>>;

async fn next_event() -> io::Result<Event> {
    loop {
        // Inline rendering also reads cursor-position replies. Keep all reads
        // on this interaction task so another reader cannot steal those replies.
        if event::poll(std::time::Duration::ZERO)? {
            return event::read();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

pub async fn run(kernel: KernelHandle) -> io::Result<()> {
    let mut screen = Screen::open()?;
    let result = interact(kernel, &mut screen).await;
    let restore = screen.finish();
    result.and(restore)
}

async fn interact(kernel: KernelHandle, screen: &mut Screen) -> io::Result<()> {
    let mut input = Input::default();
    let mut request: Option<Request> = None;
    let mut events: Option<mpsc::Receiver<RunEvent>> = None;
    // A presentation of received events, never execution authority. Request
    // ownership (the pending future), not this label, governs input admission.
    let mut status = String::new();
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    screen.line("Minuet — /help for commands", Tone::Meta)?;

    loop {
        screen.draw(&input, &status, request.is_some())?;
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
                                screen.line(&text, if is_error { Tone::Error } else { Tone::Text })?;
                            },
                            command::Input::Command(command::Command::Exit) => return Ok(()),
                            command::Input::Command(command) => {
                                let kernel = kernel.clone();
                                request = Some(Box::pin(async move { command.execute(&kernel).await.map(Reply::Command) }));
                                status = "Processing command…".into();
                            },
                            command::Input::Prompt(prompt) => {
                                let (sender, receiver) = mpsc::channel(PROGRESS_BUFFER);
                                events = Some(receiver);
                                let kernel = kernel.clone();
                                request = Some(Box::pin(async move { kernel.run(RunRequest::with_events(prompt, sender)).await.map(Reply::Run) }));
                                status = "Starting…".into();
                            },
                        }
                    },
                }
            },
            event = async { events.as_mut().unwrap().recv().await }, if events.is_some() => {
                match event {
                    Some(event) => {
                        progress(screen, &mut status, event)?;
                        // Combine a bounded batch of ready updates per redraw;
                        // input and SIGINT remain serviced between batches.
                        for _ in 1..PROGRESS_BUFFER {
                            match events.as_mut().unwrap().try_recv() {
                                Ok(event) => progress(screen, &mut status, event)?,
                                Err(_) => break,
                            }
                        }
                    },
                    None => events = None,
                }
            },
            result = async { request.as_mut().unwrap().await }, if request.is_some() => {
                // A result can be ready alongside the last events. Drain those
                // before displaying completion; never replay RunOutcome's tool
                // summary after its live events have already been presented.
                if let Some(mut remaining) = events.take() {
                    while let Ok(event) = remaining.try_recv() { progress(screen, &mut status, event)?; }
                }
                request = None;
                status.clear();
                match result {
                    Ok(Reply::Run(outcome)) => screen.markdown(&render::outcome(&outcome))?,
                    Ok(Reply::Command(text)) => screen.line(&text, Tone::Text)?,
                    Err(error) => screen.line(&format!("error: {error}"), Tone::Error)?,
                }
            },
        }
    }
}

fn progress(screen: &mut Screen, status: &mut String, event: RunEvent) -> io::Result<()> {
    match event {
        RunEvent::InferenceStarted => *status = "Waiting for model…".into(),
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
