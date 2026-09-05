use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};

#[derive(Debug, Eq, PartialEq)]
pub enum Input {
    Empty,
    Prompt(String),
    Command(Command),
    Notice { text: String, is_error: bool },
}

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Exit,
    Help,
    NewSession,
    ModelInfo,
    SetReasoningEffort(String),
    ClearReasoningEffort,
    ListTools,
    SetToolEnabled { name: String, enabled: bool },
    ContextInfo,
}

// The declaration is the source for grammar, aliases, usage and help. The
// supplied tokens are command input, not a process argv with an executable.
#[derive(Parser)]
#[command(no_binary_name = true,
    name = "Minuet commands",
    bin_name = "",
    help_template = "{subcommands}", disable_help_subcommand = true, color = clap::ColorChoice::Never)]
struct CommandLine {
    #[command(subcommand)]
    command: ParsedCommand,
}

#[derive(Subcommand)]
enum ParsedCommand {
    /// Show available commands
    #[command(name = "/help")]
    Help,
    /// Exit Minuet through shutdown
    #[command(name = "/exit", visible_alias = "/quit")]
    Exit,
    /// Start a new in-memory session
    #[command(name = "/new")]
    New,
    /// Show the active provider, model and effort
    #[command(name = "/model")]
    Model {
        #[command(subcommand)]
        command: Option<ModelCommand>,
    },
    /// List compiled tools and their state
    #[command(name = "/tools")]
    Tools {
        #[command(subcommand)]
        command: Option<ToolsCommand>,
    },
    /// Inspect committed context
    #[command(name = "/context")]
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
}

#[derive(Subcommand)]
enum ModelCommand {
    /// Set an opaque effort value; 'clear' uses the upstream default
    Effort { value: Option<String> },
}

#[derive(Subcommand)]
enum ToolsCommand {
    /// Expose a compiled tool to subsequent model calls
    Enable { name: String },
    /// Hide a tool from subsequent model calls
    Disable { name: String },
}

#[derive(Subcommand)]
enum ContextCommand {
    /// Ask upstream to count committed context and show usage
    Info,
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
    match CommandLine::try_parse_from(words) {
        Ok(parsed) => Input::Command(match parsed.command {
            ParsedCommand::Help => Command::Help,
            ParsedCommand::Exit => Command::Exit,
            ParsedCommand::New => Command::NewSession,
            ParsedCommand::Model {
                command: None | Some(ModelCommand::Effort { value: None }),
            } => Command::ModelInfo,
            ParsedCommand::Model {
                command: Some(ModelCommand::Effort { value: Some(value) }),
            } => {
                if value == "clear" {
                    Command::ClearReasoningEffort
                } else {
                    Command::SetReasoningEffort(value)
                }
            },
            ParsedCommand::Tools { command: None } => Command::ListTools,
            ParsedCommand::Tools {
                command: Some(ToolsCommand::Enable { name }),
            } => Command::SetToolEnabled {
                name,
                enabled: true,
            },
            ParsedCommand::Tools {
                command: Some(ToolsCommand::Disable { name }),
            } => Command::SetToolEnabled {
                name,
                enabled: false,
            },
            ParsedCommand::Context {
                command: ContextCommand::Info,
            } => Command::ContextInfo,
        }),
        Err(error) => Input::Notice {
            is_error: !matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ),
            text: error.to_string(),
        },
    }
}

pub fn help() -> String {
    let mut command = CommandLine::command();
    format!(
        "{}\nUse /<command> --help for subcommands and arguments.",
        command.render_help()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_prompt_whitespace_and_treats_only_slash_input_as_commands() {
        let prompt = "  中文\n    code\n";
        assert_eq!(parse(prompt), Input::Prompt(prompt.into()));
        assert_eq!(parse(" \t\n"), Input::Empty);
    }

    #[test]
    fn recognizes_subcommands_independent_of_spacing() {
        for text in ["/model effort clear", " /model  effort\t clear "] {
            assert_eq!(parse(text), Input::Command(Command::ClearReasoningEffort));
        }
        assert_eq!(
            parse("/tools   disable echo"),
            Input::Command(Command::SetToolEnabled {
                name: "echo".into(),
                enabled: false
            })
        );
        assert_eq!(parse("/context info"), Input::Command(Command::ContextInfo));
        assert_eq!(parse("/quit"), Input::Command(Command::Exit));
        assert_eq!(parse("/model effort"), Input::Command(Command::ModelInfo));
    }

    #[test]
    fn accepts_quoted_opaque_values_without_shell_expansion() {
        assert_eq!(
            parse("/model effort 'vendor depth $HOME'"),
            Input::Command(Command::SetReasoningEffort("vendor depth $HOME".into()))
        );
    }

    #[test]
    fn reports_invalid_subcommands_missing_and_extra_arguments() {
        for text in [
            "/tools enable",
            "/tools unknown",
            "/tools enable echo extra",
            "/new extra",
            "/context",
            "/unknown",
            "/model effort 'unterminated",
        ] {
            assert!(
                matches!(parse(text), Input::Notice { is_error: true, .. }),
                "{text}"
            );
        }
    }

    #[test]
    fn nested_help_is_returned_without_exiting_the_process() {
        let input = parse("/tools enable --help");
        assert!(
            matches!(&input, Input::Notice { text, is_error: false }
            if text.contains("/tools enable") && text.contains("<NAME>")),
            "{input:?}"
        );
        assert!(help().contains("/tools"));
    }
}
