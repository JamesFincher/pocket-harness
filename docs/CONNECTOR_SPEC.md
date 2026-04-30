# Connector Spec

Pocket Harness connectors adapt the parent gateway to an AI system such as Symphony, Codex, Claude
Code, or a custom internal agent.

The parent is responsible for mobile reliability. A connector is only responsible for translating a
small request into the target system and returning a small response.

## Default Protocol: JSON Process

The parent starts the connector command, writes one JSON request to stdin, and expects one JSON
response on stdout. The connector may log to stderr.

This keeps connectors language-agnostic. Python, Rust, Node, Elixir, Bash, and compiled binaries all
work.

## Request

```json
{
  "kind": "run",
  "request_id": "uuid",
  "thread_id": "main",
  "prompt": "fix the failing test",
  "cwd": "/Users/james/code/project",
  "attachments": [],
  "settings": {},
  "metadata": {
    "connector": "symphony",
    "reply_style": "normal"
  }
}
```

Supported `kind` values:

- `health`
- `capabilities`
- `status`
- `run`
- `cancel`
- `shutdown`

## Response

```json
{
  "ok": true,
  "message": "Done. Fixed the test and ran the suite.",
  "capabilities": [
    "connector.health",
    "connector.run",
    "connector.cancel",
    "threads.cwd",
    "attachments.images"
  ],
  "retryable": false,
  "metadata": {}
}
```

If `ok` is false, `message` should be safe to show on mobile. Do not include tokens, auth files,
raw logs, or private transcripts.

## Capabilities

Connectors should report capabilities using the parent registry names where applicable:

- `connector.health`
- `connector.run`
- `connector.cancel`
- `connector.status`
- `connector.stream`
- `connector.capabilities`
- `threads.cwd`
- `attachments.images`

The parent uses capabilities during config promotion so a bad connector config can be rejected
before it breaks the mobile gateway. For a selected connector, the default YAML feature set requires
`connector.health`, `connector.run`, `connector.cancel`, `threads.cwd`, and `attachments.images`.
If a thread enables watch, the selected connector must also report `connector.stream`.

## Advanced Streaming

The core is designed to add JSONL streaming later for long-running connectors. The default JSON
process protocol remains the lowest-friction path and should be enough for most LLM-created
connectors.
