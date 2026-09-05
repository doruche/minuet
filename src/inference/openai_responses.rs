use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{
    Client, StatusCode, Url,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::{Map, Value, json};
use thiserror::Error;
use tokio::time::{Duration as TokioDuration, timeout};

use super::{
    ContinuationItem, ConversationItem, InferenceBackend, InferenceError, InferenceRequest,
    InferenceResponse, ModelOutputItem, OutputEffect, TokenUsage, ToolCall,
};
use crate::tool::ToolDefinition;

pub struct OpenAiResponsesBackend {
    client: Client,
    responses_url: Url,
    input_tokens_url: Url,
}

const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;

impl OpenAiResponsesBackend {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, OpenAiResponsesBackendError> {
        if api_key.is_empty() {
            return Err(OpenAiResponsesBackendError::EmptyApiKey);
        }
        let base_url = base_url.trim_end_matches('/');
        let responses_url = Url::parse(&format!("{base_url}/responses"))?;
        let input_tokens_url = Url::parse(&format!("{base_url}/responses/input_tokens"))?;

        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(120))
            .build()?;

        Ok(Self {
            client,
            responses_url,
            input_tokens_url,
        })
    }

    async fn post(&self, url: Url, body: &Value) -> Result<Value, OpenAiResponsesBackendError> {
        let response = self.client.post(url).json(body).send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|source| {
            OpenAiResponsesBackendError::InvalidJson {
                status,
                source,
                body: lossy_body(&bytes),
            }
        })?;

        if !status.is_success() {
            return Err(OpenAiResponsesBackendError::Upstream {
                status,
                message: upstream_message(&value),
            });
        }
        Ok(value)
    }

    async fn stream(
        &self,
        body: &Value,
        observer: Option<&dyn super::InferenceObserver>,
    ) -> Result<Value, OpenAiResponsesBackendError> {
        let response = self
            .client
            .post(self.responses_url.clone())
            .json(body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let bytes = response.bytes().await?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|source| {
                OpenAiResponsesBackendError::InvalidJson {
                    status,
                    source,
                    body: lossy_body(&bytes),
                }
            })?;
            return Err(OpenAiResponsesBackendError::Upstream {
                status,
                message: upstream_message(&value),
            });
        }
        let mut bytes = response.bytes_stream();
        let mut pending = Vec::new();
        let mut data = String::new();
        let mut completed = None;
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk?;
            pending.extend_from_slice(&chunk);
            if pending.len() > MAX_SSE_EVENT_BYTES {
                return Err(OpenAiResponsesBackendError::MalformedStream(
                    "SSE event exceeded the size limit",
                ));
            }
            while let Some(line_end) = pending
                .iter()
                .position(|byte| *byte == b'\n' || *byte == b'\r')
            {
                // Keep a trailing CR until the next chunk so a CRLF split at
                // the transport boundary remains one line ending.
                if pending[line_end] == b'\r' && line_end + 1 == pending.len() {
                    break;
                }
                let break_len = usize::from(
                    pending[line_end] == b'\r' && pending.get(line_end + 1) == Some(&b'\n'),
                ) + 1;
                let line =
                    String::from_utf8(pending.drain(..line_end).collect()).map_err(|_| {
                        OpenAiResponsesBackendError::MalformedStream("stream was not UTF-8")
                    })?;
                pending.drain(..break_len);
                if line.is_empty() {
                    if !data.is_empty() {
                        let event = parse_stream_event(&data)?;
                        if let Some(value) = event {
                            if value.get("type").and_then(Value::as_str)
                                == Some("response.output_text.delta")
                                && let Some(text) = value.get("delta").and_then(Value::as_str)
                                && let Some(observer) = observer
                            {
                                timeout(TokioDuration::from_secs(120), observer.text_delta(text))
                                    .await
                                    .map_err(|_| OpenAiResponsesBackendError::ObserverTimeout)?;
                            }
                            match value.get("type").and_then(Value::as_str) {
                                Some("response.completed") => {
                                    completed = value.get("response").cloned();
                                },
                                Some("response.failed") | Some("response.incomplete") => {
                                    let response = value.get("response").unwrap_or(&value);
                                    return Err(OpenAiResponsesBackendError::StreamTerminal(
                                        upstream_message(response),
                                    ));
                                },
                                Some("error") => {
                                    return Err(OpenAiResponsesBackendError::StreamTerminal(
                                        upstream_message(&value),
                                    ));
                                },
                                _ => {},
                            }
                        }
                        data.clear();
                    }
                } else if let Some(value) = line.strip_prefix("data:") {
                    let value = value.strip_prefix(' ').unwrap_or(value);
                    if value != "[DONE]" {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(value);
                        if data.len() > MAX_SSE_EVENT_BYTES {
                            return Err(OpenAiResponsesBackendError::MalformedStream(
                                "SSE event exceeded the size limit",
                            ));
                        }
                    }
                }
            }
        }
        if !pending.is_empty() || !data.is_empty() {
            return Err(OpenAiResponsesBackendError::MalformedStream(
                "stream ended before an SSE event boundary",
            ));
        }
        completed.ok_or(OpenAiResponsesBackendError::MalformedStream(
            "stream ended without response.completed",
        ))
    }
}

#[async_trait]
impl InferenceBackend for OpenAiResponsesBackend {
    async fn respond(
        &self,
        request: InferenceRequest<'_>,
    ) -> Result<InferenceResponse, InferenceError> {
        let body = create_request(&request, true);
        let response = self.stream(&body, request.observer).await?;
        parse_response(response).map_err(Into::into)
    }

    async fn count_input_tokens(
        &self,
        request: InferenceRequest<'_>,
    ) -> Result<u64, InferenceError> {
        let body = create_request(&request, false);
        let response = self.post(self.input_tokens_url.clone(), &body).await?;
        response
            .get("input_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(|| OpenAiResponsesBackendError::MalformedResponse("missing input_tokens"))
            .map_err(Into::into)
    }
}

fn create_request(request: &InferenceRequest<'_>, generation: bool) -> Value {
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

fn parse_response(value: Value) -> Result<InferenceResponse, OpenAiResponsesBackendError> {
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

fn upstream_message(value: &Value) -> String {
    value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("upstream returned an unspecified error")
        .to_owned()
}

fn lossy_body(bytes: &[u8]) -> String {
    const LIMIT: usize = 4096;
    let end = bytes.len().min(LIMIT);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[derive(Debug, Error)]
pub enum OpenAiResponsesBackendError {
    #[error("API key must not be empty")]
    EmptyApiKey,
    #[error("invalid provider base URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("invalid authorization header: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
    #[error("failed to build or execute HTTP request: {0}")]
    Http(#[from] reqwest::Error),
    #[error("upstream returned HTTP {status}: {message}")]
    Upstream { status: StatusCode, message: String },
    #[error("upstream returned invalid JSON with HTTP {status}: {source}; body: {body}")]
    InvalidJson {
        status: StatusCode,
        source: serde_json::Error,
        body: String,
    },
    #[error("upstream response was not completed: {0}")]
    Incomplete(String),
    #[error("malformed upstream response: {0}")]
    MalformedResponse(&'static str),
    #[error("malformed streaming response: {0}")]
    MalformedStream(&'static str),
    #[error("stream ended with an upstream error: {0}")]
    StreamTerminal(String),
    #[error("stream observer did not accept output before the deadline")]
    ObserverTimeout,
}

fn parse_stream_event(data: &str) -> Result<Option<Value>, OpenAiResponsesBackendError> {
    serde_json::from_str(data)
        .map(Some)
        .map_err(|_| OpenAiResponsesBackendError::MalformedStream("invalid SSE JSON"))
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
    fn rejects_empty_api_keys_before_building_a_client() {
        assert!(matches!(
            OpenAiResponsesBackend::new("https://example.test/v1", ""),
            Err(OpenAiResponsesBackendError::EmptyApiKey)
        ));
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
