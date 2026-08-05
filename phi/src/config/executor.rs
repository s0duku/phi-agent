use serde::Deserialize;

use super::schema::defaults;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutorConfig {
    pub tool_threshold_tokens: usize,
    pub tool_preview_bytes: usize,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ExecutorConfigPatch {
    pub tool_threshold_tokens: Option<usize>,
    pub tool_preview_bytes: Option<usize>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            tool_threshold_tokens: defaults::TOOL_THRESHOLD_TOKENS,
            tool_preview_bytes: defaults::TOOL_PREVIEW_BYTES,
        }
    }
}

impl ExecutorConfig {
    pub(super) fn apply(&mut self, patch: ExecutorConfigPatch) {
        if let Some(value) = patch.tool_threshold_tokens {
            self.tool_threshold_tokens = value;
        }
        if let Some(value) = patch.tool_preview_bytes {
            self.tool_preview_bytes = value;
        }
    }
}
