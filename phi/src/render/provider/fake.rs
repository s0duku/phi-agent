use async_trait::async_trait;

use crate::{
    config::ProviderConfig,
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    message::{PhiAssistantMessage, PhiMessage, PhiReasoningBlock, PhiReasoningContent},
};

use super::{DynProvider, PhiModelResponse, PhiProviderCall};
use crate::render::PhiRenderedMessages;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FakeProfile {
    AssistantText,
    ReasoningText,
    ToolCall,
    ProviderError,
}

impl FakeProfile {
    fn parse(value: &str) -> PhiAgentRuntimeResult<Self> {
        match value.trim() {
            "assistant_text" => Ok(Self::AssistantText),
            "reasoning_text" => Ok(Self::ReasoningText),
            "tool_call" => Ok(Self::ToolCall),
            "provider_error" => Ok(Self::ProviderError),
            other => Err(PhiAgentRuntimeError::provider_request(format!(
                "unsupported fake provider profile: {other}"
            ))),
        }
    }
}

pub struct FakeClient {
    profile: FakeProfile,
}

impl FakeClient {
    pub fn new(config: ProviderConfig) -> PhiAgentRuntimeResult<Self> {
        Ok(Self {
            profile: FakeProfile::parse(&config.fake_profile)?,
        })
    }

    fn assistant_text(
        &self,
        _request: &PhiProviderCall,
        messages: &PhiRenderedMessages,
    ) -> PhiAssistantMessage {
        PhiAssistantMessage::text(format!(
            "fake assistant reply to: {}",
            last_user_text(messages).unwrap_or(""),
        ))
    }

    fn reasoning_text(
        &self,
        _request: &PhiProviderCall,
        messages: &PhiRenderedMessages,
    ) -> PhiAssistantMessage {
        PhiAssistantMessage::from_parts(
            Some(format!(
                "fake assistant reply to: {}",
                last_user_text(messages).unwrap_or(""),
            )),
            vec![PhiReasoningBlock {
                id: Some("fake-reasoning-1".to_string()),
                content: vec![PhiReasoningContent::Text {
                    text: format!(
                        "fake reasoning about: {}",
                        last_user_text(messages).unwrap_or("")
                    ),
                    signature: None,
                }],
            }],
            Vec::new(),
        )
    }

    fn tool_call(
        &self,
        request: &PhiProviderCall,
        messages: &PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiAssistantMessage> {
        if messages
            .iter()
            .any(|message| matches!(message, PhiMessage::ToolResult(_)))
        {
            return Ok(PhiAssistantMessage::text(
                "fake tool flow completed successfully",
            ));
        }

        let Some(tool) = request.tools.first() else {
            return Err(PhiAgentRuntimeError::provider_response(
                "fake tool_call profile requires at least one available tool",
            ));
        };

        let arguments = if tool.name == "bash_job" || tool.name == "powershell_job" {
            serde_json::json!({ "cmd": "printf fake-provider-tool" })
        } else {
            serde_json::json!({})
        };

        Ok(PhiAssistantMessage::tool_calls(vec![
            crate::executor::ToolCallRequest {
                id: "fake-call-1".to_string(),
                call_id: Some("fake-call-1".to_string()),
                name: tool.name.clone(),
                arguments,
            },
        ]))
    }
}

#[async_trait]
impl DynProvider for FakeClient {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        let assistant = match self.profile {
            FakeProfile::AssistantText => Ok(self.assistant_text(request, &messages)),
            FakeProfile::ReasoningText => Ok(self.reasoning_text(request, &messages)),
            FakeProfile::ToolCall => self.tool_call(request, &messages),
            FakeProfile::ProviderError => Err(PhiAgentRuntimeError::provider_request(
                "fake provider generated a configured request error",
            )),
        }?;
        Ok(PhiModelResponse::from(assistant))
    }
}

fn last_user_text(messages: &PhiRenderedMessages) -> Option<&str> {
    messages.iter_rev().find_map(|message| match message {
        PhiMessage::User(crate::message::PhiUserMessage::Text(text)) => Some(text.as_str()),
        _ => None,
    })
}
