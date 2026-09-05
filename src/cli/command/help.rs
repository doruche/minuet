use clap::Command;

pub(super) fn prepare(command: Command) -> Command {
    // Bare groups are handled at the parser boundary before typed conversion.
    // Leaf argument validation remains clap's responsibility.
    fn groups(command: Command) -> Command {
        command
            .subcommand_required(false)
            .arg_required_else_help(false)
            .disable_help_subcommand(true)
            .mut_subcommands(groups)
    }
    let mut command = groups(command);
    command.build();
    format_tree(command, true)
}

fn format_tree(mut command: Command, root: bool) -> Command {
    let mut sections = Vec::new();
    if let Some(about) = command.get_about() {
        sections.push(about.to_string());
    }
    // Slash commands have no executable prefix. Clap retains a leading space
    // for that empty prefix; normalize it at this presentation boundary.
    let usage = command.render_usage().to_string();
    let usage = usage
        .strip_prefix("Usage:")
        .expect("clap usage heading")
        .trim();
    command = command.override_usage(usage.to_owned());
    if !root {
        sections.push(format!("Usage: {usage}"));
    }
    let children: Vec<_> = command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
        .map(|child| {
            let aliases = child.get_visible_aliases().collect::<Vec<_>>();
            let suffix = if aliases.is_empty() {
                String::new()
            } else {
                format!(" (aliases: {})", aliases.join(", "))
            };
            entry(
                &format!("{}{suffix}", child.get_name()),
                child.get_about().map(ToString::to_string),
            )
        })
        .collect();
    if !children.is_empty() {
        sections.push(format!("Commands:\n{}", children.join("\n")));
    }
    let arguments: Vec<_> = command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set())
        .map(|arg| entry(&arg.to_string(), arg.get_help().map(ToString::to_string)))
        .collect();
    if !arguments.is_empty() {
        sections.push(format!("Arguments and options:\n{}", arguments.join("\n")));
    }
    command = command.override_help(sections.join("\n\n"));
    command.mut_subcommands(|child| format_tree(child, false))
}

fn entry(label: &str, description: Option<String>) -> String {
    match description {
        Some(description) => format!("{label} — {description}"),
        None => label.to_owned(),
    }
}
