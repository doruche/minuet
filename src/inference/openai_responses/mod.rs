mod sse;
mod stream;
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
use wire::{create_request, upstream_message};

pub struct OpenAiResponsesBackend {
    client: Client,
    responses_url: Url,
    input_tokens_url: Url,
}

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
    ) -> Result<InferenceResponse, OpenAiResponsesBackendError> {
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
        let mut decoder = sse::Decoder::default();
        let mut stream = stream::ResponseStream::default();
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk?;
            let mut input = chunk.as_ref();
            while let Some(data) = decoder.next(&mut input)? {
                if let Some(text) = stream.push(&data)?
                    && let Some(observer) = observer
                {
                    timeout(TokioDuration::from_secs(120), observer.text_delta(&text))
                        .await
                        .map_err(|_| OpenAiResponsesBackendError::ObserverTimeout)?;
                }
            }
        }
        decoder.finish()?;
        stream.finish()
    }
}

#[async_trait]
impl InferenceBackend for OpenAiResponsesBackend {
    async fn respond(
        &self,
        request: InferenceRequest<'_>,
    ) -> Result<InferenceResponse, InferenceError> {
        let body = create_request(&request, true);
        self.stream(&body, request.observer)
            .await
            .map_err(Into::into)
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
    #[error("Responses output integrity error at index {index:?}: {detail}")]
    OutputIntegrity {
        index: Option<usize>,
        detail: &'static str,
    },
    #[error("Responses stream resource limit exceeded: {0}")]
    ResourceLimit(&'static str),
    #[error("stream observer did not accept output before the deadline")]
    ObserverTimeout,
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
