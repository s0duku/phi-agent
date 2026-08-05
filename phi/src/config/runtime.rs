use serde::Deserialize;

use super::schema::defaults;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConfig {
    pub system: Option<String>,
    pub max_steps: usize,
    pub context_tokens: usize,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RuntimeConfigPatch {
    pub system: Option<String>,
    pub max_steps: Option<usize>,
    pub context_tokens: Option<usize>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            system: None,
            max_steps: defaults::MAX_STEPS,
            context_tokens: defaults::CONTEXT_TOKENS,
        }
    }
}

impl RuntimeConfig {
    pub(super) fn apply(&mut self, patch: RuntimeConfigPatch) {
        if let Some(value) = patch.system {
            self.system = Some(value);
        }
        if let Some(value) = patch.max_steps {
            self.max_steps = value;
        }
        if let Some(value) = patch.context_tokens {
            self.context_tokens = value;
        }
    }
}
