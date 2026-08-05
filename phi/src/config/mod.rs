mod env;
mod executor;
mod model;
mod provider;
mod runtime;
mod schema;

use serde::Deserialize;

use crate::executor::PhiToolDefinition;

pub use env::set_runtime_setting;
pub use executor::ExecutorConfig;
pub use model::{ModelConfig, ModelRequestDefaults, ReasoningConfig, ReasoningEffort};
pub use provider::ProviderConfig;
pub use runtime::RuntimeConfig;
pub use schema::PhiConfigEnv;

use env::PhiEnvOverrides;
use executor::ExecutorConfigPatch;
use model::ModelConfigPatch;
use provider::ProviderConfigPatch;
use runtime::RuntimeConfigPatch;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhiConfig {
    model: ModelConfig,
    provider: ProviderConfig,
    runtime: RuntimeConfig,
    executor: ExecutorConfig,
    tools: Vec<PhiToolDefinition>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PhiConfigFile {
    model: ModelConfigPatch,
    provider: ProviderConfigPatch,
    runtime: RuntimeConfigPatch,
    executor: ExecutorConfigPatch,
    tools: Option<Vec<PhiToolDefinition>>,
}

impl PhiConfig {
    pub fn from_yaml(bytes: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        let file: PhiConfigFile = serde_yaml::from_slice(bytes)?;
        Ok(Self::default().apply_file(file))
    }

    fn apply_env(mut self, overrides: PhiEnvOverrides) -> Self {
        self.model.apply(overrides.model);
        self.provider.apply(overrides.provider);
        self.runtime.apply(overrides.runtime);
        self.executor.apply(overrides.executor);
        if let Some(tools) = overrides.tools {
            self.tools = tools;
        }
        self
    }

    pub(crate) fn apply_process_env(self) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(self.apply_env(PhiEnvOverrides::from_process()?))
    }

    pub fn model(&self) -> &ModelConfig {
        &self.model
    }

    pub fn provider(&self) -> &ProviderConfig {
        &self.provider
    }

    pub fn runtime(&self) -> &RuntimeConfig {
        &self.runtime
    }

    pub fn executor(&self) -> &ExecutorConfig {
        &self.executor
    }

    pub fn tools(&self) -> &[PhiToolDefinition] {
        &self.tools
    }

    fn apply_file(mut self, file: PhiConfigFile) -> Self {
        self.model.apply(file.model);
        self.provider.apply(file.provider);
        self.runtime.apply(file.runtime);
        self.executor.apply(file.executor);
        if let Some(tools) = file.tools {
            self.tools = tools;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::executor::ToolOutputLimits;

    use super::*;

    #[test]
    fn yaml_and_environment_merge_into_one_typed_config() {
        let config = PhiConfig::from_yaml(
            br#"
model:
  name: home-model
  reasoning:
    enabled: false
provider:
  kind: openai_response
runtime:
  context_tokens: 1000
tools:
  - name: lookup
    description: Lookup
    parameters:
      type: object
"#,
        )
        .unwrap();
        let overrides = PhiEnvOverrides::from_values(&BTreeMap::from([
            ("PHI_MODEL".into(), "env-model".into()),
            ("PHI_ENABLE_REASONING".into(), "true".into()),
        ]))
        .unwrap();
        let config = config.apply_env(overrides);

        assert_eq!(config.model().name, "env-model");
        assert!(config.model().reasoning.enabled);
        assert_eq!(config.provider().kind, "openai_response");
        assert_eq!(config.runtime().context_tokens, 1000);
        assert_eq!(config.tools()[0].name, "lookup");
    }

    #[test]
    fn yaml_rejects_unknown_fields() {
        let error = PhiConfig::from_yaml(b"model:\n  modle: typo\n").unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn environment_rejects_invalid_typed_values() {
        let error = PhiEnvOverrides::from_values(&BTreeMap::from([(
            "PHI_CONTEXT_TOKENS".into(),
            "many".into(),
        )]))
        .unwrap_err();
        assert!(error.to_string().contains("PHI_CONTEXT_TOKENS"));
    }

    #[test]
    fn tool_output_limits_are_derived_from_typed_config() {
        let config = PhiConfig::from_yaml(
            b"executor:\n  tool_threshold_tokens: 1234\n  tool_preview_bytes: 567\n",
        )
        .unwrap();
        assert_eq!(
            ToolOutputLimits::new(
                config.executor().tool_threshold_tokens,
                config.executor().tool_preview_bytes,
            ),
            ToolOutputLimits::new(1234, 567)
        );
    }
}
