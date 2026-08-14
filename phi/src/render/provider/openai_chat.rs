use std::vec;

use async_trait::async_trait;
use serde::{Deserialize, Serialize, Serializer, de::DeserializeOwned};

use super::{DynProvider, PhiModelResponse, PhiModelTurnState, PhiProviderCall};
use crate::{
    config::ProviderConfig,
    error::{PhiAgentResult, PhiAgentRuntimeError},
    executor::PhiToolDefinition,
    message::{
        PhiAssistantMessage, PhiMessage, PhiReasoningBlock, PhiReasoningContent, PhiUserMessage,
    },
    render::PhiRenderedMessages,
};

#[derive(Clone)]
pub struct OpenAiCompatClient {
    http: reqwest::Client,
    config: ProviderConfig,
}

impl OpenAiCompatClient {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
        }
    }

    fn provider_messages(
        &self,
        messages: &PhiRenderedMessages,
    ) -> PhiAgentResult<Vec<ProviderMessage>> {
        let reasoning_format = ReasoningFormat::from_provider_context(messages.provider_context());
        ProviderMessage::from_phi_messages(messages, reasoning_format)
    }

    fn phi_assistant(
        &self,
        response: Vec<ProviderMessage>,
    ) -> PhiAgentResult<Option<PhiAssistantMessage>> {
        let mut response = response.into_iter();
        let assistant = match response.next() {
            Some(ProviderMessage::Assistant(message)) => message.into_phi_assistant()?,
            Some(_) => {
                return Err(PhiAgentRuntimeError::provider_response(
                    "openai_chat response was not an assistant message",
                ));
            }
            None => None,
        };
        if response.next().is_some() {
            return Err(PhiAgentRuntimeError::provider_response(
                "openai_chat returned multiple response messages",
            ));
        }
        Ok(assistant)
    }

    fn provider_tool(&self, tool: &PhiToolDefinition) -> ProviderToolDefinition {
        ProviderToolDefinition::from_tool_definition(tool.clone())
    }
}

#[async_trait]
impl DynProvider for OpenAiCompatClient {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentResult<PhiModelResponse> {
        let provider_messages = self.provider_messages(&messages)?;
        let provider_tools = request
            .tools
            .iter()
            .map(|tool| self.provider_tool(tool))
            .collect::<Vec<_>>();

        let mut extra = serde_json::Map::new();
        if request.enable_reasoning {
            extra.insert(
                "thinking_token_budget".to_string(),
                serde_json::json!(request.thinking_token_budget),
            );
            extra.insert(
                "reasoning_effort".to_string(),
                serde_json::json!(request.reasoning_effort.as_str()),
            );
        }

        let request = ChatCompletionRequest {
            model: request.model.clone(),
            messages: provider_messages,
            tools: if provider_tools.is_empty() {
                None
            } else {
                Some(provider_tools)
            },
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            extra,
        };

        let response = self
            .http
            .post(format!(
                "{}/chat/completions",
                self.config.api_base.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.api_key)
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                PhiAgentRuntimeError::provider_request(format!(
                    "openai_chat request failed: {error}"
                ))
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("<failed to read error body: {error}>"));
            return Err(super::http_error("openai_chat", status, body));
        }
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            PhiAgentRuntimeError::provider_response(format!(
                "openai_chat response body read failed (HTTP {status}): {error}"
            ))
        })?;
        let response = serde_json::from_str::<ChatCompletionResponse>(&body).map_err(|error| {
            PhiAgentRuntimeError::provider_response(format!(
                "openai_chat response decode failed (HTTP {status}): {error}; response body: {body}"
            ))
        })?;

        let choice = response.choices.into_iter().next().ok_or_else(|| {
            PhiAgentRuntimeError::provider_response("openai_chat provider returned no choices")
        })?;
        let assistant = self.phi_assistant(
            if choice.message.has_content() || !choice.message.tool_calls.is_empty() {
                vec![ProviderMessage::Assistant(choice.message)]
            } else {
                vec![]
            },
        )?;
        Ok(PhiModelResponse::from_assistant(
            assistant,
            PhiModelTurnState::Unspecified,
        ))
    }
}

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ProviderMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ProviderToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    max_tokens: u64,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ProviderMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant(ProviderAssistantMessage),
    Tool {
        content: String,
        tool_call_id: String,
    },
}

impl ProviderMessage {
    fn from_single_phi_message(
        message: &PhiMessage,
        reasoning_format: ReasoningFormat,
    ) -> Vec<Self> {
        match message {
            PhiMessage::System(content) => vec![Self::System {
                content: content.clone(),
            }],
            PhiMessage::User(PhiUserMessage::Text(text)) => vec![Self::User {
                content: text.clone(),
            }],
            PhiMessage::Assistant(assistant) => {
                let parts = assistant
                    .reasoning
                    .iter()
                    .flat_map(|block| block.content.iter())
                    .filter_map(PhiReasoningContent::display_text)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                vec![Self::Assistant(ProviderAssistantMessage {
                    content: assistant.content.clone(),
                    tool_calls: assistant
                        .tool_calls
                        .iter()
                        .map(|call| ProviderToolCall {
                            id: call.id.clone(),
                            call_id: call.call_id.clone(),
                            kind: "function".to_string(),
                            function: ProviderToolFunction {
                                name: call.name.clone(),
                                arguments: serde_json::to_string(&call.arguments)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            },
                        })
                        .collect(),
                    reasoning_content: reasoning_value(
                        &parts,
                        reasoning_format,
                        ReasoningFormat::PhiReasoningContent,
                    ),
                    reasoning: reasoning_value(
                        &parts,
                        reasoning_format,
                        ReasoningFormat::Reasoning,
                    ),
                })]
            }
            PhiMessage::ToolResult(result) => vec![Self::Tool {
                content: super::tool_result_text(&result.result),
                tool_call_id: result.id.clone().unwrap_or_default(),
            }],
        }
    }

    fn from_phi_messages(
        messages: &PhiRenderedMessages,
        reasoning_format: ReasoningFormat,
    ) -> PhiAgentResult<Vec<Self>> {
        Ok(messages
            .iter()
            .flat_map(|message| Self::from_single_phi_message(message, reasoning_format))
            .collect())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct ProviderAssistantMessage {
    #[serde(serialize_with = "serialize_optional_content")]
    pub content: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tool_calls: Vec<ProviderToolCall>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "reasoning_content")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn serialize_optional_content<S>(content: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(content.as_deref().unwrap_or_default())
}

impl ProviderAssistantMessage {
    fn has_content(&self) -> bool {
        self.content
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            || self
                .reasoning_content
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self
                .reasoning
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    }

    fn into_phi_assistant(self) -> PhiAgentResult<Option<PhiAssistantMessage>> {
        let provider_context = ReasoningFormat::from_response(&self).map(ReasoningFormat::context);
        let reasoning_text = self
            .reasoning_content
            .or(self.reasoning)
            .filter(|value| !value.is_empty());
        let mut reasoning = Vec::new();
        if let Some(reasoning_text) = reasoning_text {
            reasoning.push(PhiReasoningBlock {
                id: None,
                content: vec![PhiReasoningContent::Text {
                    text: reasoning_text,
                    signature: None,
                }],
            });
        }
        let mut tool_calls = Vec::new();
        for tool_call in self.tool_calls {
            let raw_arguments = tool_call.function.arguments.clone();
            let parsed_arguments = serde_json::from_str(&raw_arguments).map_err(|error| {
                PhiAgentRuntimeError::provider_response(format!(
                    "openai_chat invalid tool-call arguments JSON: {} | raw={}",
                    error, raw_arguments
                ))
            })?;
            tool_calls.push(crate::executor::ToolCallRequest {
                id: tool_call.id,
                call_id: tool_call.call_id,
                name: tool_call.function.name,
                arguments: parsed_arguments,
            });
        }
        let assistant = PhiRenderedMessages::provider_assistant(
            self.content.filter(|value| !value.is_empty()),
            reasoning,
            tool_calls,
            provider_context,
        );
        Ok((!assistant.is_empty()).then_some(assistant))
    }

    #[cfg(test)]
    fn into_phi_messages(self) -> PhiAgentResult<Vec<PhiMessage>> {
        Ok(self
            .into_phi_assistant()?
            .map(PhiMessage::Assistant)
            .into_iter()
            .collect())
    }
}

#[derive(Clone, Serialize, Eq, PartialEq)]
pub struct ProviderToolDefinition {
    #[serde(rename = "type")]
    kind: String,
    function: PhiToolDefinition,
}

impl ProviderToolDefinition {
    fn from_tool_definition(function: PhiToolDefinition) -> Self {
        Self {
            kind: "function".to_string(),
            function,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Clone, Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ProviderAssistantMessage,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ProviderToolCall {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    call_id: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    function: ProviderToolFunction,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ProviderToolFunction {
    name: String,
    arguments: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReasoningFormat {
    PhiReasoningContent,
    Reasoning,
}

impl ReasoningFormat {
    fn from_response(message: &ProviderAssistantMessage) -> Option<Self> {
        if message
            .reasoning_content
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            Some(Self::PhiReasoningContent)
        } else if message
            .reasoning
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            Some(Self::Reasoning)
        } else {
            None
        }
    }

    fn from_provider_context(context: Option<&serde_json::Value>) -> Self {
        match context.and_then(|value| {
            value
                .pointer("/openai_chat/reasoning_key")
                .and_then(serde_json::Value::as_str)
        }) {
            Some("reasoning") => Self::Reasoning,
            _ => Self::PhiReasoningContent,
        }
    }

    fn context(self) -> serde_json::Value {
        let reasoning_key = match self {
            Self::PhiReasoningContent => "reasoning_content",
            Self::Reasoning => "reasoning",
        };
        serde_json::json!({
            "openai_chat": {
                "reasoning_key": reasoning_key,
            }
        })
    }
}

fn reasoning_value(
    parts: &[String],
    current: ReasoningFormat,
    target: ReasoningFormat,
) -> Option<String> {
    if current != target {
        return None;
    }

    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ToolCallRequest;
    use crate::{config::ProviderConfig, error::PhiAgentRuntimeError, executor::PhiToolDefinition};

    fn test_config() -> ProviderConfig {
        ProviderConfig {
            kind: "openai_chat".to_string(),
            api_base: "https://example.test/v1".to_string(),
            api_key: "test-key".to_string(),
            fake_profile: "assistant_text".to_string(),
        }
    }

    #[test]
    fn phi_messages_map_to_openai_chat_roles_as_expected() {
        let client = OpenAiCompatClient::new(test_config());
        let tool = PhiToolDefinition {
            name: "bash".to_string(),
            description: "run shell commands".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"]
            }),
        };

        let mapped = vec![
            ProviderMessage::from_single_phi_message(
                &PhiMessage::system("sys"),
                ReasoningFormat::PhiReasoningContent,
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::user("hello"),
                ReasoningFormat::PhiReasoningContent,
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::user("compressed hello"),
                ReasoningFormat::PhiReasoningContent,
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::reasoning(
                    None,
                    vec![
                        PhiReasoningContent::Summary("plan".to_string()),
                        PhiReasoningContent::Text {
                            text: "detail".to_string(),
                            signature: None,
                        },
                    ],
                ),
                ReasoningFormat::PhiReasoningContent,
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::assistant("answer"),
                ReasoningFormat::PhiReasoningContent,
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::tool_call(
                    Some("call_1".to_string()),
                    "bash",
                    serde_json::json!({ "command": "pwd" }),
                ),
                ReasoningFormat::PhiReasoningContent,
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::tool_result(
                    Some("call_1".to_string()),
                    Some("bash".to_string()),
                    serde_json::json!({"stdout": "ok"}),
                ),
                ReasoningFormat::PhiReasoningContent,
            ),
        ];

        assert_eq!(
            mapped,
            vec![
                vec![ProviderMessage::System {
                    content: "sys".to_string(),
                }],
                vec![ProviderMessage::User {
                    content: "hello".to_string(),
                }],
                vec![ProviderMessage::User {
                    content: "compressed hello".to_string(),
                }],
                vec![ProviderMessage::Assistant(ProviderAssistantMessage {
                    content: None,
                    tool_calls: Vec::new(),
                    reasoning_content: Some("plan\ndetail".to_string()),
                    reasoning: None,
                })],
                vec![ProviderMessage::Assistant(ProviderAssistantMessage {
                    content: Some("answer".to_string()),
                    tool_calls: Vec::new(),
                    reasoning_content: Some(String::new()),
                    reasoning: None,
                })],
                vec![ProviderMessage::Assistant(ProviderAssistantMessage {
                    content: None,
                    tool_calls: vec![ProviderToolCall {
                        id: "call_1".to_string(),
                        call_id: Some("call_1".to_string()),
                        kind: "function".to_string(),
                        function: ProviderToolFunction {
                            name: "bash".to_string(),
                            arguments: serde_json::json!({ "command": "pwd" }).to_string(),
                        },
                    }],
                    reasoning_content: Some(String::new()),
                    reasoning: None,
                })],
                vec![ProviderMessage::Tool {
                    content: serde_json::json!({"stdout": "ok"}).to_string(),
                    tool_call_id: "call_1".to_string(),
                }],
            ]
        );

        assert_eq!(
            client.provider_tool(&tool).kind,
            "function",
            "tool definitions should use OpenAI/vLLM function tool shape",
        );
    }

    #[test]
    fn phi_message_sequence_groups_into_single_assistant_provider_message() {
        let client = OpenAiCompatClient::new(test_config());

        let mapped = client
            .provider_messages(&crate::render::test_rendered_messages(vec![
                PhiMessage::user("hello"),
                PhiMessage::user("compressed"),
                PhiMessage::Assistant(PhiAssistantMessage::from_parts(
                    Some("answer".to_string()),
                    vec![PhiReasoningBlock {
                        id: None,
                        content: vec![PhiReasoningContent::Summary("plan".to_string())],
                    }],
                    vec![ToolCallRequest {
                        id: "call_1".to_string(),
                        call_id: Some("call_1".to_string()),
                        name: "bash".to_string(),
                        arguments: serde_json::json!({ "command": "pwd" }),
                    }],
                )),
            ]))
            .expect("phi message sequence should group");

        assert_eq!(
            mapped,
            vec![
                ProviderMessage::User {
                    content: "hello".to_string(),
                },
                ProviderMessage::User {
                    content: "compressed".to_string(),
                },
                ProviderMessage::Assistant(ProviderAssistantMessage {
                    content: Some("answer".to_string()),
                    tool_calls: vec![ProviderToolCall {
                        id: "call_1".to_string(),
                        call_id: Some("call_1".to_string()),
                        kind: "function".to_string(),
                        function: ProviderToolFunction {
                            name: "bash".to_string(),
                            arguments: serde_json::json!({ "command": "pwd" }).to_string(),
                        },
                    }],
                    reasoning_content: Some("plan".to_string()),
                    reasoning: None,
                }),
            ]
        );
    }

    #[test]
    fn provider_assistant_message_preserves_reasoning_text_and_tool_call_order() {
        let messages = ProviderAssistantMessage {
            content: Some("final answer".to_string()),
            tool_calls: vec![
                ProviderToolCall {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    kind: "function".to_string(),
                    function: ProviderToolFunction {
                        name: "bash".to_string(),
                        arguments: r#"{"command":"pwd"}"#.to_string(),
                    },
                },
                ProviderToolCall {
                    id: "call_2".to_string(),
                    call_id: Some("call_2".to_string()),
                    kind: "function".to_string(),
                    function: ProviderToolFunction {
                        name: "bash".to_string(),
                        arguments: r#"{"command":"ls"}"#.to_string(),
                    },
                },
            ],
            reasoning_content: Some("think first".to_string()),
            reasoning: None,
        }
        .into_phi_messages()
        .expect("assistant payload should convert");

        assert_eq!(
            messages,
            vec![PhiMessage::Assistant(
                PhiRenderedMessages::provider_assistant(
                    Some("final answer".to_string()),
                    vec![PhiReasoningBlock {
                        id: None,
                        content: vec![PhiReasoningContent::Text {
                            text: "think first".to_string(),
                            signature: None,
                        }],
                    }],
                    vec![
                        ToolCallRequest {
                            id: "call_1".to_string(),
                            call_id: Some("call_1".to_string()),
                            name: "bash".to_string(),
                            arguments: serde_json::json!({ "command": "pwd" }),
                        },
                        ToolCallRequest {
                            id: "call_2".to_string(),
                            call_id: Some("call_2".to_string()),
                            name: "bash".to_string(),
                            arguments: serde_json::json!({ "command": "ls" }),
                        },
                    ],
                    Some(ReasoningFormat::PhiReasoningContent.context()),
                )
            )]
        );
    }

    #[test]
    fn grouped_openai_chat_assistant_round_trips_back_to_phi_sequence() {
        let client = OpenAiCompatClient::new(test_config());

        let source = vec![
            PhiMessage::user("hello"),
            PhiMessage::Assistant(PhiAssistantMessage::from_parts(
                Some("answer".to_string()),
                vec![PhiReasoningBlock {
                    id: None,
                    content: vec![PhiReasoningContent::Summary("plan".to_string())],
                }],
                vec![ToolCallRequest {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    name: "bash".to_string(),
                    arguments: serde_json::json!({ "command": "pwd" }),
                }],
            )),
            PhiMessage::tool_result(
                Some("call_1".to_string()),
                Some("bash".to_string()),
                serde_json::json!({"stdout": "ok"}),
            ),
        ];

        let provider_messages = client
            .provider_messages(&crate::render::test_rendered_messages(source.clone()))
            .expect("phi sequence should convert");
        let response = provider_messages
            .into_iter()
            .find(|message| matches!(message, ProviderMessage::Assistant(_)))
            .expect("provider history should contain an assistant turn");
        let round_tripped = client
            .phi_assistant(vec![response])
            .expect("provider assistant should convert back")
            .map(PhiMessage::Assistant)
            .into_iter()
            .collect::<Vec<_>>();

        assert_eq!(
            round_tripped,
            vec![PhiMessage::Assistant(
                PhiRenderedMessages::provider_assistant(
                    Some("answer".to_string()),
                    vec![PhiReasoningBlock {
                        id: None,
                        content: vec![PhiReasoningContent::Text {
                            text: "plan".to_string(),
                            signature: None,
                        }],
                    }],
                    vec![ToolCallRequest {
                        id: "call_1".to_string(),
                        call_id: Some("call_1".to_string()),
                        name: "bash".to_string(),
                        arguments: serde_json::json!({ "command": "pwd" }),
                    }],
                    Some(ReasoningFormat::PhiReasoningContent.context()),
                )
            )],
            "openai_chat grouping should preserve assistant reasoning/content/tool-call order",
        );
    }

    #[test]
    fn provider_assistant_message_accepts_reasoning_fallback_field() {
        let messages = ProviderAssistantMessage {
            content: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
            reasoning: Some("legacy reasoning".to_string()),
        }
        .into_phi_messages()
        .expect("reasoning field should convert");

        assert_eq!(
            messages,
            vec![PhiMessage::Assistant(
                PhiRenderedMessages::provider_assistant(
                    None,
                    vec![PhiReasoningBlock {
                        id: None,
                        content: vec![PhiReasoningContent::Text {
                            text: "legacy reasoning".to_string(),
                            signature: None,
                        }],
                    }],
                    Vec::new(),
                    Some(ReasoningFormat::Reasoning.context()),
                )
            )]
        );
    }

    #[test]
    fn provider_assistant_message_rejects_invalid_tool_call_json() {
        let error = ProviderAssistantMessage {
            content: None,
            tool_calls: vec![ProviderToolCall {
                id: "call_bad".to_string(),
                call_id: Some("call_bad".to_string()),
                kind: "function".to_string(),
                function: ProviderToolFunction {
                    name: "bash".to_string(),
                    arguments: "{not-json}".to_string(),
                },
            }],
            reasoning_content: None,
            reasoning: None,
        }
        .into_phi_messages()
        .expect_err("invalid tool call JSON should fail");

        assert!(matches!(
            error,
            PhiAgentRuntimeError::ProviderResponse { .. }
        ));
        assert!(
            error.detail().contains("invalid tool-call arguments JSON"),
            "unexpected error detail: {}",
            error.detail()
        );
    }

    #[test]
    fn provider_assistant_message_accepts_null_tool_calls() {
        let message: ProviderAssistantMessage = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": "Codex is ready",
            "reasoning_content": "started",
            "tool_calls": null
        }))
        .expect("null tool_calls should be treated as no tool calls");

        assert_eq!(message.content.as_deref(), Some("Codex is ready"));
        assert_eq!(message.reasoning_content.as_deref(), Some("started"));
        assert!(message.tool_calls.is_empty());
    }

    #[test]
    fn tool_results_use_plain_text_for_strings_and_json_for_structures() {
        let mapped = [
            PhiMessage::tool_result(
                Some("call_text".to_string()),
                Some("tool".to_string()),
                serde_json::Value::String("done".to_string()),
            ),
            PhiMessage::tool_result(
                Some("call_json".to_string()),
                Some("tool".to_string()),
                serde_json::json!({"status": "done"}),
            ),
        ]
        .iter()
        .flat_map(|message| {
            ProviderMessage::from_single_phi_message(message, ReasoningFormat::PhiReasoningContent)
        })
        .collect::<Vec<_>>();

        assert_eq!(
            mapped,
            vec![
                ProviderMessage::Tool {
                    content: "done".to_string(),
                    tool_call_id: "call_text".to_string(),
                },
                ProviderMessage::Tool {
                    content: serde_json::json!({"status": "done"}).to_string(),
                    tool_call_id: "call_json".to_string(),
                },
            ]
        );
    }

    #[test]
    fn latest_provider_context_switches_outbound_field_name() {
        let client = OpenAiCompatClient::new(test_config());
        let prior = ProviderAssistantMessage {
            content: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
            reasoning: Some("server uses reasoning".to_string()),
        }
        .into_phi_assistant()
        .expect("provider assistant should convert")
        .expect("provider assistant should not be empty");

        let mapped = client
            .provider_messages(&crate::render::test_rendered_messages(vec![
                PhiMessage::Assistant(prior),
                PhiMessage::user("continue"),
                PhiMessage::reasoning(
                    None,
                    vec![PhiReasoningContent::Summary("mirror back".to_string())],
                ),
            ]))
            .expect("reasoning message should map");

        assert_eq!(
            mapped,
            vec![
                ProviderMessage::Assistant(ProviderAssistantMessage {
                    content: None,
                    tool_calls: Vec::new(),
                    reasoning_content: None,
                    reasoning: Some("server uses reasoning".to_string()),
                }),
                ProviderMessage::User {
                    content: "continue".to_string(),
                },
                ProviderMessage::Assistant(ProviderAssistantMessage {
                    content: None,
                    tool_calls: Vec::new(),
                    reasoning_content: None,
                    reasoning: Some("mirror back".to_string()),
                }),
            ]
        );
    }

    #[test]
    fn provider_context_survives_serialization_and_skips_messages_without_context() {
        let prior = ProviderAssistantMessage {
            content: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
            reasoning: Some("server uses reasoning".to_string()),
        }
        .into_phi_assistant()
        .expect("provider assistant should convert")
        .expect("provider assistant should not be empty");
        let serialized = serde_json::to_string(&PhiMessage::Assistant(prior))
            .expect("Phi assistant should serialize");
        let restored: PhiMessage =
            serde_json::from_str(&serialized).expect("Phi assistant should deserialize");
        let messages = crate::render::test_rendered_messages(vec![
            restored,
            PhiMessage::assistant("later response without provider context"),
            PhiMessage::reasoning(
                None,
                vec![PhiReasoningContent::Summary(
                    "continue reasoning".to_string(),
                )],
            ),
        ]);

        let mapped = OpenAiCompatClient::new(test_config())
            .provider_messages(&messages)
            .expect("restored history should map");

        assert!(mapped.iter().all(|message| match message {
            ProviderMessage::Assistant(message) => message.reasoning_content.is_none(),
            _ => true,
        }));
        assert!(mapped.iter().any(|message| matches!(
            message,
            ProviderMessage::Assistant(ProviderAssistantMessage {
                reasoning: Some(reasoning),
                ..
            }) if reasoning == "continue reasoning"
        )));
    }

    #[test]
    fn assistant_without_content_serializes_empty_string() {
        let json = serde_json::to_value(ProviderMessage::Assistant(ProviderAssistantMessage {
            content: None,
            tool_calls: Vec::new(),
            reasoning_content: Some("plan".to_string()),
            reasoning: None,
        }))
        .expect("assistant should serialize");

        assert_eq!(json["content"], "");
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("reasoning").is_none());
        assert!(
            !json
                .as_object()
                .expect("assistant should serialize as an object")
                .values()
                .any(serde_json::Value::is_null)
        );
    }

    #[test]
    fn assistant_without_reasoning_serializes_empty_current_reasoning_field() {
        let messages = [PhiMessage::assistant("answer"), PhiMessage::tool_call(
            Some("call_1".to_string()),
            "lookup",
            serde_json::json!({}),
        )];

        for message in &messages {
            let mapped = ProviderMessage::from_single_phi_message(
                message,
                ReasoningFormat::PhiReasoningContent,
            );
            let json = serde_json::to_value(&mapped[0]).expect("assistant should serialize");
            assert_eq!(json["reasoning_content"], "");
            assert!(json.get("reasoning").is_none());
        }
    }

    #[test]
    fn missing_provider_context_defaults_to_reasoning_content() {
        let mapped = OpenAiCompatClient::new(test_config())
            .provider_messages(&crate::render::test_rendered_messages(vec![
                PhiMessage::reasoning(
                    None,
                    vec![PhiReasoningContent::Summary("default format".to_string())],
                ),
            ]))
            .expect("history without provider context should map");

        let json = serde_json::to_value(mapped).expect("provider messages should serialize");
        assert_eq!(json[0]["content"], "");
        assert!(json[0].get("tool_calls").is_none());
        assert_eq!(json[0]["reasoning_content"], "default format");
        assert!(json[0].get("reasoning").is_none());
        assert!(
            !json[0]
                .as_object()
                .unwrap()
                .values()
                .any(serde_json::Value::is_null)
        );
    }
}
