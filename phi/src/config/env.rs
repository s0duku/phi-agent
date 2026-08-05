use std::{
    collections::BTreeMap,
    sync::{OnceLock, RwLock},
};

use crate::executor::PhiToolDefinition;

use super::{
    executor::ExecutorConfigPatch,
    model::{ModelConfigPatch, ReasoningConfigPatch},
    provider::ProviderConfigPatch,
    runtime::RuntimeConfigPatch,
    schema::PhiConfigEnv,
};

static RUNTIME_OVERRIDES: OnceLock<RwLock<BTreeMap<PhiConfigEnv, String>>> = OnceLock::new();

#[derive(Clone, Debug, Default)]
pub(super) struct PhiEnvOverrides {
    pub(super) model: ModelConfigPatch,
    pub(super) provider: ProviderConfigPatch,
    pub(super) runtime: RuntimeConfigPatch,
    pub(super) executor: ExecutorConfigPatch,
    pub(super) tools: Option<Vec<PhiToolDefinition>>,
}

impl PhiEnvOverrides {
    pub(super) fn from_process() -> Result<Self, Box<dyn std::error::Error>> {
        let mut values = PhiConfigEnv::ALL
            .into_iter()
            .filter_map(|key| std::env::var(key.name()).ok().map(|value| (key, value)))
            .collect::<BTreeMap<_, _>>();
        for (key, value) in runtime_overrides()
            .read()
            .expect("runtime config lock was poisoned")
            .iter()
        {
            values.insert(*key, value.clone());
        }
        Self::from_typed_values(&values)
    }

    #[cfg(test)]
    pub(super) fn from_values(
        values: &BTreeMap<String, String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let values = PhiConfigEnv::ALL
            .into_iter()
            .filter_map(|key| values.get(key.name()).cloned().map(|value| (key, value)))
            .collect();
        Self::from_typed_values(&values)
    }

    fn from_typed_values(
        values: &BTreeMap<PhiConfigEnv, String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let model = ModelConfigPatch {
            name: nonempty(values.get(&PhiConfigEnv::Model)),
            temperature: parse(values, PhiConfigEnv::Temperature, "a valid float")?,
            max_tokens: parse(values, PhiConfigEnv::MaxTokens, "an unsigned integer")?,
            reasoning: ReasoningConfigPatch {
                enabled: parse_bool(values, PhiConfigEnv::EnableReasoning)?,
                effort: parse(
                    values,
                    PhiConfigEnv::ReasoningEffort,
                    "one of: minimal, low, medium, high",
                )?,
                token_budget: parse(
                    values,
                    PhiConfigEnv::ThinkingTokenBudget,
                    "an unsigned integer",
                )?,
            },
        };
        let provider = ProviderConfigPatch {
            kind: nonempty(values.get(&PhiConfigEnv::Provider)),
            api_base: nonempty(values.get(&PhiConfigEnv::ApiBase)),
            api_key: values.get(&PhiConfigEnv::ApiKey).cloned(),
            fake_profile: nonempty(values.get(&PhiConfigEnv::FakeProfile)),
        };
        let runtime = RuntimeConfigPatch {
            system: values.get(&PhiConfigEnv::System).cloned(),
            max_steps: parse(values, PhiConfigEnv::MaxSteps, "an unsigned integer")?,
            context_tokens: parse(values, PhiConfigEnv::ContextTokens, "an unsigned integer")?,
        };
        let executor = ExecutorConfigPatch {
            tool_threshold_tokens: parse(
                values,
                PhiConfigEnv::ToolThresholdTokens,
                "an unsigned integer",
            )?,
            tool_preview_bytes: parse(
                values,
                PhiConfigEnv::ToolPreviewBytes,
                "an unsigned integer",
            )?,
        };
        let tools = values
            .get(&PhiConfigEnv::Tools)
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                serde_json::from_str(value).map_err(|error| -> Box<dyn std::error::Error> {
                    format!(
                        "{} must be a JSON array of tool definitions: {error}",
                        PhiConfigEnv::Tools.name()
                    )
                    .into()
                })
            })
            .transpose()?;
        Ok(Self {
            model,
            provider,
            runtime,
            executor,
            tools,
        })
    }
}

pub fn set_runtime_setting(key: PhiConfigEnv, value: Option<String>) {
    let mut overrides = runtime_overrides()
        .write()
        .expect("runtime config lock was poisoned");
    match value {
        Some(value) => {
            overrides.insert(key, value);
        }
        None => {
            overrides.remove(&key);
        }
    }
}

fn runtime_overrides() -> &'static RwLock<BTreeMap<PhiConfigEnv, String>> {
    RUNTIME_OVERRIDES.get_or_init(|| RwLock::new(BTreeMap::new()))
}

fn nonempty(value: Option<&String>) -> Option<String> {
    value
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse<T>(
    values: &BTreeMap<PhiConfigEnv, String>,
    key: PhiConfigEnv,
    expected: &'static str,
) -> Result<Option<T>, Box<dyn std::error::Error>>
where
    T: std::str::FromStr,
{
    values
        .get(&key)
        .map(|value| {
            value
                .parse::<T>()
                .map_err(|_| format!("{} must be {expected}", key.name()).into())
        })
        .transpose()
}

fn parse_bool(
    values: &BTreeMap<PhiConfigEnv, String>,
    key: PhiConfigEnv,
) -> Result<Option<bool>, Box<dyn std::error::Error>> {
    values
        .get(&key)
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!("{} must be a valid boolean", key.name()).into()),
        })
        .transpose()
}
