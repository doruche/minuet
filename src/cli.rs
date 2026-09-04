use std::io::{self, Write};

use minuet::{
    agent_loop::RunOutcome,
    kernel::{InputTokenCount, KernelHandle},
    session::UsageSummary,
};
use tokio::io::{AsyncBufReadExt, BufReader};

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
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('/') {
            if handle_command(&kernel, line).await == CommandFlow::Exit {
                return Ok(());
            }
            continue;
        }

        match kernel.run(line).await {
            Ok(outcome) => print_outcome(&outcome),
            Err(error) => eprintln!("error: {error}"),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CommandFlow {
    Continue,
    Exit,
}

async fn handle_command(kernel: &KernelHandle, line: &str) -> CommandFlow {
    match line {
        "/exit" | "/quit" => return CommandFlow::Exit,
        "/help" => print_help(),
        "/new" => match kernel.new_session().await {
            Ok(id) => println!("started memory session {id}"),
            Err(error) => eprintln!("error: {error}"),
        },
        "/model" | "/model effort" => print_model(kernel).await,
        "/model effort clear" => match kernel.set_reasoning_effort(None).await {
            Ok(()) => println!("reasoning effort cleared; the upstream default will be used"),
            Err(error) => eprintln!("error: {error}"),
        },
        "/tools" => print_tools(kernel).await,
        "/context info" => print_context(kernel).await,
        _ => {
            if let Some(effort) = line.strip_prefix("/model effort ") {
                set_effort(kernel, effort).await;
            } else if let Some(name) = line.strip_prefix("/tools enable ") {
                set_tool(kernel, name, true).await;
            } else if let Some(name) = line.strip_prefix("/tools disable ") {
                set_tool(kernel, name, false).await;
            } else {
                eprintln!("unknown command: {line}");
            }
        },
    }
    CommandFlow::Continue
}

async fn print_model(kernel: &KernelHandle) {
    match kernel.model_info().await {
        Ok(info) => {
            println!("provider: {}", info.provider);
            println!("model: {}", info.model);
            println!(
                "reasoning effort: {}",
                info.reasoning_effort
                    .as_deref()
                    .unwrap_or("upstream default")
            );
        },
        Err(error) => eprintln!("error: {error}"),
    }
}

async fn set_effort(kernel: &KernelHandle, effort: &str) {
    let effort = effort.trim();
    if effort.is_empty() {
        eprintln!("usage: /model effort <value>|clear");
        return;
    }
    match kernel.set_reasoning_effort(Some(effort.to_owned())).await {
        Ok(()) => println!("reasoning effort set to `{effort}`"),
        Err(error) => eprintln!("error: {error}"),
    }
}

async fn print_tools(kernel: &KernelHandle) {
    match kernel.list_tools().await {
        Ok(tools) => {
            for tool in tools {
                let state = if tool.enabled { "enabled" } else { "disabled" };
                println!("{} ({state}) — {}", tool.name, tool.description);
            }
        },
        Err(error) => eprintln!("error: {error}"),
    }
}

async fn set_tool(kernel: &KernelHandle, name: &str, enabled: bool) {
    let name = name.trim();
    if name.is_empty() {
        eprintln!(
            "usage: /tools {} <name>",
            if enabled { "enable" } else { "disable" }
        );
        return;
    }
    match kernel.set_tool_enabled(name, enabled).await {
        Ok(()) => println!(
            "tool `{name}` {}",
            if enabled { "enabled" } else { "disabled" }
        ),
        Err(error) => eprintln!("error: {error}"),
    }
}

async fn print_context(kernel: &KernelHandle) {
    match kernel.context_info().await {
        Ok(info) => {
            println!("session: {}", info.session_id);
            match info.committed_input_tokens {
                InputTokenCount::Available(tokens) => {
                    println!("committed input tokens: {tokens} (upstream count)");
                },
                InputTokenCount::Unavailable(error) => {
                    println!("committed input tokens: unavailable ({error})");
                },
            }
            print_usage(&info.usage);
        },
        Err(error) => eprintln!("error: {error}"),
    }
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
