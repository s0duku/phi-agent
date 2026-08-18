pub mod builtins;
pub mod sanitizer;
pub mod tools;

use std::sync::Arc;

use crate::agent::PhiAgentRuntime;
use crate::error::PhiStructureError;
use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PhiToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolOutputLimits {
    pub output_threshold_tokens: usize,
}

impl ToolOutputLimits {
    pub const fn new(output_threshold_tokens: usize) -> Self {
        Self {
            output_threshold_tokens,
        }
    }

    pub fn stricter(self, other: Self) -> Self {
        Self {
            output_threshold_tokens: self
                .output_threshold_tokens
                .min(other.output_threshold_tokens),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ToolCallRequest {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(transparent)]
pub struct ToolCallOutput(pub serde_json::Value);

impl ToolCallOutput {
    pub fn new(value: serde_json::Value) -> Self {
        Self(value)
    }

    pub fn as_value(&self) -> &serde_json::Value {
        &self.0
    }

    pub fn into_value(self) -> serde_json::Value {
        self.0
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ToolCallResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub name: String,
    pub output: ToolCallOutput,
}

pub(crate) type PhiToolResult = Result<ToolCallResponse, Box<dyn PhiStructureError>>;

pub(crate) enum PhiToolExecutionError {
    NotFound {
        detail: String,
        request: ToolCallRequest,
    },
    Failed {
        detail: serde_json::Value,
        request: ToolCallRequest,
    },
}

impl ToolCallResponse {
    pub fn new(
        request: &ToolCallRequest,
        name: impl Into<String>,
        value: serde_json::Value,
    ) -> Self {
        Self {
            id: request.id.clone(),
            call_id: request.call_id.clone(),
            name: name.into(),
            output: ToolCallOutput::new(value),
        }
    }
}

#[async_trait]
pub(crate) trait PhiTool: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> &str;

    fn parameters(&self) -> serde_json::Value;

    fn definition(&self) -> PhiToolDefinition {
        PhiToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }

    // The tool owns the response schema. Runtime errors are reserved for
    // executor-level dispatch failures, such as an unknown tool name.
    async fn call(&self, request: &mut ToolCallRequest, runtime: &PhiAgentRuntime)
    -> PhiToolResult;
}

pub struct PhiExecutor {
    tools: IndexMap<String, Arc<dyn PhiTool>>,
    output_limits: ToolOutputLimits,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PhiExecutorBuildError {
    EmptyToolName,
    DuplicateToolName { name: String },
}

impl std::fmt::Display for PhiExecutorBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyToolName => formatter.write_str("tool name cannot be empty"),
            Self::DuplicateToolName { name } => {
                write!(
                    formatter,
                    "tool {name} is already registered on this executor"
                )
            }
        }
    }
}

impl std::error::Error for PhiExecutorBuildError {}

impl PhiExecutor {
    pub(crate) fn from_tools(
        tools: Vec<Arc<dyn PhiTool>>,
        output_limits: ToolOutputLimits,
    ) -> Result<Self, PhiExecutorBuildError> {
        let mut registered = IndexMap::new();

        for tool in tools {
            let definition = tool.definition();
            let name = definition.name;
            if name.trim().is_empty() {
                return Err(PhiExecutorBuildError::EmptyToolName);
            }
            if registered.contains_key(&name) {
                return Err(PhiExecutorBuildError::DuplicateToolName { name });
            }
            registered.insert(name, tool);
        }

        Ok(Self {
            tools: registered,
            output_limits,
        })
    }

    pub fn definitions(&self) -> Vec<PhiToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    pub(crate) fn tool(&self, name: &str) -> Option<&Arc<dyn PhiTool>> {
        self.tools.get(name)
    }

    pub(crate) async fn call_tool(
        &self,
        mut request: ToolCallRequest,
        runtime: &PhiAgentRuntime,
    ) -> Result<(ToolCallRequest, ToolCallResponse), PhiToolExecutionError> {
        let name = request.name.clone();
        let tool = self
            .tool(name.as_str())
            .ok_or_else(|| PhiToolExecutionError::NotFound {
                detail: format!("assistant requested unknown tool: {name}"),
                request: request.clone(),
            })?;
        let mut response = tool.call(&mut request, runtime).await.map_err(|error| {
            PhiToolExecutionError::Failed {
                detail: error.into_value(),
                request: request.clone(),
            }
        })?;
        response.output = sanitizer::sanitize_tool_call_output(response.output, self.output_limits);
        Ok((request, response))
    }
}
