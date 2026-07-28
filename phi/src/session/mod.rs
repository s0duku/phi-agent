pub mod command;
mod serialization;

use crate::{
    config::{ModelRequestDefaults, PhiConfig},
    error::PhiRuntimeError,
    executor::ToolCallRequest,
    expr::{PhiExprDelta, PhiStepExpr},
    message::{PhiHistory, PhiMessage},
    render::PhiProviderCall,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PhiModelRetryState {
    pub attempt: usize,
}

pub(crate) fn serde_default_request_complete_step() -> PhiAgentStep {
    PhiAgentStep::request_complete(
        "ready",
        &ModelRequestDefaults::from_config(&PhiConfig::default())
            .expect("empty settings should always produce fallback model defaults"),
    )
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhiAgentStep {
    RequestCompact,
    RequestComplete {
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
    Completed {
        detail: String,
    },
    Failed {
        error: PhiRuntimeError,
    },
}

impl PhiAgentStep {
    pub fn request_complete(detail: impl Into<String>, defaults: &ModelRequestDefaults) -> Self {
        Self::RequestComplete {
            detail: detail.into(),
            call: PhiProviderCall::from_parts(defaults, Vec::new()),
        }
    }

    pub fn request_complete_with_call(detail: impl Into<String>, call: PhiProviderCall) -> Self {
        Self::RequestComplete {
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

    pub fn completed(detail: impl Into<String>) -> Self {
        Self::Completed {
            detail: detail.into(),
        }
    }

    pub fn failed(error: PhiRuntimeError) -> Self {
        Self::Failed { error }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::RequestCompact => "request compact",
            Self::Compacted => "after compacted",
            Self::RequestComplete { detail, .. }
            | Self::RequestExecutor { detail, .. }
            | Self::Completed { detail } => detail,
            Self::Failed { error } => &error.detail(),
        }
    }

    pub fn error(&self) -> Option<&PhiRuntimeError> {
        let Self::Failed { error } = self else {
            return None;
        };
        Some(error)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }

    pub fn request_complete_call(&self) -> Option<&PhiProviderCall> {
        let Self::RequestComplete { call, .. } = self else {
            return None;
        };
        Some(call)
    }
}

#[derive(Clone, Debug)]
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

    pub(crate) fn validate(&self) -> Result<(), PhiRuntimeError> {
        fn validate_expr(expr: &PhiStepExpr) -> Result<(), PhiRuntimeError> {
            match expr.step() {
                PhiAgentStep::RequestCompact | PhiAgentStep::Failed { .. }
                    if expr.expr().is_some() && !expr.delta().is_empty() =>
                {
                    return Err(PhiRuntimeError::session(format!(
                        "{} frame must keep an empty delta",
                        expr.step().detail()
                    )));
                }
                PhiAgentStep::Compacted if expr.expr().is_none() => {
                    return Err(PhiRuntimeError::session(
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
