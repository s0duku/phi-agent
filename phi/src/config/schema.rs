#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PhiConfigEnv {
    Model,
    Temperature,
    MaxTokens,
    EnableReasoning,
    ReasoningEffort,
    Provider,
    ApiBase,
    ApiKey,
    FakeProfile,
    System,
    MaxSteps,
    ContextTokens,
    ToolThresholdTokens,
    Tools,
}

impl PhiConfigEnv {
    pub const ALL: [Self; 14] = [
        Self::Model,
        Self::Temperature,
        Self::MaxTokens,
        Self::EnableReasoning,
        Self::ReasoningEffort,
        Self::Provider,
        Self::ApiBase,
        Self::ApiKey,
        Self::FakeProfile,
        Self::System,
        Self::MaxSteps,
        Self::ContextTokens,
        Self::ToolThresholdTokens,
        Self::Tools,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Model => "PHI_MODEL",
            Self::Temperature => "PHI_TEMPERATURE",
            Self::MaxTokens => "PHI_MAX_TOKENS",
            Self::EnableReasoning => "PHI_ENABLE_REASONING",
            Self::ReasoningEffort => "PHI_REASONING_EFFORT",
            Self::Provider => "PHI_PROVIDER",
            Self::ApiBase => "PHI_API",
            Self::ApiKey => "PHI_KEY",
            Self::FakeProfile => "PHI_FAKE_PROFILE",
            Self::System => "PHI_SYSTEM",
            Self::MaxSteps => "PHI_MAX_STEPS",
            Self::ContextTokens => "PHI_CONTEXT_TOKENS",
            Self::ToolThresholdTokens => "PHI_TOOL_THRESHOLD_TOKENS",
            Self::Tools => "PHI_TOOLS",
        }
    }
}

pub(super) mod defaults {
    pub const MODEL: &str = "";
    pub const TEMPERATURE: f64 = 1.0;
    pub const MAX_TOKENS: u64 = 32768;
    pub const REASONING_ENABLED: bool = true;

    pub const PROVIDER: &str = "openai_chat";
    pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
    pub const API_KEY: &str = "";
    pub const FAKE_PROFILE: &str = "assistant_text";

    pub const MAX_STEPS: usize = 1_000_000;
    pub const CONTEXT_TOKENS: usize = 256 * 1024;

    pub const TOOL_THRESHOLD_TOKENS: usize = 8192;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::PhiConfigEnv;

    #[test]
    fn environment_names_are_unique_and_phi_scoped() {
        let names = PhiConfigEnv::ALL.map(PhiConfigEnv::name);
        assert!(names.iter().all(|name| name.starts_with("PHI_")));
        assert_eq!(
            names.len(),
            names.into_iter().collect::<BTreeSet<_>>().len()
        );
    }
}
