use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing connector definition: {0}")]
    MissingConnector(String),
    #[error("default connector `{0}` is not defined")]
    MissingDefaultConnector(String),
    #[error("connector `{0}` uses json protocol but has no command")]
    EmptyConnectorCommand(String),
    #[error("connector `{0}` has timeout_seconds < 1")]
    InvalidConnectorTimeout(String),
    #[error("thread `{0}` references unknown connector `{1}`")]
    UnknownThreadConnector(String, String),
    #[error("thread `{0}` enables watch, but features.watch.enabled is false")]
    WatchGloballyDisabled(String),
    #[error("thread `{0}` enables queue, but features.queue.enabled is false")]
    QueueGloballyDisabled(String),
    #[error("telegram is enabled but mobile.telegram.bot_token is empty")]
    MissingTelegramToken,
    #[error("llm_router is enabled but llm_router.provider is empty")]
    MissingLlmProvider,
    #[error("llm_router is enabled but llm_router.model is empty")]
    MissingLlmModel,
    #[error("feature `{0}` is not available in the parent feature registry")]
    UnknownFeature(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub gateway: GatewayConfig,
    pub recovery: RecoveryConfig,
    pub features: FeaturesConfig,
    pub mobile: MobileConfig,
    pub llm_router: LlmRouterConfig,
    pub connectors: ConnectorsConfig,
    pub threads: BTreeMap<String, ThreadConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut threads = BTreeMap::new();
        threads.insert("main".to_string(), ThreadConfig::default());

        Self {
            schema_version: 1,
            gateway: GatewayConfig::default(),
            recovery: RecoveryConfig::default(),
            features: FeaturesConfig::default(),
            mobile: MobileConfig::default(),
            llm_router: LlmRouterConfig::default(),
            connectors: ConnectorsConfig::default(),
            threads,
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_features()?;

        if !self
            .connectors
            .definitions
            .contains_key(&self.connectors.default)
        {
            return Err(ConfigError::MissingDefaultConnector(
                self.connectors.default.clone(),
            ));
        }

        for (name, connector) in &self.connectors.definitions {
            if connector.timeout_seconds < 1 {
                return Err(ConfigError::InvalidConnectorTimeout(name.clone()));
            }

            if connector.kind == ConnectorKind::Json && connector.command.is_empty() {
                return Err(ConfigError::EmptyConnectorCommand(name.clone()));
            }
        }

        for (thread_name, thread) in &self.threads {
            let connector = thread
                .connector
                .clone()
                .unwrap_or_else(|| self.connectors.default.clone());

            if !self.connectors.definitions.contains_key(&connector) {
                return Err(ConfigError::UnknownThreadConnector(
                    thread_name.clone(),
                    connector,
                ));
            }

            if thread.watch.enabled && !self.features.watch.enabled {
                return Err(ConfigError::WatchGloballyDisabled(thread_name.clone()));
            }

            if thread.queue.enabled && !self.features.queue.enabled {
                return Err(ConfigError::QueueGloballyDisabled(thread_name.clone()));
            }
        }

        if self.mobile.telegram.enabled
            && expand_string(&self.mobile.telegram.bot_token)
                .trim()
                .is_empty()
        {
            return Err(ConfigError::MissingTelegramToken);
        }

        if self.llm_router.enabled {
            if self.llm_router.provider.trim().is_empty() {
                return Err(ConfigError::MissingLlmProvider);
            }
            if self.llm_router.model.trim().is_empty() {
                return Err(ConfigError::MissingLlmModel);
            }
        }

        Ok(())
    }

    fn validate_features(&self) -> Result<(), ConfigError> {
        let known = crate::features::registry()
            .iter()
            .map(|feature| feature.key)
            .collect::<BTreeSet<_>>();

        for feature in self.enabled_feature_keys() {
            if !known.contains(feature.as_str()) {
                return Err(ConfigError::UnknownFeature(feature));
            }
        }

        Ok(())
    }

    pub fn enabled_feature_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();

        if self.mobile.telegram.enabled {
            keys.push("gateway.telegram".to_string());
        }
        if self.llm_router.enabled || self.features.llm_router.enabled {
            keys.push("gateway.llm_router".to_string());
        }
        if self.features.queue.enabled {
            keys.push("jobs.queue".to_string());
        }
        if self.features.cancel.enabled {
            keys.push("jobs.cancel".to_string());
        }
        if self.features.history.enabled {
            keys.push("jobs.history".to_string());
        }
        if self.features.threads.enabled {
            keys.push("threads.named".to_string());
            keys.push("threads.cwd".to_string());
        }
        if self.features.watch.enabled {
            keys.push("threads.watch".to_string());
        }
        if self.features.images.enabled {
            keys.push("attachments.images".to_string());
        }
        if self.features.file_transfer.enabled {
            keys.push("attachments.files".to_string());
        }
        if self.features.screenshots.enabled {
            keys.push("mac.screenshot".to_string());
        }
        if self.features.terminal.enabled {
            keys.push("mac.terminal".to_string());
        }

        keys.push("connector.health".to_string());
        keys.push("connector.run".to_string());
        keys.push("connector.status".to_string());
        keys.push("connector.capabilities".to_string());

        keys.sort();
        keys.dedup();
        keys
    }

    pub fn connector_for_thread(
        &self,
        thread_name: &str,
    ) -> Result<(&str, &ConnectorConfig), ConfigError> {
        let thread = self.threads.get(thread_name);
        let connector_name = thread
            .and_then(|thread| thread.connector.as_deref())
            .unwrap_or(&self.connectors.default);

        let connector = self
            .connectors
            .definitions
            .get(connector_name)
            .ok_or_else(|| ConfigError::MissingConnector(connector_name.to_string()))?;

        Ok((connector_name, connector))
    }

    pub fn thread_or_default(&self, thread_name: &str) -> ThreadConfig {
        self.threads
            .get(thread_name)
            .cloned()
            .or_else(|| self.threads.get("main").cloned())
            .unwrap_or_default()
    }

    pub fn data_dir(&self, config_path: &Path) -> PathBuf {
        let raw = expand_string(&self.gateway.data_dir);
        if raw.trim().is_empty() {
            default_state_dir(config_path)
        } else {
            expand_path(&raw)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    pub name: String,
    pub data_dir: String,
    pub log_level: String,
    pub hot_reload: HotReloadConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            name: "pocket-harness".to_string(),
            data_dir: "~/.pocket-harness".to_string(),
            log_level: "info".to_string(),
            hot_reload: HotReloadConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotReloadConfig {
    pub enabled: bool,
    pub poll_interval_ms: u64,
}

impl Default for HotReloadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_ms: 1500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RecoveryConfig {
    pub enabled: bool,
    pub on_invalid_config: RecoveryAction,
    pub on_connector_break: RecoveryAction,
    pub write_rejection_report: bool,
    pub keep_history: usize,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_invalid_config: RecoveryAction::KeepActive,
            on_connector_break: RecoveryAction::RollbackToLastKnownGood,
            write_rejection_report: true,
            keep_history: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    KeepActive,
    RollbackToLastKnownGood,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeaturesConfig {
    pub screenshots: FeatureToggle,
    pub file_transfer: FileTransferFeature,
    pub terminal: TerminalFeature,
    pub watch: WatchFeature,
    pub queue: QueueFeature,
    pub cancel: FeatureToggle,
    pub history: HistoryFeature,
    pub threads: FeatureToggle,
    pub images: ImageFeature,
    pub llm_router: FeatureToggle,
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            screenshots: FeatureToggle { enabled: true },
            file_transfer: FileTransferFeature::default(),
            terminal: TerminalFeature::default(),
            watch: WatchFeature::default(),
            queue: QueueFeature::default(),
            cancel: FeatureToggle { enabled: true },
            history: HistoryFeature::default(),
            threads: FeatureToggle { enabled: true },
            images: ImageFeature::default(),
            llm_router: FeatureToggle { enabled: false },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureToggle {
    pub enabled: bool,
}

impl Default for FeatureToggle {
    fn default() -> Self {
        Self { enabled: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileTransferFeature {
    pub enabled: bool,
    pub max_bytes: u64,
    pub allow_sensitive_paths: bool,
}

impl Default for FileTransferFeature {
    fn default() -> Self {
        Self {
            enabled: true,
            max_bytes: 20 * 1024 * 1024,
            allow_sensitive_paths: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalFeature {
    pub enabled: bool,
    pub max_sessions: usize,
    pub read_limit: usize,
}

impl Default for TerminalFeature {
    fn default() -> Self {
        Self {
            enabled: true,
            max_sessions: 4,
            read_limit: 4000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchFeature {
    pub enabled: bool,
    pub default_interval_seconds: u64,
}

impl Default for WatchFeature {
    fn default() -> Self {
        Self {
            enabled: true,
            default_interval_seconds: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueFeature {
    pub enabled: bool,
    pub max_depth: usize,
}

impl Default for QueueFeature {
    fn default() -> Self {
        Self {
            enabled: true,
            max_depth: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryFeature {
    pub enabled: bool,
    pub max_receipts: usize,
}

impl Default for HistoryFeature {
    fn default() -> Self {
        Self {
            enabled: true,
            max_receipts: 200,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageFeature {
    pub enabled: bool,
    pub max_images_per_message: usize,
    pub max_bytes: u64,
    pub retention_days: u64,
}

impl Default for ImageFeature {
    fn default() -> Self {
        Self {
            enabled: true,
            max_images_per_message: 10,
            max_bytes: 20 * 1024 * 1024,
            retention_days: 7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MobileConfig {
    pub telegram: TelegramConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub bot_token: String,
    pub allowed_users: Vec<i64>,
    pub allowed_chats: Vec<i64>,
    pub allow_group_chats: bool,
    pub reply_to_messages: bool,
    pub poll_timeout_seconds: u64,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: "$TELEGRAM_BOT_TOKEN".to_string(),
            allowed_users: Vec::new(),
            allowed_chats: Vec::new(),
            allow_group_chats: false,
            reply_to_messages: false,
            poll_timeout_seconds: 25,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmRouterConfig {
    pub enabled: bool,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub natural_commands: bool,
    pub polish_replies: bool,
}

impl Default for LlmRouterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "openai_compatible".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "$OPENAI_API_KEY".to_string(),
            model: "".to_string(),
            natural_commands: true,
            polish_replies: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectorsConfig {
    pub default: String,
    pub definitions: BTreeMap<String, ConnectorConfig>,
}

impl Default for ConnectorsConfig {
    fn default() -> Self {
        let mut definitions = BTreeMap::new();
        definitions.insert(
            "echo".to_string(),
            ConnectorConfig {
                kind: ConnectorKind::BuiltinEcho,
                display_name: "Echo".to_string(),
                command: Vec::new(),
                cwd: ".".to_string(),
                timeout_seconds: 30,
                env: BTreeMap::new(),
                settings: BTreeMap::new(),
            },
        );

        Self {
            default: "echo".to_string(),
            definitions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    BuiltinEcho,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectorConfig {
    #[serde(rename = "type")]
    pub kind: ConnectorKind,
    pub display_name: String,
    pub command: Vec<String>,
    pub cwd: String,
    pub timeout_seconds: u64,
    pub env: BTreeMap<String, String>,
    pub settings: BTreeMap<String, Value>,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        Self {
            kind: ConnectorKind::BuiltinEcho,
            display_name: "".to_string(),
            command: Vec::new(),
            cwd: ".".to_string(),
            timeout_seconds: 60,
            env: BTreeMap::new(),
            settings: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadConfig {
    pub connector: Option<String>,
    pub cwd: String,
    pub watch: ThreadWatchConfig,
    pub queue: ThreadQueueConfig,
    pub reply_style: ReplyStyle,
}

impl Default for ThreadConfig {
    fn default() -> Self {
        Self {
            connector: None,
            cwd: "~".to_string(),
            watch: ThreadWatchConfig::default(),
            queue: ThreadQueueConfig::default(),
            reply_style: ReplyStyle::Normal,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadWatchConfig {
    pub enabled: bool,
}

impl Default for ThreadWatchConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadQueueConfig {
    pub enabled: bool,
}

impl Default for ThreadQueueConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyStyle {
    Brief,
    Normal,
    Verbose,
}

pub fn expand_path(raw: &str) -> PathBuf {
    let expanded = expand_string(raw);
    if let Some(rest) = expanded.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }

    if expanded == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }

    PathBuf::from(expanded)
}

pub fn expand_string(raw: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }

        if i + 1 >= chars.len() {
            out.push('$');
            i += 1;
            continue;
        }

        if chars[i + 1] == '{' {
            let mut j = i + 2;
            while j < chars.len() && chars[j] != '}' {
                j += 1;
            }
            if j < chars.len() {
                let key: String = chars[i + 2..j].iter().collect();
                out.push_str(&env::var(key).unwrap_or_default());
                i = j + 1;
            } else {
                out.push('$');
                i += 1;
            }
            continue;
        }

        let mut j = i + 1;
        while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
            j += 1;
        }

        if j == i + 1 {
            out.push('$');
            i += 1;
        } else {
            let key: String = chars[i + 1..j].iter().collect();
            out.push_str(&env::var(key).unwrap_or_default());
            i = j;
        }
    }

    out
}

pub fn default_state_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".pocket-harness-state")
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

impl Default for ConnectorKind {
    fn default() -> Self {
        Self::BuiltinEcho
    }
}

impl Default for ReplyStyle {
    fn default() -> Self {
        Self::Normal
    }
}
