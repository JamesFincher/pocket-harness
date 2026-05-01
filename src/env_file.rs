use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::home_dir;

pub fn default_env_file() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pocket-harness")
        .join("env")
}

pub fn load_default_env_file(override_path: Option<&Path>) -> Result<Option<PathBuf>> {
    let path = override_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_env_file);
    if !path.exists() {
        return Ok(None);
    }
    load_env_file(&path)?;
    Ok(Some(path))
}

pub fn load_env_file(path: &Path) -> Result<()> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read env file {}", path.display()))?;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || env::var_os(key).is_some() {
            continue;
        }

        let value = unquote(value.trim());
        // Rust 2024 marks environment mutation unsafe because it can race with
        // other threads. The CLI loads this before starting any worker threads.
        unsafe {
            env::set_var(key, value);
        }
    }

    Ok(())
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::load_env_file;

    #[test]
    fn loads_env_file_without_overriding_existing_values() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("env");
        std::fs::write(
            &path,
            "POCKET_TEST_ENV_FILE_A=\"alpha\"\nPOCKET_TEST_ENV_FILE_B=beta\n",
        )
        .unwrap();

        unsafe {
            std::env::remove_var("POCKET_TEST_ENV_FILE_A");
            std::env::set_var("POCKET_TEST_ENV_FILE_B", "existing");
        }

        load_env_file(&path).unwrap();

        assert_eq!(std::env::var("POCKET_TEST_ENV_FILE_A").unwrap(), "alpha");
        assert_eq!(std::env::var("POCKET_TEST_ENV_FILE_B").unwrap(), "existing");
    }
}
