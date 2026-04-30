# Telegram Setup

Telegram can act as both the mobile prompt surface and the setup/control plane.

## Local Start

Create the config and provider catalog:

```bash
pocket-harness --config pocket-harness.yaml init
```

Set the Telegram bot token locally or through the environment:

```bash
export TELEGRAM_BOT_TOKEN="123:telegram-token"
pocket-harness --config pocket-harness.yaml set mobile.telegram.enabled true
```

Optional but recommended:

```bash
pocket-harness --config pocket-harness.yaml set mobile.telegram.allowed_users '[123456789]'
```

Start the gateway:

```bash
pocket-harness --config pocket-harness.yaml telegram
```

## In Telegram

Send:

```text
/start
/providers
/models openai
/provider openai
/model gpt-5.5
/token sk-...
/ai on
/status
/run hello from mobile
```

Plain text messages are treated like `/run <message>` on the `main` thread.

## Notes

- The Telegram bot token must be configured before the gateway can connect to Telegram.
- Provider API tokens can be pasted through `/token` after the bot is running.
- `/provider`, `/model`, `/token`, and `/ai` all update `pocket-harness.yaml` through the same
  validation path as the CLI.
- The parent sends selected provider/model metadata to connectors, but it does not send the raw API
  key across the connector boundary.
