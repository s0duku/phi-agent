pub mod command;
mod serialization;

use crate::{
    config::{ModelRequestDefaults, PhiConfig},
    error::PhiAgentRuntimeError,
    executor::ToolCallRequest,
    expr::{PhiExprDelta, PhiStepExpr},
    message::{PhiHistory, PhiMessage},
    render::PhiProviderCall,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PhiModelRetryState {
    pub attempt: usize,
}

pub(crate) fn serde_default_request_provider_step() -> PhiAgentStep {
    PhiAgentStep::request_provider(
        "ready",
        &ModelRequestDefaults::from_config(&PhiConfig::default())
            .expect("empty settings should always produce fallback model defaults"),
    )
}

#[derive(Clone, Debug, PartialEq)]
pub enum PhiAgentStep {
    ReAct(PhiReActStep),
    Failed(PhiFailedStep),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhiFailedStep {
    error: PhiAgentRuntimeError,
}

impl PhiFailedStep {
    pub fn error(&self) -> &PhiAgentRuntimeError {
        &self.error
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhiReActStep {
    RequestCompact,
    RequestProvider {
        detail: String,
        #[serde(flatten)]
        call: PhiProviderCall,
    },
    RequestExecutor {
        detail: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_messages: Vec<PhiMessage>,
        tool_calls: Vec<ToolCallRequest>,
    },
    Compacted,
    TurnEnd {
        detail: String,
    },
}

impl PhiAgentStep {
    pub fn request_provider(detail: impl Into<String>, defaults: &ModelRequestDefaults) -> Self {
        Self::ReAct(PhiReActStep::request_provider(detail, defaults))
    }

    pub fn request_provider_with_call(detail: impl Into<String>, call: PhiProviderCall) -> Self {
        Self::ReAct(PhiReActStep::request_provider_with_call(detail, call))
    }

    pub fn request_compact() -> Self {
        Self::ReAct(PhiReActStep::RequestCompact)
    }

    pub fn request_executor(
        detail: impl Into<String>,
        pending_messages: Vec<PhiMessage>,
        tool_calls: Vec<ToolCallRequest>,
    ) -> Self {
        Self::ReAct(PhiReActStep::request_executor(
            detail,
            pending_messages,
            tool_calls,
        ))
    }

    pub fn turn_end(detail: impl Into<String>) -> Self {
        Self::ReAct(PhiReActStep::turn_end(detail))
    }

    pub(crate) fn runtime_failed(failure: crate::agent::RuntimeFailureStep) -> Self {
        Self::Failed(PhiFailedStep {
            error: failure.into_error(),
        })
    }

    #[cfg(test)]
    pub(crate) fn failed(error: PhiAgentRuntimeError) -> Self {
        Self::Failed(PhiFailedStep { error })
    }

    pub fn react(&self) -> Option<&PhiReActStep> {
        let Self::ReAct(step) = self else {
            return None;
        };
        Some(step)
    }

    pub fn is_react(&self, predicate: impl FnOnce(&PhiReActStep) -> bool) -> bool {
        self.react().is_some_and(predicate)
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::ReAct(step) => step.detail(),
            Self::Failed(failed) => failed.error.detail(),
        }
    }

    pub fn error(&self) -> Option<&PhiAgentRuntimeError> {
        let Self::Failed(failed) = self else {
            return None;
        };
        Some(&failed.error)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::ReAct(PhiReActStep::TurnEnd { .. }) | Self::Failed(_)
        )
    }

    pub fn request_provider_call(&self) -> Option<&PhiProviderCall> {
        self.react().and_then(PhiReActStep::request_provider_call)
    }
}

impl PhiReActStep {
    pub fn request_provider(detail: impl Into<String>, defaults: &ModelRequestDefaults) -> Self {
        Self::RequestProvider {
            detail: detail.into(),
            call: PhiProviderCall::from_parts(defaults, Vec::new()),
        }
    }

    pub fn request_provider_with_call(detail: impl Into<String>, call: PhiProviderCall) -> Self {
        Self::RequestProvider {
            detail: detail.into(),
            call,
        }
    }

    pub fn request_compact() -> Self {
        Self::RequestCompact
    }

    pub fn request_executor(
        detail: impl Into<String>,
        pending_messages: Vec<PhiMessage>,
        tool_calls: Vec<ToolCallRequest>,
    ) -> Self {
        Self::RequestExecutor {
            detail: detail.into(),
            pending_messages,
            tool_calls,
        }
    }

    pub fn turn_end(detail: impl Into<String>) -> Self {
        Self::TurnEnd {
            detail: detail.into(),
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::RequestCompact => "request compact",
            Self::Compacted => "after compacted",
            Self::RequestProvider { detail, .. }
            | Self::RequestExecutor { detail, .. }
            | Self::TurnEnd { detail } => detail,
        }
    }

    pub fn request_provider_call(&self) -> Option<&PhiProviderCall> {
        let Self::RequestProvider { call, .. } = self else {
            return None;
        };
        Some(call)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PhiAgentStepWire {
    RequestCompact,
    RequestProvider {
        detail: String,
        #[serde(flatten)]
        call: PhiProviderCall,
    },
    RequestExecutor {
        detail: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_messages: Vec<PhiMessage>,
        tool_calls: Vec<ToolCallRequest>,
    },
    Compacted,
    TurnEnd {
        detail: String,
    },
    Failed {
        error: PhiAgentRuntimeError,
    },
}

impl From<PhiAgentStepWire> for PhiAgentStep {
    fn from(step: PhiAgentStepWire) -> Self {
        match step {
            PhiAgentStepWire::RequestCompact => Self::ReAct(PhiReActStep::RequestCompact),
            PhiAgentStepWire::RequestProvider { detail, call } => {
                Self::ReAct(PhiReActStep::RequestProvider { detail, call })
            }
            PhiAgentStepWire::RequestExecutor {
                detail,
                pending_messages,
                tool_calls,
            } => Self::ReAct(PhiReActStep::RequestExecutor {
                detail,
                pending_messages,
                tool_calls,
            }),
            PhiAgentStepWire::Compacted => Self::ReAct(PhiReActStep::Compacted),
            PhiAgentStepWire::TurnEnd { detail } => Self::ReAct(PhiReActStep::TurnEnd { detail }),
            PhiAgentStepWire::Failed { error } => Self::Failed(PhiFailedStep { error }),
        }
    }
}

impl From<&PhiAgentStep> for PhiAgentStepWire {
    fn from(step: &PhiAgentStep) -> Self {
        match step {
            PhiAgentStep::ReAct(PhiReActStep::RequestCompact) => Self::RequestCompact,
            PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, call }) => {
                Self::RequestProvider {
                    detail: detail.clone(),
                    call: call.clone(),
                }
            }
            PhiAgentStep::ReAct(PhiReActStep::RequestExecutor {
                detail,
                pending_messages,
                tool_calls,
            }) => Self::RequestExecutor {
                detail: detail.clone(),
                pending_messages: pending_messages.clone(),
                tool_calls: tool_calls.clone(),
            },
            PhiAgentStep::ReAct(PhiReActStep::Compacted) => Self::Compacted,
            PhiAgentStep::ReAct(PhiReActStep::TurnEnd { detail }) => Self::TurnEnd {
                detail: detail.clone(),
            },
            PhiAgentStep::Failed(failed) => Self::Failed {
                error: failed.error.clone(),
            },
        }
    }
}

impl Serialize for PhiAgentStep {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        PhiAgentStepWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PhiAgentStep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        PhiAgentStepWire::deserialize(deserializer).map(Into::into)
    }
}

#[derive(Clone, Debug)]
/// Serialized agent state and the external ownership boundary around a step expression.
///
/// Session-level transformations consume a `Session` and return a new one. The evaluator
/// consumes the contained expression when it builds a runtime; runtime code operates on
/// `PhiStepExpr` directly and only an agent snapshot/output wraps that expression back into
/// a `Session`.
pub struct Session(PhiStepExpr);

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SessionFrame {
    step: PhiAgentStep,
    #[serde(default, skip_serializing_if = "PhiExprDelta::is_empty")]
    delta: PhiExprDelta,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct FlatSession {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    frames: Vec<SessionFrame>,
}

impl Session {
    pub fn empty() -> Self {
        Self(PhiStepExpr::empty_root())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_root<H>(step: PhiAgentStep, delta: H) -> Self
    where
        H: Into<PhiExprDelta>,
    {
        Self(PhiStepExpr::new(step, delta))
    }

    pub(crate) fn from_expr(expr: PhiStepExpr) -> Self {
        Self(expr)
    }

    pub(crate) fn into_expr(self) -> PhiStepExpr {
        self.0
    }

    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, Box<dyn std::error::Error>> {
        serialization::load(path)
    }

    pub fn load_bytes(input: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        serialization::load_bytes(input)
    }

    pub fn step(&self) -> &PhiAgentStep {
        self.0.step()
    }

    pub fn history(&self) -> PhiHistory {
        self.0.history()
    }

    pub fn into_history(self) -> PhiHistory {
        self.0.into_history()
    }

    /// Appends messages to the outermost frame without changing its step or parent expression.
    #[must_use]
    pub fn append_messages<I>(self, messages: I) -> Self
    where
        I: IntoIterator<Item = PhiMessage>,
    {
        let step = self.0.step().clone();
        let parent = self.0.expr().cloned();
        let mut delta = self.0.delta().clone();
        for message in messages {
            delta.push_message(message);
        }

        Self(match parent {
            Some(parent) => PhiStepExpr::branch(parent, step, delta),
            None => PhiStepExpr::new(step, delta),
        })
    }

    /// Adds a new outer frame with an empty delta.
    #[must_use]
    pub fn next(self, step: PhiReActStep) -> Self {
        Self(
            self.0
                .create_next_step(PhiAgentStep::ReAct(step), PhiExprDelta::default()),
        )
    }

    /// Replaces the outermost step while preserving its parent and delta.
    #[must_use]
    pub fn replace(self, step: PhiReActStep) -> Self {
        Self(
            self.0
                .replace_base_step(PhiAgentStep::ReAct(step), PhiExprDelta::default()),
        )
    }

    /// Resolves the first call in the outer RequestExecutor step without invoking an executor.
    #[must_use]
    pub fn insert_tool_result(
        self,
        result: serde_json::Value,
        resume_call: PhiProviderCall,
    ) -> Result<Self, PhiAgentRuntimeError> {
        let PhiAgentStep::ReAct(PhiReActStep::RequestExecutor {
            pending_messages,
            tool_calls,
            ..
        }) = self.step().clone()
        else {
            return Err(PhiAgentRuntimeError::session(
                "tool-result requires the current step to be request_executor",
            ));
        };
        let mut tool_calls = tool_calls.into_iter();
        let Some(request) = tool_calls.next() else {
            return Err(PhiAgentRuntimeError::session(
                "request_executor step has no pending tool call",
            ));
        };
        let result_id = request.call_id.clone().or(Some(request.id.clone()));
        let result_name = request.name.clone();
        let (messages, next_step) = resolve_tool_result(
            pending_messages,
            request,
            tool_calls.collect(),
            result_id,
            result_name,
            result,
            resume_call,
        );
        Ok(Self(self.0.create_next_step(
            PhiAgentStep::ReAct(next_step),
            messages.into(),
        )))
    }

    /// Removes the outermost frame while preserving a root session unchanged.
    #[must_use]
    pub fn rollback(self) -> Self {
        if self.0.expr().is_none() {
            return self;
        }

        let parent = self
            .0
            .into_expr()
            .expect("a non-root expression must retain its parent");
        Self(Arc::try_unwrap(parent).unwrap_or_else(|shared| (*shared).clone()))
    }

    pub(crate) fn validate(&self) -> Result<(), PhiAgentRuntimeError> {
        fn validate_expr(expr: &PhiStepExpr) -> Result<(), PhiAgentRuntimeError> {
            match expr.step() {
                PhiAgentStep::ReAct(PhiReActStep::Compacted) if expr.expr().is_none() => {
                    return Err(PhiAgentRuntimeError::session(
                        "compacted frame must preserve a parent expr",
                    ));
                }
                _ => {}
            }

            if let Some(parent) = expr.expr() {
                validate_expr(parent)?;
            }

            Ok(())
        }

        validate_expr(&self.0)
    }

    fn from_frames(frames: Vec<SessionFrame>) -> Self {
        let mut frames = frames.into_iter();
        let Some(root) = frames.next() else {
            return Self::empty();
        };
        let expr = frames.fold(PhiStepExpr::new(root.step, root.delta), |expr, frame| {
            PhiStepExpr::branch(expr, frame.step, frame.delta)
        });
        Self::from_expr(expr)
    }

    fn frames(&self) -> Vec<SessionFrame> {
        fn collect(expr: &PhiStepExpr, frames: &mut Vec<SessionFrame>) {
            if let Some(parent) = expr.expr() {
                collect(parent, frames);
            }
            frames.push(SessionFrame {
                step: expr.step().clone(),
                delta: expr.delta().clone(),
            });
        }

        let mut frames = Vec::new();
        collect(&self.0, &mut frames);
        frames
    }

    pub fn write_json<W>(&self, writer: &mut W) -> Result<(), Box<dyn std::error::Error>>
    where
        W: std::io::Write,
    {
        serialization::write_json(self, writer)
    }

    pub fn write_stdout(&self) -> Result<(), Box<dyn std::error::Error>> {
        serialization::write_stdout(self)
    }

    pub fn save(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create session directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let mut file = std::fs::File::create(path).map_err(|error| {
            format!("failed to create session file {}: {error}", path.display())
        })?;
        self.write_json(&mut file).map_err(|error| {
            format!("failed to write session file {}: {error}", path.display()).into()
        })
    }

    pub fn create(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create session directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| {
                format!(
                    "failed to create new session file {}: {error}",
                    path.display()
                )
            })?;
        self.write_json(&mut file).map_err(|error| {
            let _ = std::fs::remove_file(path);
            format!(
                "failed to write new session file {}: {error}",
                path.display()
            )
            .into()
        })
    }
}

pub(crate) fn resolve_tool_result(
    mut pending_messages: Vec<PhiMessage>,
    request: ToolCallRequest,
    remaining_tool_calls: Vec<ToolCallRequest>,
    result_id: Option<String>,
    result_name: String,
    result: serde_json::Value,
    resume_call: PhiProviderCall,
) -> (Vec<PhiMessage>, PhiReActStep) {
    pending_messages.push(PhiMessage::tool_call(
        request.call_id.or(Some(request.id)),
        request.name,
        request.arguments,
    ));
    pending_messages.push(PhiMessage::tool_result(
        result_id,
        Some(result_name),
        result,
    ));
    let next_step = if remaining_tool_calls.is_empty() {
        PhiReActStep::request_provider_with_call(
            "tool result committed; model response is pending",
            resume_call,
        )
    } else {
        PhiReActStep::request_executor(
            "additional tool execution is pending",
            Vec::new(),
            remaining_tool_calls,
        )
    };
    (pending_messages, next_step)
}

impl Serialize for Session {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        FlatSession {
            frames: self.frames(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Session {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let session = Self::from_frames(FlatSession::deserialize(deserializer)?.frames);
        session
            .validate()
            .map_err(|error| de::Error::custom(error.detail()))
            .map(|_| session)
    }
}
