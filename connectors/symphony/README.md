# Symphony Connector

Symphony is the first intended real connector.

The connector should remain outside the Rust parent boundary. The parent owns mobile reliability,
while this connector owns Symphony-specific translation:

```text
Pocket Harness JSON request
        |
        v
Symphony connector
        |
        v
Symphony workflow / workspace / app-server session
```

## First Implementation Target

Implement a command that supports:

- `health`: verify Symphony repo path, workflow file, Elixir runtime, and app-server command.
- `capabilities`: report supported connector features.
- `run`: create or reuse a mobile Symphony thread/session and return the final assistant message.
- `status`: summarize active Symphony mobile session state.
- `cancel`: stop the active mobile run when possible.

The connector should use the same JSON request/response contract in `docs/CONNECTOR_SPEC.md`.

## Expected Capabilities

Initial:

- `connector.health`
- `connector.run`
- `connector.status`
- `connector.capabilities`
- `threads.cwd`

Later:

- `connector.cancel`
- `connector.stream`
- `attachments.images`

## Config Sketch

```yaml
connectors:
  default: symphony
  definitions:
    symphony:
      type: json
      display_name: Symphony
      command: ["mix", "symphony.mobile"]
      cwd: "/Users/james/code/symphony/elixir"
      timeout_seconds: 1200
      env:
        LINEAR_API_KEY: "$LINEAR_API_KEY"
      settings:
        workflow: "/Users/james/code/symphony/elixir/WORKFLOW.md"
```
