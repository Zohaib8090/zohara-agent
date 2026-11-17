//! The model side of Zohara Agent: providers, API keys, the wire formats and the tool-calling loop.

pub mod accounts;
pub mod agent;
pub mod claude_code;
pub mod context;
pub mod custom;
pub mod keys;
pub mod memory_agent;
#[cfg(test)]
pub(crate) mod mock;
pub mod personality_gen;
pub mod models;
pub mod probe;
pub mod profiles;
pub mod providers;
pub mod ui_prefs;
pub mod wire;

use providers::Provider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// The chosen provider and model, plus any base URL the person changed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Selection {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub base_urls: HashMap<String, String>,
    /// The person's word on whether the chosen model can look at pictures; none means "decide from the model's name".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    /// How hard the model should think (low, medium, high, xhigh, max); none means the program's own default. Used by Claude Code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The account of the chosen provider that is in use (its id; none = the Default account). See `accounts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The saved profile in use (see `profiles`): it decides which API key is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The "API format" hint of a hosted provider that is compatible with OpenAI (`chat`, `responses` or `claude`, by provider id);
    /// none means the provider's own.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub api_formats: HashMap<String, String>,
}

impl Selection {
    fn file() -> PathBuf {
        crate::audit::config_dir().join("ai.json")
    }

    pub fn load() -> Selection {
        std::fs::read_to_string(Self::file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::audit::config_dir());
        let tmp = Self::file().with_extension("json.tmp");
        if let Ok(s) = serde_json::to_string_pretty(self) {
            if !crate::tenant::fits(&Self::file(), s.len()) {
                return;
            }
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, Self::file());
            }
        }
    }

    /// The provider with the API format the person chose for it (`Provider::with_format`); the provider itself when none was.
    pub fn provider_with_format(&self, p: &'static Provider) -> &'static Provider {
        p.with_format(self.api_formats.get(p.id).map(String::as_str))
    }

    /// The account in use for `provider_id` (only the chosen provider has one): none means the Default account.
    pub fn account_for(&self, provider_id: &str) -> Option<&str> {
        if self.provider == provider_id { self.account.as_deref().filter(|a| *a != accounts::DEFAULT_ID) } else { None }
    }

    pub fn base_url_for(&self, p: &Provider) -> String {
        if let Some(u) = self.account_for(p.id).and_then(|a| accounts::address_of(p.id, a)) {
            return u;
        }
        self.base_urls.get(p.id).filter(|u| !u.trim().is_empty()).cloned().unwrap_or_else(|| p.base_url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_base_url_wins_and_an_empty_one_is_ignored() {
        let p = providers::find("custom:t-chat").unwrap();
        let mut s = Selection::default();
        assert_eq!(s.base_url_for(p), p.base_url);
        s.base_urls.insert("custom:t-chat".into(), "http://10.0.0.5:9000/v1".into());
        assert_eq!(s.base_url_for(p), "http://10.0.0.5:9000/v1");
        s.base_urls.insert("custom:t-chat".into(), "  ".into());
        assert_eq!(s.base_url_for(p), p.base_url);
    }

    #[test]
    fn provider_ids_are_unique_and_urls_look_like_urls() {
        let all = providers::all();
        for (i, p) in all.iter().enumerate() {
            assert!(p.base_url.starts_with("http") || p.protocol == providers::Protocol::ClaudeCode, "{}", p.id);
            assert!(all[i + 1..].iter().all(|q| q.id != p.id), "duplicate {}", p.id);
        }
    }
}
