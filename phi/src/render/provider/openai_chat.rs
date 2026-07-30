use std::{
    sync::{Mutex, OnceLock},
    vec,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{PhiModelResponse, PhiProvider, PhiProviderCall};
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
pub struct OpenAiCompatClient {
    http: reqwest::Client,
    config: ProviderConfig,
    reasoning_format: &'static OnceLock<Mutex<ReasoningFormat>>,
}

impl OpenAiCompatClient {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
            reasoning_format: reasoning_format_state(),
        }
    }

    fn current_reasoning_format(&self) -> PhiResult<ReasoningFormat> {
        let guard = self
            .reasoning_format
            .get_or_init(|| Mutex::new(ReasoningFormat::PhiReasoningContent))
            .lock()
            .map_err(|_| PhiRuntimeError::internal("reasoning format mutex was poisoned"))?;
        Ok(*guard)
    }

    fn observe_reasoning_format(&self, message: &ProviderAssistantMessage) -> PhiResult<()> {
        let next = if message
            .reasoning_content
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            Some(ReasoningFormat::PhiReasoningContent)
        } else if message
            .reasoning
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            Some(ReasoningFormat::Reasoning)
        } else {
            None
        };

        let Some(next) = next else {
            return Ok(());
        };

        let mut guard = self
            .reasoning_format
            .get_or_init(|| Mutex::new(ReasoningFormat::PhiReasoningContent))
            .lock()
            .map_err(|_| PhiRuntimeError::internal("reasoning format mutex was poisoned"))?;
        *guard = next;
        Ok(())
    }
}

#[async_trait]
impl PhiProvider for OpenAiCompatClient {
    type ProviderMessage = ProviderMessage;
    type ProviderTool = ProviderToolDefinition;

    fn provider_messages(
        &self,
        messages: &PhiRenderedMessages,
    ) -> PhiResult<Vec<Self::ProviderMessage>> {
        ProviderMessage::from_phi_messages(messages, self.current_reasoning_format()?)
    }

    fn phi_messages(&self, response: Vec<Self::ProviderMessage>) -> PhiResult<Vec<PhiMessage>> {
        response
            .into_iter()
            .map(|message| match message {
                ProviderMessage::System { .. }
                | ProviderMessage::User { .. }
                | ProviderMessage::Tool { .. } => Ok(vec![]),
                ProviderMessage::Assistant(message) => message.into_phi_messages(),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|messages| messages.into_iter().flatten().collect())
    }

    fn provider_tool(&self, tool: &PhiToolDefinition) -> Self::ProviderTool {
        ProviderToolDefinition::from_tool_definition(tool.clone())
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
                PhiRuntimeError::provider_request(format!("openai_chat request failed: {error}"))
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("<failed to read error body: {error}>"));
            return Err(PhiRuntimeError::provider_request(format!(
                "openai_chat HTTP {status}: {body}"
            )));
        }
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            PhiRuntimeError::provider_response(format!(
                "openai_chat response body read failed (HTTP {status}): {error}"
            ))
        })?;
        let response = serde_json::from_str::<ChatCompletionResponse>(&body).map_err(|error| {
            PhiRuntimeError::provider_response(format!(
                "openai_chat response decode failed (HTTP {status}): {error}; response body: {body}"
            ))
        })?;

        let choice = response.choices.into_iter().next().ok_or_else(|| {
            PhiRuntimeError::provider_response("openai_chat provider returned no choices")
        })?;
        self.observe_reasoning_format(&choice.message)?;

        let messages = self.phi_messages(
            if choice.message.has_content() || !choice.message.tool_calls.is_empty() {
                vec![ProviderMessage::Assistant(choice.message)]
            } else {
                vec![]
            },
        )?;
        Ok(PhiModelResponse::unspecified(messages))
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
            PhiMessage::Assistant(PhiAssistantMessage::Text(text)) => {
                vec![Self::Assistant(ProviderAssistantMessage {
                    content: Some(text.clone()),
                    tool_calls: Vec::new(),
                    reasoning_content: None,
                    reasoning: None,
                })]
            }
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning { content, .. }) => {
                let parts = content
                    .iter()
                    .filter_map(PhiReasoningContent::display_text)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                vec![Self::Assistant(ProviderAssistantMessage {
                    content: None,
                    tool_calls: Vec::new(),
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
            PhiMessage::Tool(PhiToolMessage::ToolCall {
                id,
                name,
                arguments,
            }) => vec![Self::Assistant(ProviderAssistantMessage {
                content: None,
                tool_calls: vec![ProviderToolCall {
                    id: id.clone().unwrap_or_else(|| name.clone()),
                    call_id: id.clone(),
                    kind: "function".to_string(),
                    function: ProviderToolFunction {
                        name: name.clone(),
                        arguments: serde_json::to_string(arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                }],
                reasoning_content: None,
                reasoning: None,
            })],
            PhiMessage::Tool(PhiToolMessage::ToolResult { id, result, .. }) => vec![Self::Tool {
                content: result.to_string(),
                tool_call_id: id.clone().unwrap_or_default(),
            }],
        }
    }

    fn from_phi_messages(
        messages: &PhiRenderedMessages,
        reasoning_format: ReasoningFormat,
    ) -> PhiResult<Vec<Self>> {
        let mut provider_messages = Vec::new();
        let mut assistant = ProviderAssistantMessage::default();

        for message in messages.iter() {
            match message {
                PhiMessage::System(_)
                | PhiMessage::User(_)
                | PhiMessage::Tool(PhiToolMessage::ToolResult { .. }) => {
                    if assistant.has_content() || !assistant.tool_calls.is_empty() {
                        provider_messages.push(Self::Assistant(std::mem::take(&mut assistant)));
                    }
                    provider_messages
                        .extend(Self::from_single_phi_message(message, reasoning_format));
                }
                PhiMessage::Assistant(PhiAssistantMessage::Text(text)) => {
                    assistant.push_text(text);
                }
                PhiMessage::Assistant(PhiAssistantMessage::Reasoning { content, .. }) => {
                    assistant.push_reasoning(content, reasoning_format);
                }
                PhiMessage::Tool(PhiToolMessage::ToolCall {
                    id,
                    name,
                    arguments,
                }) => {
                    assistant.tool_calls.push(ProviderToolCall {
                        id: id.clone().unwrap_or_else(|| name.clone()),
                        call_id: id.clone(),
                        kind: "function".to_string(),
                        function: ProviderToolFunction {
                            name: name.clone(),
                            arguments: serde_json::to_string(arguments)
                                .unwrap_or_else(|_| "{}".to_string()),
                        },
                    });
                }
            }
        }

        if assistant.has_content() || !assistant.tool_calls.is_empty() {
            provider_messages.push(Self::Assistant(assistant));
        }

        Ok(provider_messages)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct ProviderAssistantMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        default,
        deserialize_with = "deserialize_null_default"
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

    fn push_text(&mut self, text: &str) {
        match &mut self.content {
            Some(existing) if !existing.is_empty() => {
                existing.push('\n');
                existing.push_str(text);
            }
            Some(existing) => existing.push_str(text),
            None => self.content = Some(text.to_string()),
        }
    }

    fn push_reasoning(
        &mut self,
        content: &[PhiReasoningContent],
        reasoning_format: ReasoningFormat,
    ) {
        let parts = content
            .iter()
            .filter_map(PhiReasoningContent::display_text)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return;
        }
        let joined = parts.join("\n");
        match reasoning_format {
            ReasoningFormat::PhiReasoningContent => match &mut self.reasoning_content {
                Some(existing) if !existing.is_empty() => {
                    existing.push('\n');
                    existing.push_str(&joined);
                }
                Some(existing) => existing.push_str(&joined),
                None => self.reasoning_content = Some(joined),
            },
            ReasoningFormat::Reasoning => match &mut self.reasoning {
                Some(existing) if !existing.is_empty() => {
                    existing.push('\n');
                    existing.push_str(&joined);
                }
                Some(existing) => existing.push_str(&joined),
                None => self.reasoning = Some(joined),
            },
        }
    }

    fn into_phi_messages(self) -> PhiResult<Vec<PhiMessage>> {
        let mut messages = Vec::new();

        let reasoning = self
            .reasoning_content
            .or(self.reasoning)
            .filter(|value| !value.is_empty());

        if let Some(reasoning) = reasoning {
            messages.push(PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                id: None,
                content: vec![PhiReasoningContent::Text {
                    text: reasoning,
                    signature: None,
                }],
            }));
        }

        if let Some(text) = self.content.filter(|value| !value.is_empty()) {
            messages.push(PhiMessage::Assistant(PhiAssistantMessage::Text(text)));
        }

        for tool_call in self.tool_calls {
            let raw_arguments = tool_call.function.arguments.clone();
            let parsed_arguments = serde_json::from_str(&raw_arguments).map_err(|error| {
                PhiRuntimeError::provider_response(format!(
                    "openai_chat invalid tool-call arguments JSON: {} | raw={}",
                    error, raw_arguments
                ))
            })?;
            messages.push(PhiMessage::tool_call(
                tool_call.call_id.or(Some(tool_call.id)),
                tool_call.function.name,
                parsed_arguments,
            ));
        }

        Ok(messages)
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

fn reasoning_format_state() -> &'static OnceLock<Mutex<ReasoningFormat>> {
    static STATE: OnceLock<Mutex<ReasoningFormat>> = OnceLock::new();
    &STATE
}

fn reasoning_value(
    parts: &[String],
    current: ReasoningFormat,
    target: ReasoningFormat,
) -> Option<String> {
    if current != target || parts.is_empty() {
        return None;
    }

    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::ProviderConfig, error::PhiErrorKind, executor::PhiToolDefinition};
    use std::sync::MutexGuard;

    fn test_config() -> ProviderConfig {
        ProviderConfig {
            provider: "openai_chat".to_string(),
            api_base: "https://example.test/v1".to_string(),
            api_key: "test-key".to_string(),
            fake_profile: "assistant_text".to_string(),
        }
    }

    fn set_reasoning_format(format: ReasoningFormat) {
        let mut guard = reasoning_format_state()
            .get_or_init(|| Mutex::new(ReasoningFormat::PhiReasoningContent))
            .lock()
            .expect("reasoning format mutex should not be poisoned");
        *guard = format;
    }

    fn lock_reasoning_state() -> MutexGuard<'static, ()> {
        static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("test lock should not be poisoned")
    }

    #[test]
    fn phi_messages_map_to_openai_chat_roles_as_expected() {
        let _guard = lock_reasoning_state();
        set_reasoning_format(ReasoningFormat::PhiReasoningContent);
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
                client
                    .current_reasoning_format()
                    .expect("reasoning format should load"),
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::user("hello"),
                client
                    .current_reasoning_format()
                    .expect("reasoning format should load"),
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::user("compressed hello"),
                client
                    .current_reasoning_format()
                    .expect("reasoning format should load"),
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                    id: None,
                    content: vec![
                        PhiReasoningContent::Summary("plan".to_string()),
                        PhiReasoningContent::Text {
                            text: "detail".to_string(),
                            signature: None,
                        },
                    ],
                }),
                client
                    .current_reasoning_format()
                    .expect("reasoning format should load"),
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::assistant("answer"),
                client
                    .current_reasoning_format()
                    .expect("reasoning format should load"),
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::tool_call(
                    Some("call_1".to_string()),
                    "bash",
                    serde_json::json!({ "command": "pwd" }),
                ),
                client
                    .current_reasoning_format()
                    .expect("reasoning format should load"),
            ),
            ProviderMessage::from_single_phi_message(
                &PhiMessage::tool_result(
                    Some("call_1".to_string()),
                    Some("bash".to_string()),
                    serde_json::json!({"stdout": "ok"}),
                ),
                client
                    .current_reasoning_format()
                    .expect("reasoning format should load"),
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
                    reasoning_content: None,
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
                    reasoning_content: None,
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
        let _guard = lock_reasoning_state();
        set_reasoning_format(ReasoningFormat::PhiReasoningContent);
        let client = OpenAiCompatClient::new(test_config());

        let mapped = client
            .provider_messages(&crate::render::test_rendered_messages(vec![
                PhiMessage::user("hello"),
                PhiMessage::user("compressed"),
                PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                    id: None,
                    content: vec![PhiReasoningContent::Summary("plan".to_string())],
                }),
                PhiMessage::assistant("answer"),
                PhiMessage::tool_call(
                    Some("call_1".to_string()),
                    "bash",
                    serde_json::json!({ "command": "pwd" }),
                ),
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
        let _guard = lock_reasoning_state();
        set_reasoning_format(ReasoningFormat::PhiReasoningContent);
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
            vec![
                PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                    id: None,
                    content: vec![PhiReasoningContent::Text {
                        text: "think first".to_string(),
                        signature: None,
                    }],
                }),
                PhiMessage::Assistant(PhiAssistantMessage::Text("final answer".to_string())),
                PhiMessage::tool_call(
                    Some("call_1".to_string()),
                    "bash",
                    serde_json::json!({ "command": "pwd" }),
                ),
                PhiMessage::tool_call(
                    Some("call_2".to_string()),
                    "bash",
                    serde_json::json!({ "command": "ls" }),
                ),
            ]
        );
    }

    #[test]
    fn grouped_openai_chat_assistant_round_trips_back_to_phi_sequence() {
        let _guard = lock_reasoning_state();
        set_reasoning_format(ReasoningFormat::PhiReasoningContent);
        let client = OpenAiCompatClient::new(test_config());

        let source = vec![
            PhiMessage::user("hello"),
            PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                id: None,
                content: vec![PhiReasoningContent::Summary("plan".to_string())],
            }),
            PhiMessage::assistant("answer"),
            PhiMessage::tool_call(
                Some("call_1".to_string()),
                "bash",
                serde_json::json!({ "command": "pwd" }),
            ),
            PhiMessage::tool_result(
                Some("call_1".to_string()),
                Some("bash".to_string()),
                serde_json::json!({"stdout": "ok"}),
            ),
        ];

        let provider_messages = client
            .provider_messages(&crate::render::test_rendered_messages(source.clone()))
            .expect("phi sequence should convert");
        let round_tripped = client
            .phi_messages(provider_messages)
            .expect("provider messages should convert back");

        assert_eq!(
            round_tripped,
            vec![
                PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                    id: None,
                    content: vec![PhiReasoningContent::Text {
                        text: "plan".to_string(),
                        signature: None,
                    }],
                }),
                PhiMessage::assistant("answer"),
                PhiMessage::tool_call(
                    Some("call_1".to_string()),
                    "bash",
                    serde_json::json!({ "command": "pwd" }),
                ),
            ],
            "openai_chat grouping should preserve assistant reasoning/content/tool-call order",
        );
    }

    #[test]
    fn provider_assistant_message_accepts_reasoning_fallback_field() {
        let _guard = lock_reasoning_state();
        set_reasoning_format(ReasoningFormat::PhiReasoningContent);
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
            vec![PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                id: None,
                content: vec![PhiReasoningContent::Text {
                    text: "legacy reasoning".to_string(),
                    signature: None,
                }],
            })]
        );
    }

    #[test]
    fn provider_assistant_message_rejects_invalid_tool_call_json() {
        let _guard = lock_reasoning_state();
        set_reasoning_format(ReasoningFormat::PhiReasoningContent);
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

        assert_eq!(error.kind(), PhiErrorKind::ProviderResponse);
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
    fn reasoning_format_observation_switches_outbound_field_name() {
        let _guard = lock_reasoning_state();
        set_reasoning_format(ReasoningFormat::PhiReasoningContent);
        let client = OpenAiCompatClient::new(test_config());

        client
            .observe_reasoning_format(&ProviderAssistantMessage {
                content: None,
                tool_calls: Vec::new(),
                reasoning_content: None,
                reasoning: Some("server uses reasoning".to_string()),
            })
            .expect("reasoning format observation should succeed");

        let mapped = client
            .provider_messages(&crate::render::test_rendered_messages(vec![
                PhiMessage::Assistant(PhiAssistantMessage::Reasoning {
                    id: None,
                    content: vec![PhiReasoningContent::Summary("mirror back".to_string())],
                }),
            ]))
            .expect("reasoning message should map");

        assert_eq!(
            mapped,
            vec![ProviderMessage::Assistant(ProviderAssistantMessage {
                content: None,
                tool_calls: Vec::new(),
                reasoning_content: None,
                reasoning: Some("mirror back".to_string()),
            })]
        );
    }
}
