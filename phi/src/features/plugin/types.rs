use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginAvailability {
    Enabled { runtime: PythonRuntimeInfo },
    Disabled { reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PythonRuntimeInfo {
    pub backend: String,
    pub version: String,
    pub implementation: String,
    pub library: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct PyPluginDescriptor {
    pub command_kind: String,
    pub plugin_args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct LoadedPyPlugin {
    pub name: String,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PluginRuntimeStatus {
    pub provider: String,
    pub build: PythonBuildInfo,
    pub availability: PluginAvailability,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PythonBuildInfo {
    pub configured_backends: Vec<String>,
    pub minimum_version: String,
    pub sdk_version: String,
    pub sdk_module: String,
    pub sdk_capabilities: Vec<String>,
}
