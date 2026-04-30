use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};

use crate::config::{AppConfig, RecoveryAction, default_state_dir};

#[derive(Debug, Clone)]
pub struct ActiveConfig {
    pub config: AppConfig,
    pub source: ConfigSource,
    pub digest: String,
    pub state_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    Primary,
    LastKnownGood,
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    config_path: PathBuf,
}

impl ConfigStore {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn init_default(&self, force: bool) -> Result<()> {
        if self.config_path.exists() && !force {
            return Err(anyhow!(
                "config already exists: {} (use --force to overwrite)",
                self.config_path.display()
            ));
        }

        let config = AppConfig::default();
        let text = serde_yaml::to_string(&config).context("serialize default config")?;
        atomic_write(&self.config_path, &text)?;
        self.promote(&config, &text)?;
        Ok(())
    }

    pub fn load_primary(&self) -> Result<ActiveConfig> {
        let text = fs::read_to_string(&self.config_path)
            .with_context(|| format!("read config {}", self.config_path.display()))?;
        let config = parse_and_validate(&text)?;
        let state_dir = config.data_dir(&self.config_path);

        Ok(ActiveConfig {
            config,
            source: ConfigSource::Primary,
            digest: digest_text(&text),
            state_dir,
        })
    }

    pub fn load_with_recovery(&self) -> Result<ActiveConfig> {
        match self.load_primary() {
            Ok(active) => {
                let text = fs::read_to_string(&self.config_path)?;
                self.promote(&active.config, &text)?;
                Ok(active)
            }
            Err(primary_error) => {
                self.write_rejection("invalid_config", &primary_error)?;
                self.load_last_known_good()
                    .with_context(|| format!("primary config failed and no usable last-known-good exists: {primary_error}"))
            }
        }
    }

    pub fn stage_with_connector_check<F>(&self, check_connectors: F) -> Result<ActiveConfig>
    where
        F: Fn(&AppConfig) -> Result<()>,
    {
        let text = fs::read_to_string(&self.config_path)
            .with_context(|| format!("read config {}", self.config_path.display()))?;
        let config = parse_and_validate(&text)?;

        match check_connectors(&config) {
            Ok(()) => {
                self.promote(&config, &text)?;
                Ok(ActiveConfig {
                    state_dir: config.data_dir(&self.config_path),
                    config,
                    source: ConfigSource::Primary,
                    digest: digest_text(&text),
                })
            }
            Err(connector_error) => {
                self.write_rejection("connector_break", &connector_error)?;

                if config.recovery.enabled
                    && config.recovery.on_connector_break == RecoveryAction::RollbackToLastKnownGood
                {
                    self.restore_last_known_good_to_primary()
                        .context("restore last-known-good after connector break")?;
                    self.load_last_known_good().with_context(|| {
                        format!("connector check failed and rollback failed: {connector_error}")
                    })
                } else {
                    Err(connector_error)
                }
            }
        }
    }

    pub fn promote(&self, config: &AppConfig, text: &str) -> Result<()> {
        let state_dir = config.data_dir(&self.config_path);
        self.write_last_known_good_snapshot(config, text, &state_dir)?;

        let fallback_state_dir = default_state_dir(&self.config_path);
        if fallback_state_dir != state_dir {
            self.write_last_known_good_snapshot(config, text, &fallback_state_dir)?;
        }

        Ok(())
    }

    fn write_last_known_good_snapshot(
        &self,
        config: &AppConfig,
        text: &str,
        state_dir: &Path,
    ) -> Result<()> {
        fs::create_dir_all(state_dir)
            .with_context(|| format!("create state dir {}", state_dir.display()))?;

        let lkg_path = last_known_good_path(state_dir);
        atomic_write(&lkg_path, text)?;

        let history_dir = state_dir.join("config-history");
        fs::create_dir_all(&history_dir)?;
        let history_path = history_dir.join(format!("{}.yaml", timestamp()));
        atomic_write(&history_path, text)?;

        prune_history(&history_dir, config.recovery.keep_history)?;
        Ok(())
    }

    pub fn load_last_known_good(&self) -> Result<ActiveConfig> {
        let state_dir = self.best_state_dir();
        let lkg_path = last_known_good_path(&state_dir);
        let text = fs::read_to_string(&lkg_path)
            .with_context(|| format!("read last-known-good {}", lkg_path.display()))?;
        let config = parse_and_validate(&text)?;

        Ok(ActiveConfig {
            config,
            source: ConfigSource::LastKnownGood,
            digest: digest_text(&text),
            state_dir,
        })
    }

    pub fn restore_last_known_good_to_primary(&self) -> Result<()> {
        let state_dir = self.best_state_dir();
        let lkg_path = last_known_good_path(&state_dir);
        let text = fs::read_to_string(&lkg_path)
            .with_context(|| format!("read last-known-good {}", lkg_path.display()))?;

        if self.config_path.exists() {
            let rejected_dir = state_dir.join("rejected-configs");
            fs::create_dir_all(&rejected_dir)?;
            let rejected_path = rejected_dir.join(format!("{}.yaml", timestamp()));
            fs::copy(&self.config_path, rejected_path)?;
        }

        atomic_write(&self.config_path, &text)?;
        Ok(())
    }

    pub fn write_rejection(&self, kind: &str, error: &anyhow::Error) -> Result<()> {
        let state_dir = self.best_state_dir();
        fs::create_dir_all(&state_dir)?;
        let path = state_dir.join("config-rejections.log");
        let line = format!("{} kind={} error={}\n", timestamp(), kind, error);
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all_private(line.as_bytes())?;
        Ok(())
    }

    fn best_state_dir(&self) -> PathBuf {
        match self.load_primary() {
            Ok(active) => active.state_dir,
            Err(_) => default_state_dir(&self.config_path),
        }
    }
}

pub fn parse_and_validate(text: &str) -> Result<AppConfig> {
    let config: AppConfig = serde_yaml::from_str(text).context("parse yaml config")?;
    config.validate().context("validate config")?;
    Ok(config)
}

pub fn digest_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn last_known_good_path(state_dir: &Path) -> PathBuf {
    state_dir.join("last-known-good.yaml")
}

pub fn atomic_write(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("yaml")
    ));

    fs::write(&tmp_path, text)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{:09}", now.as_secs(), now.subsec_nanos())
}

fn prune_history(history_dir: &Path, keep: usize) -> Result<()> {
    if keep == 0 || !history_dir.exists() {
        return Ok(());
    }

    let mut entries = fs::read_dir(history_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .collect::<Vec<_>>();

    entries.sort_by_key(|entry| entry.file_name());

    if entries.len() <= keep {
        return Ok(());
    }

    let remove_count = entries.len() - keep;

    for entry in entries.into_iter().take(remove_count) {
        let _ = fs::remove_file(entry.path());
    }

    Ok(())
}

trait PrivateWrite {
    fn write_all_private(&mut self, bytes: &[u8]) -> Result<()>;
}

impl PrivateWrite for fs::File {
    fn write_all_private(&mut self, bytes: &[u8]) -> Result<()> {
        use std::io::Write;
        self.write_all(bytes)?;
        Ok(())
    }
}
