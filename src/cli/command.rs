#[derive(Debug, Eq, PartialEq)]
pub enum Input {
    Empty,
    Prompt(String),
    Command(Command),
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
    Usage(String),
    Unknown(String),
}

pub fn parse(line: &str) -> Input {
    let line = line.trim();
    if line.is_empty() {
        return Input::Empty;
    }
    if !line.starts_with('/') {
        return Input::Prompt(line.to_owned());
    }

    let command = match line {
        "/exit" | "/quit" => Command::Exit,
        "/help" => Command::Help,
        "/new" => Command::NewSession,
        "/model" | "/model effort" => Command::ModelInfo,
        "/model effort clear" => Command::ClearReasoningEffort,
        "/tools" => Command::ListTools,
        "/tools enable" => Command::Usage("usage: /tools enable <name>".to_owned()),
        "/tools disable" => Command::Usage("usage: /tools disable <name>".to_owned()),
        "/context info" => Command::ContextInfo,
        _ => {
            if let Some(effort) = line.strip_prefix("/model effort ") {
                let effort = effort.trim();
                if effort.is_empty() {
                    Command::Usage("usage: /model effort <value>|clear".to_owned())
                } else {
                    Command::SetReasoningEffort(effort.to_owned())
                }
            } else if let Some(name) = line.strip_prefix("/tools enable ") {
                let name = name.trim();
                if name.is_empty() {
                    Command::Usage("usage: /tools enable <name>".to_owned())
                } else {
                    Command::SetToolEnabled {
                        name: name.to_owned(),
                        enabled: true,
                    }
                }
            } else if let Some(name) = line.strip_prefix("/tools disable ") {
                let name = name.trim();
                if name.is_empty() {
                    Command::Usage("usage: /tools disable <name>".to_owned())
                } else {
                    Command::SetToolEnabled {
                        name: name.to_owned(),
                        enabled: false,
                    }
                }
            } else {
                Command::Unknown(line.to_owned())
            }
        },
    };

    Input::Command(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prompts_and_empty_lines() {
        assert_eq!(parse("  hello  "), Input::Prompt("hello".to_owned()));
        assert_eq!(parse("  \t"), Input::Empty);
    }

    #[test]
    fn parses_fixed_commands() {
        assert_eq!(parse("/quit"), Input::Command(Command::Exit));
        assert_eq!(parse("/model"), Input::Command(Command::ModelInfo));
        assert_eq!(
            parse("/model effort clear"),
            Input::Command(Command::ClearReasoningEffort)
        );
        assert_eq!(parse("/tools"), Input::Command(Command::ListTools));
        assert_eq!(parse("/context info"), Input::Command(Command::ContextInfo));
    }

    #[test]
    fn parses_argument_commands_and_usage_errors() {
        assert_eq!(
            parse("/model effort deep"),
            Input::Command(Command::SetReasoningEffort("deep".to_owned()))
        );
        assert_eq!(
            parse("/tools disable echo"),
            Input::Command(Command::SetToolEnabled {
                name: "echo".to_owned(),
                enabled: false,
            })
        );
        assert_eq!(
            parse("/tools enable"),
            Input::Command(Command::Usage("usage: /tools enable <name>".to_owned()))
        );
        assert_eq!(
            parse("/tools enable   "),
            Input::Command(Command::Usage("usage: /tools enable <name>".to_owned()))
        );
    }
}
