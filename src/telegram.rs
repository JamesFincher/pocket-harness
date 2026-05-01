use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, TelegramConfig, expand_string};
use crate::config_store::ConfigStore;
use crate::connector::ConnectorManager;
use crate::provider_catalog::{ProviderCatalog, format_models, format_providers};
use crate::yaml_edit::{set_value, set_values};

pub fn run_gateway(store: ConfigStore) -> Result<()> {
    let client = Client::new();
    let mut offset = None;

    loop {
        let active = store.load_with_recovery()?;
        let telegram = &active.config.mobile.telegram;
        if !telegram.enabled {
            return Err(anyhow!("mobile.telegram.enabled is false"));
        }

        let token = expand_string(&telegram.bot_token);
        if token.trim().is_empty() {
            return Err(anyhow!("mobile.telegram.bot_token is empty"));
        }

        let updates = get_updates(&client, &token, offset, telegram.poll_timeout_seconds)
            .context("poll Telegram updates")?;

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

            let catalog = ProviderCatalog::load_for_config(store.config_path(), &active.config)
                .or_else(|_| ProviderCatalog::bundled());

            let response = match catalog {
                Ok(catalog) => handle_text(store.config_path(), &active.config, &catalog, text),
                Err(error) => Err(error),
            };

            let reply = match response {
                Ok(reply) => reply,
                Err(error) => format!("Command failed: {error}"),
            };

            send_message(&client, &token, message.chat.id, &reply)
                .context("send Telegram reply")?;
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
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(help_text());
    }

    if !trimmed.starts_with('/') {
        return run_prompt(config, catalog, "main", trimmed);
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
        "/run" => {
            if args.is_empty() {
                Ok("Usage: /run <prompt>".to_string())
            } else {
                run_prompt(config, catalog, "main", args)
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

fn run_prompt(
    config: &AppConfig,
    catalog: &ProviderCatalog,
    thread: &str,
    prompt: &str,
) -> Result<String> {
    if config.llm_router.enabled {
        return crate::llm_router::run_prompt(config, catalog, prompt);
    }

    let response = ConnectorManager::new(config).run(thread, prompt)?;
    if response.ok {
        Ok(response.message)
    } else {
        Err(anyhow!("connector returned error: {}", response.message))
    }
}

fn status_text(config: &AppConfig) -> String {
    let token_state = if expand_string(&config.llm_router.api_key).trim().is_empty() {
        "missing"
    } else {
        "configured"
    };

    format!(
        "Connector: {}\nAI routing: {}\nProvider: {}\nModel: {}\nBase URL: {}\nAPI key: {}",
        config.connectors.default,
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
        "",
        "Plain text messages run on the main thread.",
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
    let response = client
        .post(url)
        .json(&GetUpdatesRequest {
            timeout,
            offset,
            allowed_updates: ["message"],
        })
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

    use super::handle_text;

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
}
