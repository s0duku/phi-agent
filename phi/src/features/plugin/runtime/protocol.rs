use serde::{Deserialize, Serialize};

use crate::executor::PhiToolDefinition;

// These protocol types describe the Rust <-> Python runtime bridge, regardless
// of whether the backend is a subprocess, embedded CPython, or another runtime
// transport in the future.

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PythonRuntimeRequest {
    Ping,
    LoadPlugin {
        source: String,
        code: String,
    },
    ListTools,
    CallTool {
        name: String,
        arguments: serde_json::Value,
    },
    RunCode {
        code: String,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct PythonRuntimeResponse {
    pub ok: bool,
    #[serde(default)]
    pub tools: Option<Vec<PhiToolDefinition>>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}
