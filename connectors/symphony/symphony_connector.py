#!/usr/bin/env python3
"""Pocket Harness connector for Symphony.

The connector is intentionally small and language-agnostic at the boundary:
read one Pocket Harness JSON request from stdin and write one JSON response to
stdout. Symphony-specific execution can be supplied through settings.run_command
without changing the Rust parent process.
"""

from __future__ import annotations

import json
import os
import shlex
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Optional


CAPABILITIES = [
    "connector.health",
    "connector.run",
    "connector.cancel",
    "connector.status",
    "connector.capabilities",
    "threads.cwd",
    "attachments.images",
]


def main() -> int:
    try:
        request = json.loads(sys.stdin.readline())
    except Exception as exc:  # noqa: BLE001 - connector boundary must never crash unclearly.
        write_response(False, f"invalid connector request: {exc}", retryable=False)
        return 0

    settings = request.get("settings") or {}
    kind = request.get("kind")

    if kind == "health":
        response = health_response(settings)
    elif kind == "capabilities":
        response = ok_response("Symphony connector capabilities")
    elif kind == "status":
        response = status_response(settings)
    elif kind == "run":
        response = run_response(request, settings)
    elif kind == "cancel":
        response = cancel_response(settings, request)
    elif kind == "shutdown":
        response = ok_response("Symphony connector shutdown accepted")
    else:
        response = {
            "ok": False,
            "message": f"unsupported connector request kind: {kind}",
            "capabilities": CAPABILITIES,
            "retryable": False,
            "metadata": {},
        }

    print(json.dumps(response, separators=(",", ":")), flush=True)
    return 0


def health_response(settings: dict[str, Any]) -> dict[str, Any]:
    paths = resolve_paths(settings)
    checks: dict[str, Any] = {}
    failures: list[str] = []
    warnings: list[str] = []

    elixir_dir = paths["elixir_dir"]
    workflow = paths["workflow"]
    mix_file = elixir_dir / "mix.exs"
    symphony_bin = paths["symphony_bin"]

    add_path_check(checks, failures, "elixir_dir", elixir_dir, "directory")
    add_path_check(checks, failures, "workflow", workflow, "file")
    add_path_check(checks, failures, "mix_exs", mix_file, "file")

    if symphony_bin.exists():
        checks["symphony_bin"] = {"path": str(symphony_bin), "ok": True}
    else:
        checks["symphony_bin"] = {"path": str(symphony_bin), "ok": False}
        warnings.append("compiled Symphony bin/symphony was not found")

    if bool_setting(settings, "require_built_binary", False) and not symphony_bin.exists():
        failures.append("missing compiled Symphony bin/symphony")

    dashboard = dashboard_health(settings)
    if dashboard is not None:
        checks["dashboard"] = dashboard
        if bool_setting(settings, "require_dashboard", False) and not dashboard["ok"]:
            failures.append("Symphony dashboard is not reachable")

    metadata = {
        "checks": checks,
        "warnings": warnings,
        "paths": path_metadata(paths),
    }

    if failures:
        return response(False, "; ".join(failures), retryable=False, metadata=metadata)

    message = "Symphony connector healthy"
    if warnings:
        message = f"{message}; {'; '.join(warnings)}"
    return response(True, message, retryable=False, metadata=metadata)


def status_response(settings: dict[str, Any]) -> dict[str, Any]:
    paths = resolve_paths(settings)
    dashboard_url = string_setting(settings, "dashboard_url")
    metadata: dict[str, Any] = {"paths": path_metadata(paths)}

    if not dashboard_url:
        return response(
            True,
            "Symphony connector configured; no dashboard_url set",
            metadata=metadata,
        )

    dashboard = fetch_dashboard_state(dashboard_url, timeout_seconds(settings, "status_timeout_seconds", 5))
    metadata["dashboard"] = dashboard

    if dashboard["ok"]:
        return response(True, "Symphony dashboard reachable", metadata=metadata)

    return response(
        False,
        "Symphony dashboard is not reachable",
        retryable=True,
        metadata=metadata,
    )


def run_response(request: dict[str, Any], settings: dict[str, Any]) -> dict[str, Any]:
    run_command = settings.get("run_command")
    run_mode = string_setting(settings, "run_mode", "auto").lower()

    if run_command and run_mode in {"auto", "command"}:
        return run_configured_command(request, settings, "run_command")

    if run_mode == "command":
        return response(
            False,
            "Symphony run_mode is command, but settings.run_command is empty",
            retryable=False,
            metadata={"mode": run_mode},
        )

    paths = resolve_paths(settings)
    return response(
        True,
        (
            "Symphony connector is installed and healthy. Configure "
            "settings.run_command to execute a Symphony mobile worker."
        ),
        metadata={
            "mode": "dry_run",
            "paths": path_metadata(paths),
            "thread_id": request.get("thread_id"),
        },
    )


def cancel_response(settings: dict[str, Any], request: dict[str, Any]) -> dict[str, Any]:
    if settings.get("cancel_command"):
        return run_configured_command(request, settings, "cancel_command")

    return response(
        True,
        "No persistent Symphony connector process is active for this request",
        metadata={"mode": "no_op"},
    )


def run_configured_command(
    request: dict[str, Any], settings: dict[str, Any], key: str
) -> dict[str, Any]:
    try:
        command = parse_command(settings.get(key))
    except ValueError as exc:
        return response(False, str(exc), retryable=False, metadata={"command_key": key})

    paths = resolve_paths(settings)
    cwd = expand_path(string_setting(settings, f"{key}_cwd") or string_setting(settings, "run_cwd") or str(paths["elixir_dir"]))
    timeout = timeout_seconds(settings, f"{key}_timeout_seconds", timeout_seconds(settings, "run_timeout_seconds", 900))
    env = os.environ.copy()
    env.update(
        {
            "POCKET_HARNESS_REQUEST_ID": str(request.get("request_id", "")),
            "POCKET_HARNESS_THREAD_ID": str(request.get("thread_id", "")),
            "POCKET_HARNESS_PROMPT": str(request.get("prompt", "")),
            "POCKET_HARNESS_CWD": str(request.get("cwd", "")),
            "SYMPHONY_ELIXIR_DIR": str(paths["elixir_dir"]),
            "SYMPHONY_WORKFLOW": str(paths["workflow"]),
        }
    )
    dashboard_url = string_setting(settings, "dashboard_url")
    if dashboard_url:
        env["SYMPHONY_DASHBOARD_URL"] = dashboard_url

    try:
        completed = subprocess.run(
            command,
            cwd=str(cwd),
            env=env,
            input=json.dumps(request) + "\n",
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except FileNotFoundError as exc:
        return response(False, f"configured command not found: {exc.filename}", retryable=False)
    except subprocess.TimeoutExpired:
        return response(
            False,
            f"configured command timed out after {timeout}s",
            retryable=True,
            metadata={"command_key": key},
        )

    if completed.stderr:
        print(completed.stderr.rstrip(), file=sys.stderr)

    if completed.returncode != 0:
        return response(
            False,
            f"configured command exited with status {completed.returncode}",
            retryable=True,
            metadata={"command_key": key, "exit_code": completed.returncode},
        )

    delegated = parse_last_json_response(completed.stdout)
    if delegated is not None:
        delegated.setdefault("capabilities", CAPABILITIES)
        delegated.setdefault("retryable", False)
        delegated.setdefault("metadata", {})
        return delegated

    message = completed.stdout.strip() or "configured Symphony command completed"
    return response(
        True,
        message,
        metadata={"command_key": key, "mode": "command"},
    )


def parse_command(raw: Any) -> list[str]:
    if isinstance(raw, list) and all(isinstance(item, str) and item for item in raw):
        return raw
    if isinstance(raw, str) and raw.strip():
        return shlex.split(raw)
    raise ValueError("configured command must be a non-empty string or string list")


def parse_last_json_response(stdout: str) -> Optional[dict[str, Any]]:
    for line in reversed(stdout.splitlines()):
        stripped = line.strip()
        if not stripped.startswith("{") or not stripped.endswith("}"):
            continue
        try:
            parsed = json.loads(stripped)
        except json.JSONDecodeError:
            continue
        if isinstance(parsed, dict) and "ok" in parsed and "message" in parsed:
            return parsed
    return None


def dashboard_health(settings: dict[str, Any]) -> Optional[dict[str, Any]]:
    dashboard_url = string_setting(settings, "dashboard_url")
    if not dashboard_url:
        return None
    return fetch_dashboard_state(dashboard_url, timeout_seconds(settings, "status_timeout_seconds", 5))


def fetch_dashboard_state(base_url: str, timeout: int) -> dict[str, Any]:
    url = base_url.rstrip("/") + "/api/v1/state"
    try:
        with urllib.request.urlopen(url, timeout=timeout) as handle:
            body = handle.read(512_000)
            status = getattr(handle, "status", 200)
    except (urllib.error.URLError, TimeoutError) as exc:
        return {"ok": False, "url": url, "error": str(exc)}

    try:
        payload = json.loads(body.decode("utf-8"))
    except json.JSONDecodeError:
        return {"ok": False, "url": url, "status": status, "error": "invalid JSON"}

    return {"ok": 200 <= int(status) < 300, "url": url, "status": status, "state": payload}


def resolve_paths(settings: dict[str, Any]) -> dict[str, Path]:
    symphony_root = first_string(settings, "symphony_root", "SYMPHONY_ROOT")
    elixir_dir_raw = first_string(settings, "elixir_dir", "SYMPHONY_ELIXIR_DIR")

    if elixir_dir_raw:
        elixir_dir = expand_path(elixir_dir_raw)
    elif symphony_root:
        elixir_dir = expand_path(symphony_root) / "elixir"
    elif (Path.cwd() / "mix.exs").exists():
        elixir_dir = Path.cwd()
    elif (Path.cwd() / "elixir" / "mix.exs").exists():
        elixir_dir = Path.cwd() / "elixir"
    else:
        elixir_dir = Path.cwd()

    workflow_raw = first_string(settings, "workflow", "SYMPHONY_WORKFLOW")
    workflow = expand_path(workflow_raw) if workflow_raw else elixir_dir / "WORKFLOW.md"

    symphony_bin_raw = string_setting(settings, "symphony_bin")
    symphony_bin = expand_path(symphony_bin_raw) if symphony_bin_raw else elixir_dir / "bin" / "symphony"

    return {
        "elixir_dir": elixir_dir,
        "workflow": workflow,
        "symphony_bin": symphony_bin,
    }


def add_path_check(
    checks: dict[str, Any], failures: list[str], name: str, path: Path, kind: str
) -> None:
    exists = path.is_dir() if kind == "directory" else path.is_file()
    checks[name] = {"path": str(path), "ok": exists, "kind": kind}
    if not exists:
        failures.append(f"missing {kind} for {name}: {path}")


def path_metadata(paths: dict[str, Path]) -> dict[str, str]:
    return {key: str(value) for key, value in paths.items()}


def first_string(settings: dict[str, Any], setting_key: str, env_key: str) -> str:
    return string_setting(settings, setting_key) or os.environ.get(env_key, "")


def string_setting(settings: dict[str, Any], key: str, default: str = "") -> str:
    value = settings.get(key, default)
    if value is None:
        return default
    return str(value)


def bool_setting(settings: dict[str, Any], key: str, default: bool) -> bool:
    value = settings.get(key, default)
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    return bool(value)


def timeout_seconds(settings: dict[str, Any], key: str, default: int) -> int:
    try:
        value = int(settings.get(key, default))
    except (TypeError, ValueError):
        return default
    return max(value, 1)


def expand_path(raw: str) -> Path:
    return Path(os.path.expandvars(os.path.expanduser(raw))).resolve()


def ok_response(message: str, metadata: Optional[dict[str, Any]] = None) -> dict[str, Any]:
    return response(True, message, metadata=metadata or {})


def response(
    ok: bool,
    message: str,
    retryable: bool = False,
    metadata: Optional[dict[str, Any]] = None,
) -> dict[str, Any]:
    return {
        "ok": ok,
        "message": message,
        "capabilities": CAPABILITIES,
        "retryable": retryable,
        "metadata": metadata or {},
    }


def write_response(ok: bool, message: str, retryable: bool) -> None:
    print(
        json.dumps(response(ok, message, retryable=retryable), separators=(",", ":")),
        flush=True,
    )


if __name__ == "__main__":
    raise SystemExit(main())
