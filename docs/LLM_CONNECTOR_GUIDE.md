# LLM Connector Guide

This repo is designed so a coding agent can create a connector without learning the whole parent
runtime.

## Instructions For A Coding Agent

1. Read `docs/CONNECTOR_SPEC.md`.
2. Choose any language.
3. Implement a command that reads one JSON object from stdin.
4. Switch on `kind`.
5. Print one JSON object to stdout.
6. Log diagnostics to stderr only.
7. Never print secrets, raw auth files, private transcripts, or unredacted logs in `message`.
8. Add a config entry under `connectors.definitions`.
9. Run `cargo run -- check --health`.
10. Run `cargo run -- run --thread main <prompt>`.

## Minimum Connector

```python
#!/usr/bin/env python3
import json
import sys

request = json.loads(sys.stdin.readline())

if request["kind"] == "health":
    print(json.dumps({"ok": True, "message": "healthy"}))
elif request["kind"] == "run":
    print(json.dumps({"ok": True, "message": "handled: " + request["prompt"]}))
else:
    print(json.dumps({"ok": True, "message": "accepted"}))
```

## Good Connector Behavior

- Return quickly for `health`.
- Make `message` useful on a phone.
- Use `cwd` as the user-selected working directory when supported.
- Respect `request_id` in logs.
- Return `retryable: true` for transient backend/network failures.
- Use `capabilities` to tell the parent what is safe to enable.

## Symphony Connector Target

The Symphony connector should eventually:

- keep Symphony app-server/session behavior behind the connector boundary
- translate `run` requests into synthetic mobile work items
- return a final assistant message to the parent
- expose `connector.health`, `connector.run`, `connector.status`, `connector.cancel`,
  `connector.capabilities`, `threads.cwd`, and later `connector.stream`
