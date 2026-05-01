use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::AppConfig;
use crate::provider_catalog::catalog_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetTarget {
    Config,
    Service,
    Data,
    Logs,
    All,
}

pub fn confirm(target: ResetTarget, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }

    print!("This will reset {target:?}. Type YES to continue: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim() == "YES" {
        Ok(())
    } else {
        bail!("reset cancelled")
    }
}

pub fn reset_config(config_path: &Path, env_file: &Path) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let catalog = catalog_path(config_path, &AppConfig::default());

    remove_file(config_path, &mut removed)?;
    remove_file(&catalog, &mut removed)?;
    remove_file(env_file, &mut removed)?;

    Ok(removed)
}

pub fn reset_data(config_path: &Path) -> Result<Vec<PathBuf>> {
    let state_dir = default_installed_state_dir(config_path);
    let mut removed = Vec::new();

    remove_path(&state_dir.join("last-known-good.yaml"), &mut removed)?;
    remove_path(&state_dir.join("config-history"), &mut removed)?;
    remove_path(&state_dir.join("config-rejections.log"), &mut removed)?;
    remove_path(&state_dir.join("rejected-configs"), &mut removed)?;

    Ok(removed)
}

pub fn reset_logs(config_path: &Path) -> Result<Vec<PathBuf>> {
    let logs_dir = default_installed_state_dir(config_path).join("logs");
    let mut removed = Vec::new();
    remove_path(&logs_dir, &mut removed)?;
    Ok(removed)
}

fn default_installed_state_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn remove_file(path: &Path, removed: &mut Vec<PathBuf>) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
        removed.push(path.to_path_buf());
    }
    Ok(())
}

fn remove_path(path: &Path, removed: &mut Vec<PathBuf>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    if path.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    } else {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    removed.push(path.to_path_buf());
    Ok(())
}
