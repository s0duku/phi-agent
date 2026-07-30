use std::{future, sync::Arc};

use async_trait::async_trait;
use tokio::sync::Notify;

use crate::{
    error::PhiAgentRuntimeResult,
    message::{PhiHistory, PhiMessage},
    render::{PhiModelResponse, PhiProviderCall, TestClient},
    session::{PhiAgentStep, Session},
    tests::support::{step_agent_builder, test_model_defaults},
};

struct PendingProvider {
    started: Arc<Notify>,
}

#[async_trait]
impl TestClient for PendingProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        self.started.notify_one();
        future::pending().await
    }
}

#[tokio::test]
async fn interrupted_step_leaves_the_pre_call_session_checkpoint_serializable() {
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("keep me")],
    );
    let started = Arc::new(Notify::new());
    let mut agent = step_agent_builder(session)
        .with_client(Arc::new(PendingProvider {
            started: Arc::clone(&started),
        }))
        .build()
        .unwrap();
    let checkpoint = agent.session();
    let interrupt = async move {
        started.notified().await;
        Ok(())
    };

    assert!(
        crate::step_or_interrupt(&mut agent, interrupt)
            .await
            .unwrap()
    );

    let mut serialized = Vec::new();
    checkpoint.write_json(&mut serialized).unwrap();
    let restored = Session::load_bytes(&serialized).unwrap();
    assert_eq!(restored.history(), checkpoint.history());
    assert_eq!(restored.step(), checkpoint.step());
}
