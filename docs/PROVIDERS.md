# Provider Catalog

`providers.yaml` is the model/provider index for Pocket Harness.

The main `pocket-harness.yaml` stores the active selection:

```yaml
llm_router:
  enabled: false
  catalog_path: providers.yaml
  provider: openai
  base_url: https://api.openai.com/v1
  api_key: "$OPENAI_API_KEY"
  model: gpt-5.5
```

`providers.yaml` stores provider endpoints, default models, context windows, token prices, and source
links. It is deliberately plain YAML so humans, Telegram commands, and future LLM-authored setup
flows can edit it without touching Rust code.

## Current Sources

The bundled catalog was checked against official provider docs on 2026-04-30:

- OpenAI: `https://developers.openai.com/api/docs/models` and
  `https://platform.openai.com/docs/pricing`
- Anthropic: `https://platform.claude.com/docs/en/docs/about-claude/models` and
  `https://platform.claude.com/docs/en/about-claude/pricing`
- Google: `https://ai.google.dev/gemini-api/docs/models` and
  `https://ai.google.dev/gemini-api/docs/pricing`
- Mistral: `https://docs.mistral.ai/models/overview` and `https://mistral.ai/pricing`
- xAI: `https://docs.x.ai/developers/models`
- DeepSeek: `https://api-docs.deepseek.com/quick_start/pricing`
- Groq: `https://console.groq.com/docs/models` and `https://groq.com/pricing`
- OpenRouter: `https://openrouter.ai/docs/overview/models` and
  `https://openrouter.ai/docs/quickstart`

Prices and availability change frequently. Treat `providers.yaml` as an editable starting point, not
a permanent authority. If exact cost matters, verify the provider's current page before a long run.

## CLI

List providers:

```bash
pocket-harness --config pocket-harness.yaml providers
```

List models:

```bash
pocket-harness --config pocket-harness.yaml models openai
```

## Telegram Commands

The Telegram gateway reads `providers.yaml` and updates `pocket-harness.yaml`:

```text
/providers
/models [provider]
/provider <provider>
/use <provider> <model>
/model <model>
/token <provider_api_key>
/ai on
/ai off
```

`/token` stores the provider API key locally in `pocket-harness.yaml`. It never echoes the token
back, but Telegram has already seen the message, so delete the token message after setup.

## Adding Providers

Add a provider under `providers`:

```yaml
providers:
  example:
    display_name: Example AI
    api_format: openai_compatible
    base_url: https://api.example.com/v1
    token_env: EXAMPLE_API_KEY
    default_model: example-large
    allow_custom_models: false
    docs:
      - https://docs.example.com/models
    models:
      example-large:
        display_name: Example Large
        provider_model_id: example-large
        context_window: 200000
        max_output_tokens: 32000
        input_usd_per_1m: 1.00
        output_usd_per_1m: 5.00
        capabilities: [text, reasoning, tools, coding]
```

Set `allow_custom_models: true` when the provider is a gateway such as OpenRouter and users should
be able to select model IDs not prelisted in the catalog.
