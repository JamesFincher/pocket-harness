# Reliability Model

Pocket Harness treats config changes as staged deployments.

```text
read YAML
  -> parse
  -> typed validation
  -> connector health/capability checks
  -> promote to active runtime
  -> snapshot as last-known-good
```

If a config edit fails, the parent process keeps the current active runtime. If the edit breaks the
connector boundary and recovery is configured to roll back, the parent restores
`last-known-good.yaml` into the primary config path and continues.

## Last Known Good

Last-known-good is written only after a config parses and validates. When connector health checks are
requested, it is written only after connectors pass.

State files:

```text
~/.pocket-harness/last-known-good.yaml
~/.pocket-harness/config-history/
~/.pocket-harness/config-rejections.log
~/.pocket-harness/rejected-configs/
```

## Connector Failure

The parent never assumes a connector is reliable.

Connector failures should not stop:

- `/status`
- `/jobs`
- config reload
- health reporting
- queue visibility
- future Telegram polling

Current scaffold behavior:

- `check --health` validates all connectors and rejects missing capabilities required by enabled
  YAML features.
- `watch` polls config, health-checks connector changes, and rolls back on connector break.
- `run` reports connector failure without crashing the parent command.
- Parent-owned job primitives queue, start, finish, cancel, and retain bounded safe receipts without
  needing connector-specific state.

## Design Rule

Any mobile-visible feature should be parent-owned unless it must be system-specific. The connector
should remain a small adapter to the target AI system.
