pub mod command;
mod serialization;
pub mod state;

use crate::{
    config::{ModelRequestDefaults, PhiConfig},
    error::PhiAgentRuntimeError,
    expr::{PhiExprDelta, PhiStepExpr},
    message::{PhiAssistantMessage, PhiHistory, PhiMessage, PhiToolResultMessage},
    render::PhiProviderCall,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{convert::Infallible, io::Read, path::PathBuf, str::FromStr, sync::Arc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStepKind {
    RequestCompact,
    RequestProvider,
    RequestExecutor,
    Compacted,
    TurnEnd,
    Failed,
}

impl FromStr for SessionStepKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "request_compact" => Ok(Self::RequestCompact),
            "request_provider" => Ok(Self::RequestProvider),
            "request_executor" => Ok(Self::RequestExecutor),
            "compacted" => Ok(Self::Compacted),
            "turn_end" => Ok(Self::TurnEnd),
            "failed" => Ok(Self::Failed),
            _ => Err(format!(
                "invalid step kind `{value}`; expected request_compact, request_provider, request_executor, compacted, turn_end, or failed"
            )),
        }
    }
}

impl SessionStepKind {
    fn matches(self, step: &PhiAgentStep) -> bool {
        match (self, step) {
            (Self::Failed, PhiAgentStep::Failed(_)) => true,
            (Self::RequestCompact, PhiAgentStep::ReAct(PhiReActStep::RequestCompact { .. }))
            | (Self::RequestProvider, PhiAgentStep::ReAct(PhiReActStep::RequestProvider { .. }))
            | (Self::RequestExecutor, PhiAgentStep::ReAct(PhiReActStep::RequestExecutor { .. }))
            | (Self::Compacted, PhiAgentStep::ReAct(PhiReActStep::Compacted))
            | (Self::TurnEnd, PhiAgentStep::ReAct(PhiReActStep::TurnEnd { .. })) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SessionTarget {
    Stdio,
    File(PathBuf),
}

impl FromStr for SessionTarget {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(if value == "-" {
            Self::Stdio
        } else {
            Self::File(PathBuf::from(value))
        })
    }
}

impl SessionTarget {
    pub(crate) fn load(&self) -> Result<Session, Box<dyn std::error::Error>> {
        match self {
            Self::File(path) => Session::load(path),
            Self::Stdio => {
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input)?;
                Session::load_bytes(&input)
            }
        }
    }

    pub(crate) fn persist(&self, session: &Session) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::File(path) => session.save(path),
            Self::Stdio => session.write_stdout(),
        }
    }

    pub(crate) fn checkpoint(&self, session: &Session) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::File(path) => session.save(path),
            Self::Stdio => Ok(()),
        }
    }

    pub(crate) fn create(self, session: &Session) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::File(path) => session.create(path),
            Self::Stdio => session.write_stdout(),
        }
    }

    pub(crate) fn file(&self) -> Option<&std::path::Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Stdio => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PhiModelRetryState {
    pub attempt: usize,
}

pub(crate) fn serde_default_request_provider_step() -> PhiAgentStep {
    PhiAgentStep::request_provider("ready", &ModelRequestDefaults::from(&PhiConfig::default()))
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
    RequestCompact {
        retain_rate: f32,
    },
    RequestProvider {
        detail: String,
        #[serde(flatten)]
        call: PhiProviderCall,
    },
    RequestExecutor {
        detail: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_messages: Vec<PhiMessage>,
        assistant: PhiAssistantMessage,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_results: Vec<PhiToolResultMessage>,
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
        Self::ReAct(PhiReActStep::request_compact())
    }

    pub fn request_executor(
        detail: impl Into<String>,
        pending_messages: Vec<PhiMessage>,
        assistant: PhiAssistantMessage,
    ) -> Self {
        Self::ReAct(PhiReActStep::request_executor(
            detail,
            pending_messages,
            assistant,
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
        Self::RequestCompact { retain_rate: 0.1 }
    }

    pub(crate) fn request_compact_with_retain_rate(retain_rate: f32) -> Self {
        Self::RequestCompact { retain_rate }
    }

    pub fn request_executor(
        detail: impl Into<String>,
        pending_messages: Vec<PhiMessage>,
        assistant: PhiAssistantMessage,
    ) -> Self {
        Self::RequestExecutor {
            detail: detail.into(),
            pending_messages,
            assistant,
            pending_results: Vec::new(),
        }
    }

    pub(crate) fn request_executor_turn(
        detail: impl Into<String>,
        pending_messages: Vec<PhiMessage>,
        assistant: PhiAssistantMessage,
        pending_results: Vec<PhiToolResultMessage>,
    ) -> Result<Self, PhiAgentRuntimeError> {
        let step = Self::RequestExecutor {
            detail: detail.into(),
            pending_messages,
            assistant,
            pending_results,
        };
        validate_react_step(&step)?;
        Ok(step)
    }

    pub fn turn_end(detail: impl Into<String>) -> Self {
        Self::TurnEnd {
            detail: detail.into(),
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::RequestCompact { .. } => "request compact",
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
    RequestCompact {
        retain_rate: f32,
    },
    RequestProvider {
        detail: String,
        #[serde(flatten)]
        call: PhiProviderCall,
    },
    RequestExecutor {
        detail: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_messages: Vec<PhiMessage>,
        assistant: PhiAssistantMessage,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_results: Vec<PhiToolResultMessage>,
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
            PhiAgentStepWire::RequestCompact { retain_rate } => {
                Self::ReAct(PhiReActStep::RequestCompact { retain_rate })
            }
            PhiAgentStepWire::RequestProvider { detail, call } => {
                Self::ReAct(PhiReActStep::RequestProvider { detail, call })
            }
            PhiAgentStepWire::RequestExecutor {
                detail,
                pending_messages,
                assistant,
                pending_results,
            } => Self::ReAct(PhiReActStep::RequestExecutor {
                detail,
                pending_messages,
                assistant,
                pending_results,
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
            PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate }) => {
                Self::RequestCompact {
                    retain_rate: *retain_rate,
                }
            }
            PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, call }) => {
                Self::RequestProvider {
                    detail: detail.clone(),
                    call: call.clone(),
                }
            }
            PhiAgentStep::ReAct(PhiReActStep::RequestExecutor {
                detail,
                pending_messages,
                assistant,
                pending_results,
            }) => Self::RequestExecutor {
                detail: detail.clone(),
                pending_messages: pending_messages.clone(),
                assistant: assistant.clone(),
                pending_results: pending_results.clone(),
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

    /// Stores a JSON value in the outermost frame's variable-effect delta.
    #[must_use]
    pub fn store_json(
        self,
        key: impl Into<String>,
        value: serde_json::Value,
    ) -> Result<Self, PhiAgentRuntimeError> {
        let key = key.into();
        if key.is_empty() {
            return Err(PhiAgentRuntimeError::session("store key must not be empty"));
        }
        let step = self.0.step().clone();
        let mut delta = self.0.delta().clone();
        delta.store_json(key, value);
        let session = Self(self.0.replace_base_step_with_delta(step, delta));
        session.validate()?;
        Ok(session)
    }

    /// Records removal of a key in the outermost frame's variable-effect delta.
    #[must_use]
    pub fn remove_key(self, key: impl Into<String>) -> Result<Self, PhiAgentRuntimeError> {
        let key = key.into();
        if key.is_empty() {
            return Err(PhiAgentRuntimeError::session(
                "remove key must not be empty",
            ));
        }
        let step = self.0.step().clone();
        let mut delta = self.0.delta().clone();
        delta.remove_key(key);
        let session = Self(self.0.replace_base_step_with_delta(step, delta));
        session.validate()?;
        Ok(session)
    }

    /// Adds a new outer frame with an empty delta.
    #[must_use]
    pub fn next(self, step: PhiReActStep) -> Result<Self, PhiAgentRuntimeError> {
        let session = Self(
            self.0
                .create_next_step(PhiAgentStep::ReAct(step), PhiExprDelta::default()),
        );
        session.validate()?;
        Ok(session)
    }

    /// Replaces the outermost step while preserving its parent and delta.
    #[must_use]
    pub fn replace(self, step: PhiReActStep) -> Result<Self, PhiAgentRuntimeError> {
        let session = Self(
            self.0
                .replace_base_step(PhiAgentStep::ReAct(step), PhiExprDelta::default()),
        );
        session.validate()?;
        Ok(session)
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
            assistant,
            mut pending_results,
            ..
        }) = self.step().clone()
        else {
            return Err(PhiAgentRuntimeError::session(
                "tool-result requires the current step to be request_executor",
            ));
        };
        let Some(request) = assistant.tool_calls.get(pending_results.len()).cloned() else {
            return Err(PhiAgentRuntimeError::session(
                "request_executor step has no pending tool call",
            ));
        };
        let result_id = request.call_id.clone().or(Some(request.id.clone()));
        let result_name = request.name.clone();
        pending_results.push(PhiToolResultMessage {
            id: result_id,
            name: Some(result_name),
            result,
        });
        Ok(
            match resolve_tool_result(pending_messages, assistant, pending_results, resume_call)? {
                ToolResultResolution::Pending(step) => Self(
                    self.0
                        .replace_base_step(PhiAgentStep::ReAct(step), PhiExprDelta::default()),
                ),
                ToolResultResolution::Complete { messages, step } => Self(
                    self.0
                        .create_next_step(PhiAgentStep::ReAct(step), messages.into()),
                ),
            },
        )
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

    /// Removes outer frames until the current frame has the requested kind.
    pub fn rollback_to(self, kind: SessionStepKind) -> Result<Self, PhiAgentRuntimeError> {
        let frames = self.frames();
        let Some(index) = frames.iter().rposition(|frame| kind.matches(&frame.step)) else {
            return Err(PhiAgentRuntimeError::session(format!(
                "rollback target step kind {kind:?} was not found"
            )));
        };
        Ok(Self::from_frames(
            frames.into_iter().take(index + 1).collect(),
        ))
    }

    pub(crate) fn validate(&self) -> Result<(), PhiAgentRuntimeError> {
        fn validate_expr(expr: &PhiStepExpr) -> Result<(), PhiAgentRuntimeError> {
            validate_agent_step(expr.step())?;
            match expr.step() {
                PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate })
                    if !(0.0..=0.5).contains(retain_rate) || *retain_rate == 0.0 =>
                {
                    return Err(PhiAgentRuntimeError::session(format!(
                        "request_compact retain_rate must be greater than 0 and at most 0.5, got {retain_rate}"
                    )));
                }
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
        self.validate()?;
        serialization::write_json(self, writer)
    }

    pub fn write_stdout(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.validate()?;
        serialization::write_stdout(self)
    }

    pub fn save(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.validate()?;
        let path = path.as_ref();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        if parent != std::path::Path::new(".") {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create session directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        serialization::save_atomic(self, path, parent)
    }

    pub fn create(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.validate()?;
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

pub(crate) enum ToolResultResolution {
    Pending(PhiReActStep),
    Complete {
        messages: Vec<PhiMessage>,
        step: PhiReActStep,
    },
}

pub(crate) fn resolve_tool_result(
    mut pending_messages: Vec<PhiMessage>,
    assistant: PhiAssistantMessage,
    pending_results: Vec<PhiToolResultMessage>,
    resume_call: PhiProviderCall,
) -> Result<ToolResultResolution, PhiAgentRuntimeError> {
    if pending_results.len() >= assistant.tool_calls.len() {
        pending_messages.push(PhiMessage::Assistant(assistant));
        pending_messages.extend(pending_results.into_iter().map(PhiMessage::ToolResult));
        return Ok(ToolResultResolution::Complete {
            messages: pending_messages,
            step: PhiReActStep::request_provider_with_call(
                "tool result committed; model response is pending",
                resume_call,
            ),
        });
    }
    Ok(ToolResultResolution::Pending(
        PhiReActStep::request_executor_turn(
            "additional tool execution is pending",
            pending_messages,
            assistant,
            pending_results,
        )?,
    ))
}

pub(crate) fn validate_react_step(step: &PhiReActStep) -> Result<(), PhiAgentRuntimeError> {
    if let PhiReActStep::RequestCompact { retain_rate } = step
        && (!(0.0..=0.5).contains(retain_rate) || *retain_rate == 0.0)
    {
        return Err(PhiAgentRuntimeError::session(format!(
            "request_compact retain_rate must be greater than 0 and at most 0.5, got {retain_rate}"
        )));
    }
    if let PhiReActStep::RequestExecutor {
        assistant,
        pending_results,
        ..
    } = step
    {
        if assistant.tool_calls.is_empty() {
            return Err(PhiAgentRuntimeError::session(
                "request_executor must contain at least one tool call",
            ));
        }
        if pending_results.len() >= assistant.tool_calls.len() {
            return Err(PhiAgentRuntimeError::session(format!(
                "request_executor has {} completed results for {} tool calls",
                pending_results.len(),
                assistant.tool_calls.len()
            )));
        }
        crate::error::validate_completed_tool_results(
            assistant,
            pending_results,
            "request_executor",
        )?;
    }
    Ok(())
}

pub(crate) fn validate_agent_step(step: &PhiAgentStep) -> Result<(), PhiAgentRuntimeError> {
    match step {
        PhiAgentStep::ReAct(step) => validate_react_step(step),
        PhiAgentStep::Failed(failed) => match failed.error() {
            PhiAgentRuntimeError::ToolError { turn, .. }
            | PhiAgentRuntimeError::ToolNotFound { turn, .. } => turn.validate(),
            _ => Ok(()),
        },
    }
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
