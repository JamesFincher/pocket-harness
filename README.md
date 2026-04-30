# Pocket Harness

Pocket Harness is a local-first mobile gateway for AI coding systems.

The parent process is a Rust binary that owns the mobile connection, YAML config, feature flags,
queueing, hot reload, connector health checks, and last-known-good recovery. AI systems plug in as
connectors behind a small language-agnostic JSON interface.

The first real target is Symphony, but the core is intentionally agent-agnostic: Codex, Claude Code,
shell scripts, hosted agents, and custom internal systems should all fit behind the same connector
boundary.

## Current Status

This repo is an early scaffold with a working core:

- one canonical YAML config: `pocket-harness.yaml`
- predefined feature registry
- hot-reload loop
- last-known-good config snapshots
- rollback when connector health breaks after a config change
- generic one-shot JSON connector runner
- built-in echo connector for local smoke tests
- transaction-style `set` command for updating YAML values

Telegram and the Symphony connector are next layers; the core is ready for them.

## Quick Start

```bash
cargo run -- init --force
cargo run -- check --health
cargo run -- features
cargo run -- run --thread main hello from mobile
```

Run the hot-reload loop:

```bash
cargo run -- watch
```

Update a setting in the YAML through the parent process:

```bash
cargo run -- set threads.main.watch.enabled true
cargo run -- check --health
```

## Architecture

```text
Telegram / future mobile gateways
        |
        v
Rust parent process
        |
        | one config, hot reload, queues, feature flags, recovery
        v
Connector boundary
        |
        +--> Symphony
        +--> Codex
        +--> Claude Code
        +--> custom systems
```

The parent must stay responsive even when a connector is broken. Connectors can be written in any
language because the default connector interface is JSON over stdin/stdout.

## Important Docs

- [Connector Spec](docs/CONNECTOR_SPEC.md)
- [Config Guide](docs/CONFIG.md)
- [Reliability Model](docs/RELIABILITY.md)
- [LLM Connector Guide](docs/LLM_CONNECTOR_GUIDE.md)

## Git History

The repo is initialized with commits for the Rust scaffold and the core config/connector foundation.
Keep changes grouped into intentional commits so coding agents and human maintainers can review the
evolution of the harness.
