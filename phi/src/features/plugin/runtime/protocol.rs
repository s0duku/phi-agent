use serde::{Deserialize, Serialize};

use crate::executor::PhiToolDefinition;

// These protocol types describe the Rust <-> Python runtime bridge, regardless
// of whether the backend is a subprocess, embedded CPython, or another runtime
// transport in the future.

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PythonRuntimeResponse {
    Pong {},
    PluginLoaded { name: String },
    ToolsListed { tools: Vec<PhiToolDefinition> },
    ToolCalled { output: String },
    CodeRan { output: String },
    Failed { error: String },
}

#[cfg(test)]
mod tests {
    use super::PythonRuntimeResponse;

    #[test]
    fn responses_require_the_exact_current_wire_shape() {
        assert!(
            serde_json::from_value::<PythonRuntimeResponse>(serde_json::json!({
                "kind": "plugin_loaded"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<PythonRuntimeResponse>(serde_json::json!({
                "kind": "pong",
                "ok": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<PythonRuntimeResponse>(serde_json::json!({
                "ok": true
            }))
            .is_err()
        );
    }
}
