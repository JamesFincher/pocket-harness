use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, expand_path};
use crate::config_store::atomic_write;

pub const DEFAULT_CATALOG: &str = include_str!("../providers.yaml");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderCatalog {
    pub schema_version: u32,
    pub updated: String,
    pub providers: BTreeMap<String, ProviderDefinition>,
}

impl Default for ProviderCatalog {
    fn default() -> Self {
        Self {
            schema_version: 1,
            updated: String::new(),
            providers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderDefinition {
    pub display_name: String,
    pub api_format: String,
    pub base_url: String,
    pub token_env: String,
    pub default_model: String,
    pub allow_custom_models: bool,
    pub docs: Vec<String>,
    pub models: BTreeMap<String, ModelDefinition>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelDefinition {
    pub display_name: String,
    pub provider_model_id: String,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub input_usd_per_1m: Option<f64>,
    pub output_usd_per_1m: Option<f64>,
    pub cached_input_usd_per_1m: Option<f64>,
    pub capabilities: Vec<String>,
    pub notes: String,
}

impl ProviderCatalog {
    pub fn bundled() -> Result<Self> {
        serde_yaml::from_str(DEFAULT_CATALOG).context("parse bundled providers.yaml")
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read provider catalog {}", path.display()))?;
        serde_yaml::from_str(&text)
            .with_context(|| format!("parse provider catalog {}", path.display()))
    }

    pub fn load_for_config(config_path: &Path, config: &AppConfig) -> Result<Self> {
        Self::load(&catalog_path(config_path, config))
    }

    pub fn provider(&self, provider_id: &str) -> Result<&ProviderDefinition> {
        self.providers
            .get(provider_id)
            .ok_or_else(|| anyhow!("unknown provider `{provider_id}`"))
    }

    pub fn model(&self, provider_id: &str, model_id: &str) -> Result<&ModelDefinition> {
        let provider = self.provider(provider_id)?;
        provider
            .models
            .get(model_id)
            .ok_or_else(|| anyhow!("unknown model `{model_id}` for provider `{provider_id}`"))
    }

    pub fn model_or_custom(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<&ModelDefinition>> {
        let provider = self.provider(provider_id)?;
        if let Some(model) = provider.models.get(model_id) {
            Ok(Some(model))
        } else if provider.allow_custom_models {
            Ok(None)
        } else {
            Err(anyhow!(
                "unknown model `{model_id}` for provider `{provider_id}`"
            ))
        }
    }

    pub fn default_model_for(&self, provider_id: &str) -> Result<&str> {
        let provider = self.provider(provider_id)?;
        if provider.default_model.trim().is_empty() {
            provider
                .models
                .keys()
                .next()
                .map(String::as_str)
                .ok_or_else(|| anyhow!("provider `{provider_id}` has no models"))
        } else {
            Ok(provider.default_model.as_str())
        }
    }
}

pub fn catalog_path(config_path: &Path, config: &AppConfig) -> PathBuf {
    resolve_catalog_path(config_path, &config.llm_router.catalog_path)
}

pub fn resolve_catalog_path(config_path: &Path, raw_path: &str) -> PathBuf {
    let expanded = expand_path(raw_path);
    if expanded.is_absolute() {
        expanded
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(expanded)
    }
}

pub fn ensure_default_catalog(
    config_path: &Path,
    config: &AppConfig,
    force: bool,
) -> Result<PathBuf> {
    let path = catalog_path(config_path, config);
    if path.exists() && !force {
        return Ok(path);
    }

    atomic_write(&path, DEFAULT_CATALOG)?;
    Ok(path)
}

pub fn format_providers(catalog: &ProviderCatalog) -> String {
    catalog
        .providers
        .iter()
        .map(|(id, provider)| format!("{id} - {}", provider.display_name))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_models(catalog: &ProviderCatalog, provider_id: &str) -> Result<String> {
    let provider = catalog.provider(provider_id)?;
    let mut lines = vec![format!("{} models:", provider.display_name)];
    for (id, model) in &provider.models {
        let context = model
            .context_window
            .map(human_number)
            .unwrap_or_else(|| "unknown context".to_string());
        let price = match (model.input_usd_per_1m, model.output_usd_per_1m) {
            (Some(input), Some(output)) => format!("${input:.2}/1M in, ${output:.2}/1M out"),
            _ => "pricing in provider docs".to_string(),
        };
        lines.push(format!("- {id} ({context}; {price})"));
    }
    Ok(lines.join("\n"))
}

fn human_number(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{}M context", value / 1_000_000)
    } else if value >= 1_000 {
        format!("{}K context", value / 1_000)
    } else {
        format!("{value} context")
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderCatalog, format_models};

    #[test]
    fn bundled_catalog_loads_major_providers() {
        let catalog = ProviderCatalog::bundled().unwrap();

        assert!(catalog.providers.contains_key("openai"));
        assert!(catalog.providers.contains_key("anthropic"));
        assert!(catalog.providers.contains_key("google"));
        assert!(catalog.providers.contains_key("openrouter"));
        assert!(
            catalog
                .provider("openai")
                .unwrap()
                .models
                .contains_key("gpt-5.5")
        );
    }

    #[test]
    fn model_formatter_includes_context_and_price() {
        let catalog = ProviderCatalog::bundled().unwrap();
        let models = format_models(&catalog, "openai").unwrap();

        assert!(models.contains("gpt-5.5"));
        assert!(models.contains("1M context"));
        assert!(models.contains("$5.00/1M in"));
    }
}
