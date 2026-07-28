use std::{
    fs,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;

use crate::{
    agent::PhiAgent,
    error::PhiRuntimeResult,
    home::LocalPhiHome,
    message::{PhiHistory, PhiMessage},
    render::{PhiProviderCall, TestClient},
    session::{PhiAgentStep, Session},
    tests::support::{env_lock, test_model_defaults},
};

struct CaptureProvider {
    seen_messages: Arc<Mutex<Vec<PhiMessage>>>,
    response: Vec<PhiMessage>,
}

#[async_trait]
impl TestClient for CaptureProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        messages: &PhiHistory,
    ) -> PhiRuntimeResult<Vec<PhiMessage>> {
        *self
            .seen_messages
            .lock()
            .expect("capture lock should be healthy") = messages.to_messages();
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn command_template_renders_transient_provider_messages_only() {
    let _lock = env_lock();
    let root = unique_temp_dir("phi-render-command-template");
    fs::create_dir_all(root.join("templates")).expect("template dir should be creatable");
    fs::write(
        root.join("templates").join("single_system.j2"),
        r#"<system>templated {{ messages | length }} {{ request.model }}</system>"#,
    )
    .expect("template should be writable");

    let seen_messages = Arc::new(Mutex::new(Vec::new()));
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );
    let command = crate::agent::PhiAgentCommand::Step(
        crate::agent::PhiAgentCommand::step().with_template(Some("single_system".to_string())),
    );

    let outcome = PhiAgent::builder(session, command)
        .with_home(Arc::new(LocalPhiHome::new(root.clone())))
        .with_model_defaults(test_model_defaults())
        .with_client(Arc::new(CaptureProvider {
            seen_messages: seen_messages.clone(),
            response: vec![PhiMessage::assistant("ok")],
        }))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        *seen_messages
            .lock()
            .expect("capture lock should be healthy"),
        vec![PhiMessage::system("templated 1 test-model")]
    );
    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("hello"), PhiMessage::assistant("ok")]
    );

    fs::remove_dir_all(root).expect("temp home should be removable");
}

#[tokio::test]
async fn phi_template_from_home_config_is_used_when_command_template_is_absent() {
    let _lock = env_lock();
    let root = unique_temp_dir("phi-render-home-template");
    fs::create_dir_all(root.join("templates")).expect("template dir should be creatable");
    fs::write(
        root.join("config.toml"),
        "PHI_TEMPLATE = \"single_message\"\n",
    )
    .expect("config should be writable");
    fs::write(
        root.join("templates").join("single_message.j2"),
        r#"<system>from-home-template</system>"#,
    )
    .expect("template should be writable");

    let seen_messages = Arc::new(Mutex::new(Vec::new()));
    let session = Session::from_root(
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );
    let command = crate::agent::PhiAgentCommand::Step(crate::agent::PhiAgentCommand::step());

    let outcome = PhiAgent::builder(session, command)
        .with_home(Arc::new(LocalPhiHome::new(root.clone())))
        .with_model_defaults(test_model_defaults())
        .with_client(Arc::new(CaptureProvider {
            seen_messages: seen_messages.clone(),
            response: vec![PhiMessage::assistant("ok")],
        }))
        .build()
        .expect("agent should build")
        .run_single_step()
        .await;

    assert_eq!(
        *seen_messages
            .lock()
            .expect("capture lock should be healthy"),
        vec![PhiMessage::system("from-home-template")]
    );
    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("hello"), PhiMessage::assistant("ok")]
    );

    fs::remove_dir_all(root).expect("temp home should be removable");
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nanos}"))
}
