use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use pocket_harness::config_store::{ConfigSource, ConfigStore, digest_text};
use pocket_harness::connector::ConnectorManager;

#[derive(Debug, Parser)]
#[command(name = "pocket-harness")]
#[command(about = "A config-driven mobile harness gateway for local AI systems.")]
struct Cli {
    #[arg(short, long, default_value = "pocket-harness.yaml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write a complete default YAML config.
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Validate config and optionally run connector health checks.
    Check {
        #[arg(long)]
        health: bool,
    },
    /// Print predefined parent features and connector capability keys.
    Features,
    /// Run a prompt through a configured connector.
    Run {
        #[arg(short, long, default_value = "main")]
        thread: String,
        #[arg(required = true, trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// Check one connector, or all connectors when omitted.
    Health {
        connector: Option<String>,
    },
    /// Update a YAML value transactionally.
    Set {
        path: String,
        value: String,
    },
    /// Poll config and hot-promote valid changes.
    Watch {
        #[arg(long)]
        once: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = ConfigStore::new(cli.config);

    match cli.command {
        Command::Init { force } => {
            store.init_default(force)?;
            println!("initialized {}", store.config_path().display());
        }
        Command::Check { health } => {
            if health {
                let active = store.stage_with_connector_check(validate_all_connectors)?;
                print_active("config ok", &active);
            } else {
                let active = store.load_with_recovery()?;
                print_active("config ok", &active);
            }
        }
        Command::Features => {
            for feature in pocket_harness::features::registry() {
                let capability = feature
                    .connector_capability
                    .map(|capability| format!(" connector_capability={capability}"))
                    .unwrap_or_default();
                println!("{} - {}{}", feature.key, feature.description, capability);
            }
        }
        Command::Run { thread, prompt } => {
            let active = store.load_with_recovery()?;
            let prompt = prompt.join(" ");
            let manager = ConnectorManager::new(&active.config);
            let response = manager.run(&thread, &prompt)?;
            if response.ok {
                println!("{}", response.message);
            } else {
                anyhow::bail!("connector returned error: {}", response.message);
            }
        }
        Command::Health { connector } => {
            let active = store.load_with_recovery()?;
            let manager = ConnectorManager::new(&active.config);

            if let Some(name) = connector {
                let connector = active
                    .config
                    .connectors
                    .definitions
                    .get(&name)
                    .with_context(|| format!("unknown connector `{name}`"))?;
                let response = manager.health(&name, connector)?;
                println!("{}: {}", name, response.message);
                print_capabilities(&response.capabilities);
            } else {
                manager.check_all()?;
                println!("all connectors healthy");
            }
        }
        Command::Set { path, value } => {
            pocket_harness::yaml_edit::set_value(store.config_path(), &path, &value)?;
            let active = store.load_with_recovery()?;
            print_active("updated config", &active);
        }
        Command::Watch { once } => {
            watch_config(&store, once)?;
        }
    }

    Ok(())
}

fn validate_all_connectors(config: &pocket_harness::config::AppConfig) -> Result<()> {
    ConnectorManager::new(config).check_all()
}

fn watch_config(store: &ConfigStore, once: bool) -> Result<()> {
    let mut active = store.stage_with_connector_check(validate_all_connectors)?;
    print_active("active", &active);

    if once {
        return Ok(());
    }

    loop {
        let interval = if active.config.gateway.hot_reload.enabled {
            active.config.gateway.hot_reload.poll_interval_ms
        } else {
            5000
        };

        thread::sleep(Duration::from_millis(interval.max(250)));

        if !active.config.gateway.hot_reload.enabled {
            continue;
        }

        let text = fs::read_to_string(store.config_path())
            .with_context(|| format!("read config {}", store.config_path().display()))?;
        let digest = digest_text(&text);

        if digest == active.digest {
            continue;
        }

        match store.stage_with_connector_check(validate_all_connectors) {
            Ok(next) => {
                if next.source == ConfigSource::LastKnownGood {
                    println!("config changed but connector health failed; rolled back to last-known-good");
                } else {
                    println!("hot-promoted config {}", store.config_path().display());
                }
                active = next;
            }
            Err(error) => {
                let _ = store.write_rejection("hot_reload", &error);
                println!("rejected config change: {error}");
            }
        }
    }
}

fn print_active(label: &str, active: &pocket_harness::config_store::ActiveConfig) {
    let source = match active.source {
        ConfigSource::Primary => "primary",
        ConfigSource::LastKnownGood => "last-known-good",
    };
    println!(
        "{}: source={} data_dir={} digest={}",
        label,
        source,
        active.state_dir.display(),
        &active.digest[..12]
    );
}

fn print_capabilities(capabilities: &[String]) {
    if capabilities.is_empty() {
        return;
    }
    println!("capabilities:");
    for capability in capabilities {
        println!("- {capability}");
    }
}
