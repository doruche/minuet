use std::{env, sync::Arc};

use minuet::{
    agent_loop::ReactLoop,
    context::FullContext,
    inference::{InferenceBackend, OpenAiResponsesBackend},
    kernel::{InputTokenCount, KernelComponents, KernelOptions, start},
    model::{ModelName, ModelSelection, ProviderId, ReasoningEffort},
    session::MemorySessionStore,
    tool::ToolRegistry,
};

#[tokio::test]
#[ignore = "requires MINIMAX_API_KEY and calls the live MiniMax API"]
async fn responses_tool_loop_and_input_count() {
    let api_key = env::var("MINIMAX_API_KEY").expect("MINIMAX_API_KEY must be set");
    let base_url =
        env::var("MINIMAX_BASE_URL").unwrap_or_else(|_| "https://api.minimax.cn/v1".to_owned());
    let model = env::var("MINIMAX_MODEL").unwrap_or_else(|_| "MiniMax-M3".to_owned());
    let backend: Arc<dyn InferenceBackend> =
        Arc::new(OpenAiResponsesBackend::new(&base_url, &api_key).unwrap());
    let running = start(
        KernelComponents {
            backend,
            store: Box::new(MemorySessionStore::default()),
            tools: ToolRegistry::with_builtins(&["echo".to_owned()]).unwrap(),
            agent_loop: Arc::new(ReactLoop::new(4).unwrap()),
            context: Arc::new(FullContext),
        },
        KernelOptions {
            model: ModelSelection {
                provider: ProviderId::new("minimax").unwrap(),
                model: ModelName::new(model).unwrap(),
            },
            default_reasoning_effort: Some(ReasoningEffort::new("low").unwrap()),
        },
    )
    .unwrap();
    let handle = running.handle();

    let outcome = handle
        .run(
            "Call the echo function exactly once with text `live-probe`. After its result, reply exactly DONE.",
        )
        .await
        .unwrap();
    assert_eq!(outcome.tool_activity.len(), 1);
    assert_eq!(outcome.tool_activity[0].name, "echo");
    assert!(outcome.text.contains("DONE"));
    assert!(outcome.usage.reported_calls >= 2);

    let context = handle.context_info().await.unwrap();
    assert!(matches!(
        context.committed_input_tokens,
        InputTokenCount::Available(tokens) if tokens > 0
    ));
    running.shutdown().await.unwrap();
}
