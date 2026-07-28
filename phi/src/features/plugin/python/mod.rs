mod backend;
mod subprocess;

pub use backend::PhiPythonRuntime;

use super::types::{PluginAvailability, PluginRuntimeStatus, PythonBuildInfo};

pub(crate) fn python_plugin_status() -> PluginRuntimeStatus {
    PluginRuntimeStatus {
        provider: "python".to_string(),
        build: PythonBuildInfo {
            configured_backends: configured_backend_names(),
            minimum_version: backend::minimum_version_string(),
            sdk_version: super::PHI_PYTHON_SDK_VERSION.to_string(),
            sdk_module: super::PHI_PYTHON_MODULE_NAME.to_string(),
            sdk_capabilities: super::PHI_PYTHON_SDK_CAPABILITIES
                .iter()
                .map(|capability| capability.to_string())
                .collect(),
        },
        availability: match load_backend() {
            Ok(runtime) => PluginAvailability::Enabled {
                runtime: runtime.runtime_info().clone(),
            },
            Err(reason) => PluginAvailability::Disabled { reason },
        },
    }
}

pub(crate) fn load_backend() -> Result<Box<dyn PhiPythonRuntime>, String> {
    subprocess::load_backend()
}

fn configured_backend_names() -> Vec<String> {
    vec!["subprocess".to_string()]
}

pub(crate) fn worker_template_source() -> &'static str {
    include_str!("scripts/worker.py")
}

pub(crate) fn sdk_module_source() -> &'static str {
    include_str!("scripts/sdk.py")
}
