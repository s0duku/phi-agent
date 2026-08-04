pub(crate) mod fake;
pub(crate) mod openai_chat;
pub(crate) mod openai_response;

use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    config::{ModelRequestDefaults, ProviderConfig, ReasoningEffort},
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    executor::PhiToolDefinition,
    message::PhiMessage,
};

use self::{fake::FakeClient, openai_chat::OpenAiCompatClient, openai_response::ResponsesClient};
use super::PhiRenderedMessages;

pub(super) fn http_error(
    prefix: &str,
    status: reqwest::StatusCode,
    body: String,
) -> PhiAgentRuntimeError {
    let context_exceeded = is_context_limit_error(Some(status), &body);
    let detail = format!("{prefix} HTTP {status}: {body}");
    if context_exceeded {
        PhiAgentRuntimeError::compact_exceeded_limit(detail, 0.0)
    } else {
        PhiAgentRuntimeError::provider_request(detail)
    }
}

/// Recognizes provider context overflow without confusing it with rate limits,
/// quotas, output-token validation, or byte-sized payload failures.
/// Structured error codes win; text matching is only a conservative fallback
/// for OpenAI-compatible gateways that discard the original error fields.
pub(super) fn is_context_limit_error(status: Option<reqwest::StatusCode>, body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    let structured_context = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .is_some_and(|value| {
            [
                "/error/code",
                "/error/type",
                "/response/error/code",
                "/response/error/type",
                "/code",
                "/type",
            ]
            .into_iter()
            .filter_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
            .any(|code| {
                matches!(
                    code,
                    "context_length_exceeded" | "model_context_window_exceeded"
                )
            })
        });
    if structured_context {
        return true;
    }

    if [
        "rate limit",
        "rate_limit",
        "too many requests",
        "quota",
        "per minute",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return false;
    }

    if status.is_some_and(|status| {
        !matches!(
            status,
            reqwest::StatusCode::BAD_REQUEST
                | reqwest::StatusCode::PAYLOAD_TOO_LARGE
                | reqwest::StatusCode::UNPROCESSABLE_ENTITY
        )
    }) {
        return false;
    }

    [
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "token limit",
        "maximum tokens",
        "exceeds the available context",
        "exceeds the model's context",
        "exceeds model's maximum context",
        "range of input length should be",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod error_tests {
    use super::is_context_limit_error;

    #[test]
    fn recognizes_structured_and_compatible_context_overflow_errors() {
        for body in [
            r#"{"error":{"code":"context_length_exceeded"}}"#,
            r#"{"error":{"type":"model_context_window_exceeded"}}"#,
            r#"{"error":{"message":"Input length exceeds model's maximum context length of 131072 tokens."}}"#,
        ] {
            assert!(
                is_context_limit_error(Some(reqwest::StatusCode::BAD_REQUEST), body),
                "expected context overflow: {body}"
            );
        }
    }

    #[test]
    fn rejects_non_context_errors_and_ambiguous_payload_limits() {
        for (status, body) in [
            (
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                "too many tokens, please retry",
            ),
            (
                reqwest::StatusCode::BAD_REQUEST,
                r#"{"error":{"code":"max_tokens"}}"#,
            ),
            (
                reqwest::StatusCode::PAYLOAD_TOO_LARGE,
                "request body is too large",
            ),
            (
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                "context window unavailable",
            ),
        ] {
            assert!(
                !is_context_limit_error(Some(status), body),
                "false positive: {status} {body}"
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PhiModelTurnState {
    Complete,
    Continue,
    #[default]
    Unspecified,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PhiModelResponse {
    pub(crate) messages: Vec<PhiMessage>,
    pub(crate) turn_state: PhiModelTurnState,
}

impl PhiModelResponse {
    pub(crate) fn unspecified(messages: Vec<PhiMessage>) -> Self {
        Self {
            messages,
            turn_state: PhiModelTurnState::Unspecified,
        }
    }

    pub(crate) fn new(messages: Vec<PhiMessage>, turn_state: PhiModelTurnState) -> Self {
        Self {
            messages,
            turn_state,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PhiProviderCall {
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<PhiToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    pub max_tokens: u64,
    pub enable_reasoning: bool,
    pub thinking_token_budget: u64,
    pub reasoning_effort: ReasoningEffort,
}

impl PhiProviderCall {
    pub fn from_parts(defaults: &ModelRequestDefaults, tools: Vec<PhiToolDefinition>) -> Self {
        Self {
            model: defaults.model.clone(),
            tools,
            temperature: defaults.temperature,
            max_tokens: defaults.max_tokens,
            enable_reasoning: defaults.enable_reasoning,
            thinking_token_budget: defaults.thinking_token_budget,
            reasoning_effort: defaults.reasoning_effort,
        }
    }
}

#[async_trait]
pub(in crate::render) trait PhiProvider: Send + Sync {
    type ProviderMessage: Clone + Send + Sync + Serialize + DeserializeOwned;
    type ProviderTool: Clone + Send + Sync + Serialize;

    // Provider conversion and requests run inside agent evaluation. Their
    // failures intentionally use PhiAgentRuntimeResult so retry/recovery policy
    // can persist and classify the failed provider step.
    fn provider_messages(
        &self,
        messages: &PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<Vec<Self::ProviderMessage>>;

    fn phi_messages(
        &self,
        response: Vec<Self::ProviderMessage>,
    ) -> PhiAgentRuntimeResult<Vec<PhiMessage>>;

    fn provider_tool(&self, tool: &PhiToolDefinition) -> Self::ProviderTool;

    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: &PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse>;
}

#[async_trait]
pub(in crate::render) trait DynProvider: Send + Sync {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse>;
}

#[async_trait]
impl<T> DynProvider for T
where
    T: PhiProvider,
{
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        PhiProvider::complete(self, request, &messages).await
    }
}

pub(in crate::render) fn build_provider(
    config: ProviderConfig,
) -> PhiAgentRuntimeResult<Box<dyn DynProvider>> {
    if config.provider.trim() != "fake" && config.api_key.trim().is_empty() {
        eprintln!(
            "phi provider: PHI_KEY is not configured; model requests will likely fail until an API key is set"
        );
    }

    match config.provider.trim() {
        "fake" => Ok(Box::new(FakeClient::new(config)?)),
        "openai_chat" => Ok(Box::new(OpenAiCompatClient::new(config))),
        "openai_response" => Ok(Box::new(ResponsesClient::new(config))),
        other => Err(PhiAgentRuntimeError::provider_request(format!(
            "unsupported provider: {other}"
        ))),
    }
}
