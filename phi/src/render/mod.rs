mod compact;
mod provider;
mod template;
mod utils;

use std::sync::Arc;

use crate::{
    config::ProviderConfig,
    error::PhiRuntimeResult,
    home::PhiHome,
    message::{PhiHistory, PhiMessage},
};

use provider::DynProvider;
pub(crate) use provider::{PhiModelResponse, PhiModelTurnState, PhiProviderCall};
pub(crate) use utils::{approx_history_token_count, approx_text_token_count};

#[derive(Clone, Debug)]
pub(in crate::render) struct PhiRenderedMessages(Vec<Arc<PhiMessage>>);

fn render_messages(messages: Vec<Arc<PhiMessage>>) -> PhiRenderedMessages {
    PhiRenderedMessages(messages)
}

impl PhiRenderedMessages {
    pub(crate) fn from_history(history: PhiHistory) -> Self {
        render_messages(history.into_arcs())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &PhiMessage> {
        self.0.iter().map(|message| message.as_ref())
    }

    pub(crate) fn iter_rev(&self) -> impl Iterator<Item = &PhiMessage> {
        self.0.iter().rev().map(|message| message.as_ref())
    }

    pub(crate) fn to_history(&self) -> PhiHistory {
        PhiHistory::from_arcs(self.0.clone())
    }
}

pub(crate) struct PhiRender {
    provider: Arc<dyn DynProvider>,
    #[cfg(test)]
    compact_override:
        Option<Arc<dyn Fn(&PhiHistory) -> PhiRuntimeResult<PhiHistory> + Send + Sync>>,
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
        home: &dyn PhiHome,
        template: Option<&str>,
        request: &PhiProviderCall,
        history: &PhiHistory,
    ) -> PhiRuntimeResult<PhiModelResponse> {
        let rendered_messages = self.render_messages(history);
        let rendered_messages =
            template::render_template(home, template, request, &rendered_messages)?;
        self.complete_rendered(request, rendered_messages).await
    }

    pub(crate) fn provider_history_token_count(
        &self,
        home: &dyn PhiHome,
        template: Option<&str>,
        request: &PhiProviderCall,
        history: &PhiHistory,
    ) -> PhiRuntimeResult<usize> {
        let rendered_messages = self.render_messages(history);
        let rendered_messages =
            template::render_template(home, template, request, &rendered_messages)?;
        Ok(approx_history_token_count(&rendered_messages.to_history()))
    }

    fn render_messages(&self, history: &PhiHistory) -> PhiRenderedMessages {
        PhiRenderedMessages::from_history(history.clone())
    }

    pub(in crate::render) async fn complete_rendered(
        &self,
        request: &PhiProviderCall,
        messages: PhiRenderedMessages,
    ) -> PhiRuntimeResult<PhiModelResponse> {
        self.provider.complete(request, messages).await
    }

    pub(crate) async fn compact(
        &self,
        request: PhiProviderCall,
        history: PhiHistory,
    ) -> PhiRuntimeResult<PhiHistory> {
        #[cfg(test)]
        if let Some(compact_override) = &self.compact_override {
            return compact_override(&history);
        }

        compact::compact_history(self, &request, &history).await
    }
}

pub(crate) fn build(config: ProviderConfig) -> PhiRuntimeResult<PhiRender> {
    Ok(PhiRender::new(Arc::from(provider::build_provider(config)?)))
}

#[cfg(test)]
#[async_trait::async_trait]
pub(crate) trait TestClient: Send + Sync {
    async fn complete(
        &self,
        request: &PhiProviderCall,
        messages: &PhiHistory,
    ) -> PhiRuntimeResult<PhiModelResponse>;
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
    ) -> PhiRuntimeResult<PhiModelResponse> {
        self.client.complete(request, &messages.to_history()).await
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
        compact_override: Arc<dyn Fn(&PhiHistory) -> PhiRuntimeResult<PhiHistory> + Send + Sync>,
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

#[cfg(test)]
mod tests {
    use crate::message::PhiMessage;

    use super::test_rendered_messages;

    #[test]
    fn rendered_messages_preserve_history_shape() {
        let rendered = test_rendered_messages(vec![
            PhiMessage::system("sys"),
            PhiMessage::user("hello"),
            PhiMessage::assistant("world"),
        ]);

        assert_eq!(
            rendered.to_history().to_messages(),
            vec![
                PhiMessage::system("sys"),
                PhiMessage::user("hello"),
                PhiMessage::assistant("world"),
            ]
        );
    }
}
