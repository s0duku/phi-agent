use std::{
    sync::{Mutex, OnceLock},
    vec,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{PhiModelResponse, PhiModelTurnState, PhiProvider, PhiProviderCall};
use crate::{
    config::ProviderConfig,
    error::{PhiResult, PhiRuntimeError},
    executor::PhiToolDefinition,
    message::{
        PhiAssistantMessage, PhiMessage, PhiReasoningContent, PhiToolMessage, PhiUserMessage,
    },
    render::PhiRenderedMessages,
};

#[derive(Clone)]
pub struct ResponsesClient {
    http: reqwest::Client,
    config: ProviderConfig,
    reasoning_format: &'static OnceLock<Mutex<ResponsesReasoningFormat>>,
}

impl ResponsesClient {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
            reasoning_format: reasoning_format_state(),
        }
    }

    fn current_reasoning_format(&self) -> PhiResult<ResponsesReasoningFormat> {
        let guard = self
            .reasoning_format
            .get_or_init(|| Mutex::new(ResponsesReasoningFormat::Summary))
            .lock()
            .map_err(|_| {
                PhiRuntimeError::internal("responses reasoning format mutex was poisoned")
            })?;
        Ok(*guard)
    }

    fn observe_reasoning_format(&self, _response: &ResponsesCreateResponse) -> PhiResult<()> {
        let mut guard = self
            .reasoning_format
            .get_or_init(|| Mutex::new(ResponsesReasoningFormat::Summary))
            .lock()
            .map_err(|_| {
                PhiRuntimeError::internal("responses reasoning format mutex was poisoned")
            })?;
        *guard = ResponsesReasoningFormat::Summary;
        Ok(())
    }
}

#[async_trait]
impl PhiProvider for ResponsesClient {
    type ProviderMessage = ProviderMessage;
    type ProviderTool = ResponsesToolDefinition;

    fn provider_messages(
        &self,
        messages: &PhiRenderedMessages,
    ) -> PhiResult<Vec<Self::ProviderMessage>> {
        Ok(messages
            .iter()
            .flat_map(ProviderMessage::from_phi_message)
            .collect())
    }

    fn phi_messages(&self, response: Vec<Self::ProviderMessage>) -> PhiResult<Vec<PhiMessage>> {
        response
            .into_iter()
            .map(ProviderMessage::into_phi_messages)
            .collect::<Result<Vec<_>, _>>()
            .map(|messages| messages.into_iter().flatten().collect())
    }

    fn provider_tool(&self, tool: &PhiToolDefinition) -> Self::ProviderTool {
        ResponsesToolDefinition::from_tool_definition(tool.clone())
    }

    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: &PhiRenderedMessages,
    ) -> PhiResult<PhiModelResponse> {
        let provider_messages = self.provider_messages(messages)?;
        let provider_tools = request
            .tools
            .iter()
            .map(|tool| self.provider_tool(tool))
            .collect::<Vec<_>>();

        let request = ResponsesRequest::from_provider_messages(
            request,
            &provider_messages,
            &provider_tools,
            self.current_reasoning_format()?,
        );

        let response = self
            .http
            .post(format!(
                "{}/responses",
                self.config.api_base.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.api_key)
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                PhiRuntimeError::provider_request(format!(
                    "openai_response request failed: {error}"
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("<failed to read error body: {error}>"));
            return Err(PhiRuntimeError::provider_request(format!(
                "HTTP {status} from responses API: {body}"
            )));
        }

        let response = response
            .json::<ResponsesCreateResponse>()
            .await
            .map_err(|error| {
                PhiRuntimeError::provider_response(format!(
                    "openai_response response decode failed: {error}"
                ))
            })?;
        self.observe_reasoning_format(&response)?;
        let turn_state = response.turn_state();
        let messages = self.phi_messages(response.into_provider_messages())?;
        Ok(PhiModelResponse::new(messages, turn_state))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponsesReasoningFormat {
    Summary,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderMessage {
    Message {
        role: String,
        content: Vec<ResponsesContentPart>,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
    Reasoning {
        summary: Vec<ResponsesReasoningSummary>,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
}

impl ProviderMessage {
    fn from_phi_message(message: &PhiMessage) -> Vec<Self> {
        match message {
            PhiMessage::System(_) => vec![],
            PhiMessage::User(PhiUserMessage::Text(text)) => vec![Self::Message {
                role: "user".to_string(),
                content: vec![ResponsesContentPart::InputText { text: text.clone() }],
            }],
            PhiMessage::Assistant(PhiAssistantMessage::Text(text)) => vec![Self::Message {
                role: "assistant".to_string(),
                content: vec![ResponsesContentPart::OutputText { text: text.clone() }],
            }],
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning { content, .. }) => {
                vec![Self::Reasoning {
                    summary: content
                        .iter()
                        .filter_map(PhiReasoningContent::display_text)
                        .map(|text: &str| ResponsesReasoningSummary::SummaryText {
                            text: text.to_string(),
                        })
                        .collect(),
                }]
            }
            PhiMessage::Tool(PhiToolMessage::ToolCall {
                id,
                name,
                arguments,
            }) => vec![Self::FunctionCall {
                call_id: id.clone().unwrap_or_else(|| name.clone()),
                name: name.clone(),
                arguments: serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string()),
            }],
            PhiMessage::Tool(PhiToolMessage::ToolResult { id, result, .. }) => {
                vec![Self::FunctionCallOutput {
                    call_id: id.clone().unwrap_or_default(),
                    output: result.to_string(),
                }]
            }
        }
    }

    fn into_phi_messages(self) -> PhiResult<Vec<PhiMessage>> {
        Ok(match self {
            Self::Message { role, content } => {
                let text = content
                    .into_iter()
                    .filter_map(|part| match part {
                        ResponsesContentPart::InputText { text }
                        | ResponsesContentPart::OutputText { text } => Some(text),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() {
                    vec![]
                } else if role == "assistant" {
                    vec![PhiMessage::Assistant(PhiAssistantMessage::Text(text))]
                } else {
                    vec![PhiMessage::User(PhiUserMessage::Text(text))]
                }
            }
            Self::FunctionCallOutput { call_id, output } => vec![PhiMessage::tool_result(
                Some(call_id),
                None,
                serde_json::from_str(&output).unwrap_or_else(|_| serde_json::Value::String(output)),
            )],
            Self::Reasoning { summary } => {
                let reasoning_summary = summary
                    .into_iter()
                    .filter_map(|item| match item {
                        ResponsesReasoningSummary::SummaryText { text } => Some(text),
                        ResponsesReasoningSummary::Other => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if reasoning_summary.is_empty() {
                    vec![]
                } else {
                    vec![PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                        id: None,
                        content: vec![PhiReasoningContent::Summary(reasoning_summary)],
                    })]
                }
            }
            Self::FunctionCall {
                call_id,
                name,
                arguments,
            } => vec![PhiMessage::tool_call(
                Some(call_id),
                name,
                serde_json::from_str(&arguments).map_err(|error| {
                    PhiRuntimeError::provider_response(format!(
                        "openai_response invalid tool-call arguments JSON: {} | raw={}",
                        error, arguments
                    ))
                })?,
            )],
        })
    }
}

#[derive(Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ProviderMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<String>,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ResponsesReasoningRequest>,
}

impl ResponsesRequest {
    fn from_provider_messages(
        request: &PhiProviderCall,
        messages: &[ProviderMessage],
        tools: &[ResponsesToolDefinition],
        reasoning_format: ResponsesReasoningFormat,
    ) -> Self {
        Self {
            model: request.model.clone(),
            input: messages.to_vec(),
            instructions: None,
            previous_response_id: None,
            store: true,
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
            temperature: request.temperature,
            max_output_tokens: Some(request.max_tokens),
            reasoning: if request.enable_reasoning {
                Some(ResponsesReasoningRequest::from_reasoning_format(
                    reasoning_format,
                    request.thinking_token_budget,
                    request.reasoning_effort.as_str(),
                ))
            } else {
                None
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct ResponsesCreateResponse {
    #[serde(default)]
    output: Vec<ResponsesOutputItem>,
    #[serde(default)]
    end_turn: Option<bool>,
}

impl ResponsesCreateResponse {
    fn turn_state(&self) -> PhiModelTurnState {
        match self.end_turn {
            Some(true) => PhiModelTurnState::Complete,
            Some(false) => PhiModelTurnState::Continue,
            None => PhiModelTurnState::Unspecified,
        }
    }

    fn into_provider_messages(self) -> Vec<ProviderMessage> {
        self.output
            .into_iter()
            .filter_map(|item| match item {
                ResponsesOutputItem::Message { content, .. } => Some(ProviderMessage::Message {
                    role: "assistant".to_string(),
                    content: content
                        .into_iter()
                        .filter_map(|part| match part {
                            ResponsesOutputContent::OutputText { text }
                            | ResponsesOutputContent::SummaryText { text } => {
                                Some(ResponsesContentPart::OutputText { text })
                            }
                            ResponsesOutputContent::Refusal { refusal } => {
                                Some(ResponsesContentPart::OutputText { text: refusal })
                            }
                            ResponsesOutputContent::Other => None,
                        })
                        .collect(),
                }),
                ResponsesOutputItem::FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments,
                    ..
                } => Some(ProviderMessage::FunctionCall {
                    call_id: id.unwrap_or(call_id),
                    name,
                    arguments,
                }),
                ResponsesOutputItem::Reasoning { summary } => {
                    Some(ProviderMessage::Reasoning { summary })
                }
                ResponsesOutputItem::Other => None,
            })
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesOutputItem {
    Message {
        #[serde(rename = "role")]
        _role: Option<String>,
        #[serde(default)]
        content: Vec<ResponsesOutputContent>,
    },
    FunctionCall {
        #[serde(default)]
        id: Option<String>,
        call_id: String,
        name: String,
        arguments: String,
        #[serde(rename = "status")]
        _status: Option<String>,
    },
    Reasoning {
        #[serde(default)]
        summary: Vec<ResponsesReasoningSummary>,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsesContentPart {
    InputText { text: String },
    OutputText { text: String },
}

#[derive(Clone, Serialize)]
pub struct ResponsesToolDefinition {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl ResponsesToolDefinition {
    fn from_tool_definition(function: PhiToolDefinition) -> Self {
        Self {
            kind: "function".to_string(),
            name: function.name,
            description: function.description,
            parameters: function.parameters,
        }
    }
}

#[derive(Serialize)]
struct ResponsesReasoningRequest {
    effort: String,
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
}

impl ResponsesReasoningRequest {
    fn from_reasoning_format(_format: ResponsesReasoningFormat, budget: u64, effort: &str) -> Self {
        Self {
            effort: effort.to_string(),
            summary: "auto".to_string(),
            max_output_tokens: Some(budget),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesOutputContent {
    OutputText {
        text: String,
    },
    Refusal {
        refusal: String,
    },
    SummaryText {
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsesReasoningSummary {
    SummaryText {
        text: String,
    },
    #[serde(other)]
    Other,
}

fn reasoning_format_state() -> &'static OnceLock<Mutex<ResponsesReasoningFormat>> {
    static STATE: OnceLock<Mutex<ResponsesReasoningFormat>> = OnceLock::new();
    &STATE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_response_end_turn_to_model_turn_state() {
        for (json, expected) in [
            (
                r#"{"output":[],"end_turn":true}"#,
                PhiModelTurnState::Complete,
            ),
            (
                r#"{"output":[],"end_turn":false}"#,
                PhiModelTurnState::Continue,
            ),
            (r#"{"output":[]}"#, PhiModelTurnState::Unspecified),
        ] {
            let response = serde_json::from_str::<ResponsesCreateResponse>(json)
                .expect("response should deserialize");
            assert_eq!(response.turn_state(), expected);
        }
    }

    #[test]
    fn parses_function_call_without_id() {
        let response = serde_json::from_str::<ResponsesCreateResponse>(
            r#"{
                "id": "resp_test",
                "store": false,
                "output": [
                    {
                        "type": "function_call",
                        "call_id": "call_123",
                        "name": "bash",
                        "arguments": "{\"command\":\"ls\"}"
                    }
                ]
            }"#,
        )
        .expect("response should deserialize");

        let messages = response
            .into_provider_messages()
            .into_iter()
            .map(ProviderMessage::into_phi_messages)
            .collect::<Result<Vec<_>, _>>()
            .expect("provider messages should parse")
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        assert_eq!(
            messages,
            vec![PhiMessage::tool_call(
                Some("call_123".to_string()),
                "bash",
                serde_json::json!({ "command": "ls" }),
            )]
        );
    }

    #[test]
    fn serializes_tool_results_using_call_id_when_present() {
        let items = ProviderMessage::from_phi_message(&PhiMessage::tool_result(
            Some("call_123".to_string()),
            Some("bash".to_string()),
            serde_json::Value::String("done".to_string()),
        ));

        assert_eq!(
            items,
            vec![ProviderMessage::FunctionCallOutput {
                call_id: "call_123".to_string(),
                output: "\"done\"".to_string(),
            }]
        );
    }
}
