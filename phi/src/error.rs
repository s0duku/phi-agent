use serde::{Deserialize, Serialize};

use crate::executor::ToolCallRequest;
use crate::message::{PhiAssistantMessage, PhiMessage, PhiToolResultMessage};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PhiFailedToolTurn {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_messages: Vec<PhiMessage>,
    pub assistant: PhiAssistantMessage,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_results: Vec<PhiToolResultMessage>,
}

impl PhiFailedToolTurn {
    pub(crate) fn new(
        pending_messages: Vec<PhiMessage>,
        assistant: PhiAssistantMessage,
        pending_results: Vec<PhiToolResultMessage>,
    ) -> Result<Self, PhiAgentRuntimeError> {
        let turn = Self {
            pending_messages,
            assistant,
            pending_results,
        };
        turn.validate()?;
        Ok(turn)
    }

    pub fn pending_messages(&self) -> &[PhiMessage] {
        &self.pending_messages
    }

    pub fn assistant(&self) -> &PhiAssistantMessage {
        &self.assistant
    }

    pub fn pending_results(&self) -> &[PhiToolResultMessage] {
        &self.pending_results
    }

    pub fn failed_request(&self) -> Option<&ToolCallRequest> {
        self.assistant.tool_calls.get(self.pending_results.len())
    }

    pub fn remaining_requests(&self) -> &[ToolCallRequest] {
        self.assistant
            .tool_calls
            .get(self.pending_results.len().saturating_add(1)..)
            .unwrap_or_default()
    }

    pub(crate) fn validate(&self) -> Result<(), PhiAgentRuntimeError> {
        if self.failed_request().is_none() {
            return Err(PhiAgentRuntimeError::session(format!(
                "failed tool turn has {} completed results for {} tool calls",
                self.pending_results.len(),
                self.assistant.tool_calls.len()
            )));
        }
        validate_completed_tool_results(&self.assistant, &self.pending_results, "failed tool turn")
    }
}

#[cfg(test)]
mod failed_tool_turn_tests {
    use super::*;

    #[test]
    fn rejects_a_turn_without_a_failed_request() {
        let error = PhiFailedToolTurn::new(
            Vec::new(),
            PhiAssistantMessage::tool_calls(Vec::new()),
            Vec::new(),
        )
        .expect_err("failed turn must identify its request");

        assert!(error.detail().contains("failed tool turn has"));
    }
}

/// Object-safe bridge for tool-owned errors crossing into agent evaluation.
///
/// Concrete tool errors retain their own Rust types until `PhiExecutor` must
/// persist a failed step. At that boundary they are converted to JSON because
/// a trait object cannot be cloned, compared, or deserialized as session data.
pub trait PhiStructureError: Send + Sync + 'static {
    fn into_value(self: Box<Self>) -> serde_json::Value;
}

/// A recoverable failure produced while evaluating `PhiAgentRuntime`.
///
/// This is the persisted error language of `PhiAgentStep::Failed`, not a
/// general-purpose error for every Phi module. Home, storage, CLI, and build
/// layers should define their own errors and convert them only when their
/// operation participates in an agent step. Variants carry only the context a
/// recovery policy needs to inspect or resume that failed evaluation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhiAgentRuntimeError {
    RequestCompact {
        detail: String,
    },
    ContextExceededLimit {
        detail: String,
    },
    ProviderRequest {
        detail: String,
    },
    ProviderResponse {
        detail: String,
    },
    Module {
        detail: String,
    },
    Home {
        detail: String,
    },
    ToolError {
        detail: serde_json::Value,
        turn: Box<PhiFailedToolTurn>,
    },
    ToolNotFound {
        detail: String,
        turn: Box<PhiFailedToolTurn>,
    },
    ModelCandidateRejected {
        detail: String,
    },
    Session {
        detail: String,
    },
}

/// Result channel for operations that participate in agent step evaluation and
/// may therefore become `PhiAgentStep::Failed`.
pub type PhiAgentRuntimeResult<T> = Result<T, PhiAgentRuntimeError>;

pub(crate) fn validate_completed_tool_results(
    assistant: &PhiAssistantMessage,
    pending_results: &[PhiToolResultMessage],
    context: &str,
) -> PhiAgentRuntimeResult<()> {
    for (index, (result, request)) in pending_results
        .iter()
        .zip(&assistant.tool_calls)
        .enumerate()
    {
        let expected_id = request.call_id.as_deref().unwrap_or(&request.id);
        if result.id.as_deref() != Some(expected_id) {
            return Err(PhiAgentRuntimeError::session(format!(
                "{context} result {index} does not match its tool call id"
            )));
        }
    }
    Ok(())
}

pub(crate) type PhiAgentResult<T> = PhiAgentRuntimeResult<T>;

impl PhiStructureError for crate::headlessterm::HeadlessTermError {
    fn into_value(self: Box<Self>) -> serde_json::Value {
        serde_json::to_value(*self).expect("HeadlessTermError must serialize")
    }
}

/// PhiError is the outer, user-facing error carrier.
///
/// It intentionally does not mirror runtime error structure: runtime code
/// should stay on `PhiAgentRuntimeError`, and only API/CLI boundaries should
/// convert into this lossy wrapper.
#[non_exhaustive]
pub struct PhiError {
    detail: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl PhiAgentRuntimeError {
    pub fn request_compact(detail: impl Into<String>) -> Self {
        Self::RequestCompact {
            detail: detail.into(),
        }
    }

    pub fn context_exceeded_limit(detail: impl Into<String>) -> Self {
        Self::ContextExceededLimit {
            detail: detail.into(),
        }
    }

    pub fn provider_request(detail: impl Into<String>) -> Self {
        Self::ProviderRequest {
            detail: detail.into(),
        }
    }

    pub fn provider_response(detail: impl Into<String>) -> Self {
        Self::ProviderResponse {
            detail: detail.into(),
        }
    }

    pub fn module(detail: impl Into<String>) -> Self {
        Self::Module {
            detail: detail.into(),
        }
    }

    pub fn home(detail: impl Into<String>) -> Self {
        Self::Home {
            detail: detail.into(),
        }
    }

    pub(crate) fn tool_error(detail: serde_json::Value, turn: PhiFailedToolTurn) -> Self {
        Self::ToolError {
            detail,
            turn: Box::new(turn),
        }
    }

    pub(crate) fn tool_not_found(detail: impl Into<String>, turn: PhiFailedToolTurn) -> Self {
        Self::ToolNotFound {
            detail: detail.into(),
            turn: Box::new(turn),
        }
    }

    pub fn model_candidate_rejected(detail: impl Into<String>) -> Self {
        Self::ModelCandidateRejected {
            detail: detail.into(),
        }
    }

    pub fn session(detail: impl Into<String>) -> Self {
        Self::Session {
            detail: detail.into(),
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::RequestCompact { detail, .. }
            | Self::ContextExceededLimit { detail }
            | Self::ProviderRequest { detail, .. }
            | Self::ProviderResponse { detail, .. }
            | Self::Module { detail, .. }
            | Self::Home { detail, .. }
            | Self::ToolNotFound { detail, .. }
            | Self::ModelCandidateRejected { detail, .. }
            | Self::Session { detail, .. } => detail,
            Self::ToolError { .. } => "tool execution failed",
        }
    }

    pub fn tool_error_detail(&self) -> Option<&serde_json::Value> {
        match self {
            Self::ToolError { detail, .. } => Some(detail),
            _ => None,
        }
    }

    pub fn assistant(&self) -> Option<&PhiAssistantMessage> {
        match self {
            Self::ToolNotFound { turn, .. } | Self::ToolError { turn, .. } => {
                Some(turn.assistant())
            }
            _ => None,
        }
    }

    pub fn pending_results(&self) -> Option<&[PhiToolResultMessage]> {
        match self {
            Self::ToolNotFound { turn, .. } | Self::ToolError { turn, .. } => {
                Some(turn.pending_results())
            }
            _ => None,
        }
    }

    pub fn tool_request(&self) -> Option<&ToolCallRequest> {
        match self {
            Self::ToolNotFound { turn, .. } | Self::ToolError { turn, .. } => turn.failed_request(),
            _ => None,
        }
    }

    pub fn pending_messages(&self) -> Option<&[PhiMessage]> {
        match self {
            Self::ToolNotFound { turn, .. } | Self::ToolError { turn, .. } => {
                Some(turn.pending_messages())
            }
            _ => None,
        }
    }

    pub fn remaining_tool_requests(&self) -> Option<&[ToolCallRequest]> {
        match self {
            Self::ToolNotFound { turn, .. } | Self::ToolError { turn, .. } => {
                Some(turn.remaining_requests())
            }
            _ => None,
        }
    }
}

impl PhiError {
    pub fn new<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            detail: error.to_string(),
            source: Some(Box::new(error)),
        }
    }

    pub fn message(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            source: None,
        }
    }

    pub fn from_runtime_error(error: PhiAgentRuntimeError) -> Self {
        Self {
            detail: error.detail().to_string(),
            source: None,
        }
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for PhiAgentRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail())
    }
}

impl std::error::Error for PhiAgentRuntimeError {}

impl std::fmt::Display for PhiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail())
    }
}

impl std::fmt::Debug for PhiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhiError")
            .field("detail", &self.detail)
            .finish_non_exhaustive()
    }
}

impl std::error::Error for PhiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}
