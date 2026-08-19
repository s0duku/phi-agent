use serde::Deserialize;

use super::schema::defaults;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutorConfig {
    pub tool_output_token_limit: usize,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ExecutorConfigPatch {
    pub tool_output_token_limit: Option<usize>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            tool_output_token_limit: defaults::TOOL_OUTPUT_TOKEN_LIMIT,
        }
    }
}

impl ExecutorConfig {
    pub(super) fn apply(&mut self, patch: ExecutorConfigPatch) {
        if let Some(value) = patch.tool_output_token_limit {
            self.tool_output_token_limit = value;
        }
    }
}
