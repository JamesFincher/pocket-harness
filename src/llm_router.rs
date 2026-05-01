use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{AppConfig, expand_string};
use crate::provider_catalog::{ProviderCatalog, ProviderDefinition};

pub fn run_prompt(config: &AppConfig, catalog: &ProviderCatalog, prompt: &str) -> Result<String> {
    let client = Client::new();
    run_prompt_with_client(&client, config, catalog, prompt)
}

pub fn run_prompt_with_client(
    client: &Client,
    config: &AppConfig,
    catalog: &ProviderCatalog,
    prompt: &str,
) -> Result<String> {
    if !config.llm_router.enabled {
        bail!("llm_router.enabled is false");
    }

    let provider = catalog.provider(&config.llm_router.provider)?;
    let model = model_id(config, catalog)?;
    let api_key = expand_string(&config.llm_router.api_key);
    if api_key.trim().is_empty() {
        bail!("llm_router.api_key is empty");
    }

    match provider.api_format.as_str() {
        "google_gemini" => run_google_gemini(client, provider, &model, &api_key, prompt),
        "openai_compatible" => run_openai_compatible(client, provider, &model, &api_key, prompt),
        "anthropic" => run_anthropic(client, provider, &model, &api_key, prompt),
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

fn run_google_gemini(
    client: &Client,
    provider: &ProviderDefinition,
    model: &str,
    api_key: &str,
    prompt: &str,
) -> Result<String> {
    let url = format!(
        "{}/models/{}:generateContent",
        provider.base_url.trim_end_matches('/'),
        model
    );
    let body = json!({
        "contents": [
            {
                "role": "user",
                "parts": [{ "text": prompt }]
            }
        ]
    });

    let value = post_json(client, &url, Some(("x-goog-api-key", api_key)), &body)
        .context("call Google Gemini")?;
    extract_gemini_text(&value)
}

fn run_openai_compatible(
    client: &Client,
    provider: &ProviderDefinition,
    model: &str,
    api_key: &str,
    prompt: &str,
) -> Result<String> {
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );
    let body = json!({
        "model": model,
        "messages": [
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
    prompt: &str,
) -> Result<String> {
    let url = format!("{}/messages", provider.base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "max_tokens": 2048,
        "messages": [
            { "role": "user", "content": prompt }
        ]
    });

    let value = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .with_context(|| format!("post {url}"))?;
    response_json(value)
        .context("call Anthropic")
        .and_then(|value| extract_anthropic_text(&value))
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
    let response = request.send().with_context(|| format!("post {url}"))?;
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
    use serde_json::json;

    use super::{extract_anthropic_text, extract_gemini_text, extract_openai_text};

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
}
