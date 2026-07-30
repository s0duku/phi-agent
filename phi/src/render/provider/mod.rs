pub(crate) mod fake;
pub(crate) mod openai_chat;
pub(crate) mod openai_response;

use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    config::{ModelRequestDefaults, ProviderConfig, ReasoningEffort},
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    executor::PhiToolDefinition,
    message::PhiMessage,
};

use self::{fake::FakeClient, openai_chat::OpenAiCompatClient, openai_response::ResponsesClient};
use super::PhiRenderedMessages;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PhiModelTurnState {
    Complete,
    Continue,
    #[default]
    Unspecified,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PhiModelResponse {
    pub(crate) messages: Vec<PhiMessage>,
    pub(crate) turn_state: PhiModelTurnState,
}

impl PhiModelResponse {
    pub(crate) fn unspecified(messages: Vec<PhiMessage>) -> Self {
        Self {
            messages,
            turn_state: PhiModelTurnState::Unspecified,
        }
    }

    pub(crate) fn new(messages: Vec<PhiMessage>, turn_state: PhiModelTurnState) -> Self {
        Self {
            messages,
            turn_state,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PhiProviderCall {
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<PhiToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    pub max_tokens: u64,
    pub enable_reasoning: bool,
    pub thinking_token_budget: u64,
    pub reasoning_effort: ReasoningEffort,
}

impl PhiProviderCall {
    pub fn from_parts(defaults: &ModelRequestDefaults, tools: Vec<PhiToolDefinition>) -> Self {
        Self {
            model: defaults.model.clone(),
            tools,
            temperature: defaults.temperature,
            max_tokens: defaults.max_tokens,
            enable_reasoning: defaults.enable_reasoning,
            thinking_token_budget: defaults.thinking_token_budget,
            reasoning_effort: defaults.reasoning_effort,
        }
    }
}

#[async_trait]
pub(in crate::render) trait PhiProvider: Send + Sync {
    type ProviderMessage: Clone + Send + Sync + Serialize + DeserializeOwned;
    type ProviderTool: Clone + Send + Sync + Serialize;

    // Provider conversion and requests run inside agent evaluation. Their
    // failures intentionally use PhiAgentRuntimeResult so retry/recovery policy
    // can persist and classify the failed provider step.
    fn provider_messages(
        &self,
        messages: &PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<Vec<Self::ProviderMessage>>;

    fn phi_messages(
        &self,
        response: Vec<Self::ProviderMessage>,
    ) -> PhiAgentRuntimeResult<Vec<PhiMessage>>;

    fn provider_tool(&self, tool: &PhiToolDefinition) -> Self::ProviderTool;

    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: &PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse>;
}

#[async_trait]
pub(in crate::render) trait DynProvider: Send + Sync {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse>;
}

#[async_trait]
impl<T> DynProvider for T
where
    T: PhiProvider,
{
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        PhiProvider::complete(self, request, &messages).await
    }
}

pub(in crate::render) fn build_provider(
    config: ProviderConfig,
) -> PhiAgentRuntimeResult<Box<dyn DynProvider>> {
    if config.provider.trim() != "fake" && config.api_key.trim().is_empty() {
        eprintln!(
            "phi provider: PHI_KEY is not configured; model requests will likely fail until an API key is set"
        );
    }

    match config.provider.trim() {
        "fake" => Ok(Box::new(FakeClient::new(config)?)),
        "openai_chat" => Ok(Box::new(OpenAiCompatClient::new(config))),
        "openai_response" => Ok(Box::new(ResponsesClient::new(config))),
        other => Err(PhiAgentRuntimeError::provider_request(format!(
            "unsupported provider: {other}"
        ))),
    }
}
