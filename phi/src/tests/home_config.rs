use std::{
    ffi::OsString,
    fs,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    agent::PhiAgentBuildContext,
    config::PhiConfig,
    home::LocalPhiHome,
    message::PhiMessage,
    module::PhiModule,
    session::Session,
    tests::support::{ambient_step_agent_builder, env_lock, stub_client},
};

struct CaptureEnvModule {
    seen: Arc<Mutex<Option<PhiConfig>>>,
}

impl PhiModule for CaptureEnvModule {
    type ProbInfo = ();

    fn init_context(
        &mut self,
        context: &mut PhiAgentBuildContext,
    ) -> crate::error::PhiAgentRuntimeResult<()> {
        *self.seen.lock().expect("capture lock should be healthy") = Some(context.config().clone());
        Ok(())
    }
}

#[test]
fn home_config_provides_base_settings_and_env_overrides_them() {
    let _lock = env_lock();
    let root = unique_temp_dir("phi-home-config-merge");
    fs::create_dir_all(&root).expect("temp home root should be creatable");
    fs::write(
        root.join("config.toml"),
        "PHI_MODEL = \"home-model\"\nPHI_ENABLE_REASONING = false\n",
    )
    .expect("config.toml should be writable");

    let previous_phi_model = std::env::var_os("PHI_MODEL");
    let previous_phi_enable_reasoning = std::env::var_os("PHI_ENABLE_REASONING");
    unsafe {
        std::env::set_var("PHI_MODEL", "env-model");
        std::env::set_var("PHI_ENABLE_REASONING", "true");
    }

    let seen = Arc::new(Mutex::new(None));
    let middleware = CaptureEnvModule { seen: seen.clone() };

    ambient_step_agent_builder(Session::empty())
        .with_home(Arc::new(LocalPhiHome::new(root.clone())))
        .with_module(middleware)
        .with_client(stub_client(Vec::new()))
        .build()
        .expect("agent build should succeed with home config");

    let captured = seen
        .lock()
        .expect("capture lock should be healthy")
        .clone()
        .expect("module should observe merged env");
    assert_eq!(captured.get("PHI_MODEL"), Some("env-model"));
    assert_eq!(captured.get("PHI_ENABLE_REASONING"), Some("true"));

    unsafe {
        restore_env("PHI_MODEL", previous_phi_model);
        restore_env("PHI_ENABLE_REASONING", previous_phi_enable_reasoning);
    }
    fs::remove_dir_all(root).expect("temp home should be removable");
}

#[test]
fn home_config_is_committed_when_constructing_a_session() {
    let _lock = env_lock();
    let root = unique_temp_dir("phi-home-config-init-middleware");
    fs::create_dir_all(&root).expect("temp home root should be creatable");
    fs::write(
        root.join("config.toml"),
        "PHI_SYSTEM = \"You are the home-config system prompt.\"\n",
    )
    .expect("config.toml should be writable");

    let previous_phi_system = std::env::var_os("PHI_SYSTEM");
    unsafe {
        std::env::remove_var("PHI_SYSTEM");
    }

    let session = crate::new_session(&LocalPhiHome::new(root.clone()))
        .expect("session should initialize with home config");

    assert_eq!(
        session.history(),
        &[PhiMessage::system("You are the home-config system prompt.")]
    );

    unsafe {
        restore_env("PHI_SYSTEM", previous_phi_system);
    }
    fs::remove_dir_all(root).expect("temp home should be removable");
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nanos}"))
}

unsafe fn restore_env(name: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
}
