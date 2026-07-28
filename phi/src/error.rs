use serde::{Deserialize, Serialize};

use crate::executor::ToolCallRequest;
use crate::message::PhiMessage;
use crate::session::PhiModelRetryState;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PhiErrorKind {
    RequestCompact,
    ProviderRequest,
    ProviderResponse,
    Module,
    ToolExecution,
    ToolNotFound,
    ModelCandidateRejected,
    Session,
    Internal,
}

// PhiRuntimeError is the interpreter-internal, recoverable error type used by
// the agent step runtime. Session::step = Failed(...) persists this type
// directly so retry/intervene policies can reason about a finite, structured
// runtime error space.
//
// Design note:
// - keep runtime errors as an enum rather than "kind + optional payload bags"
// - only variants that truly need extra recovery context carry it
// - today that is mainly ToolNotFound, because recovery policies need the
//   original request plus pending tool-step payload
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhiRuntimeError {
    RequestCompact {
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
    ProviderRequest {
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<PhiModelRetryState>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
    ProviderResponse {
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<PhiModelRetryState>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
    Module {
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
    ToolExecution {
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
    ToolNotFound {
        detail: String,
        tool_request: ToolCallRequest,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_messages: Vec<PhiMessage>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remaining_tool_requests: Vec<ToolCallRequest>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
    ModelCandidateRejected {
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
    Session {
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
    Internal {
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_step: Option<String>,
    },
}

pub type PhiRuntimeResult<T> = Result<T, PhiRuntimeError>;

pub(crate) type PhiResult<T> = PhiRuntimeResult<T>;

/// PhiError is the outer, user-facing error carrier.
///
/// It intentionally does not mirror runtime error structure: runtime code
/// should stay on `PhiRuntimeError`, and only API/CLI boundaries should
/// convert into this lossy wrapper.
#[non_exhaustive]
pub struct PhiError {
    detail: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl PhiRuntimeError {
    pub fn request_compact(detail: impl Into<String>) -> Self {
        Self::RequestCompact {
            detail: detail.into(),
            source_step: None,
        }
    }

    pub fn provider_request(detail: impl Into<String>) -> Self {
        Self::ProviderRequest {
            detail: detail.into(),
            retry: None,
            source_step: None,
        }
    }

    pub fn provider_response(detail: impl Into<String>) -> Self {
        Self::ProviderResponse {
            detail: detail.into(),
            retry: None,
            source_step: None,
        }
    }

    pub fn module(detail: impl Into<String>) -> Self {
        Self::Module {
            detail: detail.into(),
            source_step: None,
        }
    }

    pub fn tool_execution(detail: impl Into<String>) -> Self {
        Self::ToolExecution {
            detail: detail.into(),
            source_step: None,
        }
    }

    pub fn tool_not_found(detail: impl Into<String>, tool_request: ToolCallRequest) -> Self {
        Self::ToolNotFound {
            detail: detail.into(),
            tool_request,
            pending_messages: Vec::new(),
            remaining_tool_requests: Vec::new(),
            source_step: None,
        }
    }

    pub fn model_candidate_rejected(detail: impl Into<String>) -> Self {
        Self::ModelCandidateRejected {
            detail: detail.into(),
            source_step: None,
        }
    }

    pub fn session(detail: impl Into<String>) -> Self {
        Self::Session {
            detail: detail.into(),
            source_step: None,
        }
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::Internal {
            detail: detail.into(),
            source_step: None,
        }
    }

    pub fn kind(&self) -> PhiErrorKind {
        match self {
            Self::RequestCompact { .. } => PhiErrorKind::RequestCompact,
            Self::ProviderRequest { .. } => PhiErrorKind::ProviderRequest,
            Self::ProviderResponse { .. } => PhiErrorKind::ProviderResponse,
            Self::Module { .. } => PhiErrorKind::Module,
            Self::ToolExecution { .. } => PhiErrorKind::ToolExecution,
            Self::ToolNotFound { .. } => PhiErrorKind::ToolNotFound,
            Self::ModelCandidateRejected { .. } => PhiErrorKind::ModelCandidateRejected,
            Self::Session { .. } => PhiErrorKind::Session,
            Self::Internal { .. } => PhiErrorKind::Internal,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::RequestCompact { detail, .. }
            | Self::ProviderRequest { detail, .. }
            | Self::ProviderResponse { detail, .. }
            | Self::Module { detail, .. }
            | Self::ToolExecution { detail, .. }
            | Self::ToolNotFound { detail, .. }
            | Self::ModelCandidateRejected { detail, .. }
            | Self::Session { detail, .. }
            | Self::Internal { detail, .. } => detail,
        }
    }

    pub fn source_step(&self) -> Option<&str> {
        match self {
            Self::RequestCompact { source_step, .. }
            | Self::ProviderRequest { source_step, .. }
            | Self::ProviderResponse { source_step, .. }
            | Self::Module { source_step, .. }
            | Self::ToolExecution { source_step, .. }
            | Self::ToolNotFound { source_step, .. }
            | Self::ModelCandidateRejected { source_step, .. }
            | Self::Session { source_step, .. }
            | Self::Internal { source_step, .. } => source_step.as_deref(),
        }
    }

    pub fn with_source_step(self, source_step: impl Into<String>) -> Self {
        let source_step = Some(source_step.into());
        match self {
            Self::RequestCompact { detail, .. } => Self::RequestCompact {
                detail,
                source_step,
            },
            Self::ProviderRequest { detail, retry, .. } => Self::ProviderRequest {
                detail,
                retry,
                source_step,
            },
            Self::ProviderResponse { detail, retry, .. } => Self::ProviderResponse {
                detail,
                retry,
                source_step,
            },
            Self::Module { detail, .. } => Self::Module {
                detail,
                source_step,
            },
            Self::ToolExecution { detail, .. } => Self::ToolExecution {
                detail,
                source_step,
            },
            Self::ToolNotFound {
                detail,
                tool_request,
                pending_messages,
                remaining_tool_requests,
                ..
            } => Self::ToolNotFound {
                detail,
                tool_request,
                pending_messages,
                remaining_tool_requests,
                source_step,
            },
            Self::ModelCandidateRejected { detail, .. } => Self::ModelCandidateRejected {
                detail,
                source_step,
            },
            Self::Session { detail, .. } => Self::Session {
                detail,
                source_step,
            },
            Self::Internal { detail, .. } => Self::Internal {
                detail,
                source_step,
            },
        }
    }

    pub fn with_retry(self, retry: PhiModelRetryState) -> Self {
        match self {
            Self::ProviderRequest {
                detail,
                source_step,
                ..
            } => Self::ProviderRequest {
                detail,
                retry: Some(retry),
                source_step,
            },
            Self::ProviderResponse {
                detail,
                source_step,
                ..
            } => Self::ProviderResponse {
                detail,
                retry: Some(retry),
                source_step,
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

    pub fn with_pending_messages(self, pending_messages: Vec<PhiMessage>) -> Self {
        match self {
            Self::ToolNotFound {
                detail,
                tool_request,
                remaining_tool_requests,
                source_step,
                ..
            } => Self::ToolNotFound {
                detail,
                tool_request,
                pending_messages,
                remaining_tool_requests,
                source_step,
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
                source_step,
                ..
            } => Self::ToolNotFound {
                detail,
                tool_request,
                pending_messages,
                remaining_tool_requests,
                source_step,
            },
            other => other,
        }
    }

    pub fn tool_request(&self) -> Option<&ToolCallRequest> {
        match self {
            Self::ToolNotFound { tool_request, .. } => Some(tool_request),
            _ => None,
        }
    }

    pub fn pending_messages(&self) -> Option<&[PhiMessage]> {
        match self {
            Self::ToolNotFound {
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
            } => Some(remaining_tool_requests),
            _ => None,
        }
    }

    pub fn from_boxed(
        error: Box<dyn std::error::Error>,
        fallback_kind: PhiErrorKind,
        source_step: impl Into<String>,
    ) -> Self {
        let source_step = source_step.into();
        match error.downcast::<Self>() {
            Ok(runtime_error) => {
                if runtime_error.source_step().is_some() {
                    *runtime_error
                } else {
                    runtime_error.with_source_step(source_step)
                }
            }
            Err(error) => {
                let detail = error.to_string();
                match fallback_kind {
                    PhiErrorKind::RequestCompact => {
                        Self::request_compact(detail).with_source_step(source_step)
                    }
                    PhiErrorKind::ProviderRequest => {
                        Self::provider_request(detail).with_source_step(source_step)
                    }
                    PhiErrorKind::ProviderResponse => {
                        Self::provider_response(detail).with_source_step(source_step)
                    }
                    PhiErrorKind::Module => Self::module(detail).with_source_step(source_step),
                    PhiErrorKind::ToolExecution => {
                        Self::tool_execution(detail).with_source_step(source_step)
                    }
                    PhiErrorKind::ToolNotFound => {
                        Self::internal(detail).with_source_step(source_step)
                    }
                    PhiErrorKind::ModelCandidateRejected => {
                        Self::model_candidate_rejected(detail).with_source_step(source_step)
                    }
                    PhiErrorKind::Session => Self::session(detail).with_source_step(source_step),
                    PhiErrorKind::Internal => Self::internal(detail).with_source_step(source_step),
                }
            }
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

    pub fn from_runtime_error(error: PhiRuntimeError) -> Self {
        Self {
            detail: error.detail().to_string(),
            source: None,
        }
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for PhiRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail())
    }
}

impl std::error::Error for PhiRuntimeError {}

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
