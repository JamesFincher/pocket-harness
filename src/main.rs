use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use pocket_harness::config_store::{ConfigSource, ConfigStore, digest_text};
use pocket_harness::connector::ConnectorManager;
use pocket_harness::env_file::{default_env_file, load_default_env_file};
use pocket_harness::provider_catalog::{
    ProviderCatalog, ensure_default_catalog, format_models, format_providers,
};
use pocket_harness::reset::ResetTarget;
use pocket_harness::service::ServiceOptions;
use pocket_harness::supervisor::{BackoffPolicy, BackoffSupervisor};

#[derive(Debug, Parser)]
#[command(name = "pocket-harness")]
#[command(version)]
#[command(about = "A config-driven mobile harness gateway for local AI systems.")]
struct Cli {
    #[arg(short, long, default_value = "pocket-harness.yaml")]
    config: PathBuf,

    #[arg(long)]
    env_file: Option<PathBuf>,

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
    Health { connector: Option<String> },
    /// Update a YAML value transactionally.
    Set { path: String, value: String },
    /// Poll config and hot-promote valid changes.
    Watch {
        #[arg(long)]
        once: bool,
    },
    /// Poll Telegram and handle setup/run commands.
    Telegram,
    /// List providers from providers.yaml.
    Providers,
    /// List models for a provider from providers.yaml.
    Models { provider: Option<String> },
    /// Install, control, or inspect the background service.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Reset installed config, service files, runtime data, or logs.
    Reset {
        target: ResetCliTarget,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Install and start the Pocket Harness service.
    Install {
        #[arg(long)]
        name: Option<String>,
    },
    /// Uninstall the Pocket Harness service.
    Uninstall {
        #[arg(long)]
        name: Option<String>,
    },
    /// Start the service.
    Start {
        #[arg(long)]
        name: Option<String>,
    },
    /// Stop the service.
    Stop {
        #[arg(long)]
        name: Option<String>,
    },
    /// Restart the service.
    Restart {
        #[arg(long)]
        name: Option<String>,
    },
    /// Print service manager status.
    Status {
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ResetCliTarget {
    Config,
    Service,
    Data,
    Logs,
    All,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let env_file = cli
        .env_file
        .or_else(|| std::env::var_os("POCKET_HARNESS_ENV_FILE").map(PathBuf::from))
        .unwrap_or_else(default_env_file);
    if let Err(error) = load_default_env_file(Some(&env_file)) {
        if should_continue_after_env_load_error(&cli.command) {
            log_cli_warning(&format!(
                "event=env_file_load_failure command=telegram path={:?} error={error:?}",
                env_file.display().to_string(),
            ));
        } else {
            return Err(error);
        }
    }
    let store = ConfigStore::new(cli.config);

    match cli.command {
        Command::Init { force } => {
            store.init_default(force)?;
            println!("initialized {}", store.config_path().display());
            let active = store.load_with_recovery()?;
            let catalog_path = ensure_default_catalog(store.config_path(), &active.config, force)?;
            println!("initialized {}", catalog_path.display());
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
            if active.config.llm_router.enabled {
                let catalog =
                    ProviderCatalog::load_for_config(store.config_path(), &active.config)?;
                let mut local_tools = pocket_harness::local_tools::LocalToolState::default();
                println!(
                    "{}",
                    pocket_harness::llm_router::run_prompt(
                        store.config_path(),
                        &active.config,
                        &catalog,
                        &thread,
                        &prompt,
                        &mut local_tools,
                    )?
                );
            } else {
                let manager = ConnectorManager::new(&active.config);
                let response = manager.run(&thread, &prompt)?;
                if response.ok {
                    println!("{}", response.message);
                } else {
                    anyhow::bail!("connector returned error: {}", response.message);
                }
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
        Command::Telegram => {
            pocket_harness::telegram::run_gateway(store)?;
        }
        Command::Providers => {
            let active = store.load_with_recovery()?;
            let catalog = ProviderCatalog::load_for_config(store.config_path(), &active.config)?;
            println!("{}", format_providers(&catalog));
        }
        Command::Models { provider } => {
            let active = store.load_with_recovery()?;
            let catalog = ProviderCatalog::load_for_config(store.config_path(), &active.config)?;
            let provider = provider.unwrap_or(active.config.llm_router.provider);
            println!("{}", format_models(&catalog, &provider)?);
        }
        Command::Service { command } => {
            run_service_command(command, store.config_path().to_path_buf(), env_file)?;
        }
        Command::Reset { target, yes } => {
            run_reset_command(target, yes, store.config_path().to_path_buf(), env_file)?;
        }
    }

    Ok(())
}

fn run_service_command(
    command: ServiceCommand,
    config_path: PathBuf,
    env_file: PathBuf,
) -> Result<()> {
    match command {
        ServiceCommand::Install { name } => {
            let options = ServiceOptions::new(config_path, env_file, name);
            let path = pocket_harness::service::install(&options)?;
            println!("installed service definition {}", path.display());
        }
        ServiceCommand::Uninstall { name } => {
            let options = ServiceOptions::new(config_path, env_file, name);
            pocket_harness::service::uninstall(&options)?;
            println!("uninstalled service");
        }
        ServiceCommand::Start { name } => {
            let options = ServiceOptions::new(config_path, env_file, name);
            pocket_harness::service::start(&options)?;
        }
        ServiceCommand::Stop { name } => {
            let options = ServiceOptions::new(config_path, env_file, name);
            pocket_harness::service::stop(&options)?;
        }
        ServiceCommand::Restart { name } => {
            let options = ServiceOptions::new(config_path, env_file, name);
            pocket_harness::service::restart(&options)?;
        }
        ServiceCommand::Status { name } => {
            let options = ServiceOptions::new(config_path, env_file, name);
            pocket_harness::service::status(&options)?;
        }
    }

    Ok(())
}

fn run_reset_command(
    target: ResetCliTarget,
    yes: bool,
    config_path: PathBuf,
    env_file: PathBuf,
) -> Result<()> {
    let target = match target {
        ResetCliTarget::Config => ResetTarget::Config,
        ResetCliTarget::Service => ResetTarget::Service,
        ResetCliTarget::Data => ResetTarget::Data,
        ResetCliTarget::Logs => ResetTarget::Logs,
        ResetCliTarget::All => ResetTarget::All,
    };

    pocket_harness::reset::confirm(target, yes)?;

    let mut removed = Vec::new();
    match target {
        ResetTarget::Config => {
            removed.extend(pocket_harness::reset::reset_config(
                &config_path,
                &env_file,
            )?);
        }
        ResetTarget::Service => {
            let options = ServiceOptions::new(config_path, env_file, None);
            pocket_harness::service::uninstall(&options)?;
        }
        ResetTarget::Data => {
            removed.extend(pocket_harness::reset::reset_data(&config_path)?);
        }
        ResetTarget::Logs => {
            removed.extend(pocket_harness::reset::reset_logs(&config_path)?);
        }
        ResetTarget::All => {
            let options = ServiceOptions::new(config_path.clone(), env_file.clone(), None);
            let _ = pocket_harness::service::uninstall(&options);
            removed.extend(pocket_harness::reset::reset_logs(&config_path)?);
            removed.extend(pocket_harness::reset::reset_data(&config_path)?);
            removed.extend(pocket_harness::reset::reset_config(
                &config_path,
                &env_file,
            )?);
        }
    }

    for path in removed {
        println!("removed {}", path.display());
    }
    println!("reset complete");
    Ok(())
}

fn validate_all_connectors(config: &pocket_harness::config::AppConfig) -> Result<()> {
    ConnectorManager::new(config).check_all()
}

fn should_continue_after_env_load_error(command: &Command) -> bool {
    matches!(command, Command::Telegram)
}

fn log_cli_warning(line: &str) {
    use std::io::Write;

    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{line}");
    let _ = stderr.flush();
}

fn watch_config(store: &ConfigStore, once: bool) -> Result<()> {
    let mut supervisor = BackoffSupervisor::new(BackoffPolicy::default());
    let mut active = loop {
        match store.stage_with_connector_check(validate_all_connectors) {
            Ok(active) => break active,
            Err(error) if once => return Err(error),
            Err(error) => {
                let delay = supervisor.record_failure();
                let _ = store.write_rejection("watch_startup", &error);
                println!(
                    "event=watch_config_failure phase=startup consecutive_failures={} retry_delay_ms={} error={error}",
                    delay.consecutive_failures,
                    delay.delay.as_millis(),
                );
                thread::sleep(delay.delay);
            }
        }
    };
    supervisor.record_success();
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

        let text = match fs::read_to_string(store.config_path())
            .with_context(|| format!("read config {}", store.config_path().display()))
        {
            Ok(text) => text,
            Err(error) => {
                let delay = supervisor.record_failure();
                let _ = store.write_rejection("hot_reload_read", &error);
                println!(
                    "event=watch_config_failure phase=read consecutive_failures={} retry_delay_ms={} error={error}",
                    delay.consecutive_failures,
                    delay.delay.as_millis(),
                );
                thread::sleep(delay.delay);
                continue;
            }
        };
        let digest = digest_text(&text);

        if digest == active.digest {
            continue;
        }

        match store.stage_with_connector_check(validate_all_connectors) {
            Ok(next) => {
                supervisor.record_success();
                if next.source == ConfigSource::LastKnownGood {
                    println!(
                        "config changed but connector health failed; rolled back to last-known-good"
                    );
                } else {
                    println!("hot-promoted config {}", store.config_path().display());
                }
                active = next;
            }
            Err(error) => {
                let _ = store.write_rejection("hot_reload", &error);
                let delay = supervisor.record_failure();
                println!(
                    "event=watch_config_failure phase=promote consecutive_failures={} retry_delay_ms={} error={error}",
                    delay.consecutive_failures,
                    delay.delay.as_millis(),
                );
                thread::sleep(delay.delay);
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

#[cfg(test)]
mod tests {
    use super::{Command, should_continue_after_env_load_error};

    #[test]
    fn telegram_keeps_running_after_env_file_load_error() {
        assert!(should_continue_after_env_load_error(&Command::Telegram));
        assert!(!should_continue_after_env_load_error(&Command::Check {
            health: false
        }));
    }
}
