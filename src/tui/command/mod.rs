mod context;
mod help;
mod model;
mod session;
mod tools;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind};
use minuet::kernel::{KernelError, KernelHandle};

#[derive(Debug, Eq, PartialEq)]
pub enum Input {
    Empty,
    Prompt(String),
    Command(Command),
    Notice { text: String, is_error: bool },
}

// Each subtree owns its grammar and execution. This root only registers and
// dispatches command families; help reads the same declaration as the parser.
#[derive(Debug, Eq, PartialEq, Subcommand)]
pub enum Command {
    /// Show available commands
    #[command(name = "/help")]
    Help,
    /// Exit Minuet through shutdown
    #[command(name = "/exit", visible_alias = "/quit")]
    Exit,
    /// Manage sessions
    #[command(name = "/session", subcommand)]
    Session(session::Command),
    /// Inspect and configure the active model
    #[command(name = "/model", subcommand)]
    Model(model::Command),
    /// Inspect and configure compiled tools
    #[command(name = "/tools", subcommand)]
    Tools(tools::Command),
    /// Inspect committed context
    #[command(name = "/context", subcommand)]
    Context(context::Command),
}

impl Command {
    pub async fn execute(self, kernel: &KernelHandle) -> Result<String, KernelError> {
        match self {
            Self::Model(command) => command.execute(kernel).await,
            Self::Tools(command) => command.execute(kernel).await,
            Self::Context(command) => command.execute(kernel).await,
            Self::Session(command) => command.execute(kernel).await,
            Self::Exit | Self::Help => unreachable!("handled by the interaction/parser boundary"),
        }
    }
}

#[derive(Parser)]
#[command(no_binary_name = true, name = "Minuet commands", bin_name = "",
    disable_help_subcommand = true, color = clap::ColorChoice::Never)]
struct CommandLine {
    #[command(subcommand)]
    command: Command,
}

pub fn parse(line: &str) -> Input {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Input::Empty;
    }
    if !trimmed.starts_with('/') {
        // Prompt whitespace belongs to the user, including pasted code blocks.
        return Input::Prompt(line.to_owned());
    }
    let Some(words) = shlex::split(trimmed) else {
        return Input::Notice {
            text: "invalid command: unmatched quote or trailing escape".into(),
            is_error: true,
        };
    };
    let mut declaration = help::prepare(CommandLine::command());
    match declaration.try_get_matches_from_mut(words) {
        Ok(matches) => {
            let mut node = &declaration;
            let mut arguments = &matches;
            while let Some((name, child)) = arguments.subcommand() {
                node = node
                    .find_subcommand(name)
                    .expect("parsed command belongs to declaration");
                arguments = child;
            }
            // A bare group is an informational help request. Required arguments
            // on leaf commands have already been validated by clap above.
            if node.get_subcommands().next().is_some() || node.get_name() == "/help" {
                let node = if node.get_name() == "/help" {
                    &declaration
                } else {
                    node
                };
                return Input::Notice {
                    text: node.clone().render_help().to_string().trim_end().to_owned(),
                    is_error: false,
                };
            }
            Input::Command(
                CommandLine::from_arg_matches(&matches)
                    .expect("validated leaf matches its typed command")
                    .command,
            )
        },
        Err(error) => Input::Notice {
            is_error: !matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ),
            text: error.to_string().trim_end().to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(line: &str, error: bool) -> String {
        let Input::Notice { text, is_error } = parse(line) else {
            panic!("expected notice for {line}");
        };
        assert_eq!(is_error, error, "{line}: {text}");
        text
    }

    #[test]
    fn preserves_prompt_whitespace_and_quoted_opaque_values() {
        let prompt = "  中文\n    code\n";
        assert_eq!(parse(prompt), Input::Prompt(prompt.into()));
        assert_eq!(parse(" \t\n"), Input::Empty);
        assert_eq!(
            parse("/model effort 'vendor depth $HOME'"),
            Input::Command(Command::Model(model::Command::Effort {
                value: "vendor depth $HOME".into()
            }))
        );
    }

    #[test]
    fn dispatches_typed_subcommands_and_aliases() {
        assert_eq!(
            parse(" /tools  disable\t echo "),
            Input::Command(Command::Tools(tools::Command::Disable {
                name: "echo".into()
            }))
        );
        assert_eq!(
            parse("/tools list"),
            Input::Command(Command::Tools(tools::Command::List))
        );
        assert_eq!(
            parse("/context info"),
            Input::Command(Command::Context(context::Command::Info))
        );
        assert_eq!(parse("/clear"), Input::Command(Command::Clear));
        assert_eq!(
            parse("/model info"),
            Input::Command(Command::Model(model::Command::Info))
        );
        assert_eq!(parse("/quit"), Input::Command(Command::Exit));
    }

    #[test]
    fn bare_groups_and_explicit_help_share_unindented_layout() {
        for group in ["/tools", "/context", "/model"] {
            let text = notice(group, false);
            assert_eq!(text, notice(&format!("{group} --help"), false));
            assert!(text.contains(&format!("Usage: {group}")), "{text}");
            assert_eq!(text.trim(), text);
            assert!(text.lines().all(|line| !line.starts_with(' ')), "{text}");
        }
        let root = notice("/help", false);
        assert!(root.contains("/tools —"));
        assert!(root.contains("/quit"));
        let nested = notice("/tools enable --help", false);
        assert!(nested.contains("Usage: /tools enable <NAME>"), "{nested}");
        assert!(nested.contains("<NAME>"));
        assert_eq!(nested.trim(), nested);
    }

    #[test]
    fn leaf_errors_remain_errors() {
        for line in [
            "/tools enable",
            "/tools unknown",
            "/tools enable echo extra",
            "/new extra",
            "/context unknown",
            "/model effort",
            "/unknown",
            "/model effort 'unterminated",
        ] {
            assert!(!notice(line, true).is_empty());
        }
    }
}
