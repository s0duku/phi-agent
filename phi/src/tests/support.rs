use std::sync::Arc;
#[cfg(test)]
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::{
    agent::{PhiAgent, PhiAgentCommand},
    config::{ModelRequestDefaults, ReasoningEffort},
    error::PhiAgentRuntimeResult,
    home::LocalPhiHome,
    message::{PhiHistory, PhiMessage},
    render::{PhiModelResponse, PhiProviderCall, TestClient},
    session::Session,
};

pub(crate) fn test_model_defaults() -> ModelRequestDefaults {
    ModelRequestDefaults {
        model: "test-model".to_string(),
        temperature: Some(0.0),
        max_tokens: 1024,
        enable_reasoning: true,
        reasoning_effort: ReasoningEffort::Medium,
    }
}

pub(crate) struct StubProvider {
    pub(crate) response: Vec<PhiMessage>,
}

#[async_trait]
impl TestClient for StubProvider {
    async fn complete(
        &self,
        _request: &PhiProviderCall,
        _messages: &PhiHistory,
    ) -> PhiAgentRuntimeResult<PhiModelResponse> {
        Ok(PhiModelResponse::unspecified(self.response.clone()))
    }
}

pub(crate) fn step_agent_builder(session: Session) -> crate::agent::PhiAgentBuilder {
    isolated_step_agent_builder(session)
}

pub(crate) fn default_step_agent_builder(
    session: Session,
) -> crate::agent::PreparedPhiAgentBuilder {
    let builder = isolated_step_agent_builder(session)
        .prepare()
        .expect("test agent builder should prepare successfully");
    let modules = crate::features::build_default_modules(builder.context());
    builder.with_module_layout(modules)
}

pub(crate) fn ambient_step_agent_builder(session: Session) -> crate::agent::PhiAgentBuilder {
    PhiAgent::builder(session, PhiAgentCommand::Step(PhiAgentCommand::step()))
        .with_model_defaults(test_model_defaults())
}

pub(crate) fn isolated_step_agent_builder(session: Session) -> crate::agent::PhiAgentBuilder {
    ambient_step_agent_builder(session).with_home(Arc::new(LocalPhiHome::new(unique_test_home())))
}

pub(crate) fn stub_client(response: Vec<PhiMessage>) -> Arc<dyn TestClient> {
    Arc::new(StubProvider { response })
}

pub(crate) fn unique_test_home() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("phi-test-home-{}-{nanos}", std::process::id()))
}

pub(crate) fn shell_tool_name() -> &'static str {
    if cfg!(windows) {
        "powershell_job"
    } else {
        "bash_job"
    }
}

pub(crate) fn shell_echo_ok_command() -> &'static str {
    if cfg!(windows) {
        "Write-Output ok; exit 0"
    } else {
        "echo ok"
    }
}

pub(crate) fn shell_echo_rewritten_command() -> &'static str {
    if cfg!(windows) {
        "Write-Output rewritten; exit 0"
    } else {
        "echo rewritten"
    }
}

pub(crate) fn shell_stdout_ok() -> &'static str {
    "ok"
}

#[cfg(test)]
pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
