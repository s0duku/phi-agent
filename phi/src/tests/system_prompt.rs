use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::{
    error::PhiRuntimeResult,
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
    ) -> PhiRuntimeResult<PhiModelResponse> {
        *self
            .seen_messages
            .lock()
            .expect("capture lock should be healthy") = messages.to_messages();
        Ok(PhiModelResponse::unspecified(self.response.clone()))
    }
}

#[tokio::test]
async fn phi_system_bootstraps_new_session_as_first_message() {
    let _lock = env_lock();
    let previous = std::env::var_os("PHI_SYSTEM");
    unsafe {
        std::env::set_var("PHI_SYSTEM", "You are the bootstrap system prompt.");
    }

    let seen_messages = Arc::new(Mutex::new(Vec::new()));
    let mut builder = isolated_step_agent_builder(Session::empty())
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
        PhiAgentStep::request_complete("ready", &test_model_defaults()),
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
async fn built_in_system_prompt_bootstraps_when_not_configured() {
    let _lock = env_lock();
    let previous = std::env::var_os("PHI_SYSTEM");
    unsafe {
        std::env::remove_var("PHI_SYSTEM");
    }

    let seen_messages = Arc::new(Mutex::new(Vec::new()));
    let mut builder = isolated_step_agent_builder(Session::empty())
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

    let default_prompt = include_str!("../prompts/system.txt").trim();
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

unsafe fn restore_env(name: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
}
