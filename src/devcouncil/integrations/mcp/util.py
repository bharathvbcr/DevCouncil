"""Shared helpers for MCP tool handlers."""

from __future__ import annotations

import json
import logging
import subprocess
import sys
from collections.abc import Awaitable, Callable
from pathlib import Path

from mcp.types import TextContent

logger = logging.getLogger(__name__)

_CLI_TIMEOUT_SECONDS = 120
CLI_TIMEOUT_SECONDS = _CLI_TIMEOUT_SECONDS
_GIT_APPLY_TIMEOUT_SECONDS = 120
_CLI_OUTPUT_LIMIT = 20_000


def read_log_file(path: str | None) -> str:
    """Best-effort read of a persisted stdout/stderr log."""
    if not path:
        return ""
    try:
        return Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def optional_string_list_argument(arguments: dict, name: str) -> tuple[list[str], list[TextContent] | None]:
    value = arguments.get(name)
    if value is None:
        return [], None
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        return [], error_text(f"{name} must be a string array", code="invalid_arguments", argument=name)
    return value, None


def json_text(payload: dict[str, object]) -> list[TextContent]:
    return [TextContent(type="text", text=json.dumps(payload, indent=2))]


def error_text(message: str, *, code: str = "error", **details: object) -> list[TextContent]:
    return json_text({"ok": False, "error": message, "code": code, **details})


def annotate_stale(contents: list[TextContent], coordinator: object) -> list[TextContent]:
    """Attach top-level stale/sync metadata when a freshness wait timed out."""
    text = contents[0].text if contents else ""
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        return contents
    if not isinstance(payload, dict):
        return contents
    status = coordinator.status()  # type: ignore[attr-defined]
    payload["stale"] = True
    payload["fresh"] = False
    existing_sync = payload.get("sync")
    sync_base = existing_sync if isinstance(existing_sync, dict) else {}
    payload["sync"] = {
        **sync_base,
        "pending": list(getattr(status, "pending", []) or []),
        "state": getattr(status, "state", "pending"),
        "fresh": False,
    }
    return json_text(payload)


async def with_codeintel_freshness(
    root: Path,
    produce: Callable[[], Awaitable[list[TextContent]]],
    *,
    timeout: float = 2.0,
) -> list[TextContent]:
    """Await pending sync, run ``produce``, and annotate stale responses."""
    import asyncio

    from devcouncil.codeintel.sync import get_sync_coordinator

    coordinator = get_sync_coordinator(root)
    fresh = await asyncio.to_thread(coordinator.wait_until_fresh, timeout=timeout)
    contents = await produce()
    if not fresh and contents:
        return annotate_stale(contents, coordinator)
    return contents


def normalize_arguments(arguments: object) -> dict:
    return arguments if isinstance(arguments, dict) else {}


def int_argument(arguments: dict, name: str, default: int, *, minimum: int, maximum: int) -> int:
    value = arguments.get(name, default)
    if not isinstance(value, int) or isinstance(value, bool):
        value = default
    return max(minimum, min(value, maximum))


def optional_string_argument(arguments: dict, name: str) -> str | None:
    value = arguments.get(name)
    if value is None:
        return None
    return value if isinstance(value, str) else ""


def optional_bool_argument(arguments: dict, name: str) -> tuple[bool | None, list[TextContent] | None]:
    """Return ``(value, None)`` when absent/valid, or ``(None, error)`` when malformed.

    Absent → ``(None, None)`` so callers can apply their own default.
    """
    if name not in arguments or arguments.get(name) is None:
        return None, None
    value = arguments.get(name)
    if not isinstance(value, bool):
        return None, error_text(f"{name} must be a boolean", code="invalid_arguments", argument=name)
    return value, None


def required_string_argument(arguments: dict, name: str) -> tuple[str | None, list[TextContent] | None]:
    value = arguments.get(name)
    if value is None or value == "":
        return None, error_text(f"Missing {name}", code="missing_argument", argument=name)
    if not isinstance(value, str):
        return None, error_text(f"{name} must be a string", code="invalid_arguments", argument=name)
    return value, None


def truncate_text(value: str | bytes | None, limit: int = _CLI_OUTPUT_LIMIT) -> tuple[str, bool]:
    if value is None:
        return "", False
    if isinstance(value, bytes):
        value = value.decode("utf-8", errors="replace")
    if len(value) <= limit:
        return value, False
    marker = f"\n...[truncated to {limit} characters]"
    return value[:limit] + marker, True


def is_git_repo(root: Path) -> bool:
    try:
        from devcouncil.utils.proc import run_git

        result = run_git(["rev-parse", "--is-inside-work-tree"], cwd=root)
        return result.returncode == 0 and result.stdout.strip() == "true"
    except Exception as exc:
        logger.debug("git work-tree check failed for %s: %s", root, exc)
        return False


def within_root(root: Path, rel_or_abs: str) -> Path | None:
    raw = rel_or_abs.strip().strip('"').replace("\\", "/")
    try:
        candidate = Path(raw)
        resolved = candidate.resolve() if candidate.is_absolute() else (root / raw).resolve()
        resolved.relative_to(root.resolve())
        return resolved
    except (OSError, ValueError):
        return None


def diff_target_paths(unified_diff: str) -> list[str]:
    targets: list[str] = []
    seen: set[str] = set()

    def _clean(token: str) -> str | None:
        token = token.strip()
        if len(token) >= 2 and token.startswith('"') and token.endswith('"'):
            try:
                token = token[1:-1].encode("utf-8").decode("unicode_escape")
            except Exception:
                token = token[1:-1]
        if not token or token == "/dev/null":
            return None
        if token[:2] in ("a/", "b/"):
            token = token[2:]
        return token or None

    def _add(path: str | None) -> None:
        if path and path not in seen:
            seen.add(path)
            targets.append(path)

    for line in unified_diff.splitlines():
        if line.startswith("--- ") or line.startswith("+++ "):
            _add(_clean(line[4:]))
        elif line.startswith(("rename from ", "rename to ", "copy from ", "copy to ")):
            _add(_clean(line.split(" ", 2)[2]))
        elif line.startswith("diff --git "):
            parts = line[len("diff --git "):].split()
            if len(parts) == 2:
                _add(_clean(parts[0]))
                _add(_clean(parts[1]))
    return targets


def lease_ttl_seconds(root: Path) -> int:
    try:
        from devcouncil.app.config import load_config

        return max(0, int(load_config(root).execution.lease_ttl_seconds))
    except Exception as exc:
        logger.debug("Could not load lease_ttl_seconds for %s: %s", root, exc)
        return 1800


def allowed_next_tools(status: str, has_blocking_gaps: bool) -> list[str]:
    if status == "verified":
        return ["devcouncil_release_task"]
    if status in {"done", "cancelled"}:
        return []
    if status in {"running", "blocked"} or has_blocking_gaps:
        return [
            "devcouncil_read_file",
            "devcouncil_get_evidence",
            "devcouncil_get_diff",
            "devcouncil_run_command",
            "devcouncil_apply_patch",
            "devcouncil_write_file",
            "devcouncil_update_task_scope",
            "devcouncil_verify_task",
        ]
    return [
        "devcouncil_checkout_task",
        "devcouncil_read_file",
        "devcouncil_get_diff",
    ]


def parse_cli_json(result: dict[str, object]) -> tuple[dict | None, list[TextContent] | None]:
    """Parse JSON stdout from a CLI subprocess, even when exit code is non-zero."""
    stdout = str(result.get("stdout") or "").strip()
    if stdout:
        try:
            return json.loads(stdout), None
        except json.JSONDecodeError:
            pass
    if not result.get("ok"):
        stderr = str(result.get("stderr") or "CLI command failed")
        return None, error_text(stderr, code="cli_failed")
    return None, error_text("CLI command returned invalid JSON", code="cli_parse_error")


def run_cli_command(args: list[str], root: Path) -> dict[str, object]:
    command = [sys.executable, "-m", "devcouncil", *args, "--project-root", str(root)]
    try:
        result = subprocess.run(
            command,
            cwd=root,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=_CLI_TIMEOUT_SECONDS,
        )
        stdout, stdout_truncated = truncate_text(result.stdout)
        stderr, stderr_truncated = truncate_text(result.stderr)
        return {
            "ok": result.returncode == 0,
            "returncode": result.returncode,
            "stdout": stdout,
            "stderr": stderr,
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
            "timed_out": False,
        }
    except subprocess.TimeoutExpired as exc:
        stdout, stdout_truncated = truncate_text(exc.output)
        stderr, stderr_truncated = truncate_text(exc.stderr)
        return {
            "ok": False,
            "returncode": None,
            "stdout": stdout,
            "stderr": stderr,
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
            "timed_out": True,
            "timeout_seconds": _CLI_TIMEOUT_SECONDS,
        }


GIT_APPLY_TIMEOUT_SECONDS = _GIT_APPLY_TIMEOUT_SECONDS


def is_secret_path(root: Path, rel_or_abs: str) -> bool:
    """True when a path matches a protected secret/credential glob."""
    from devcouncil.execution.policy_engine import SECRET_PATH_PATTERNS
    import fnmatch as _fnmatch

    normalized = rel_or_abs.strip().strip('"').replace("\\", "/")
    try:
        candidate = Path(normalized)
        if candidate.is_absolute():
            resolved = candidate.resolve()
            try:
                normalized = resolved.relative_to(root.resolve()).as_posix()
            except ValueError:
                normalized = resolved.as_posix()
    except OSError:
        pass
    return any(_fnmatch.fnmatch(normalized, pattern) for pattern in SECRET_PATH_PATTERNS)
