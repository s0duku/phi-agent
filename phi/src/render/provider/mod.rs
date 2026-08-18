pub(crate) mod fake;
pub(crate) mod openai_chat;
pub(crate) mod openai_response;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    config::{ModelRequestDefaults, ProviderConfig, ReasoningEffort},
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    executor::PhiToolDefinition,
    message::PhiAssistantMessage,
};

use self::{fake::FakeClient, openai_chat::OpenAiCompatClient, openai_response::ResponsesClient};
use super::PhiRenderedMessages;

pub(super) fn tool_result_text(result: &serde_json::Value) -> String {
    match result {
        serde_json::Value::String(text) => text.clone(),
        result => result.to_string(),
    }
}

pub(super) fn http_error(
    prefix: &str,
    status: reqwest::StatusCode,
    body: String,
) -> PhiAgentRuntimeError {
    let context_exceeded = is_context_limit_error(Some(status), &body);
    let detail = format!("{prefix} HTTP {status}: {body}");
    if context_exceeded {
        PhiAgentRuntimeError::context_exceeded_limit(detail)
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
    pub(crate) assistant: Option<PhiAssistantMessage>,
    pub(crate) turn_state: PhiModelTurnState,
}

impl PhiModelResponse {
    #[cfg(test)]
    pub(crate) fn unspecified(messages: Vec<crate::message::PhiMessage>) -> Self {
        Self::new(messages, PhiModelTurnState::Unspecified)
    }

    pub(crate) fn from_assistant(
        assistant: Option<PhiAssistantMessage>,
        turn_state: PhiModelTurnState,
    ) -> Self {
        Self {
            assistant,
            turn_state,
        }
    }

    #[cfg(test)]
    pub(crate) fn new(
        messages: Vec<crate::message::PhiMessage>,
        turn_state: PhiModelTurnState,
    ) -> Self {
        let mut messages = messages.into_iter();
        let assistant = messages.next().map(|message| match message {
            crate::message::PhiMessage::Assistant(assistant) => assistant,
            _ => panic!("model response must contain only an assistant message"),
        });
        assert!(
            messages.next().is_none(),
            "model response must contain at most one assistant message"
        );
        Self::from_assistant(assistant, turn_state)
    }

    pub(crate) fn assistant(assistant: PhiAssistantMessage, turn_state: PhiModelTurnState) -> Self {
        Self::from_assistant(Some(assistant), turn_state)
    }
}

impl From<PhiAssistantMessage> for PhiModelResponse {
    fn from(assistant: PhiAssistantMessage) -> Self {
        Self::assistant(assistant, PhiModelTurnState::Unspecified)
    }
}

impl From<Option<PhiAssistantMessage>> for PhiModelResponse {
    fn from(assistant: Option<PhiAssistantMessage>) -> Self {
        Self::from_assistant(assistant, PhiModelTurnState::Unspecified)
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
            reasoning_effort: defaults.reasoning_effort,
        }
    }
}

#[async_trait]
pub(in crate::render) trait DynProvider: Send + Sync {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse>;
}

pub(in crate::render) fn build_provider(
    config: ProviderConfig,
) -> PhiAgentRuntimeResult<Box<dyn DynProvider>> {
    if config.kind.trim() != "fake" && config.api_key.trim().is_empty() {
        eprintln!(
            "phi provider: PHI_KEY is not configured; model requests will likely fail until an API key is set"
        );
    }

    match config.kind.trim() {
        "fake" => Ok(Box::new(FakeClient::new(config)?)),
        "openai_chat" => Ok(Box::new(OpenAiCompatClient::new(config))),
        "openai_response" => Ok(Box::new(ResponsesClient::new(config))),
        other => Err(PhiAgentRuntimeError::provider_request(format!(
            "unsupported provider: {other}"
        ))),
    }
}
