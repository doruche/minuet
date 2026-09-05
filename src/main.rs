mod cli;
mod tui;

use std::{error::Error, process::ExitCode, sync::Arc};

use minuet::{
    agent_loop::ReactLoop,
    config::{Config, Protocol},
    context::FullContext,
    inference::{InferenceBackend, OpenAiResponsesBackend},
    kernel::{KernelComponents, KernelOptions, start},
    session::MemorySessionStore,
    tool::ToolRegistry,
};

fn main() -> ExitCode {
    let result = (|| {
        let invocation = cli::parse()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        runtime.block_on(run(invocation))
    })();
    match result {
        Ok(exit) => exit,
        Err(error) => report(error),
    }
}

fn report(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("minuet: {error}");
    ExitCode::FAILURE
}

async fn run(invocation: cli::Invocation) -> Result<ExitCode, Box<dyn Error>> {
    let config = Config::load()?;
    let backend: Arc<dyn InferenceBackend> = match config.provider.protocol {
        Protocol::OpenAiResponses => Arc::new(OpenAiResponsesBackend::new(
            &config.provider.base_url,
            config.provider.api_key(),
        )?),
    };
    let tools = ToolRegistry::with_builtins(&config.enabled_tools)?;
    let agent_loop = Arc::new(ReactLoop::new(config.loop_max_steps)?);
    let running = start(
        KernelComponents {
            backend,
            store: Box::new(MemorySessionStore::default()),
            tools,
            agent_loop,
            context: Arc::new(FullContext),
        },
        KernelOptions {
            model: config.model,
            default_reasoning_effort: config.default_reasoning_effort,
        },
    )?;
    let frontend_result = match invocation {
        cli::Invocation::Tui => tui::run(running.handle())
            .await
            .map(|()| ExitCode::SUCCESS)
            .map_err(Into::into),
        cli::Invocation::Chat(prompt) => cli::chat::run(running.handle(), prompt).await,
    };
    // Every frontend result, including output failure and interruption, passes
    // through the same join. Report both errors if frontend and shutdown fail.
    let shutdown_result = running.shutdown().await;
    let exit = match frontend_result {
        Ok(exit) => exit,
        Err(error) => report(error),
    };
    Ok(match shutdown_result {
        Ok(()) => exit,
        Err(error) => report(error),
    })
}
