use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, TelegramConfig, expand_string};
use crate::config_store::{ConfigSource, ConfigStore};
use crate::connector::ConnectorManager;
use crate::local_tools::{LocalToolCall, LocalToolState, try_parse_natural};
use crate::provider_catalog::{ProviderCatalog, format_models, format_providers};
use crate::supervisor::{BackoffPolicy, BackoffSupervisor, FailureDecision};
use crate::yaml_edit::{set_value, set_values};

pub fn run_gateway(store: ConfigStore) -> Result<()> {
    let mut supervisor = BackoffSupervisor::new(BackoffPolicy::default());
    log_gateway_info(&format!(
        "event=telegram_gateway_start config_path={:?}",
        store.config_path().display().to_string()
    ));

    let client = loop {
        match Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
        {
            Ok(client) => break client,
            Err(error) => {
                let error = anyhow!(error);
                let delay = supervisor.record_failure();
                log_gateway_failure(
                    GatewayFailureKind::HttpClient,
                    "create Telegram HTTP client",
                    delay,
                    None,
                    &error,
                );
                thread::sleep(delay.delay);
            }
        }
    };
    supervisor.record_success();
    let mut offset = None;
    let mut local_tools = LocalToolState::default();
    let mut logged_polling_ready = false;
    let mut logged_first_poll_success = false;

    loop {
        let active = match store.load_with_recovery() {
            Ok(active) => active,
            Err(error) => {
                logged_polling_ready = false;
                logged_first_poll_success = false;
                let delay = supervisor.record_failure();
                log_gateway_failure(
                    GatewayFailureKind::ConfigLoad,
                    "load active config",
                    delay,
                    None,
                    &error,
                );
                thread::sleep(delay.delay);
                continue;
            }
        };
        let telegram = &active.config.mobile.telegram;
        if !telegram.enabled {
            logged_polling_ready = false;
            logged_first_poll_success = false;
            let error = anyhow!("mobile.telegram.enabled is false");
            let delay = supervisor.record_failure();
            log_gateway_failure(
                GatewayFailureKind::TelegramDisabled,
                "check Telegram readiness",
                delay,
                None,
                &error,
            );
            thread::sleep(delay.delay);
            continue;
        }

        let token = expand_string(&telegram.bot_token);
        if token.trim().is_empty() {
            logged_polling_ready = false;
            logged_first_poll_success = false;
            let error = anyhow!("mobile.telegram.bot_token is empty");
            let delay = supervisor.record_failure();
            log_gateway_failure(
                GatewayFailureKind::MissingToken,
                "check Telegram readiness",
                delay,
                None,
                &error,
            );
            thread::sleep(delay.delay);
            continue;
        }

        if !logged_polling_ready {
            log_gateway_info(&format!(
                "event=telegram_gateway_ready state=polling config_source={} data_dir={:?} poll_timeout_seconds={} token=configured",
                config_source_name(&active.source),
                active.state_dir.display().to_string(),
                telegram.poll_timeout_seconds,
            ));
            logged_polling_ready = true;
        }

        let updates = match get_updates(&client, &token, offset, telegram.poll_timeout_seconds) {
            Ok(updates) => {
                supervisor.record_success();
                if !logged_first_poll_success {
                    log_gateway_info(&format!(
                        "event=telegram_gateway_poll_success update_count={} next_offset={}",
                        updates.len(),
                        offset
                            .map(|offset| offset.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                    ));
                    logged_first_poll_success = true;
                }
                updates
            }
            Err(error) => {
                logged_polling_ready = false;
                logged_first_poll_success = false;
                let delay = supervisor.record_failure();
                log_gateway_failure(
                    GatewayFailureKind::TelegramPoll,
                    "poll Telegram updates",
                    delay,
                    Some(&token),
                    &error,
                );
                thread::sleep(delay.delay);
                continue;
            }
        };

        for update in updates {
            offset = Some(update.update_id + 1);
            let Some(message) = update.message else {
                continue;
            };
            let Some(text) = message.text.as_deref() else {
                continue;
            };

            let active = match store.load_with_recovery() {
                Ok(active) => active,
                Err(error) => {
                    let _ = send_message(
                        &client,
                        &token,
                        message.chat.id,
                        &format!("Config load failed: {error}"),
                    );
                    continue;
                }
            };

            if !allowed(telegram, &message) {
                let _ = send_message(
                    &client,
                    &token,
                    message.chat.id,
                    "This chat is not allowed to control Pocket Harness.",
                );
                continue;
            }

            let reply = command_reply(|| {
                let catalog = ProviderCatalog::load_for_config(store.config_path(), &active.config)
                    .or_else(|_| ProviderCatalog::bundled())?;
                handle_text_with_state(
                    store.config_path(),
                    &active.config,
                    &catalog,
                    text,
                    &mut local_tools,
                )
            });

            if let Err(error) = send_message(&client, &token, message.chat.id, &reply) {
                let delay = supervisor.record_failure();
                log_gateway_failure(
                    GatewayFailureKind::TelegramSend,
                    "send Telegram reply",
                    delay,
                    Some(&token),
                    &error,
                );
            }
        }

        thread::sleep(Duration::from_millis(250));
    }
}

pub fn handle_text(
    config_path: &Path,
    config: &AppConfig,
    catalog: &ProviderCatalog,
    text: &str,
) -> Result<String> {
    let mut local_tools = LocalToolState::default();
    handle_text_with_state(config_path, config, catalog, text, &mut local_tools)
}

fn handle_text_with_state(
    config_path: &Path,
    config: &AppConfig,
    catalog: &ProviderCatalog,
    text: &str,
    local_tools: &mut LocalToolState,
) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(help_text());
    }

    if !trimmed.starts_with('/') {
        if let Some(call) = try_parse_natural(trimmed) {
            if config.llm_router.enabled && should_route_tool_request_through_ai(trimmed, &call) {
                return run_prompt(config_path, config, catalog, "main", trimmed, local_tools);
            }
            return local_tools.run_tool(config_path, config, "main", &call);
        }
        return run_prompt(config_path, config, catalog, "main", trimmed, local_tools);
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or_default();
    let args = parts.next().unwrap_or_default().trim();
    let command = command
        .split('@')
        .next()
        .unwrap_or(command)
        .to_ascii_lowercase();

    match command.as_str() {
        "/start" | "/help" => Ok(help_text()),
        "/status" => Ok(status_text(config)),
        "/providers" => Ok(format_providers(catalog)),
        "/models" => {
            let provider = if args.is_empty() {
                config.llm_router.provider.as_str()
            } else {
                args
            };
            format_models(catalog, provider)
        }
        "/provider" => set_provider(config_path, catalog, args),
        "/use" => set_provider_and_model(config_path, catalog, args),
        "/model" => set_model(config_path, config, catalog, args),
        "/token" => set_token(config_path, args),
        "/ai" => set_ai_enabled(config_path, args),
        "/check" => {
            ConnectorManager::new(config).check_all()?;
            Ok("All connectors healthy.".to_string())
        }
        "/pwd" | "/cd" | "/ls" | "/find" | "/grep" | "/cat" | "/sh" | "/bg" | "/sudo"
        | "/sudo-bg" => {
            let Some(call) = parse_local_command(command.as_str(), args) else {
                return Ok(format!("Unknown command `{command}`.\n\n{}", help_text()));
            };
            local_tools.run_tool(config_path, config, "main", &call)
        }
        "/term" => {
            let Some(call) = parse_terminal_command(args) else {
                return Ok("Usage: /term list|tail <id>|kill <id>".to_string());
            };
            local_tools.run_tool(config_path, config, "main", &call)
        }
        "/run" => {
            if args.is_empty() {
                Ok("Usage: /run <prompt>".to_string())
            } else {
                run_prompt(config_path, config, catalog, "main", args, local_tools)
            }
        }
        _ => Ok(format!("Unknown command `{command}`.\n\n{}", help_text())),
    }
}

fn set_provider(config_path: &Path, catalog: &ProviderCatalog, args: &str) -> Result<String> {
    if args.is_empty() {
        return Ok("Usage: /provider <provider_id>".to_string());
    }

    let provider_id = args.split_whitespace().next().unwrap_or_default();
    let provider = catalog.provider(provider_id)?;
    let model = catalog.default_model_for(provider_id)?;

    set_values(
        config_path,
        &[
            ("llm_router.provider", provider_id),
            ("llm_router.base_url", &provider.base_url),
            ("llm_router.model", model),
        ],
    )?;

    Ok(format!(
        "Provider set to {provider_id}. Default model set to {model}."
    ))
}

fn set_provider_and_model(
    config_path: &Path,
    catalog: &ProviderCatalog,
    args: &str,
) -> Result<String> {
    let mut parts = args.split_whitespace();
    let provider_id = parts.next().unwrap_or_default();
    let model_id = parts.next().unwrap_or_default();
    if provider_id.is_empty() || model_id.is_empty() {
        return Ok("Usage: /use <provider_id> <model_id>".to_string());
    }

    let provider = catalog.provider(provider_id)?;
    let _ = catalog.model_or_custom(provider_id, model_id)?;

    set_values(
        config_path,
        &[
            ("llm_router.provider", provider_id),
            ("llm_router.base_url", &provider.base_url),
            ("llm_router.model", model_id),
        ],
    )?;

    Ok(format!("Provider/model set to {provider_id}/{model_id}."))
}

fn set_model(
    config_path: &Path,
    config: &AppConfig,
    catalog: &ProviderCatalog,
    args: &str,
) -> Result<String> {
    let model_id = args.split_whitespace().next().unwrap_or_default();
    if model_id.is_empty() {
        return Ok("Usage: /model <model_id>".to_string());
    }

    let _ = catalog.model_or_custom(&config.llm_router.provider, model_id)?;
    set_value(config_path, "llm_router.model", model_id)?;

    Ok(format!(
        "Model set to {} for provider {}.",
        model_id, config.llm_router.provider
    ))
}

fn set_token(config_path: &Path, args: &str) -> Result<String> {
    if args.trim().is_empty() {
        return Ok("Usage: /token <provider_api_key>".to_string());
    }

    set_value(config_path, "llm_router.api_key", args.trim())?;
    Ok(
        "Provider API key saved locally. Delete the Telegram message that contained the token."
            .to_string(),
    )
}

fn set_ai_enabled(config_path: &Path, args: &str) -> Result<String> {
    let value = match args.to_ascii_lowercase().as_str() {
        "on" | "true" | "enabled" | "enable" => "true",
        "off" | "false" | "disabled" | "disable" => "false",
        _ => return Ok("Usage: /ai on|off".to_string()),
    };

    set_values(
        config_path,
        &[
            ("llm_router.enabled", value),
            ("features.llm_router.enabled", value),
        ],
    )?;

    Ok(format!("AI model routing is now {args}."))
}

fn parse_local_command(command: &str, args: &str) -> Option<LocalToolCall> {
    let name = match command {
        "/pwd" => "pwd",
        "/cd" => "cd",
        "/ls" => "ls",
        "/find" => "find",
        "/grep" => "grep",
        "/cat" => "cat",
        "/sh" => "sh",
        "/bg" => "bg",
        "/sudo" => "sudo",
        "/sudo-bg" => "sudo_bg",
        _ => return None,
    };
    let parsed_args = match name {
        "pwd" => Vec::new(),
        "sudo" | "sudo_bg" => vec![args.to_string()],
        _ => args
            .split_whitespace()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    };
    Some(LocalToolCall {
        name: name.to_string(),
        args: parsed_args,
    })
}

fn parse_terminal_command(args: &str) -> Option<LocalToolCall> {
    let mut parts = args.split_whitespace();
    let subcommand = parts.next()?.to_ascii_lowercase();
    let name = match subcommand.as_str() {
        "list" | "ls" => "term_list",
        "tail" | "log" => "term_tail",
        "kill" | "stop" => "term_kill",
        _ => return None,
    };
    Some(LocalToolCall {
        name: name.to_string(),
        args: parts.map(ToString::to_string).collect(),
    })
}

fn should_route_tool_request_through_ai(text: &str, call: &LocalToolCall) -> bool {
    if matches!(call.name.as_str(), "sh" | "bg") {
        return true;
    }

    let lower = text.to_ascii_lowercase();
    ["tell me", "summarize", "explain", "why", "how", "what", "?"]
        .iter()
        .any(|marker| lower.contains(marker))
}

fn run_prompt(
    config_path: &Path,
    config: &AppConfig,
    catalog: &ProviderCatalog,
    thread: &str,
    prompt: &str,
    local_tools: &mut LocalToolState,
) -> Result<String> {
    if config.llm_router.enabled {
        return crate::llm_router::run_prompt(
            config_path,
            config,
            catalog,
            thread,
            prompt,
            local_tools,
        );
    }

    let response = ConnectorManager::new(config).run(thread, prompt)?;
    if response.ok {
        Ok(response.message)
    } else {
        Err(anyhow!("connector returned error: {}", response.message))
    }
}

fn command_reply<F>(handler: F) -> String
where
    F: FnOnce() -> Result<String>,
{
    match panic::catch_unwind(AssertUnwindSafe(handler)) {
        Ok(Ok(reply)) => reply,
        Ok(Err(error)) => format!("Command failed: {error}"),
        Err(payload) => {
            let error = anyhow!("handler panic: {}", panic_payload(&payload));
            log_gateway_error(
                GatewayFailureKind::HandlerPanic,
                "handle Telegram command",
                None,
                &error,
            );
            "Command failed: internal handler panic isolated; gateway is still running.".to_string()
        }
    }
}

fn status_text(config: &AppConfig) -> String {
    let token_state = if expand_string(&config.llm_router.api_key).trim().is_empty() {
        "missing"
    } else {
        "configured"
    };
    let telegram_token_state = if expand_string(&config.mobile.telegram.bot_token)
        .trim()
        .is_empty()
    {
        "missing"
    } else {
        "configured"
    };

    format!(
        "Connector: {}\nTelegram: {}\nTelegram token: {}\nAI routing: {}\nProvider: {}\nModel: {}\nBase URL: {}\nAPI key: {}",
        config.connectors.default,
        if config.mobile.telegram.enabled {
            "enabled"
        } else {
            "disabled"
        },
        telegram_token_state,
        if config.llm_router.enabled {
            "enabled"
        } else {
            "disabled"
        },
        config.llm_router.provider,
        if config.llm_router.model.trim().is_empty() {
            "(not set)"
        } else {
            config.llm_router.model.as_str()
        },
        config.llm_router.base_url,
        token_state
    )
}

fn help_text() -> String {
    [
        "Pocket Harness commands:",
        "/status",
        "/providers",
        "/models [provider]",
        "/provider <provider>",
        "/use <provider> <model>",
        "/model <model>",
        "/token <provider_api_key>",
        "/ai on|off",
        "/check",
        "/run <prompt>",
        "/pwd",
        "/cd <path>",
        "/ls [path]",
        "/find <pattern> [path]",
        "/grep <pattern> [path]",
        "/cat <path>",
        "/sh <command>",
        "/bg <command>",
        "/term list|tail <id>|kill <id>",
        "/sudo <password> -- <command>",
        "/sudo-bg <password> -- <command>",
        "",
        "Plain text messages run on the main thread. Directory/list/search/read requests use local parent tools. Explicit terminal commands can run foreground or background sessions.",
        "Delete Telegram messages that contain API keys or sudo passwords.",
    ]
    .join("\n")
}

fn allowed(config: &TelegramConfig, message: &TelegramMessage) -> bool {
    let user_allowed = config.allowed_users.is_empty()
        || message
            .from
            .as_ref()
            .is_some_and(|user| config.allowed_users.contains(&user.id));
    let chat_allowed =
        config.allowed_chats.is_empty() || config.allowed_chats.contains(&message.chat.id);
    let private_or_group_allowed = message.chat.kind == "private" || config.allow_group_chats;

    user_allowed && chat_allowed && private_or_group_allowed
}

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: T,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    chat: TelegramChat,
    from: Option<TelegramUser>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    id: i64,
}

#[derive(Debug, Serialize)]
struct GetUpdatesRequest {
    timeout: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<i64>,
    allowed_updates: [&'static str; 1],
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
    disable_web_page_preview: bool,
}

fn get_updates(
    client: &Client,
    token: &str,
    offset: Option<i64>,
    timeout: u64,
) -> Result<Vec<TelegramUpdate>> {
    let url = telegram_url(token, "getUpdates");
    let request_timeout = Duration::from_secs(timeout.saturating_add(10).clamp(15, 120));
    let response = client
        .post(url)
        .json(&GetUpdatesRequest {
            timeout,
            offset,
            allowed_updates: ["message"],
        })
        .timeout(request_timeout)
        .send()?
        .error_for_status()?
        .json::<TelegramResponse<Vec<TelegramUpdate>>>()?;

    if response.ok {
        Ok(response.result)
    } else {
        Err(anyhow!(
            "{}",
            response
                .description
                .unwrap_or_else(|| "Telegram API error".to_string())
        ))
    }
}

fn send_message(client: &Client, token: &str, chat_id: i64, text: &str) -> Result<()> {
    let url = telegram_url(token, "sendMessage");
    let truncated = truncate_message(text);
    let response = client
        .post(url)
        .json(&SendMessageRequest {
            chat_id,
            text: &truncated,
            disable_web_page_preview: true,
        })
        .timeout(Duration::from_secs(15))
        .send()?
        .error_for_status()?
        .json::<TelegramResponse<serde_json::Value>>()?;

    if response.ok {
        Ok(())
    } else {
        Err(anyhow!(
            "{}",
            response
                .description
                .unwrap_or_else(|| "Telegram API error".to_string())
        ))
    }
}

fn telegram_url(token: &str, method: &str) -> String {
    format!("https://api.telegram.org/bot{token}/{method}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayFailureKind {
    HttpClient,
    ConfigLoad,
    TelegramDisabled,
    MissingToken,
    TelegramPoll,
    TelegramSend,
    HandlerPanic,
}

impl GatewayFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::HttpClient => "http_client",
            Self::ConfigLoad => "config_load",
            Self::TelegramDisabled => "telegram_disabled",
            Self::MissingToken => "missing_token",
            Self::TelegramPoll => "telegram_poll",
            Self::TelegramSend => "telegram_send",
            Self::HandlerPanic => "handler_panic",
        }
    }
}

fn log_gateway_failure(
    kind: GatewayFailureKind,
    context: &str,
    decision: FailureDecision,
    token: Option<&str>,
    error: &anyhow::Error,
) {
    let circuit_state = if decision.circuit_open {
        "open"
    } else {
        "closed"
    };
    let message = token
        .map(|token| redact_token(&error.to_string(), token))
        .unwrap_or_else(|| error.to_string());
    write_gateway_log(&format!(
        "event=telegram_gateway_failure kind={} context={context:?} consecutive_failures={} retry_delay_ms={} circuit={circuit_state} error={message:?}",
        kind.as_str(),
        decision.consecutive_failures,
        decision.delay.as_millis(),
    ));
}

fn log_gateway_error(
    kind: GatewayFailureKind,
    context: &str,
    token: Option<&str>,
    error: &anyhow::Error,
) {
    let message = token
        .map(|token| redact_token(&error.to_string(), token))
        .unwrap_or_else(|| error.to_string());
    write_gateway_log(&format!(
        "event=telegram_gateway_error kind={} context={context:?} error={message:?}",
        kind.as_str()
    ));
}

fn log_gateway_info(line: &str) {
    write_gateway_log(line);
}

fn write_gateway_log(line: &str) {
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{line}");
    let _ = stderr.flush();
}

fn config_source_name(source: &ConfigSource) -> &'static str {
    match source {
        ConfigSource::Primary => "primary",
        ConfigSource::LastKnownGood => "last-known-good",
    }
}

fn panic_payload(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_string())
}

fn redact_token(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_string()
    } else {
        text.replace(token, "[redacted]")
    }
}

fn truncate_message(text: &str) -> String {
    const LIMIT: usize = 3900;
    if text.chars().count() <= LIMIT {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(LIMIT).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::config::AppConfig;
    use crate::config_store::ConfigStore;
    use crate::provider_catalog::{ProviderCatalog, ensure_default_catalog};

    use crate::local_tools::LocalToolState;

    use super::{command_reply, handle_text, handle_text_with_state, redact_token, status_text};

    fn test_store(temp: &tempfile::TempDir) -> (ConfigStore, std::path::PathBuf) {
        let config_path = temp.path().join("pocket-harness.yaml");
        let mut config = AppConfig::default();
        config.gateway.data_dir = temp.path().join("state").to_string_lossy().to_string();
        fs::write(&config_path, serde_yaml::to_string(&config).unwrap()).unwrap();
        (ConfigStore::new(&config_path), config_path)
    }

    #[test]
    fn telegram_commands_update_provider_model_and_token() {
        let temp = tempfile::tempdir().unwrap();
        let (store, config_path) = test_store(&temp);
        let active = store.load_with_recovery().unwrap();
        ensure_default_catalog(&config_path, &active.config, false).unwrap();
        let catalog = ProviderCatalog::load_for_config(&config_path, &active.config).unwrap();

        let reply = handle_text(
            &config_path,
            &active.config,
            &catalog,
            "/provider anthropic",
        )
        .unwrap();
        assert!(reply.contains("Provider set to anthropic"));

        let active = store.load_with_recovery().unwrap();
        let reply = handle_text(
            &config_path,
            &active.config,
            &catalog,
            "/model claude-opus-4-7",
        )
        .unwrap();
        assert!(reply.contains("claude-opus-4-7"));

        let active = store.load_with_recovery().unwrap();
        let reply = handle_text(
            &config_path,
            &active.config,
            &catalog,
            "/token sk-test-secret",
        )
        .unwrap();
        assert!(reply.contains("Provider API key saved"));
        assert!(!reply.contains("sk-test-secret"));

        let text = fs::read_to_string(config_path).unwrap();
        assert!(text.contains("provider: anthropic"));
        assert!(text.contains("model: claude-opus-4-7"));
        assert!(text.contains("api_key: sk-test-secret"));
    }

    #[test]
    fn telegram_command_lists_models_from_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let (store, config_path) = test_store(&temp);
        let active = store.load_with_recovery().unwrap();
        ensure_default_catalog(&config_path, &active.config, false).unwrap();
        let catalog = ProviderCatalog::load_for_config(&config_path, &active.config).unwrap();

        let reply = handle_text(&config_path, &active.config, &catalog, "/models openai").unwrap();

        assert!(reply.contains("gpt-5.5"));
        assert!(reply.contains("$5.00/1M in"));
    }

    #[test]
    fn telegram_status_reports_gateway_readiness_without_secrets() {
        let mut config = AppConfig::default();
        config.mobile.telegram.enabled = true;
        config.mobile.telegram.bot_token = "".to_string();
        config.llm_router.enabled = true;
        config.llm_router.model = "gpt-5.5".to_string();
        config.llm_router.api_key = "".to_string();

        let status = status_text(&config);

        assert!(status.contains("Telegram: enabled"));
        assert!(status.contains("Telegram token: missing"));
        assert!(status.contains("AI routing: enabled"));
        assert!(status.contains("API key: missing"));
    }

    #[test]
    fn telegram_command_panics_are_isolated_into_replies() {
        let reply = command_reply(|| -> anyhow::Result<String> {
            panic!("simulated handler panic");
        });

        assert!(reply.contains("internal handler panic isolated"));
    }

    #[test]
    fn telegram_plain_text_uses_llm_router_when_enabled() {
        let temp = tempfile::tempdir().unwrap();
        let (store, config_path) = test_store(&temp);
        let mut active = store.load_with_recovery().unwrap();
        active.config.llm_router.enabled = true;
        active.config.llm_router.provider = "openai".to_string();
        active.config.llm_router.model = "gpt-5.5".to_string();
        active.config.llm_router.api_key = "".to_string();
        ensure_default_catalog(&config_path, &active.config, false).unwrap();
        let catalog = ProviderCatalog::load_for_config(&config_path, &active.config).unwrap();

        let error = handle_text(&config_path, &active.config, &catalog, "hello").unwrap_err();
        let message = error.to_string();

        assert!(message.contains("llm_router.api_key is empty"));
        assert!(!message.contains("echo thread="));
    }

    #[test]
    fn telegram_natural_terminal_requests_route_through_ai_for_reflection() {
        let temp = tempfile::tempdir().unwrap();
        let (store, config_path) = test_store(&temp);
        let mut active = store.load_with_recovery().unwrap();
        active.config.llm_router.enabled = true;
        active.config.llm_router.provider = "openai".to_string();
        active.config.llm_router.model = "gpt-5.5".to_string();
        active.config.llm_router.api_key = "".to_string();
        ensure_default_catalog(&config_path, &active.config, false).unwrap();
        let catalog = ProviderCatalog::load_for_config(&config_path, &active.config).unwrap();

        let error = handle_text(
            &config_path,
            &active.config,
            &catalog,
            "run printf ok and tell me what it printed",
        )
        .unwrap_err();

        assert!(error.to_string().contains("llm_router.api_key is empty"));
    }

    #[test]
    fn telegram_local_tools_update_cwd_and_keep_terminal_state() {
        let temp = tempfile::tempdir().unwrap();
        let (store, config_path) = test_store(&temp);
        let active = store.load_with_recovery().unwrap();
        ensure_default_catalog(&config_path, &active.config, false).unwrap();
        let catalog = ProviderCatalog::load_for_config(&config_path, &active.config).unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("marker.txt"), "hello").unwrap();
        let mut local_tools = LocalToolState::default();

        let reply = handle_text_with_state(
            &config_path,
            &active.config,
            &catalog,
            &format!("go to {}", project.display()),
            &mut local_tools,
        )
        .unwrap();
        assert!(reply.contains("cwd set to"));

        let active = store.load_with_recovery().unwrap();
        let listing = handle_text_with_state(
            &config_path,
            &active.config,
            &catalog,
            "/ls",
            &mut local_tools,
        )
        .unwrap();
        assert!(listing.contains("marker.txt"));

        let started = handle_text_with_state(
            &config_path,
            &active.config,
            &catalog,
            "/bg printf ready; sleep 30",
            &mut local_tools,
        )
        .unwrap();
        assert!(started.contains("terminal t1"));

        let listed = handle_text_with_state(
            &config_path,
            &active.config,
            &catalog,
            "/term list",
            &mut local_tools,
        )
        .unwrap();
        assert!(listed.contains("t1"));

        let killed = handle_text_with_state(
            &config_path,
            &active.config,
            &catalog,
            "/term kill t1",
            &mut local_tools,
        )
        .unwrap();
        assert!(killed.contains("killed terminal t1"));
    }

    #[test]
    fn redacts_telegram_token_from_error_text() {
        let token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi";
        let text = format!("request failed for https://api.telegram.org/bot{token}/getUpdates");

        assert_eq!(
            redact_token(&text, token),
            "request failed for https://api.telegram.org/bot[redacted]/getUpdates"
        );
    }
}
