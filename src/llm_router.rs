use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{AppConfig, expand_string};
use crate::local_tools::{LocalToolCall, LocalToolState, current_cwd, is_terminal_request};
use crate::provider_catalog::{ProviderCatalog, ProviderDefinition};

const LLM_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub fn run_prompt(
    config_path: &Path,
    config: &AppConfig,
    catalog: &ProviderCatalog,
    thread: &str,
    prompt: &str,
    local_tools: &mut LocalToolState,
) -> Result<String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("create LLM HTTP client")?;
    run_prompt_with_client(
        &client,
        config_path,
        config,
        catalog,
        thread,
        prompt,
        local_tools,
    )
}

pub fn run_prompt_with_client(
    client: &Client,
    config_path: &Path,
    config: &AppConfig,
    catalog: &ProviderCatalog,
    thread: &str,
    prompt: &str,
    local_tools: &mut LocalToolState,
) -> Result<String> {
    if !config.llm_router.enabled {
        bail!("llm_router.enabled is false");
    }

    if config.llm_router.provider.trim().is_empty() {
        bail!("llm_router.provider is empty");
    }
    if config.llm_router.model.trim().is_empty() {
        bail!("llm_router.model is empty");
    }

    let provider = catalog.provider(&config.llm_router.provider)?;
    let model = model_id(config, catalog)?;
    let api_key = expand_string(&config.llm_router.api_key);
    if api_key.trim().is_empty() {
        bail!("llm_router.api_key is empty");
    }

    match provider.api_format.as_str() {
        "google_gemini" => run_google_gemini(
            client,
            provider,
            &model,
            &api_key,
            PromptRuntime {
                config_path,
                config,
                thread,
            },
            prompt,
            local_tools,
        ),
        "openai_compatible" => {
            run_openai_compatible(client, provider, &model, &api_key, config, thread, prompt)
        }
        "anthropic" => run_anthropic(client, provider, &model, &api_key, config, thread, prompt),
        other => bail!("unsupported provider api_format `{other}`"),
    }
}

fn model_id(config: &AppConfig, catalog: &ProviderCatalog) -> Result<String> {
    let selected = config.llm_router.model.trim();
    let model = catalog.model_or_custom(&config.llm_router.provider, selected)?;
    Ok(model
        .and_then(|model| {
            let id = model.provider_model_id.trim();
            (!id.is_empty()).then(|| id.to_string())
        })
        .unwrap_or_else(|| selected.to_string()))
}

#[derive(Clone, Copy)]
struct PromptRuntime<'a> {
    config_path: &'a Path,
    config: &'a AppConfig,
    thread: &'a str,
}

fn run_google_gemini(
    client: &Client,
    provider: &ProviderDefinition,
    model: &str,
    api_key: &str,
    runtime: PromptRuntime<'_>,
    prompt: &str,
    local_tools: &mut LocalToolState,
) -> Result<String> {
    let url = format!(
        "{}/models/{}:generateContent",
        provider.base_url.trim_end_matches('/'),
        model
    );
    let terminal_allowed = is_terminal_request(prompt);
    let mut contents = vec![json!({
        "role": "user",
        "parts": [{ "text": prompt }]
    })];
    let mut seen_tool_calls = Vec::new();

    for _ in 0..8 {
        let body = json!({
            "systemInstruction": {
                "parts": [{ "text": system_prompt(runtime.config, runtime.thread, terminal_allowed) }]
            },
            "contents": contents,
            "tools": [{ "functionDeclarations": local_tool_declarations(terminal_allowed) }]
        });

        let value = post_json(client, &url, Some(("x-goog-api-key", api_key)), &body)
            .context("call Google Gemini")?;
        let calls = extract_gemini_function_calls(&value)?;
        if calls.is_empty() {
            return extract_gemini_text(&value);
        }

        if let Some(content) = value.pointer("/candidates/0/content").cloned() {
            contents.push(content);
        }

        let mut response_parts = Vec::new();
        for tool_call in calls {
            let name = tool_call.call.name.clone();
            let signature = format!("{} {:?}", tool_call.call.name, tool_call.call.args);
            let repeated = seen_tool_calls.contains(&signature);
            seen_tool_calls.push(signature);
            let result = if repeated {
                "Repeated local tool call suppressed. Use the previous tool output to answer the user instead of calling this tool again.".to_string()
            } else {
                run_local_tool_isolated(
                    runtime.config_path,
                    runtime.config,
                    runtime.thread,
                    local_tools,
                    &tool_call.call,
                )
            };
            let mut response = json!({
                "functionResponse": {
                    "name": name,
                    "response": { "result": result }
                }
            });
            if let Some(id) = tool_call.id {
                response["functionResponse"]["id"] = json!(id);
            }
            response_parts.push(response);
        }
        contents.push(json!({
            "role": "user",
            "parts": response_parts
        }));

        if seen_tool_calls.len() >= 2
            && seen_tool_calls
                .last()
                .is_some_and(|last| seen_tool_calls[..seen_tool_calls.len() - 1].contains(last))
        {
            return finalize_google_gemini(
                client,
                &url,
                api_key,
                runtime,
                terminal_allowed,
                contents,
            );
        }
    }

    finalize_google_gemini(client, &url, api_key, runtime, terminal_allowed, contents)
}

fn finalize_google_gemini(
    client: &Client,
    url: &str,
    api_key: &str,
    runtime: PromptRuntime<'_>,
    terminal_allowed: bool,
    contents: Vec<Value>,
) -> Result<String> {
    let body = json!({
        "systemInstruction": {
            "parts": [{
                "text": format!(
                    "{}\nYou have reached the local tool-call limit. Do not call another tool. Use the tool outputs already present in the conversation to answer the user, or state exactly what is still missing.",
                    system_prompt(runtime.config, runtime.thread, terminal_allowed)
                )
            }]
        },
        "contents": contents
    });

    let value = post_json(client, url, Some(("x-goog-api-key", api_key)), &body)
        .context("call Google Gemini finalization")?;
    extract_gemini_text(&value)
}

fn run_openai_compatible(
    client: &Client,
    provider: &ProviderDefinition,
    model: &str,
    api_key: &str,
    config: &AppConfig,
    thread: &str,
    prompt: &str,
) -> Result<String> {
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt(config, thread, false) },
            { "role": "user", "content": prompt }
        ]
    });

    let value = post_json(
        client,
        &url,
        Some(("authorization", &format!("Bearer {api_key}"))),
        &body,
    )
    .context("call OpenAI-compatible provider")?;
    extract_openai_text(&value)
}

fn run_anthropic(
    client: &Client,
    provider: &ProviderDefinition,
    model: &str,
    api_key: &str,
    config: &AppConfig,
    thread: &str,
    prompt: &str,
) -> Result<String> {
    let url = format!("{}/messages", provider.base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "max_tokens": 2048,
        "system": system_prompt(config, thread, false),
        "messages": [
            { "role": "user", "content": prompt }
        ]
    });

    let value = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .timeout(LLM_REQUEST_TIMEOUT)
        .send()
        .with_context(|| format!("post {url}"))?;
    response_json(value)
        .context("call Anthropic")
        .and_then(|value| extract_anthropic_text(&value))
}

fn system_prompt(config: &AppConfig, thread: &str, terminal_allowed: bool) -> String {
    let cwd = current_cwd(config, thread);
    let terminal_policy = if terminal_allowed {
        "The user explicitly asked for terminal execution. Use sh for quick foreground commands. Use bg plus term_list/term_tail/term_kill only for long-running commands or when the user asks for a background terminal. After a command returns useful output, answer the user instead of calling more tools. For Tailscale or tailnet address requests, prefer `tailscale ip -4`; if that command is unavailable or returns no address, report the exact failure briefly."
    } else {
        "Do not run terminal commands. For filesystem questions use pwd, cd, ls, find, grep, and cat."
    };

    format!(
        "You are Pocket Harness, a mobile assistant controlling the user's local computer through parent-owned tools.\n\
Current thread: {thread}\n\
Current working directory: {}\n\
Filesystem paths are real local folders and files, not rooms or places. If the user asks to go to, open, inspect, search, or read a path, use the local filesystem tools before answering.\n\
The parent tools provide only operating-system basics. Connector-specific tools belong to the configured connector and are not automatically available here.\n\
Treat every local tool response as a conversation turn. Reflect on the returned output and answer the user from it when enough information is available.\n\
{terminal_policy}",
        cwd.display()
    )
}

fn local_tool_declarations(terminal_allowed: bool) -> Vec<Value> {
    let mut declarations = vec![
        function_declaration(
            "pwd",
            "Print the current working directory.",
            object_schema(&[], &[]),
        ),
        function_declaration(
            "cd",
            "Change the persistent working directory for this thread.",
            object_schema(&[("path", "Directory path")], &["path"]),
        ),
        function_declaration(
            "ls",
            "List files in a directory. Omit path to list the current directory.",
            object_schema(&[("path", "Optional directory path")], &[]),
        ),
        function_declaration(
            "find",
            "Find files by name under a directory.",
            object_schema(
                &[
                    ("pattern", "Filename search text"),
                    ("path", "Optional root path"),
                ],
                &["pattern"],
            ),
        ),
        function_declaration(
            "grep",
            "Search file contents under a path.",
            object_schema(
                &[
                    ("pattern", "Text or regex to search for"),
                    ("path", "Optional root path"),
                ],
                &["pattern"],
            ),
        ),
        function_declaration(
            "cat",
            "Read a text file.",
            object_schema(&[("path", "File path")], &["path"]),
        ),
    ];

    if terminal_allowed {
        declarations.extend([
            function_declaration(
                "sh",
                "Run a short foreground shell command in the current working directory.",
                object_schema(&[("command", "Shell command")], &["command"]),
            ),
            function_declaration(
                "bg",
                "Start a long-running background terminal command.",
                object_schema(&[("command", "Shell command")], &["command"]),
            ),
            function_declaration(
                "term_list",
                "List persistent background terminal sessions.",
                object_schema(&[], &[]),
            ),
            function_declaration(
                "term_tail",
                "Read recent output from a background terminal session.",
                object_schema(&[("id", "Terminal session id such as t1")], &["id"]),
            ),
            function_declaration(
                "term_kill",
                "Kill a background terminal session.",
                object_schema(&[("id", "Terminal session id such as t1")], &["id"]),
            ),
        ]);
    }

    declarations
}

fn function_declaration(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "parameters": parameters
    })
}

fn object_schema(properties: &[(&str, &str)], required: &[&str]) -> Value {
    let mut props = serde_json::Map::new();
    for (name, description) in properties {
        props.insert(
            (*name).to_string(),
            json!({
                "type": "string",
                "description": description
            }),
        );
    }
    json!({
        "type": "object",
        "properties": props,
        "required": required
    })
}

fn extract_gemini_function_calls(value: &Value) -> Result<Vec<GeminiToolCall>> {
    let Some(parts) = value
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };

    let mut calls = Vec::new();
    for part in parts {
        let Some(function_call) = part.get("functionCall") else {
            continue;
        };
        let id = function_call
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let name = function_call
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Gemini function call did not include a name"))?;
        let args = function_call
            .get("args")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        calls.push(GeminiToolCall {
            id,
            call: local_call_from_function_args(name, &args)?,
        });
    }
    Ok(calls)
}

struct GeminiToolCall {
    id: Option<String>,
    call: LocalToolCall,
}

fn local_call_from_function_args(
    name: &str,
    args: &serde_json::Map<String, Value>,
) -> Result<LocalToolCall> {
    let value = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let call_args = match name {
        "pwd" | "term_list" => Vec::new(),
        "cd" | "ls" | "cat" => vec![value("path")],
        "find" | "grep" => {
            let mut args = vec![value("pattern")];
            let path = value("path");
            if !path.is_empty() {
                args.push(path);
            }
            args
        }
        "sh" | "bg" => vec![value("command")],
        "term_tail" | "term_kill" => vec![value("id")],
        other => bail!("unsupported local tool `{other}`"),
    };
    Ok(LocalToolCall {
        name: name.to_string(),
        args: call_args,
    })
}

fn run_local_tool_isolated(
    config_path: &Path,
    config: &AppConfig,
    thread: &str,
    local_tools: &mut LocalToolState,
    call: &LocalToolCall,
) -> String {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        local_tools.run_tool(config_path, config, thread, call)
    })) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => format!("Tool failed: {error}"),
        Err(payload) => format!(
            "Tool panicked and was isolated: {}",
            panic_payload(&payload)
        ),
    }
}

fn panic_payload(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_string())
}

fn post_json<T: Serialize + ?Sized>(
    client: &Client,
    url: &str,
    header: Option<(&str, &str)>,
    body: &T,
) -> Result<Value> {
    let mut request = client.post(url).json(body);
    if let Some((key, value)) = header {
        request = request.header(key, value);
    }
    let response = request
        .timeout(LLM_REQUEST_TIMEOUT)
        .send()
        .with_context(|| format!("post {url}"))?;
    response_json(response)
}

fn response_json(response: reqwest::blocking::Response) -> Result<Value> {
    let status = response.status();
    let text = response.text().context("read provider response")?;
    let value = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "raw": text }));
    if status.is_success() {
        Ok(value)
    } else {
        Err(anyhow!(
            "provider returned HTTP {status}: {}",
            provider_error(&value)
        ))
    }
}

fn provider_error(value: &Value) -> String {
    value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/error").and_then(Value::as_str))
        .or_else(|| value.pointer("/raw").and_then(Value::as_str))
        .unwrap_or("unknown provider error")
        .to_string()
}

fn extract_gemini_text(value: &Value) -> Result<String> {
    let parts = value
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Gemini response did not include candidates[0].content.parts"))?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    non_empty_text(text, "Gemini")
}

fn extract_openai_text(value: &Value) -> Result<String> {
    let text = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!("OpenAI-compatible response did not include choices[0].message.content")
        })?;
    non_empty_text(text.to_string(), "OpenAI-compatible")
}

fn extract_anthropic_text(value: &Value) -> Result<String> {
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Anthropic response did not include content"))?;
    let text = content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    non_empty_text(text, "Anthropic")
}

fn non_empty_text(text: String, provider: &str) -> Result<String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        bail!("{provider} response did not include text");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use reqwest::blocking::Client;
    use serde_json::json;

    use crate::config::AppConfig;
    use crate::local_tools::LocalToolState;
    use crate::provider_catalog::ProviderCatalog;

    use super::{
        extract_anthropic_text, extract_gemini_text, extract_openai_text, local_tool_declarations,
        run_prompt_with_client,
    };

    #[test]
    fn extracts_gemini_text_parts() {
        let value = json!({
            "candidates": [
                { "content": { "parts": [{ "text": "hello" }, { "text": " world" }] } }
            ]
        });
        assert_eq!(extract_gemini_text(&value).unwrap(), "hello world");
    }

    #[test]
    fn extracts_openai_compatible_text() {
        let value = json!({
            "choices": [
                { "message": { "content": "hello" } }
            ]
        });
        assert_eq!(extract_openai_text(&value).unwrap(), "hello");
    }

    #[test]
    fn extracts_anthropic_text() {
        let value = json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "text", "text": " world" }
            ]
        });
        assert_eq!(extract_anthropic_text(&value).unwrap(), "hello world");
    }

    #[test]
    fn terminal_tools_are_only_declared_when_explicitly_allowed() {
        let names = local_tool_declarations(false)
            .into_iter()
            .filter_map(|tool| {
                tool.get("name")
                    .and_then(|name| name.as_str())
                    .map(str::to_string)
            })
            .collect::<Vec<_>>();
        assert!(names.contains(&"pwd".to_string()));
        assert!(!names.contains(&"sh".to_string()));

        let names = local_tool_declarations(true)
            .into_iter()
            .filter_map(|tool| {
                tool.get("name")
                    .and_then(|name| name.as_str())
                    .map(str::to_string)
            })
            .collect::<Vec<_>>();
        assert!(names.contains(&"sh".to_string()));
        assert!(names.contains(&"bg".to_string()));
        assert!(names.contains(&"term_kill".to_string()));
    }

    #[test]
    fn missing_llm_runtime_settings_fail_without_network_calls() {
        let catalog = ProviderCatalog::bundled().unwrap();
        let client = Client::new();
        let mut local_tools = LocalToolState::default();
        let mut config = AppConfig::default();
        config.llm_router.enabled = true;
        config.llm_router.api_key = "sk-test".to_string();

        let missing_model = run_prompt_with_client(
            &client,
            Path::new("config.yaml"),
            &config,
            &catalog,
            "main",
            "hello",
            &mut local_tools,
        )
        .unwrap_err()
        .to_string();
        assert!(missing_model.contains("llm_router.model is empty"));

        config.llm_router.model = "gpt-5.5".to_string();
        config.llm_router.provider = "".to_string();
        let missing_provider = run_prompt_with_client(
            &client,
            Path::new("config.yaml"),
            &config,
            &catalog,
            "main",
            "hello",
            &mut local_tools,
        )
        .unwrap_err()
        .to_string();
        assert!(missing_provider.contains("llm_router.provider is empty"));

        config.llm_router.provider = "openai".to_string();
        config.llm_router.api_key = "".to_string();
        let missing_key = run_prompt_with_client(
            &client,
            Path::new("config.yaml"),
            &config,
            &catalog,
            "main",
            "hello",
            &mut local_tools,
        )
        .unwrap_err()
        .to_string();
        assert!(missing_key.contains("llm_router.api_key is empty"));
    }
}
