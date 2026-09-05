use std::{
    future::Future,
    io::{self, BufRead},
    pin::Pin,
};

use crossterm::event::{self, Event};
use minuet::{
    agent_loop::{RunEvent, RunOutcome},
    kernel::{KernelError, KernelHandle},
};
use tokio::sync::mpsc;

use super::{
    command, handler,
    input::{Action, Input},
    render::{self, Tone},
    terminal::Screen,
};

const PROGRESS_BUFFER: usize = 32;

enum Reply {
    Run(RunOutcome),
    Command(handler::Effect),
}
type Request = Pin<Box<dyn Future<Output = Result<Reply, KernelError>>>>;

enum UserInput {
    Terminal,
    Lines(mpsc::Receiver<io::Result<String>>),
}

enum InputEvent {
    Terminal(Event),
    Line(String),
}

impl UserInput {
    fn new(interactive: bool) -> io::Result<Self> {
        if interactive {
            return Ok(Self::Terminal);
        }
        let (sender, receiver) = mpsc::channel(1);
        // Blocking pipe/console reads cannot be cancelled portably. Keep this
        // process-lifetime reader outside Tokio's blocking pool so runtime
        // teardown never joins a read waiting for external input. It owns only
        // stdin and this bounded sender; after CLI exit a blocked read is
        // reclaimed at process exit, and a completed read observes disconnect.
        std::thread::Builder::new()
            .name("minuet-stdin".into())
            .spawn(move || {
                for line in io::stdin().lock().lines() {
                    let failed = line.is_err();
                    if sender.blocking_send(line).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self::Lines(receiver))
    }

    async fn next(&mut self) -> io::Result<Option<InputEvent>> {
        match self {
            Self::Terminal => loop {
                // Ratatui's inline operations also read cursor-position replies.
                // Poll only on this interaction task, never through a competing
                // EventStream/background reader that can consume those replies.
                if event::poll(std::time::Duration::ZERO)? {
                    break event::read().map(|event| Some(InputEvent::Terminal(event)));
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            },
            Self::Lines(lines) => lines
                .recv()
                .await
                .transpose()
                .map(|line| line.map(InputEvent::Line)),
        }
    }
}

pub async fn run(kernel: KernelHandle) -> io::Result<()> {
    let mut screen = Screen::open()?;
    let result = interact(kernel, &mut screen).await;
    let restore = screen.finish();
    result.and(restore)
}

async fn interact(kernel: KernelHandle, screen: &mut Screen) -> io::Result<()> {
    let interactive = screen.interactive();
    let mut source = UserInput::new(interactive)?;
    let mut input = Input::default();
    let mut request: Option<Request> = None;
    let mut events: Option<mpsc::Receiver<RunEvent>> = None;
    // A presentation of received events, never execution authority. Request
    // ownership (the pending future), not this label, governs input admission.
    let mut status = String::new();
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    if interactive {
        screen.line("Minuet — /help for commands", Tone::Meta)?;
    }

    loop {
        screen.draw(&input, &status, request.is_some())?;
        tokio::select! {
            biased;
            signal = &mut interrupt => {
                signal?;
                screen.line("Exiting; waiting for shutdown.", Tone::Meta)?;
                return Ok(());
            },
            incoming = source.next(), if interactive || request.is_none() => {
                let Some(incoming) = incoming? else { return Ok(()); };
                let action = match incoming {
                    InputEvent::Terminal(event) => input.handle(event, request.is_some()),
                    InputEvent::Line(line) => Action::Submit(line),
                };
                match action {
                    Action::Exit => {
                        screen.line("Exiting; waiting for shutdown.", Tone::Meta)?;
                        return Ok(());
                    },
                    Action::Edit => {},
                    Action::Submit(line) => {
                        if interactive && !line.trim().is_empty() {
                            screen.line(&format!("› {line}"), Tone::User)?;
                        }
                        match command::parse(&line) {
                            command::Input::Empty => {},
                            command::Input::Notice { text, is_error } => {
                                screen.line(&text, if is_error { Tone::Error } else { Tone::Text })?;
                            },
                            command::Input::Command(command::Command::Exit) => return Ok(()),
                            command::Input::Command(command) => {
                                let kernel = kernel.clone();
                                request = Some(Box::pin(async move { handler::handle(&kernel, command).await.map(Reply::Command) }));
                                status = "Processing command…".into();
                            },
                            command::Input::Prompt(prompt) => {
                                let (sender, receiver) = mpsc::channel(PROGRESS_BUFFER);
                                events = Some(receiver);
                                let kernel = kernel.clone();
                                request = Some(Box::pin(async move { kernel.run_with_events(prompt, sender).await.map(Reply::Run) }));
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
                    Ok(Reply::Run(outcome)) => screen.line(&render::outcome(&outcome), Tone::Text)?,
                    Ok(Reply::Command(handler::Effect::Exit)) => return Ok(()),
                    Ok(Reply::Command(effect)) => screen.line(&render::effect(effect), Tone::Text)?,
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
