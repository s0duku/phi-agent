use serde::Serialize;

use crate::{
    config::ReasoningEffort,
    error::PhiAgentRuntimeError,
    executor::{PhiToolDefinition, ToolCallRequest},
    message::{PhiAssistantMessage, PhiMessage, PhiToolResultMessage},
};

use super::{PhiAgentStep, PhiReActStep, Session};

#[derive(Debug, Serialize)]
pub struct PhiSessionState<'a> {
    pub history_messages: usize,
    #[serde(flatten)]
    pub step: PhiStepState<'a>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PhiStepState<'a> {
    RequestCompact {
        retain_rate: f32,
    },
    RequestProvider {
        detail: &'a str,
        request: PhiProviderRequestState<'a>,
    },
    RequestExecutor {
        detail: &'a str,
        pending_messages: &'a [PhiMessage],
        assistant: &'a PhiAssistantMessage,
        completed_results: &'a [PhiToolResultMessage],
        next_tool_call: &'a ToolCallRequest,
        remaining_tool_calls: &'a [ToolCallRequest],
    },
    Compacted,
    TurnEnd {
        detail: &'a str,
    },
    Failed {
        failure: PhiFailureState<'a>,
    },
}

#[derive(Debug, Serialize)]
pub struct PhiProviderRequestState<'a> {
    pub model: &'a str,
    pub tools: &'a [PhiToolDefinition],
    pub temperature: Option<f64>,
    pub max_tokens: u64,
    pub enable_reasoning: bool,
    pub reasoning_effort: ReasoningEffort,
}

#[derive(Debug, Serialize)]
pub struct PhiFailedToolState<'a> {
    pub request: &'a ToolCallRequest,
    pub pending_messages: &'a [PhiMessage],
    pub assistant: &'a PhiAssistantMessage,
    pub completed_results: &'a [PhiToolResultMessage],
    pub remaining_tool_calls: &'a [ToolCallRequest],
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhiFailureState<'a> {
    RequestCompact {
        detail: &'a str,
    },
    ContextExceededLimit {
        detail: &'a str,
    },
    ProviderRequest {
        detail: &'a str,
    },
    ProviderResponse {
        detail: &'a str,
    },
    ModelOutputLimit {
        detail: &'a str,
    },
    ModelToolParseError {
        detail: &'a str,
    },
    Module {
        detail: &'a str,
    },
    Home {
        detail: &'a str,
    },
    ToolError {
        detail: &'a serde_json::Value,
        tool: PhiFailedToolState<'a>,
    },
    ToolNotFound {
        detail: &'a str,
        tool: PhiFailedToolState<'a>,
    },
    ModelCandidateRejected {
        detail: &'a str,
    },
    Session {
        detail: &'a str,
    },
}

impl Session {
    pub fn state(&self) -> Result<PhiSessionState<'_>, PhiAgentRuntimeError> {
        self.validate()?;
        Ok(PhiSessionState {
            history_messages: self.history().len(),
            step: PhiStepState::try_from(self.step())?,
        })
    }
}

impl<'a> TryFrom<&'a PhiAgentStep> for PhiStepState<'a> {
    type Error = PhiAgentRuntimeError;

    fn try_from(step: &'a PhiAgentStep) -> Result<Self, Self::Error> {
        Ok(match step {
            PhiAgentStep::ReAct(PhiReActStep::RequestCompact { retain_rate }) => {
                Self::RequestCompact {
                    retain_rate: *retain_rate,
                }
            }
            PhiAgentStep::ReAct(PhiReActStep::RequestProvider { detail, call }) => {
                Self::RequestProvider {
                    detail,
                    request: PhiProviderRequestState {
                        model: &call.model,
                        tools: &call.tools,
                        temperature: call.temperature,
                        max_tokens: call.max_tokens,
                        enable_reasoning: call.enable_reasoning,
                        reasoning_effort: call.reasoning_effort,
                    },
                }
            }
            PhiAgentStep::ReAct(PhiReActStep::RequestExecutor {
                detail,
                pending_messages,
                assistant,
                pending_results,
            }) => {
                let next_index = pending_results.len();
                let next_tool_call = assistant.tool_calls.get(next_index).ok_or_else(|| {
                    PhiAgentRuntimeError::session("request_executor state has no pending tool call")
                })?;
                Self::RequestExecutor {
                    detail,
                    pending_messages,
                    assistant,
                    completed_results: pending_results,
                    next_tool_call,
                    remaining_tool_calls: assistant
                        .tool_calls
                        .get(next_index + 1..)
                        .unwrap_or_default(),
                }
            }
            PhiAgentStep::ReAct(PhiReActStep::Compacted) => Self::Compacted,
            PhiAgentStep::ReAct(PhiReActStep::TurnEnd { detail }) => Self::TurnEnd { detail },
            PhiAgentStep::Failed(failed) => Self::Failed {
                failure: PhiFailureState::try_from(failed.error())?,
            },
        })
    }
}

impl<'a> TryFrom<&'a PhiAgentRuntimeError> for PhiFailureState<'a> {
    type Error = PhiAgentRuntimeError;

    fn try_from(error: &'a PhiAgentRuntimeError) -> Result<Self, Self::Error> {
        Ok(match error {
            PhiAgentRuntimeError::RequestCompact { detail } => Self::RequestCompact { detail },
            PhiAgentRuntimeError::ContextExceededLimit { detail } => {
                Self::ContextExceededLimit { detail }
            }
            PhiAgentRuntimeError::ProviderRequest { detail } => Self::ProviderRequest { detail },
            PhiAgentRuntimeError::ProviderResponse { detail } => Self::ProviderResponse { detail },
            PhiAgentRuntimeError::ModelOutputLimit { detail } => Self::ModelOutputLimit { detail },
            PhiAgentRuntimeError::ModelToolParseError { detail } => {
                Self::ModelToolParseError { detail }
            }
            PhiAgentRuntimeError::Module { detail } => Self::Module { detail },
            PhiAgentRuntimeError::Home { detail } => Self::Home { detail },
            PhiAgentRuntimeError::ToolError { detail, turn } => Self::ToolError {
                detail,
                tool: failed_tool_state(turn)?,
            },
            PhiAgentRuntimeError::ToolNotFound { detail, turn } => Self::ToolNotFound {
                detail,
                tool: failed_tool_state(turn)?,
            },
            PhiAgentRuntimeError::ModelCandidateRejected { detail } => {
                Self::ModelCandidateRejected { detail }
            }
            PhiAgentRuntimeError::Session { detail } => Self::Session { detail },
        })
    }
}

fn failed_tool_state(
    turn: &crate::error::PhiFailedToolTurn,
) -> Result<PhiFailedToolState<'_>, PhiAgentRuntimeError> {
    Ok(PhiFailedToolState {
        request: turn
            .failed_request()
            .ok_or_else(|| PhiAgentRuntimeError::session("failed tool state has no request"))?,
        pending_messages: turn.pending_messages(),
        assistant: turn.assistant(),
        completed_results: turn.pending_results(),
        remaining_tool_calls: turn.remaining_requests(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::{PhiAgentRuntimeError, PhiFailedToolTurn},
        message::PhiAssistantMessage,
        session::PhiAgentStep,
    };

    #[test]
    fn request_executor_state_identifies_the_next_call() {
        let first = tool_request("first");
        let second = tool_request("second");
        let session = Session::from_root(
            PhiAgentStep::request_executor(
                "pending",
                vec![PhiMessage::user("run tools")],
                PhiAssistantMessage::tool_calls(vec![first, second.clone()]),
            ),
            Vec::new(),
        )
        .insert_tool_result(serde_json::json!({"ok": true}), test_provider_call())
        .expect("first result should resolve");

        let value = serde_json::to_value(session.state().expect("state should be valid"))
            .expect("state should serialize");
        assert_eq!(value["state"], "request_executor");
        assert_eq!(value["next_tool_call"]["name"], second.name);
        assert_eq!(value["completed_results"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn failed_state_explains_an_unknown_tool_without_text_parsing() {
        let request = tool_request("missing");
        let error = PhiAgentRuntimeError::tool_not_found(
            "assistant requested unknown tool: missing",
            PhiFailedToolTurn::new(
                Vec::new(),
                PhiAssistantMessage::tool_calls(vec![request.clone()]),
                Vec::new(),
            )
            .expect("failed tool turn fixture must be valid"),
        );
        let session = Session::from_root(PhiAgentStep::failed(error), Vec::new());

        let value = serde_json::to_value(session.state().expect("state should be valid"))
            .expect("state should serialize");
        assert_eq!(value["state"], "failed");
        assert_eq!(value["failure"]["kind"], "tool_not_found");
        assert_eq!(value["failure"]["tool"]["request"]["name"], "missing");
    }

    #[test]
    fn provider_failures_do_not_duplicate_model_retry_state() {
        for error in [
            PhiAgentRuntimeError::provider_request("request failed"),
            PhiAgentRuntimeError::provider_response("response failed"),
        ] {
            let error_value = serde_json::to_value(&error).expect("error should serialize");
            assert!(error_value.get("retry").is_none());

            let session = Session::from_root(PhiAgentStep::failed(error), Vec::new());
            let state_value = serde_json::to_value(session.state().expect("state should be valid"))
                .expect("state should serialize");
            assert_eq!(state_value["state"], "failed");
            assert!(state_value["failure"].get("retry").is_none());
        }
    }

    #[test]
    fn state_rejects_an_empty_tool_queue() {
        let session = Session::from_root(
            PhiAgentStep::request_executor(
                "pending",
                Vec::new(),
                PhiAssistantMessage::tool_calls(Vec::new()),
            ),
            Vec::new(),
        );
        let error = session
            .state()
            .expect_err("state boundary must reject an empty tool queue");

        assert_eq!(
            error.detail(),
            "request_executor must contain at least one tool call"
        );
    }

    #[test]
    fn state_returns_an_error_for_an_invalid_internal_session() {
        let session = Session::from_root(
            PhiAgentStep::ReAct(PhiReActStep::RequestExecutor {
                detail: "invalid".to_string(),
                pending_messages: Vec::new(),
                assistant: PhiAssistantMessage::tool_calls(Vec::new()),
                pending_results: Vec::new(),
            }),
            Vec::new(),
        );

        let error = session
            .state()
            .expect_err("state must reject an invalid internal session");
        assert_eq!(
            error.detail(),
            "request_executor must contain at least one tool call"
        );
    }

    #[test]
    fn replace_rejects_a_root_compacted_step() {
        let error = Session::empty()
            .replace(PhiReActStep::Compacted)
            .expect_err("root compacted step must be rejected");

        assert_eq!(
            error.detail(),
            "compacted frame must preserve a parent expr"
        );
    }

    fn tool_request(name: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: format!("call_{name}"),
            call_id: Some(format!("call_{name}")),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }
    }

    fn test_provider_call() -> crate::render::PhiProviderCall {
        crate::render::PhiProviderCall::from_parts(
            &crate::config::ModelRequestDefaults::from(&crate::config::PhiConfig::default()),
            Vec::new(),
        )
    }
}
