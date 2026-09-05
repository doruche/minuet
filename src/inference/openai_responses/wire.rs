use serde_json::{Map, Value, json};

use super::OpenAiResponsesBackendError;
use crate::inference::{
    ContinuationItem, ConversationItem, InferenceRequest, InferenceResponse, ModelOutputItem,
    OutputEffect, TokenUsage, ToolCall,
};
use crate::tool::ToolDefinition;

pub(super) fn create_request(request: &InferenceRequest<'_>, generation: bool) -> Value {
    let input = request
        .input
        .iter()
        .map(encode_input_item)
        .collect::<Vec<_>>();
    let tools = request.tools.iter().map(encode_tool).collect::<Vec<_>>();

    let mut body = Map::from_iter([
        ("model".to_owned(), json!(request.model)),
        ("input".to_owned(), Value::Array(input)),
        ("tools".to_owned(), Value::Array(tools)),
    ]);
    if let Some(effort) = request.reasoning_effort {
        body.insert("reasoning".to_owned(), json!({ "effort": effort.as_str() }));
    }
    if generation {
        body.insert("stream".to_owned(), Value::Bool(true));
        body.insert("store".to_owned(), Value::Bool(false));
        // Stateless OpenAI reasoning turns need this opaque continuation data.
        // Compatible providers may omit it from their response, in which case
        // their returned reasoning item is still replayed unchanged.
        body.insert("include".to_owned(), json!(["reasoning.encrypted_content"]));
        // Full context is locally managed. Disabling server truncation keeps an
        // overflow observable instead of silently changing the conversation.
        body.insert(
            "truncation".to_owned(),
            Value::String("disabled".to_owned()),
        );
    }
    Value::Object(body)
}

fn encode_input_item(item: &ConversationItem) -> Value {
    match item {
        ConversationItem::UserText(text) => json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }]
        }),
        ConversationItem::Continuation(item) => item.protocol_value().clone(),
        ConversationItem::FunctionCallOutput { call_id, output } => json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": output
        }),
    }
}

fn encode_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters
    })
}

pub(super) fn parse_response(
    value: Value,
) -> Result<InferenceResponse, OpenAiResponsesBackendError> {
    if value.get("status").and_then(Value::as_str) != Some("completed") {
        return Err(OpenAiResponsesBackendError::Incomplete(upstream_message(
            &value,
        )));
    }

    let raw_output = value.get("output").and_then(Value::as_array).ok_or(
        OpenAiResponsesBackendError::MalformedResponse("missing output array"),
    )?;
    let output = raw_output
        .iter()
        .cloned()
        .map(parse_output_item)
        .collect::<Result<Vec<_>, _>>()?;
    let usage = value
        .get("usage")
        .filter(|usage| !usage.is_null())
        .map(parse_usage)
        .transpose()?;

    Ok(InferenceResponse { output, usage })
}

fn parse_output_item(value: Value) -> Result<ModelOutputItem, OpenAiResponsesBackendError> {
    let item_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let effect = match item_type {
        "message" => {
            value.get("content").and_then(Value::as_array).ok_or(
                OpenAiResponsesBackendError::MalformedResponse("message content"),
            )?;
            let text = value
                .get("content")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .filter(|content| {
                    content.get("type").and_then(Value::as_str) == Some("output_text")
                })
                .map(|content| {
                    content.get("text").and_then(Value::as_str).ok_or(
                        OpenAiResponsesBackendError::MalformedResponse("output_text text"),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect::<String>();
            if text.is_empty() {
                OutputEffect::None
            } else {
                OutputEffect::Text(text)
            }
        },
        "function_call" => OutputEffect::ToolCall(ToolCall {
            call_id: required_string(&value, "call_id")?.to_owned(),
            name: required_string(&value, "name")?.to_owned(),
            arguments: required_string(&value, "arguments")?.to_owned(),
        }),
        _ => OutputEffect::None,
    };

    Ok(ModelOutputItem::new(
        ContinuationItem::from_protocol_value(value),
        effect,
    ))
}

fn parse_usage(value: &Value) -> Result<TokenUsage, OpenAiResponsesBackendError> {
    Ok(TokenUsage {
        input_tokens: required_u64(value, "input_tokens")?,
        output_tokens: required_u64(value, "output_tokens")?,
        total_tokens: required_u64(value, "total_tokens")?,
    })
}

fn required_string<'a>(
    value: &'a Value,
    field: &'static str,
) -> Result<&'a str, OpenAiResponsesBackendError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(OpenAiResponsesBackendError::MalformedResponse(field))
}

fn required_u64(value: &Value, field: &'static str) -> Result<u64, OpenAiResponsesBackendError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(OpenAiResponsesBackendError::MalformedResponse(field))
}

pub(super) fn upstream_message(value: &Value) -> String {
    value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("upstream returned an unspecified error")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ReasoningEffort;

    fn definition() -> ToolDefinition {
        ToolDefinition {
            name: "echo".to_owned(),
            description: "Echo text".to_owned(),
            parameters: json!({"type":"object"}),
        }
    }

    #[test]
    fn generation_request_is_stateless_and_streaming() {
        let items = [ConversationItem::UserText("hello".to_owned())];
        let tools = [definition()];
        let effort = ReasoningEffort::new("vendor-special").unwrap();
        let request = InferenceRequest {
            model: "model",
            input: &items,
            tools: &tools,
            reasoning_effort: Some(&effort),
            observer: None,
        };

        let body = create_request(&request, true);
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["truncation"], "disabled");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["reasoning"]["effort"], "vendor-special");
        assert!(body.get("instructions").is_none());
    }

    #[test]
    fn token_count_request_omits_generation_controls() {
        let items = [ConversationItem::UserText("hello".to_owned())];
        let request = InferenceRequest {
            model: "model",
            input: &items,
            tools: &[],
            reasoning_effort: None,
            observer: None,
        };

        let body = create_request(&request, false);
        assert!(body.get("stream").is_none());
        assert!(body.get("store").is_none());
        assert!(body.get("truncation").is_none());
        assert!(body.get("include").is_none());
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn parses_all_continuations_and_projects_effects() {
        let response = json!({
            "status": "completed",
            "output": [
                {"type":"reasoning","id":"r1","summary":[]},
                {"type":"message","content":[{"type":"output_text","text":"calling"}]},
                {"type":"function_call","call_id":"call-1","name":"echo","arguments":"{\"text\":\"hi\"}"}
            ],
            "usage": {"input_tokens":10,"output_tokens":4,"total_tokens":14}
        });

        let parsed = parse_response(response).unwrap();
        assert_eq!(parsed.output.len(), 3);
        assert!(matches!(parsed.output[0].effect(), OutputEffect::None));
        assert!(matches!(parsed.output[1].effect(), OutputEffect::Text(text) if text == "calling"));
        assert!(
            matches!(parsed.output[2].effect(), OutputEffect::ToolCall(call) if call.name == "echo")
        );
        assert_eq!(parsed.usage.unwrap().total_tokens, 14);
    }

    #[test]
    fn replays_continuations_without_interpreting_them() {
        let raw = json!({"type":"reasoning","id":"opaque","vendor_field":17});
        let items = [ConversationItem::Continuation(
            ContinuationItem::from_protocol_value(raw.clone()),
        )];
        let request = InferenceRequest {
            model: "model",
            input: &items,
            tools: &[],
            reasoning_effort: None,
            observer: None,
        };

        let body = create_request(&request, true);
        assert_eq!(body["input"][0], raw);
    }

    #[test]
    fn treats_explicitly_null_usage_as_unreported() {
        let response = json!({
            "status": "completed",
            "output": [{"type":"message","content":[]}],
            "usage": null
        });

        assert_eq!(parse_response(response).unwrap().usage, None);
    }
}
