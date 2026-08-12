use std::sync::{Arc, Mutex};

use crate::{
    agent::{PhiAgent, PhiAgentCommand},
    error::PhiAgentRuntimeResult,
    executor::PhiTool,
    home::LocalPhiHome,
    message::PhiMessage,
    module::PhiModule,
    session::{PhiAgentStep, Session},
    tests::support::{test_model_defaults, unique_test_home},
};

#[derive(Default, Debug, Eq, PartialEq)]
struct BuildFlags {
    init_context_called: bool,
    module_tools_called: bool,
}

struct BuildProbeModule {
    flags: Arc<Mutex<BuildFlags>>,
}

impl PhiModule for BuildProbeModule {
    fn init_context(
        &mut self,
        _context: &mut crate::agent::PhiAgentBuildContext,
    ) -> PhiAgentRuntimeResult<()> {
        self.flags
            .lock()
            .expect("flags mutex should lock")
            .init_context_called = true;
        Ok(())
    }

    fn module_tools(
        &mut self,
        _context: &crate::agent::PhiAgentBuildContext,
    ) -> Vec<Arc<dyn PhiTool>> {
        self.flags
            .lock()
            .expect("flags mutex should lock")
            .module_tools_called = true;
        Vec::new()
    }
}

fn assert_command_triggers_full_build(command: PhiAgentCommand) {
    let flags = Arc::new(Mutex::new(BuildFlags::default()));
    let agent = PhiAgent::builder(Session::empty(), command)
        .with_home(Arc::new(LocalPhiHome::new(unique_test_home())))
        .with_module(BuildProbeModule {
            flags: flags.clone(),
        })
        .build()
        .expect("agent should build");

    let observed = flags.lock().expect("flags mutex should lock");
    assert!(observed.init_context_called);
    assert!(observed.module_tools_called);

    drop(agent);
}

#[test]
fn commands_use_full_agent_build() {
    for command in [
        PhiAgentCommand::Doctor(PhiAgentCommand::doctor()),
        PhiAgentCommand::History(PhiAgentCommand::history()),
        PhiAgentCommand::Yolo(PhiAgentCommand::yolo()),
    ] {
        assert_command_triggers_full_build(command);
    }
}

#[test]
fn model_retry_module_is_installed_only_for_an_explicit_budget() {
    let build = |command| {
        crate::agent::build_agent(
            Session::from_root(
                PhiAgentStep::request_provider("ready", &test_model_defaults()),
                vec![PhiMessage::user("hello")],
            ),
            command,
            Arc::new(LocalPhiHome::new(unique_test_home())),
        )
        .expect("agent should build")
    };

    let default_agent = build(PhiAgentCommand::Step(PhiAgentCommand::step()));
    let default_module_count = default_agent.runtime().module_count();

    let retry_agent = build(PhiAgentCommand::Step(
        PhiAgentCommand::step().with_max_model_request_retries(Some(3)),
    ));
    assert_eq!(
        retry_agent.runtime().module_count(),
        default_module_count + 1
    );
}

#[test]
fn null_executor_builds_an_empty_executor_without_loading_module_tools() {
    let flags = Arc::new(Mutex::new(BuildFlags::default()));
    let command = PhiAgentCommand::Step(PhiAgentCommand::step().with_null_executor(true));
    let agent = PhiAgent::builder(Session::empty(), command)
        .with_home(Arc::new(LocalPhiHome::new(unique_test_home())))
        .with_module(BuildProbeModule {
            flags: flags.clone(),
        })
        .build()
        .expect("null-executor agent should build");

    let observed = flags.lock().expect("flags mutex should lock");
    assert!(observed.init_context_called);
    assert!(!observed.module_tools_called);
    assert!(agent.runtime().tool_definitions().is_empty());
}

#[test]
fn doctor_report_includes_home_debug_information() {
    let root = unique_test_home();
    let agent = PhiAgent::builder(
        Session::empty(),
        PhiAgentCommand::Doctor(PhiAgentCommand::doctor()),
    )
    .with_home(Arc::new(LocalPhiHome::new(root.clone())))
    .build()
    .expect("agent should build");

    let report = agent.doctor_report();
    assert_eq!(report.home.kind, "local");
    assert_eq!(report.home.source, "explicit");
    assert_eq!(report.home.root, root.display().to_string());
    assert_eq!(
        report.home.config_path,
        root.join("config.yml").display().to_string()
    );
    assert_eq!(report.home.tmp_path, root.join("tmp").display().to_string());
}
