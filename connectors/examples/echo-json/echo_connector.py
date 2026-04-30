#!/usr/bin/env python3
"""Tiny JSON connector example for Pocket Harness."""

from __future__ import annotations

import json
import sys


CAPABILITIES = [
    "connector.health",
    "connector.run",
    "connector.cancel",
    "connector.status",
    "connector.capabilities",
    "threads.cwd",
    "attachments.images",
]


def respond(**payload: object) -> None:
    print(json.dumps(payload), flush=True)


def main() -> int:
    raw = sys.stdin.readline()
    if not raw.strip():
        respond(ok=False, message="empty connector request", retryable=False)
        return 0

    request = json.loads(raw)
    kind = request.get("kind")

    if kind == "health":
        respond(ok=True, message="echo-json connector healthy", capabilities=CAPABILITIES)
    elif kind == "capabilities":
        respond(ok=True, message="echo-json capabilities", capabilities=CAPABILITIES)
    elif kind == "run":
        respond(
            ok=True,
            message=f"echo-json thread={request.get('thread_id')} cwd={request.get('cwd')} prompt={request.get('prompt')}",
            capabilities=CAPABILITIES,
        )
    else:
        respond(ok=True, message=f"echo-json accepted {kind}", capabilities=CAPABILITIES)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
