use std::{
    collections::BTreeMap,
    sync::{OnceLock, RwLock},
};

use crate::executor::{PhiToolDefinition, ToolOutputLimits};

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MAX_STEPS: usize = 1_000_000;
const DEFAULT_CONTEXT_TOKENS: usize = 256 * 1024;
const DEFAULT_TOOL_THRESHOLD_TOKENS: usize = (24 * 1024) / 4;
const DEFAULT_TOOL_PREVIEW_BYTES: usize = 2 * 1024;
static RUNTIME_OVERRIDES: OnceLock<RwLock<BTreeMap<String, String>>> = OnceLock::new();

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PhiConfig {
    values: BTreeMap<String, String>,
}

impl PhiConfig {
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Self { values }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    pub fn insert(&mut self, name: String, value: String) {
        self.values.insert(name, value);
    }

    pub fn extend(&mut self, other: &Self) {
        for (name, value) in &other.values {
            self.values.insert(name.clone(), value.clone());
        }
    }
}

#[derive(Clone)]
pub struct ProviderConfig {
    pub provider: String,
    pub api_base: String,
    pub api_key: String,
    pub fake_profile: String,
}

impl ProviderConfig {
    pub fn defaults() -> Self {
        Self {
            provider: "openai_chat".to_string(),
            api_base: DEFAULT_OPENAI_BASE_URL.to_string(),
            api_key: String::new(),
            fake_profile: "assistant_text".to_string(),
        }
    }

    pub fn from_config(config: &PhiConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let defaults = Self::defaults();
        Ok(Self {
            provider: setting_from_config(config, "PHI_PROVIDER").unwrap_or(defaults.provider),
            api_base: setting_from_config(config, "PHI_API").unwrap_or(defaults.api_base),
            api_key: optional_string_from_config(config, "PHI_KEY").unwrap_or(defaults.api_key),
            fake_profile: setting_from_config(config, "PHI_FAKE_PROFILE")
                .unwrap_or(defaults.fake_profile),
        })
    }
}

#[derive(Clone)]
pub struct ModelRequestDefaults {
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: u64,
    pub enable_reasoning: bool,
    pub thinking_token_budget: u64,
    pub reasoning_effort: ReasoningEffort,
}

impl ModelRequestDefaults {
    pub fn defaults() -> Self {
        Self {
            model: String::new(),
            temperature: Some(1.0),
            max_tokens: 32000,
            enable_reasoning: true,
            thinking_token_budget: 4096,
            reasoning_effort: ReasoningEffort::Medium,
        }
    }

    pub fn from_config(config: &PhiConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let defaults = Self::defaults();
        Ok(Self {
            model: optional_string_from_config(config, "PHI_MODEL").unwrap_or(defaults.model),
            temperature: optional_f64_from_config(config, "PHI_TEMPERATURE")?
                .or(defaults.temperature),
            max_tokens: optional_u64_from_config(config, "PHI_MAX_TOKENS")?
                .unwrap_or(defaults.max_tokens),
            enable_reasoning: optional_bool_from_config(config, "PHI_ENABLE_REASONING")?
                .unwrap_or(defaults.enable_reasoning),
            thinking_token_budget: optional_u64_from_config(config, "PHI_THINKING_TOKEN_BUDGET")?
                .unwrap_or(defaults.thinking_token_budget),
            reasoning_effort: optional_reasoning_effort_from_config(
                config,
                "PHI_REASONING_EFFORT",
            )?
            .unwrap_or(defaults.reasoning_effort),
        })
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, Eq, PartialEq)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

pub const fn default_max_steps() -> usize {
    DEFAULT_MAX_STEPS
}

pub const fn default_context_tokens() -> usize {
    DEFAULT_CONTEXT_TOKENS
}

pub const fn default_auto_compact_threshold_tokens() -> usize {
    DEFAULT_CONTEXT_TOKENS * 9 / 10
}

pub const fn default_tool_threshold_tokens() -> usize {
    DEFAULT_TOOL_THRESHOLD_TOKENS
}

pub const fn default_tool_preview_bytes() -> usize {
    DEFAULT_TOOL_PREVIEW_BYTES
}

pub fn tool_output_limits_from_config(config: &PhiConfig) -> ToolOutputLimits {
    ToolOutputLimits::new(
        optional_public_usize_from_config(config, "PHI_TOOL_THRESHOLD_TOKENS")
            .ok()
            .flatten()
            .unwrap_or(default_tool_threshold_tokens()),
        optional_public_usize_from_config(config, "PHI_TOOL_PREVIEW_BYTES")
            .ok()
            .flatten()
            .unwrap_or(default_tool_preview_bytes()),
    )
}

pub fn set_runtime_setting(name: &str, value: Option<String>) {
    let mut overrides = runtime_overrides()
        .write()
        .expect("runtime config lock was poisoned");
    match value {
        Some(value) => {
            overrides.insert(name.to_string(), value);
        }
        None => {
            overrides.remove(name);
        }
    }
}

pub(crate) fn ambient_config() -> PhiConfig {
    let mut values = std::env::vars().collect::<BTreeMap<_, _>>();
    for (key, value) in runtime_overrides()
        .read()
        .expect("runtime config lock was poisoned")
        .iter()
    {
        values.insert(key.clone(), value.clone());
    }
    PhiConfig::new(values)
}

fn runtime_overrides() -> &'static RwLock<BTreeMap<String, String>> {
    RUNTIME_OVERRIDES.get_or_init(|| RwLock::new(BTreeMap::new()))
}

fn setting_from_config(config: &PhiConfig, name: &str) -> Option<String> {
    config.get(name).map(str::to_string)
}

fn optional_string_from_config(config: &PhiConfig, primary: &'static str) -> Option<String> {
    setting_from_config(config, primary).filter(|value| !value.trim().is_empty())
}

fn optional_f64_from_config(
    config: &PhiConfig,
    primary: &'static str,
) -> Result<Option<f64>, Box<dyn std::error::Error>> {
    setting_from_config(config, primary)
        .map(|value| {
            value
                .parse::<f64>()
                .map_err(|_| format!("{primary} must be a valid float").into())
        })
        .transpose()
}

fn optional_u64_from_config(
    config: &PhiConfig,
    primary: &'static str,
) -> Result<Option<u64>, Box<dyn std::error::Error>> {
    setting_from_config(config, primary)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("{primary} must be a valid unsigned integer").into())
        })
        .transpose()
}

pub fn optional_public_u64_from_config(
    config: &PhiConfig,
    name: &'static str,
) -> Result<Option<u64>, Box<dyn std::error::Error>> {
    optional_u64_from_config(config, name)
}

pub fn optional_public_usize_from_config(
    config: &PhiConfig,
    name: &'static str,
) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    optional_u64_from_config(config, name)?
        .map(usize::try_from)
        .transpose()
        .map_err(|_| format!("{name} must fit into usize").into())
}

pub(crate) fn phi_tools_from_config(
    config: &PhiConfig,
) -> Result<Vec<PhiToolDefinition>, Box<dyn std::error::Error>> {
    let Some(raw) = optional_string_from_config(config, "PHI_TOOLS") else {
        return Ok(Vec::new());
    };

    serde_json::from_str::<Vec<PhiToolDefinition>>(&raw).map_err(|error| {
        format!("PHI_TOOLS must be a JSON array of tool definitions: {error}").into()
    })
}

fn optional_bool_from_config(
    config: &PhiConfig,
    primary: &'static str,
) -> Result<Option<bool>, Box<dyn std::error::Error>> {
    match setting_from_config(config, primary) {
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(Some(true)),
            "0" | "false" | "no" | "off" => Ok(Some(false)),
            _ => Err(format!("{primary} must be a valid boolean").into()),
        },
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn phi_tools_parse_from_json_string() {
        let mut values = BTreeMap::new();
        values.insert(
            "PHI_TOOLS".to_string(),
            r#"[{"name":"external_lookup","description":"External lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}}]"#
                .to_string(),
        );
        let config = PhiConfig::new(values);

        let tools = phi_tools_from_config(&config).expect("tools should parse");
        assert_eq!(
            tools,
            vec![PhiToolDefinition {
                name: "external_lookup".to_string(),
                description: "External lookup".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            }]
        );
    }

    #[test]
    fn phi_tools_default_to_empty_when_unset() {
        let config = PhiConfig::default();

        assert!(
            phi_tools_from_config(&config)
                .expect("unset tools should parse as empty")
                .is_empty()
        );
    }

    #[test]
    fn tool_output_limits_are_derived_from_global_config() {
        let config = PhiConfig::new(BTreeMap::from([
            ("PHI_TOOL_THRESHOLD_TOKENS".to_string(), "1234".to_string()),
            ("PHI_TOOL_PREVIEW_BYTES".to_string(), "567".to_string()),
        ]));

        assert_eq!(
            tool_output_limits_from_config(&config),
            ToolOutputLimits::new(1234, 567)
        );
    }
}

fn optional_reasoning_effort_from_config(
    config: &PhiConfig,
    primary: &'static str,
) -> Result<Option<ReasoningEffort>, Box<dyn std::error::Error>> {
    match setting_from_config(config, primary) {
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "minimal" => Ok(Some(ReasoningEffort::Minimal)),
            "low" => Ok(Some(ReasoningEffort::Low)),
            "medium" => Ok(Some(ReasoningEffort::Medium)),
            "high" => Ok(Some(ReasoningEffort::High)),
            _ => Err(format!("{primary} must be one of: minimal, low, medium, high").into()),
        },
        None => Ok(None),
    }
}
