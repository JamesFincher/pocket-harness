# Config Guide

Pocket Harness uses one user-facing YAML file: `pocket-harness.yaml`.

The Rust parent owns this file. Mobile commands and CLI commands update it transactionally, and the
hot-reload loop promotes valid changes without restart.

## Core Sections

- `gateway`: parent process name, data dir, log level, hot reload interval.
- `recovery`: last-known-good behavior.
- `features`: every parent-owned feature and its enablement settings.
- `mobile`: mobile gateways such as Telegram.
- `llm_router`: optional parent-level natural command routing.
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

## Private State

The app keeps private reliability state under `gateway.data_dir`:

- `last-known-good.yaml`
- `config-history/`
- `config-rejections.log`
- `rejected-configs/`

This state is not a second user config file. It exists so bad edits do not take the mobile gateway
down.
