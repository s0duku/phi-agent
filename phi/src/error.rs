use serde::{Deserialize, Serialize};

use crate::executor::ToolCallRequest;
use crate::message::PhiMessage;
use crate::session::PhiModelRetryState;

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
    CompactExceededLimit {
        detail: String,
        retain_rate: f32,
    },
    ProviderRequest {
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<PhiModelRetryState>,
    },
    ProviderResponse {
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<PhiModelRetryState>,
    },
    Module {
        detail: String,
    },
    Home {
        detail: String,
    },
    ToolError {
        detail: serde_json::Value,
        tool_request: ToolCallRequest,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_messages: Vec<PhiMessage>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remaining_tool_requests: Vec<ToolCallRequest>,
    },
    ToolNotFound {
        detail: String,
        tool_request: ToolCallRequest,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_messages: Vec<PhiMessage>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remaining_tool_requests: Vec<ToolCallRequest>,
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

    pub fn compact_exceeded_limit(detail: impl Into<String>, retain_rate: f32) -> Self {
        Self::CompactExceededLimit {
            detail: detail.into(),
            retain_rate,
        }
    }

    pub fn provider_request(detail: impl Into<String>) -> Self {
        Self::ProviderRequest {
            detail: detail.into(),
            retry: None,
        }
    }

    pub fn provider_response(detail: impl Into<String>) -> Self {
        Self::ProviderResponse {
            detail: detail.into(),
            retry: None,
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

    pub fn tool_error(detail: serde_json::Value, tool_request: ToolCallRequest) -> Self {
        Self::ToolError {
            detail,
            tool_request,
            pending_messages: Vec::new(),
            remaining_tool_requests: Vec::new(),
        }
    }

    pub fn tool_not_found(detail: impl Into<String>, tool_request: ToolCallRequest) -> Self {
        Self::ToolNotFound {
            detail: detail.into(),
            tool_request,
            pending_messages: Vec::new(),
            remaining_tool_requests: Vec::new(),
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
            | Self::CompactExceededLimit { detail, .. }
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

    pub fn with_retry(self, retry: PhiModelRetryState) -> Self {
        match self {
            Self::ProviderRequest { detail, .. } => Self::ProviderRequest {
                detail,
                retry: Some(retry),
            },
            Self::ProviderResponse { detail, .. } => Self::ProviderResponse {
                detail,
                retry: Some(retry),
            },
            other => other,
        }
    }

    pub fn retry(&self) -> Option<&PhiModelRetryState> {
        match self {
            Self::ProviderRequest { retry, .. } | Self::ProviderResponse { retry, .. } => {
                retry.as_ref()
            }
            _ => None,
        }
    }

    pub fn compact_retain_rate(&self) -> Option<f32> {
        match self {
            Self::CompactExceededLimit { retain_rate, .. } => Some(*retain_rate),
            _ => None,
        }
    }

    pub fn with_pending_messages(self, pending_messages: Vec<PhiMessage>) -> Self {
        match self {
            Self::ToolNotFound {
                detail,
                tool_request,
                remaining_tool_requests,
                ..
            } => Self::ToolNotFound {
                detail,
                tool_request,
                pending_messages,
                remaining_tool_requests,
            },
            Self::ToolError {
                detail,
                tool_request,
                remaining_tool_requests,
                ..
            } => Self::ToolError {
                detail,
                tool_request,
                pending_messages,
                remaining_tool_requests,
            },
            other => other,
        }
    }

    pub fn with_remaining_tool_requests(
        self,
        remaining_tool_requests: Vec<ToolCallRequest>,
    ) -> Self {
        match self {
            Self::ToolNotFound {
                detail,
                tool_request,
                pending_messages,
                ..
            } => Self::ToolNotFound {
                detail,
                tool_request,
                pending_messages,
                remaining_tool_requests,
            },
            Self::ToolError {
                detail,
                tool_request,
                pending_messages,
                ..
            } => Self::ToolError {
                detail,
                tool_request,
                pending_messages,
                remaining_tool_requests,
            },
            other => other,
        }
    }

    pub fn tool_request(&self) -> Option<&ToolCallRequest> {
        match self {
            Self::ToolNotFound { tool_request, .. } | Self::ToolError { tool_request, .. } => {
                Some(tool_request)
            }
            _ => None,
        }
    }

    pub fn pending_messages(&self) -> Option<&[PhiMessage]> {
        match self {
            Self::ToolNotFound {
                pending_messages, ..
            }
            | Self::ToolError {
                pending_messages, ..
            } => Some(pending_messages),
            _ => None,
        }
    }

    pub fn remaining_tool_requests(&self) -> Option<&[ToolCallRequest]> {
        match self {
            Self::ToolNotFound {
                remaining_tool_requests,
                ..
            }
            | Self::ToolError {
                remaining_tool_requests,
                ..
            } => Some(remaining_tool_requests),
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
