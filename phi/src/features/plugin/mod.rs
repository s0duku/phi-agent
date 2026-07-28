mod module;
mod python;
mod runtime;
mod sdk;
mod types;

use std::sync::Arc;

use crate::agent::{PhiAgentBuildContext, PhiAgentCommand};
#[allow(unused_imports)]
pub use python::PhiPythonRuntime;
pub(crate) use python::python_plugin_status;
pub(crate) use sdk::{PHI_PYTHON_MODULE_NAME, PHI_PYTHON_SDK_CAPABILITIES, PHI_PYTHON_SDK_VERSION};
pub use types::PluginRuntimeStatus;
pub(crate) use types::PyPluginDescriptor;

#[cfg(test)]
pub(crate) use types::PluginAvailability;

pub(crate) fn build_plugin_module(
    context: &PhiAgentBuildContext,
) -> Option<Box<dyn crate::module::DynPhiModule>> {
    let descriptor = descriptor_from_context(context);
    let Some(home) = context.home().map(Arc::clone) else {
        if !context.command.plugin_args().is_empty() {
            eprintln!("phi plugin: home is not attached to the init context");
        }
        return None;
    };
    let plugins = match home.list_plugins() {
        Ok(plugins) => plugins,
        Err(error) => {
            eprintln!("phi plugin: failed to discover plugins from phi home: {error}");
            return None;
        }
    };

    if plugins.is_empty() && context.command.plugin_args().is_empty() {
        return None;
    }

    let runtime = match python::load_backend() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("phi plugin: python runtime unavailable: {error}");
            return None;
        }
    };
    Some(Box::new(module::PyPluginModule::new(
        descriptor, home, runtime, plugins,
    )))
}

pub(crate) fn descriptor_from_context(context: &PhiAgentBuildContext) -> PyPluginDescriptor {
    PyPluginDescriptor {
        command_kind: command_kind(context.command()).to_string(),
        plugin_args: context.command.plugin_args().to_vec(),
    }
}

fn command_kind(command: &PhiAgentCommand) -> &'static str {
    match command {
        PhiAgentCommand::Run(_) => "run",
        PhiAgentCommand::Yolo(_) => "yolo",
        PhiAgentCommand::Step(_) => "step",
        PhiAgentCommand::Probe(_) => "probe",
        PhiAgentCommand::Doctor(_) => "doctor",
        PhiAgentCommand::History(_) => "history",
    }
}
