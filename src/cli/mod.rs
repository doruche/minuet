pub mod chat;

use std::io::{self, IsTerminal, Read};

use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};

#[derive(Parser)]
#[command(
    version,
    about = "An agent harness with terminal and one-shot chat interfaces"
)]
struct Arguments {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run one conversation turn and print its final text
    Chat {
        /// Prompt text, or - to read the entire UTF-8 prompt from stdin
        prompt: String,
    },
    /// Open the interactive terminal interface (the default)
    Tui,
}

/// Resolved process input. Frontends never interpret argv or choose a mode
/// based on their renderer; chat text is literal, including slash commands.
pub enum Invocation {
    Chat(String),
    Tui,
}

pub fn parse() -> io::Result<Invocation> {
    match Arguments::parse().command.unwrap_or(Command::Tui) {
        Command::Chat { mut prompt } => {
            if prompt == "-" {
                // Read before creating a runtime or kernel. A blocked stdin read
                // then needs no background reader or asynchronous teardown owner;
                // SIGINT still has its normal process-level behavior here.
                prompt.clear();
                io::stdin().read_to_string(&mut prompt)?;
            }
            if prompt.trim().is_empty() {
                Arguments::command()
                    .error(ErrorKind::ValueValidation, "prompt must not be empty")
                    .exit();
            }
            Ok(Invocation::Chat(prompt))
        },
        Command::Tui => {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                Arguments::command()
                    .error(
                        ErrorKind::InvalidValue,
                        "TUI requires terminal stdin and stdout; use `minuet chat <PROMPT>` or `minuet chat -`",
                    )
                    .exit();
            }
            Ok(Invocation::Tui)
        },
    }
}
