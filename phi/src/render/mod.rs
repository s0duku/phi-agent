mod compact;
mod provider;
mod utils;

use std::sync::Arc;

use crate::{
    config::ProviderConfig,
    error::PhiAgentRuntimeResult,
    executor::ToolCallRequest,
    message::{PhiAssistantMessage, PhiHistory, PhiMessage, PhiReasoningBlock},
};

use provider::DynProvider;
pub(crate) use provider::{PhiModelResponse, PhiModelTurnState, PhiProviderCall};
pub(crate) use utils::{
    approx_history_token_count, approx_message_token_count, approx_text_token_count,
};

pub(crate) fn compact_prompt_token_count() -> usize {
    compact::compact_prompt_token_count()
}

#[derive(Clone, Debug)]
pub(in crate::render) struct PhiRenderedMessages {
    messages: Vec<Arc<PhiMessage>>,
    provider_context: Option<serde_json::Value>,
}

impl PhiRenderedMessages {
    pub(crate) fn from_history(history: PhiHistory) -> Self {
        let provider_context = history.latest_provider_context();
        Self {
            messages: history.into_arcs(),
            provider_context,
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &PhiMessage> {
        self.messages.iter().map(|message| message.as_ref())
    }

    pub(crate) fn iter_rev(&self) -> impl Iterator<Item = &PhiMessage> {
        self.messages.iter().rev().map(|message| message.as_ref())
    }

    pub(in crate::render) fn provider_context(&self) -> Option<&serde_json::Value> {
        self.provider_context.as_ref()
    }

    pub(in crate::render) fn provider_assistant(
        content: Option<String>,
        reasoning: Vec<PhiReasoningBlock>,
        tool_calls: Vec<ToolCallRequest>,
        provider_context: Option<serde_json::Value>,
    ) -> PhiAssistantMessage {
        PhiAssistantMessage::from_provider_parts(content, reasoning, tool_calls, provider_context)
    }
}

pub(crate) struct PhiRender {
    provider: Arc<dyn DynProvider>,
    #[cfg(test)]
    compact_override:
        Option<Arc<dyn Fn(&PhiHistory) -> PhiAgentRuntimeResult<PhiHistory> + Send + Sync>>,
}

impl PhiRender {
    fn new(provider: Arc<dyn DynProvider>) -> Self {
        Self {
            provider,
            #[cfg(test)]
            compact_override: None,
        }
    }

    pub(crate) async fn complete(
        &self,
        request: &PhiProviderCall,
        history: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        self.complete_rendered(request, self.render_messages(history))
            .await
    }

    pub(crate) fn provider_history_token_count(&self, history: &PhiHistory) -> usize {
        approx_history_token_count(history)
    }

    fn render_messages(&self, history: &PhiHistory) -> PhiRenderedMessages {
        PhiRenderedMessages::from_history(history.clone())
    }

    pub(in crate::render) async fn complete_rendered(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        self.provider.complete(request, messages).await
    }

    pub(crate) async fn compact(
        &self,
        request: PhiProviderCall,
        history: PhiHistory,
        retain_rate: f32,
    ) -> PhiAgentRuntimeResult<PhiHistory> {
        #[cfg(test)]
        if let Some(compact_override) = &self.compact_override {
            return compact_override(&history);
        }

        compact::compact_history(self, &request, &history, retain_rate).await
    }
}

pub(crate) fn build(config: ProviderConfig) -> PhiAgentRuntimeResult<PhiRender> {
    Ok(PhiRender::new(Arc::from(provider::build_provider(config)?)))
}

#[cfg(test)]
#[async_trait::async_trait]
pub(crate) trait TestClient: Send + Sync {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse>;
}

#[cfg(test)]
struct TestClientAdapter {
    client: Arc<dyn TestClient>,
}

#[cfg(test)]
#[async_trait::async_trait]
impl DynProvider for TestClientAdapter {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        let history = PhiHistory::from_messages(messages.iter().cloned().collect());
        self.client.complete(request, &history).await
    }
}

impl PhiRender {
    #[cfg(test)]
    pub(crate) fn from_test_client(client: Arc<dyn TestClient>) -> Self {
        Self::new(Arc::new(TestClientAdapter { client }))
    }

    #[cfg(test)]
    pub(crate) fn with_compact_override(
        mut self,
        compact_override: Arc<
            dyn Fn(&PhiHistory) -> PhiAgentRuntimeResult<PhiHistory> + Send + Sync,
        >,
    ) -> Self {
        self.compact_override = Some(compact_override);
        self
    }
}

#[cfg(test)]
pub(in crate::render) fn test_rendered_messages(
    messages: impl Into<PhiHistory>,
) -> PhiRenderedMessages {
    PhiRenderedMessages::from_history(messages.into())
}
