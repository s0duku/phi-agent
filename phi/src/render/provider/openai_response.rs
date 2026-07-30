use std::vec;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{PhiModelResponse, PhiModelTurnState, PhiProvider, PhiProviderCall};
use crate::{
    config::ProviderConfig,
    error::{PhiAgentResult, PhiAgentRuntimeError},
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
}

impl ResponsesClient {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
        }
    }
}

#[async_trait]
impl PhiProvider for ResponsesClient {
    type ProviderMessage = ProviderMessage;
    type ProviderTool = ResponsesToolDefinition;

    fn provider_messages(
        &self,
        messages: &PhiRenderedMessages,
    ) -> PhiAgentResult<Vec<Self::ProviderMessage>> {
        Ok(ResponsesPrompt::from_phi_messages(messages).input)
    }

    fn phi_messages(
        &self,
        response: Vec<Self::ProviderMessage>,
    ) -> PhiAgentResult<Vec<PhiMessage>> {
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
    ) -> PhiAgentResult<PhiModelResponse> {
        let prompt = ResponsesPrompt::from_phi_messages(messages);
        let provider_tools = request
            .tools
            .iter()
            .map(|tool| self.provider_tool(tool))
            .collect::<Vec<_>>();

        let request = ResponsesRequest::from_prompt(request, &prompt, &provider_tools);

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
                PhiAgentRuntimeError::provider_request(format!(
                    "openai_response request failed: {error}"
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("<failed to read error body: {error}>"));
            return Err(PhiAgentRuntimeError::provider_request(format!(
                "HTTP {status} from responses API: {body}"
            )));
        }

        let status = response.status();
        let body = response.text().await.map_err(|error| {
            PhiAgentRuntimeError::provider_response(format!(
                "openai_response response body read failed (HTTP {status}): {error}"
            ))
        })?;
        let response = serde_json::from_str::<ResponsesCreateResponse>(&body).map_err(|error| {
            PhiAgentRuntimeError::provider_response(format!(
                "openai_response response decode failed (HTTP {status}): {error}; response body: {body}"
            ))
        })?;
        response.validate_status()?;
        let turn_state = response.turn_state();
        let messages = self.phi_messages(response.into_provider_messages())?;
        Ok(PhiModelResponse::new(messages, turn_state))
    }
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
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        summary: Vec<ResponsesReasoningSummary>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ResponsesReasoningContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
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
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning { id, content }) => {
                vec![Self::Reasoning {
                    id: id.clone(),
                    summary: content
                        .iter()
                        .filter_map(|part| match part {
                            PhiReasoningContent::Summary(text)
                            | PhiReasoningContent::Redacted { data: text } => {
                                Some(ResponsesReasoningSummary::SummaryText { text: text.clone() })
                            }
                            PhiReasoningContent::Text { .. }
                            | PhiReasoningContent::Encrypted(_) => None,
                        })
                        .collect(),
                    content: content
                        .iter()
                        .filter_map(|part| match part {
                            PhiReasoningContent::Text { text, .. } => {
                                Some(ResponsesReasoningContent::ReasoningText {
                                    text: text.clone(),
                                })
                            }
                            PhiReasoningContent::Summary(_)
                            | PhiReasoningContent::Redacted { .. }
                            | PhiReasoningContent::Encrypted(_) => None,
                        })
                        .collect(),
                    encrypted_content: content.iter().find_map(|part| match part {
                        PhiReasoningContent::Encrypted(data) => Some(data.clone()),
                        _ => None,
                    }),
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

    fn into_phi_messages(self) -> PhiAgentResult<Vec<PhiMessage>> {
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
            Self::Reasoning {
                id,
                summary,
                content,
                encrypted_content,
            } => {
                let mut reasoning = summary
                    .into_iter()
                    .filter_map(|item| match item {
                        ResponsesReasoningSummary::SummaryText { text } => {
                            Some(PhiReasoningContent::Summary(text))
                        }
                        ResponsesReasoningSummary::Other => None,
                    })
                    .chain(content.into_iter().filter_map(|item| match item {
                        ResponsesReasoningContent::ReasoningText { text } => {
                            Some(PhiReasoningContent::Text {
                                text,
                                signature: None,
                            })
                        }
                        ResponsesReasoningContent::Other => None,
                    }))
                    .collect::<Vec<_>>();
                if let Some(encrypted_content) = encrypted_content {
                    reasoning.push(PhiReasoningContent::Encrypted(encrypted_content));
                }
                if reasoning.is_empty() {
                    vec![]
                } else {
                    vec![PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                        id,
                        content: reasoning,
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
                    PhiAgentRuntimeError::provider_response(format!(
                        "openai_response invalid tool-call arguments JSON: {} | raw={}",
                        error, arguments
                    ))
                })?,
            )],
        })
    }
}

struct ResponsesPrompt {
    instructions: Option<String>,
    input: Vec<ProviderMessage>,
}

impl ResponsesPrompt {
    fn from_phi_messages(messages: &PhiRenderedMessages) -> Self {
        let mut instructions = Vec::new();
        let mut input = Vec::new();
        for message in messages.iter() {
            match message {
                PhiMessage::System(content) if !content.is_empty() => {
                    instructions.push(content.as_str());
                }
                _ => input.extend(ProviderMessage::from_phi_message(message)),
            }
        }
        let instructions = instructions.join("\n\n");
        Self {
            instructions: (!instructions.is_empty()).then_some(instructions),
            input,
        }
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
    tool_choice: String,
    parallel_tool_calls: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    include: Vec<String>,
}

impl ResponsesRequest {
    fn from_prompt(
        request: &PhiProviderCall,
        prompt: &ResponsesPrompt,
        tools: &[ResponsesToolDefinition],
    ) -> Self {
        Self {
            model: request.model.clone(),
            input: prompt.input.clone(),
            instructions: prompt.instructions.clone(),
            previous_response_id: None,
            store: false,
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
            temperature: request.temperature,
            max_output_tokens: Some(request.max_tokens),
            reasoning: if request.enable_reasoning {
                Some(ResponsesReasoningRequest::new(
                    request.reasoning_effort.as_str(),
                ))
            } else {
                None
            },
            tool_choice: "auto".to_string(),
            parallel_tool_calls: false,
            include: request
                .enable_reasoning
                .then(|| "reasoning.encrypted_content".to_string())
                .into_iter()
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct ResponsesCreateResponse {
    #[serde(default)]
    output: Vec<ResponsesOutputItem>,
    #[serde(default)]
    end_turn: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error: Option<ResponsesError>,
    #[serde(default)]
    incomplete_details: Option<ResponsesIncompleteDetails>,
}

impl ResponsesCreateResponse {
    fn validate_status(&self) -> PhiAgentResult<()> {
        match self.status.as_deref() {
            None | Some("completed") => Ok(()),
            Some("failed") => {
                let detail = self
                    .error
                    .as_ref()
                    .map(ResponsesError::detail)
                    .unwrap_or_else(|| "response failed without error details".to_string());
                Err(PhiAgentRuntimeError::provider_response(format!(
                    "openai_response failed: {detail}"
                )))
            }
            Some("incomplete") => {
                let reason = self
                    .incomplete_details
                    .as_ref()
                    .and_then(|details| details.reason.as_deref())
                    .unwrap_or("unknown reason");
                Err(PhiAgentRuntimeError::provider_response(format!(
                    "openai_response incomplete: {reason}"
                )))
            }
            Some(status) => Err(PhiAgentRuntimeError::provider_response(format!(
                "openai_response returned non-terminal status: {status}"
            ))),
        }
    }

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
                    call_id,
                    name,
                    arguments,
                    ..
                } => Some(ProviderMessage::FunctionCall {
                    call_id,
                    name,
                    arguments,
                }),
                ResponsesOutputItem::Reasoning {
                    id,
                    summary,
                    content,
                    encrypted_content,
                } => Some(ProviderMessage::Reasoning {
                    id,
                    summary,
                    content,
                    encrypted_content,
                }),
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
        #[serde(default, rename = "id")]
        _id: Option<String>,
        call_id: String,
        name: String,
        arguments: String,
        #[serde(rename = "status")]
        _status: Option<String>,
    },
    Reasoning {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        summary: Vec<ResponsesReasoningSummary>,
        #[serde(default)]
        content: Vec<ResponsesReasoningContent>,
        #[serde(default)]
        encrypted_content: Option<String>,
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
    strict: bool,
}

impl ResponsesToolDefinition {
    fn from_tool_definition(function: PhiToolDefinition) -> Self {
        Self {
            kind: "function".to_string(),
            name: function.name,
            description: function.description,
            parameters: function.parameters,
            strict: false,
        }
    }
}

#[derive(Serialize)]
struct ResponsesReasoningRequest {
    effort: String,
    summary: String,
}

impl ResponsesReasoningRequest {
    fn new(effort: &str) -> Self {
        Self {
            effort: effort.to_string(),
            summary: "auto".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsesReasoningContent {
    ReasoningText {
        text: String,
    },
    #[serde(other)]
    Other,
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

#[derive(Clone, Debug, Deserialize)]
struct ResponsesError {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl ResponsesError {
    fn detail(&self) -> String {
        match (&self.code, &self.message) {
            (Some(code), Some(message)) => format!("{code}: {message}"),
            (Some(code), None) => code.clone(),
            (None, Some(message)) => message.clone(),
            (None, None) => "response failed without error details".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct ResponsesIncompleteDetails {
    #[serde(default)]
    reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::ModelRequestDefaults, render::test_rendered_messages};

    fn default_request() -> PhiProviderCall {
        PhiProviderCall::from_parts(&ModelRequestDefaults::defaults(), Vec::new())
    }

    #[test]
    fn maps_system_messages_to_instructions_and_not_input() {
        let messages = test_rendered_messages(vec![
            PhiMessage::system("base instructions"),
            PhiMessage::user("hello"),
            PhiMessage::system("additional instructions"),
        ]);
        let prompt = ResponsesPrompt::from_phi_messages(&messages);
        let request = ResponsesRequest::from_prompt(&default_request(), &prompt, &[]);

        let json = serde_json::to_value(request).expect("request should serialize");

        assert_eq!(
            json["instructions"],
            "base instructions\n\nadditional instructions"
        );
        assert_eq!(json["input"].as_array().expect("input array").len(), 1);
        assert_eq!(json["input"][0]["role"], "user");
        assert_eq!(json["store"], false);
        assert_eq!(json["tool_choice"], "auto");
        assert_eq!(json["parallel_tool_calls"], false);
        assert_eq!(
            json["include"],
            serde_json::json!(["reasoning.encrypted_content"])
        );
        assert!(json["reasoning"].get("max_output_tokens").is_none());
    }

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
    fn function_call_uses_call_id_instead_of_item_id() {
        let response = serde_json::from_str::<ResponsesCreateResponse>(
            r#"{
                "id": "resp_test",
                "store": false,
                "output": [
                    {
                        "type": "function_call",
                        "id": "fc_456",
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
    fn reasoning_state_round_trips_through_phi_history() {
        let response = serde_json::from_str::<ResponsesCreateResponse>(
            r#"{
                "status": "completed",
                "output": [{
                    "type": "reasoning",
                    "id": "rs_123",
                    "summary": [{"type":"summary_text","text":"summary"}],
                    "content": [{"type":"reasoning_text","text":"reasoning"}],
                    "encrypted_content": "encrypted-state"
                }]
            }"#,
        )
        .expect("response should deserialize");
        let provider_message = response
            .into_provider_messages()
            .into_iter()
            .next()
            .expect("reasoning item should exist");
        let phi_message = provider_message
            .into_phi_messages()
            .expect("reasoning should map into Phi")
            .into_iter()
            .next()
            .expect("Phi reasoning should exist");

        assert_eq!(
            phi_message,
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                id: Some("rs_123".to_string()),
                content: vec![
                    PhiReasoningContent::Summary("summary".to_string()),
                    PhiReasoningContent::Text {
                        text: "reasoning".to_string(),
                        signature: None,
                    },
                    PhiReasoningContent::Encrypted("encrypted-state".to_string()),
                ],
            })
        );
        assert_eq!(
            ProviderMessage::from_phi_message(&phi_message),
            vec![ProviderMessage::Reasoning {
                id: Some("rs_123".to_string()),
                summary: vec![ResponsesReasoningSummary::SummaryText {
                    text: "summary".to_string(),
                }],
                content: vec![ResponsesReasoningContent::ReasoningText {
                    text: "reasoning".to_string(),
                }],
                encrypted_content: Some("encrypted-state".to_string()),
            }]
        );
    }

    #[test]
    fn rejects_failed_and_incomplete_responses() {
        for (json, expected_detail) in [
            (
                r#"{
                    "status":"failed",
                    "error":{"code":"rate_limit_exceeded","message":"try later"}
                }"#,
                "openai_response failed: rate_limit_exceeded: try later",
            ),
            (
                r#"{
                    "status":"incomplete",
                    "incomplete_details":{"reason":"max_output_tokens"}
                }"#,
                "openai_response incomplete: max_output_tokens",
            ),
        ] {
            let response = serde_json::from_str::<ResponsesCreateResponse>(json)
                .expect("response should deserialize");
            let error = response
                .validate_status()
                .expect_err("non-completed response should fail");
            assert_eq!(error.detail(), expected_detail);
        }
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
