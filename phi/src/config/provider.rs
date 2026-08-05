use serde::Deserialize;

use super::schema::defaults;

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderConfig {
    pub kind: String,
    pub api_base: String,
    pub api_key: String,
    pub fake_profile: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ProviderConfigPatch {
    pub kind: Option<String>,
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub fake_profile: Option<String>,
}

impl ProviderConfig {
    pub(super) fn apply(&mut self, patch: ProviderConfigPatch) {
        if let Some(value) = patch.kind {
            self.kind = value;
        }
        if let Some(value) = patch.api_base {
            self.api_base = value;
        }
        if let Some(value) = patch.api_key {
            self.api_key = value;
        }
        if let Some(value) = patch.fake_profile {
            self.fake_profile = value;
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: defaults::PROVIDER.to_string(),
            api_base: defaults::OPENAI_BASE_URL.to_string(),
            api_key: defaults::API_KEY.to_string(),
            fake_profile: defaults::FAKE_PROFILE.to_string(),
        }
    }
}
