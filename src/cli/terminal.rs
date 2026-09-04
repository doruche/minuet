use std::io::{self, Write};

use minuet::{agent_loop::RunOutcome, kernel::KernelHandle, session::UsageSummary};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{command, handler};

pub async fn run(kernel: KernelHandle) -> io::Result<()> {
    println!("Minuet — type /help for commands");
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print!("minuet> ");
        io::stdout().flush()?;
        let Some(line) = lines.next_line().await? else {
            println!();
            return Ok(());
        };

        match command::parse(&line) {
            command::Input::Empty => {},
            command::Input::Prompt(prompt) => match kernel.run(prompt).await {
                Ok(outcome) => print_outcome(&outcome),
                Err(error) => eprintln!("error: {error}"),
            },
            command::Input::Command(command) => match handler::handle(&kernel, command).await {
                Ok(handler::Effect::Exit) => return Ok(()),
                Ok(effect) => render(effect),
                Err(error) => eprintln!("error: {error}"),
            },
        }
    }
}

fn render(effect: handler::Effect) {
    match effect {
        handler::Effect::Exit => unreachable!("exit effects are handled by the terminal loop"),
        handler::Effect::Help => print_help(),
        handler::Effect::NewSession(id) => println!("started memory session {id}"),
        handler::Effect::Model(info) => print_model(&info),
        handler::Effect::ReasoningEffortSet(Some(effort)) => {
            println!("reasoning effort set to `{effort}`")
        },
        handler::Effect::ReasoningEffortSet(None) => {
            println!("reasoning effort cleared; the upstream default will be used")
        },
        handler::Effect::Tools(tools) => print_tools(&tools),
        handler::Effect::ToolEnabled { name, enabled } => println!(
            "tool `{name}` {}",
            if enabled { "enabled" } else { "disabled" }
        ),
        handler::Effect::Context(info) => print_context(&info),
        handler::Effect::Notice(message) => eprintln!("{message}"),
    }
}

fn print_model(info: &minuet::kernel::ModelInfo) {
    println!("provider: {}", info.provider);
    println!("model: {}", info.model);
    println!(
        "reasoning effort: {}",
        info.reasoning_effort
            .as_deref()
            .unwrap_or("upstream default")
    );
}

fn print_tools(tools: &[minuet::tool::ToolStatus]) {
    for tool in tools {
        let state = if tool.enabled { "enabled" } else { "disabled" };
        println!("{} ({state}) — {}", tool.name, tool.description);
    }
}

fn print_context(info: &minuet::kernel::ContextInfo) {
    println!("session: {}", info.session_id);
    match &info.committed_input_tokens {
        minuet::kernel::InputTokenCount::Available(tokens) => {
            println!("committed input tokens: {tokens} (upstream count)");
        },
        minuet::kernel::InputTokenCount::Unavailable(error) => {
            println!("committed input tokens: unavailable ({error})");
        },
    }
    print_usage(&info.usage);
}

fn print_usage(usage: &UsageSummary) {
    if let Some(latest) = usage.latest {
        println!(
            "last reported usage: input {}, output {}, total {}",
            latest.input_tokens, latest.output_tokens, latest.total_tokens
        );
    } else if usage.reported_calls + usage.unreported_calls == 0 {
        println!("last reported usage: no model calls yet");
    } else {
        println!("last reported usage: unavailable for the latest model call");
    }
    let qualifier = if usage.unreported_calls == 0 {
        ""
    } else {
        " (partial: at least one call omitted usage)"
    };
    println!(
        "session reported usage: input {}, output {}, total {}{qualifier}",
        usage.input_tokens, usage.output_tokens, usage.total_tokens
    );
}

fn print_outcome(outcome: &RunOutcome) {
    for activity in &outcome.tool_activity {
        let state = if activity.is_error { "error" } else { "ok" };
        println!("[tool {}: {state}] {}", activity.name, activity.output);
    }
    if outcome.text.is_empty() {
        println!("(no text output)");
    } else {
        println!("{}", outcome.text);
    }
}

fn print_help() {
    println!(
        "\
/help                       show this help
/exit, /quit                exit Minuet
/new                        start a new in-memory session
/model                      show the active provider, model, and effort
/model effort <value>       pass an opaque reasoning effort to the provider
/model effort clear         use the provider's default effort
/tools                      list compiled tools and their state
/tools enable <name>        expose a compiled tool to subsequent model calls
/tools disable <name>       hide a tool from subsequent model calls
/context info               ask upstream to count committed context and show usage"
    );
}
