mod wire;

use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{
    Client, StatusCode, Url,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::Value;
use thiserror::Error;
use tokio::time::{Duration as TokioDuration, timeout};

use super::{InferenceBackend, InferenceError, InferenceRequest, InferenceResponse};
use wire::{create_request, parse_response, upstream_message};

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
            if !pending.iter().any(|byte| *byte == b'\n' || *byte == b'\r')
                && pending.len() > MAX_SSE_EVENT_BYTES
            {
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
    #[test]
    fn rejects_empty_api_keys_before_building_a_client() {
        assert!(matches!(
            OpenAiResponsesBackend::new("https://example.test/v1", ""),
            Err(OpenAiResponsesBackendError::EmptyApiKey)
        ));
    }
}
