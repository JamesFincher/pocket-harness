# Config Guide

Pocket Harness uses one user-facing YAML file: `pocket-harness.yaml`.

The Rust parent owns this file. Mobile commands and CLI commands update it transactionally, and the
hot-reload loop promotes valid changes without restart.

## Core Sections

- `gateway`: parent process name, data dir, log level, hot reload interval.
- `recovery`: last-known-good behavior.
- `features`: every parent-owned feature and its enablement settings.
- `mobile`: mobile gateways such as Telegram.
- `llm_router`: selected provider/model/API key and the provider catalog path.
- `connectors`: connector definitions.
- `threads`: per-thread connector, cwd, queue, watch, and reply preferences.

## Updating Settings

Use the CLI to update config safely:

```bash
cargo run -- set threads.main.watch.enabled true
cargo run -- set features.terminal.enabled false
```

The setter edits YAML, validates it, and writes atomically. The running gateway can then hot-promote
the change.

## Environment Variables

String settings can reference environment variables:

```yaml
mobile:
  telegram:
    bot_token: "$TELEGRAM_BOT_TOKEN"
```

Unset variables expand to an empty string.

Installed systems load secrets from `~/.pocket-harness/env` before validating YAML. The installer
writes that file with `chmod 600` and configures YAML to reference variables instead of embedding
raw tokens:

```env
TELEGRAM_BOT_TOKEN=123:telegram-token
OPENAI_API_KEY=sk-...
```

Use a custom env file with:

```bash
pocket-harness --config ~/.pocket-harness/config.yaml --env-file /path/to/env check --health
```

## Connectors

```yaml
connectors:
  default: echo
  definitions:
    echo:
      type: builtin_echo
      display_name: Echo
      command: []
      cwd: .
      timeout_seconds: 30
      env: {}
      settings: {}
```

For a JSON process connector:

```yaml
connectors:
  definitions:
    my_agent:
      type: json
      display_name: My Agent
      command: ["python3", "connectors/my_agent.py"]
      cwd: "."
      timeout_seconds: 900
      env:
        MY_AGENT_TOKEN: "$MY_AGENT_TOKEN"
      settings:
        mode: safe
```

## Provider Catalog

`providers.yaml` is the model/provider index. The main config points at it:

```yaml
llm_router:
  catalog_path: providers.yaml
  provider: openai
  base_url: https://api.openai.com/v1
  api_key: "$OPENAI_API_KEY"
  model: gpt-5.5
```

Use:

```bash
pocket-harness providers
pocket-harness models openai
```

Telegram setup commands read the same catalog and update these `llm_router` fields.

## Private State

The app keeps private reliability state under `gateway.data_dir`:

- `last-known-good.yaml`
- `config-history/`
- `config-rejections.log`
- `rejected-configs/`

This state is not a second user config file. It exists so bad edits do not take the mobile gateway
down.

## Service and Reset Commands

The CLI owns service management for installed systems:

```bash
pocket-harness --config ~/.pocket-harness/config.yaml --env-file ~/.pocket-harness/env service install
pocket-harness --config ~/.pocket-harness/config.yaml --env-file ~/.pocket-harness/env service status
pocket-harness --config ~/.pocket-harness/config.yaml --env-file ~/.pocket-harness/env service restart
```

Supported service managers are user `systemd` on Linux, `launchd` on macOS, and Windows scheduled
tasks when available.

Reset commands require confirmation unless `--yes` is passed:

```bash
pocket-harness --config ~/.pocket-harness/config.yaml --env-file ~/.pocket-harness/env reset config
pocket-harness --config ~/.pocket-harness/config.yaml --env-file ~/.pocket-harness/env reset service
pocket-harness --config ~/.pocket-harness/config.yaml --env-file ~/.pocket-harness/env reset data
pocket-harness --config ~/.pocket-harness/config.yaml --env-file ~/.pocket-harness/env reset logs
pocket-harness --config ~/.pocket-harness/config.yaml --env-file ~/.pocket-harness/env reset all
```
