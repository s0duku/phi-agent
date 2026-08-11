use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::{
    error::PhiAgentRuntimeResult,
    home::LocalPhiHome,
    message::{PhiHistory, PhiMessage},
    render::{PhiModelResponse, PhiProviderCall, TestClient},
    session::{PhiAgentStep, Session},
    tests::support::{env_lock, isolated_step_agent_builder, test_model_defaults},
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
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        *self
            .seen_messages
            .lock()
            .expect("capture lock should be healthy") = messages.to_messages();
        Ok(PhiModelResponse::unspecified(self.response.clone()))
    }
}

#[tokio::test]
async fn phi_system_is_committed_when_session_is_created() {
    let _lock = env_lock();
    let previous = std::env::var_os("PHI_SYSTEM");
    unsafe {
        std::env::set_var("PHI_SYSTEM", "You are the bootstrap system prompt.");
    }

    let session = crate::new_session(&LocalPhiHome::new(crate::tests::support::unique_test_home()))
        .expect("session should initialize from home config");
    assert_eq!(
        session.history(),
        &[PhiMessage::system("You are the bootstrap system prompt.")]
    );

    let seen_messages = Arc::new(Mutex::new(Vec::new()));
    let mut builder = isolated_step_agent_builder(session)
        .prepare()
        .expect("builder should prepare successfully");
    let init_modules = crate::features::build_init_modules(builder.context());
    builder = builder.with_module_layout(init_modules);
    let outcome = builder
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
        vec![PhiMessage::system("You are the bootstrap system prompt.")]
    );
    assert_eq!(
        outcome.session.history(),
        &[
            PhiMessage::system("You are the bootstrap system prompt."),
            PhiMessage::assistant("ok"),
        ]
    );

    unsafe {
        restore_env("PHI_SYSTEM", previous);
    }
}

#[tokio::test]
async fn phi_system_does_not_prepend_existing_history() {
    let _lock = env_lock();
    let previous = std::env::var_os("PHI_SYSTEM");
    unsafe {
        std::env::set_var("PHI_SYSTEM", "You are the bootstrap system prompt.");
    }

    let seen_messages = Arc::new(Mutex::new(Vec::new()));
    let session = Session::from_root(
        PhiAgentStep::request_provider("ready", &test_model_defaults()),
        vec![PhiMessage::user("hello")],
    );
    let mut builder = isolated_step_agent_builder(session)
        .prepare()
        .expect("builder should prepare successfully");
    let init_modules = crate::features::build_init_modules(builder.context());
    builder = builder.with_module_layout(init_modules);
    let outcome = builder
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
        vec![PhiMessage::user("hello")]
    );
    assert_eq!(
        outcome.session.history(),
        &[PhiMessage::user("hello"), PhiMessage::assistant("ok")]
    );

    unsafe {
        restore_env("PHI_SYSTEM", previous);
    }
}

#[tokio::test]
async fn built_in_system_prompt_is_committed_when_session_is_created() {
    let _lock = env_lock();
    let previous = std::env::var_os("PHI_SYSTEM");
    unsafe {
        std::env::remove_var("PHI_SYSTEM");
    }

    let session = crate::new_session(&LocalPhiHome::new(crate::tests::support::unique_test_home()))
        .expect("session should initialize from default config");
    let default_prompt = include_str!("../prompts/system.txt").trim();
    assert_eq!(session.history(), &[PhiMessage::system(default_prompt)]);

    let seen_messages = Arc::new(Mutex::new(Vec::new()));
    let mut builder = isolated_step_agent_builder(session)
        .prepare()
        .expect("builder should prepare successfully");
    let init_modules = crate::features::build_init_modules(builder.context());
    builder = builder.with_module_layout(init_modules);
    let outcome = builder
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
        vec![PhiMessage::system(default_prompt)]
    );
    assert_eq!(
        outcome.session.history(),
        &[
            PhiMessage::system(default_prompt),
            PhiMessage::assistant("ok")
        ]
    );

    unsafe {
        restore_env("PHI_SYSTEM", previous);
    }
}

#[tokio::test]
async fn empty_phi_system_commits_an_empty_system_prompt() {
    let _lock = env_lock();
    let previous = std::env::var_os("PHI_SYSTEM");
    unsafe {
        std::env::set_var("PHI_SYSTEM", "");
    }

    let session = crate::new_session(&LocalPhiHome::new(crate::tests::support::unique_test_home()))
        .expect("session should initialize with an empty system prompt");
    assert_eq!(session.history(), &[PhiMessage::system("")]);

    unsafe {
        restore_env("PHI_SYSTEM", previous);
    }
}

#[tokio::test]
async fn empty_yaml_system_commits_an_empty_system_prompt() {
    let _lock = env_lock();
    let root = crate::tests::support::unique_test_home();
    std::fs::create_dir_all(&root).expect("test home should be creatable");
    std::fs::write(root.join("config.yml"), "runtime:\n  system: \"\"\n")
        .expect("config should be writable");
    let previous = std::env::var_os("PHI_SYSTEM");
    unsafe {
        std::env::remove_var("PHI_SYSTEM");
    }

    let session = crate::new_session(&LocalPhiHome::new(root.clone()))
        .expect("session should initialize with an empty system prompt");
    assert_eq!(session.history(), &[PhiMessage::system("")]);

    unsafe {
        restore_env("PHI_SYSTEM", previous);
    }
    std::fs::remove_dir_all(root).expect("test home should be removable");
}

unsafe fn restore_env(name: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
}
