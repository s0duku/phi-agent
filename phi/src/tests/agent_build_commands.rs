use std::sync::{Arc, Mutex};

use crate::{
    agent::{PhiAgent, PhiAgentCommand},
    error::PhiRuntimeResult,
    executor::PhiTool,
    home::LocalPhiHome,
    module::PhiModule,
    session::Session,
    tests::support::unique_test_home,
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
    type ProbInfo = ();

    fn init_context(
        &mut self,
        _context: &mut crate::agent::PhiAgentBuildContext,
    ) -> PhiRuntimeResult<()> {
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
        root.join("config.toml").display().to_string()
    );
    assert_eq!(
        report.home.plugins_path,
        root.join("plugins").display().to_string()
    );
    assert_eq!(report.home.tmp_path, root.join("tmp").display().to_string());
}
