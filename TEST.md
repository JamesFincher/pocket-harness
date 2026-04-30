# Testing Guide

Pocket Harness is a Rust crate with focused unit and integration tests. The
current suite exercises config validation, connector capability negotiation,
last-known-good recovery, connector execution, parent-owned job state, and YAML
value parsing.

## Test Commands

Run the full suite:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Run one integration test file:

```bash
cargo test --test core
cargo test --test cli_behavior
cargo test --test capability_behavior
cargo test --test connector_behavior
cargo test --test config_behavior
cargo test --test job_behavior
cargo test --test symphony_connector_behavior
```

Run one named test:

```bash
cargo test json_connector_runs_over_stdin_stdout
```

Run the CLI manually against the default config:

```bash
cargo run -- check --health
cargo run -- run --thread main hello
```

For isolation testing, run the compiled binary with a temporary `HOME` and
temporary config path. This avoids touching the developer's real
`~/.pocket-harness` state.

## Current Test Coverage

`src/yaml_edit.rs`

- `parses_scalar_types` covers scalar parsing for booleans, integers, and
  unquoted strings used by the `set` command.

`src/provider_catalog.rs`

- Covers parsing the bundled `providers.yaml` catalog and formatting provider
  model summaries with context and price data.

`src/telegram.rs`

- Covers Telegram setup commands that update provider/model/token settings in
  YAML without making live Telegram network calls.

`src/jobs.rs`

- `queues_starts_finishes_and_records_receipts` covers the basic parent-owned
  job lifecycle.
- `enforces_queue_depth_and_receipt_retention` covers queue limits and bounded
  history.

`tests/capability_behavior.rs`

- Verifies that selected connectors must report capabilities required by
  enabled YAML features.
- Verifies that disabling connector-dependent features reduces capability
  requirements.
- Verifies that per-thread watch requires `connector.stream`.
- Verifies that unselected connectors only need the base health/run capability
  set.

`tests/cli_behavior.rs`

- Covers the compiled binary boundary for `init`, `check --health`, `run`,
  `set`, provider/model catalog listing, and the no-overwrite behavior of
  `init` without `--force`.
- Uses a temporary `HOME` so CLI tests do not touch the developer's real
  `~/.pocket-harness` state.

`tests/config_behavior.rs`

- Covers connector default validation, JSON command validation, timeout
  validation, thread connector selection, globally disabled watch/queue
  behavior, missing Telegram/LLM required fields, YAML defaults, path/env
  expansion helpers, and default feature-key generation.

`tests/connector_behavior.rs`

- Covers built-in echo health/capabilities, JSON connector stdout parsing with
  log noise, non-zero exits, timeouts, environment expansion, and unknown thread
  fallback to `main` thread settings.
- Covers parent rejection when a connector health response reports `ok: false`.
- Covers selected LLM provider/model metadata propagation to connectors without
  leaking the raw API key.

`tests/symphony_connector_behavior.rs`

- Covers the bundled Symphony connector against a fake local Symphony tree.
- Verifies health and capability reporting, dry-run behavior without a worker
  command, rejection when the workflow is missing, and delegation through
  `settings.run_command`.

`tests/core.rs`

- `default_config_validates_and_builtin_echo_runs` verifies that
  `AppConfig::default()` validates and that the built-in echo connector can run
  through `ConnectorManager`.
- `config_store_promotes_and_loads_last_known_good` verifies config store
  initialization, primary config loading, and fallback to last-known-good when
  the primary YAML becomes invalid.
- `json_connector_runs_over_stdin_stdout` creates a temporary executable JSON
  connector, health-checks it, and verifies a run response over stdin/stdout.

`tests/job_behavior.rs`

- Covers multi-job thread behavior, cancellation, unknown-job errors,
  no-running-job errors, and prompt preview normalization/truncation.

## Adding Tests

Prefer tests that exercise public crate behavior through `pocket_harness::*`
types unless a small private helper needs direct unit coverage.

Use unit tests for:

- Pure parsing or transformation helpers.
- Validation rules with small input/output assertions.
- Error cases that do not need the filesystem or subprocesses.

Use integration tests in `tests/` for:

- Config lifecycle behavior across `ConfigStore`, validation, and recovery.
- Connector manager behavior that crosses module boundaries.
- Capability negotiation between YAML features and connector health responses.
- Parent-owned job/queue/history behavior.
- CLI-level scenarios once command behavior needs end-to-end coverage.

Use `tempfile` for tests that write config files or executable connector
fixtures. Keep generated files inside the temporary directory so tests can run
in parallel without touching the developer's real `pocket-harness.yaml` or
`~/.pocket-harness` state.

## Connector Test Conventions

Connector tests should use small local fixtures instead of real AI systems.
Good connector tests should cover:

- `health` requests and required capability reporting.
- `run` requests and the response shape expected by the parent process.
- Non-zero exits, malformed JSON, missing capabilities, and timeout behavior.
- Environment, `cwd`, and command argument handling when those settings matter.

When adding a connector fixture that will be used with `check --health`, make
the fixture report every capability required by the active YAML feature set. The
default config currently expects selected connectors to support:

- `connector.health`
- `connector.run`
- `connector.cancel`
- `threads.cwd`
- `attachments.images`

If a test intentionally omits one of those capabilities, assert that the parent
rejects the connector during health/capability validation.

For external connector examples, prefer temporary shell or Python scripts that
read one JSON request from stdin and write one JSON response to stdout. Avoid
network calls and long sleeps in the default suite.

Bundled connector tests should not require real credentials or live AI systems.
Use fake local project trees, command fixtures, and optional status endpoints so
the default suite remains fast and deterministic.

## Reliability Test Conventions

Reliability tests should focus on parent-owned guarantees:

- Invalid YAML does not replace a known-good active config.
- Last-known-good snapshots are promoted only after successful parse and
  validation.
- Connector health failures prevent promotion when health checks are required.
- Rollback restores a valid config after a connector-breaking change.
- Rejected configs and rejection logs are written when failure paths require
  diagnostics.

Keep these tests deterministic. Use temporary config paths and assert on the
selected `ConfigSource`, persisted YAML content, and any expected diagnostic
files.

## Future Coverage Checklist

As features are added, add tests near the behavior boundary:

- Unit tests for new parsers, validators, feature flag rules, and queue state
  transitions.
- Integration tests for multi-step config edits, hot reload promotion, rollback,
  and connector capability negotiation.
- Job-store tests for queue persistence, restart behavior, retry metadata,
  status snapshots, and receipt redaction.
- CLI tests for every command whose stdout/stderr or exit status becomes a
  supported user interface.
- Connector contract tests for every bundled connector, using fixtures that can
  run locally without credentials.
- Telegram command tests for setup flows, authorization rules, and YAML updates
  without live Telegram calls.
- Provider catalog tests whenever new provider metadata fields or update flows
  are added.
- Reliability tests for timeout, retry, crash, malformed response, and partial
  write scenarios.
