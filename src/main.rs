mod cli;

use std::{error::Error, sync::Arc};

use minuet::{
    agent_loop::ReactLoop,
    config::{Config, Protocol},
    context::FullContext,
    inference::{InferenceBackend, OpenAiResponsesBackend},
    kernel::{KernelComponents, KernelOptions, start},
    session::MemorySessionStore,
    tool::ToolRegistry,
};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("minuet: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;
    let backend: Arc<dyn InferenceBackend> = match config.provider.protocol {
        Protocol::OpenAiResponses => {
            let api_key = config.provider.api_key()?;
            Arc::new(OpenAiResponsesBackend::new(
                &config.provider.base_url,
                &api_key,
            )?)
        },
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
    let cli_result = cli::run(running.handle()).await;
    let shutdown_result = running.shutdown().await;
    cli_result?;
    shutdown_result?;
    Ok(())
}
