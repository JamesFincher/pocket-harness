# Symphony Connector

This directory contains the bundled Pocket Harness connector for Symphony.

The connector stays outside the Rust parent boundary. The parent owns mobile reliability, YAML,
hot reload, queueing, and recovery. This connector owns only the Symphony-specific translation:

```text
Pocket Harness JSON request
        |
        v
Symphony connector
        |
        v
Symphony workflow / workspace / app-server session
```

## Current Implementation

`symphony_connector.py` is a stdlib-only Python JSON process connector. It supports:

- `health`: verify the Symphony Elixir directory, `mix.exs`, and workflow file.
- `capabilities`: report supported connector features.
- `status`: summarize local paths and optionally query Symphony's dashboard JSON API.
- `run`: dry-run by default, or delegate to `settings.run_command`.
- `cancel`: no-op by default, or delegate to `settings.cancel_command`.
- `shutdown`: acknowledge parent shutdown.

The connector uses the JSON request/response contract in
[`docs/CONNECTOR_SPEC.md`](../../docs/CONNECTOR_SPEC.md).

This first connector does not try to reimplement Symphony's orchestration inside Pocket Harness.
When Symphony adds or exposes a mobile worker entrypoint, configure `settings.run_command` to call
that worker. The connector passes the full Pocket Harness request on stdin and useful environment
variables such as `POCKET_HARNESS_PROMPT`, `POCKET_HARNESS_THREAD_ID`, `SYMPHONY_ELIXIR_DIR`, and
`SYMPHONY_WORKFLOW`.

## Reported Capabilities

- `connector.health`
- `connector.run`
- `connector.cancel`
- `connector.status`
- `connector.capabilities`
- `threads.cwd`
- `attachments.images`

It does not report `connector.stream` yet. If a Pocket Harness thread enables watch, the parent will
correctly reject this connector until streaming is implemented.

## Config

```yaml
connectors:
  default: symphony
  definitions:
    symphony:
      type: json
      display_name: Symphony
      command: ["python3", "connectors/symphony/symphony_connector.py"]
      cwd: "/Users/james/code/pocket-harness"
      timeout_seconds: 1200
      env:
        LINEAR_API_KEY: "$LINEAR_API_KEY"
      settings:
        elixir_dir: "/Users/james/code/symphony/elixir"
        workflow: "/Users/james/code/symphony/elixir/WORKFLOW.md"
        dashboard_url: "http://127.0.0.1:4000"
        run_mode: auto
        run_command: []
        cancel_command: []
```

`run_mode` values:

- `auto`: use `run_command` when present, otherwise return a safe dry-run response.
- `command`: require `run_command`; fail if it is empty.
- `dry_run`: never execute a command.

Recommended command shape:

```yaml
settings:
  run_command:
    - "/path/to/symphony-mobile-worker"
```

The command receives one JSON request on stdin. It can either write a Pocket Harness JSON response
as its final stdout line, or write plain stdout text that the connector returns as the mobile
message.

## Dashboard Status

When `dashboard_url` is set, `status` and `health` can read:

```text
<dashboard_url>/api/v1/state
```

Set `require_dashboard: true` if an unreachable dashboard should fail connector health. By default,
dashboard access is optional so local config validation still works when Symphony is stopped.
