use serde::Deserialize;

use super::schema::defaults;

#[derive(Clone, Debug, PartialEq)]
pub struct ModelConfig {
    pub name: String,
    pub temperature: Option<f64>,
    pub max_tokens: u64,
    pub reasoning: ReasoningConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningConfig {
    pub enabled: bool,
    pub effort: ReasoningEffort,
    pub token_budget: u64,
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

#[derive(Clone, Copy, Debug, Deserialize, serde::Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

impl Default for ReasoningEffort {
    fn default() -> Self {
        Self::Medium
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            _ => Err("expected one of: minimal, low, medium, high".to_string()),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ModelConfigPatch {
    pub name: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub reasoning: ReasoningConfigPatch,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ReasoningConfigPatch {
    pub enabled: Option<bool>,
    pub effort: Option<ReasoningEffort>,
    pub token_budget: Option<u64>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: defaults::MODEL.to_string(),
            temperature: Some(defaults::TEMPERATURE),
            max_tokens: defaults::MAX_TOKENS,
            reasoning: ReasoningConfig {
                enabled: defaults::REASONING_ENABLED,
                effort: ReasoningEffort::default(),
                token_budget: defaults::REASONING_TOKEN_BUDGET,
            },
        }
    }
}

impl ModelConfig {
    pub(super) fn apply(&mut self, patch: ModelConfigPatch) {
        if let Some(value) = patch.name {
            self.name = value;
        }
        if let Some(value) = patch.temperature {
            self.temperature = Some(value);
        }
        if let Some(value) = patch.max_tokens {
            self.max_tokens = value;
        }
        if let Some(value) = patch.reasoning.enabled {
            self.reasoning.enabled = value;
        }
        if let Some(value) = patch.reasoning.effort {
            self.reasoning.effort = value;
        }
        if let Some(value) = patch.reasoning.token_budget {
            self.reasoning.token_budget = value;
        }
    }
}

impl ModelRequestDefaults {
    pub fn defaults() -> Self {
        Self::from(&super::PhiConfig::default())
    }
}

impl From<&super::PhiConfig> for ModelRequestDefaults {
    fn from(config: &super::PhiConfig) -> Self {
        Self {
            model: config.model().name.clone(),
            temperature: config.model().temperature,
            max_tokens: config.model().max_tokens,
            enable_reasoning: config.model().reasoning.enabled,
            thinking_token_budget: config.model().reasoning.token_budget,
            reasoning_effort: config.model().reasoning.effort,
        }
    }
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
