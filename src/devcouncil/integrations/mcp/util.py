"""Shared helpers for MCP tool handlers."""

from __future__ import annotations

import json
import logging
import subprocess
import sys
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
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
    # Imported here, not at module scope, because `mcp` is expensive and this is
    # the only place in the module that needs it at *runtime* — every other
    # mention is an annotation, and `from __future__ import annotations` makes
    # those strings.
    #
    # Two modules the CLI always loads reach this one for `allowed_next_tools`
    # and `lease_ttl_seconds` (`devcouncil.cli.commands.lease` →
    # `execution.lease_ops` → here), so the module-scope import made every
    # `dev` invocation pay for the MCP client stack whether or not it spoke MCP:
    # 208 ms of a 490 ms `devcouncil.cli.main` import, on a command like
    # `dev map` that never constructs a `TextContent` at all.
    from mcp.types import TextContent

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


def _map_artifact_freshness(root: Path) -> tuple[bool | None, str]:
    """Whether `repo_map.json` still describes the code, by fingerprint.

    Reads the artifact the Rust kernel writes (fingerprints stamped by
    `devmap_engine.stamp_freshness`) and compares it against git. Not a second
    engine's opinion — the kernel's own output, checked.

    Returns ``(stale, reason)`` as a tri-state:

    * ``(True, "")``  — fingerprints disagree with git: verified stale.
    * ``(False, "")`` — fingerprints match: verified *not* stale.
    * ``(None, why)`` — the check could not run, so freshness is **unknown**.

    The third case used to collapse into ``False``. An absent map, an
    unreadable one and a probe that raised all reported exactly what a check
    that ran and passed reports, which is the one thing this codebase's Class A
    rule forbids. The caller decides what to do with the unknown; it no longer
    has to guess that it happened.
    """
    try:
        from devcouncil.indexing.repo_mapper import RepoMapper
        from devcouncil.utils.json_persist import read_json

        map_path = root / ".devcouncil" / "repo_map.json"
        if not map_path.is_file():
            return None, "no repo map at .devcouncil/repo_map.json (run `dev map`)"
        data = read_json(map_path) or {}
        if not isinstance(data, dict) or not data:
            return None, "repo_map.json is empty or not a JSON object"
        return bool(RepoMapper(project_root=root).map_is_stale(data)), ""
    except Exception as exc:  # noqa: BLE001 - a probe that failed stays unknown
        logger.debug("repo-map fingerprint check failed", exc_info=True)
        return None, f"repo-map fingerprint check failed: {exc}"


def _map_artifact_is_stale(root: Path) -> bool:
    """Boolean view for the branch where the kernel already gave an answer.

    Only a *verified* stale overrides the kernel here; unknown leaves the
    kernel's verdict standing rather than manufacturing one.
    """
    stale, _ = _map_artifact_freshness(root)
    return stale is True


class _MapFreshness:
    """Freshness derived from `repo_map.json` when the kernel cannot be reached.

    Shaped for `annotate_stale`, which wants `.status()` with `pending`/`state`.
    There is no pending queue to report from an artifact, so it is empty and the
    reason names why the kernel did not answer.
    """

    def __init__(self, reason: str) -> None:
        self._reason = reason

    def status(self):
        return type("S", (), {"pending": [], "state": "stale", "reason": self._reason})()


def annotate_freshness_unknown(
    contents: list[TextContent], reason: str
) -> list[TextContent]:
    """Mark freshness as *unknown* — neither fresh nor verified stale.

    `annotate_stale` asserts `stale: True`, which is a claim. When the Rust
    kernel cannot answer, we do not know whether the map is stale, and the
    honest annotation says so rather than picking whichever of the two answers
    happens to be safer to render.

    The alternative — falling through to `get_sync_coordinator` — substitutes
    the *Python* store's sync state for a question about the Rust kernel's. Two
    stores, one answer, and nothing in the payload saying which was measured.
    That is the SC23 shape, and the X14 rule this codebase already follows says
    unknown must never be reported as a verified value.
    """
    text = contents[0].text if contents else ""
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        return contents
    if not isinstance(payload, dict):
        return contents
    existing_sync = payload.get("sync")
    sync_base = existing_sync if isinstance(existing_sync, dict) else {}
    payload["stale"] = None
    payload["fresh"] = None
    payload["sync"] = {
        **sync_base,
        "pending": [],
        "state": "unknown",
        "fresh": None,
        "reason": reason,
    }
    return json_text(payload)


def annotate_freshness_from_artifact(
    contents: list[TextContent], kernel_reason: str
) -> list[TextContent]:
    """Record that freshness was proved by the map artifact, not by the kernel.

    The fingerprint check in `repo_map.json` ran and passed, so `stale` stays a
    verified `False` — dropping to `unknown` here would discard evidence we
    actually have. What must not vanish is *which* check answered: the kernel
    was unreachable, and a reader who cannot tell "the kernel confirmed this"
    from "the kernel never answered" is one step from treating an unbuilt store
    as a healthy one. That reason used to be computed and then thrown away.
    """
    text = contents[0].text if contents else ""
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        return contents
    if not isinstance(payload, dict):
        return contents
    existing_sync = payload.get("sync")
    sync_base = existing_sync if isinstance(existing_sync, dict) else {}
    payload["stale"] = False
    payload["fresh"] = True
    payload["sync"] = {
        **sync_base,
        "pending": [],
        "state": "artifact",
        "fresh": True,
        "verified_by": "repo_map_fingerprint",
        "kernel_unavailable": kernel_reason,
    }
    return json_text(payload)


def _payload_asserts_stale(contents: list[TextContent]) -> bool:
    """Whether the handler's own payload already claims a verified stale map."""
    text = contents[0].text if contents else ""
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        return False
    return isinstance(payload, dict) and payload.get("stale") is True


async def with_codeintel_freshness(
    root: Path,
    produce: Callable[[], Awaitable[list[TextContent]]],
    *,
    timeout: float = 2.0,
) -> list[TextContent]:
    """Run ``produce`` and annotate its freshness from the Rust kernel.

    ``timeout`` is retained for call-site compatibility and is no longer used:
    it bounded a wait on the Python sync coordinator, which is no longer
    consulted. No caller passes it.
    """
    del timeout

    from devcouncil.devmap_client import DevMapClientError, try_connect

    unavailable = ""
    client = try_connect(root)
    if client is not None:
        try:
            # Staleness needs *both* signals, and stale wins.
            #
            # `client.is_map_stale()` is `not is_fresh or pending_count > 0`,
            # and `is_fresh` is itself `pending_count == 0` — so it only asks
            # whether the kernel has queued work. A CLI-driven build leaves that
            # queue empty, so it answered "fresh" for a map whose source had
            # demonstrably changed: a confident wrong value, which is worse than
            # an unknown one.
            #
            # The fingerprints in `repo_map.json` are the evidence that actually
            # answers "does this map still describe the code" — they are
            # compared against git HEAD, the tracked file set and content
            # stat. Either signal reporting stale makes it stale.
            fresh = not client.is_map_stale() and not _map_artifact_is_stale(root)
            contents = await produce()
            if not fresh and contents:
                class _RustFreshness:
                    def status(self):
                        st = client.status()
                        return type(
                            "S",
                            (),
                            {
                                "pending": (
                                    [f"pending:{st.pending_count}"]
                                    if st.pending_count
                                    else []
                                ),
                                "state": "pending" if st.pending_count else "fresh",
                            },
                        )()

                return annotate_stale(contents, _RustFreshness())
            return contents
        except DevMapClientError as exc:
            logger.warning("devmap (Rust) freshness failed: %s", exc)
            unavailable = str(exc)
    else:
        unavailable = "no built devmap store (run `dev map`)"

    # The sync coordinator is deliberately *not* consulted. It reports the
    # Python store's pending state — a different store's freshness standing in
    # for a question about the kernel's, with nothing in the payload saying
    # which was measured. That is the SC23 shape.
    #
    # `RepoMapper.map_is_stale` is a different matter and is used: it compares
    # `repo_map.json` — the artifact the Rust kernel itself writes, fingerprints
    # included — against git. That is the same question answered from the
    # kernel's own output, not a second engine's opinion, and it is exactly what
    # `verification/checks/stale_map.py` does.
    contents = await produce()
    if not contents:
        return contents
    artifact_stale, artifact_reason = _map_artifact_freshness(root)
    if artifact_stale is True:
        return annotate_stale(contents, _MapFreshness(unavailable))
    if artifact_stale is None:
        # Neither the kernel nor the artifact could answer. This used to be
        # reached only when the map file was absent; with the map present and a
        # fingerprint probe that raised, the response went out untouched — a
        # `stale: False` nobody had verified, and the kernel's own reason
        # dropped on the floor.
        reason = "; ".join(part for part in (unavailable, artifact_reason) if part)
        if _payload_asserts_stale(contents):
            # Stale wins: a handler that already fail-closed keeps its verdict.
            # Unknown must never soften a stale another signal proved.
            return annotate_stale(contents, _MapFreshness(reason))
        return annotate_freshness_unknown(contents, reason)
    return annotate_freshness_from_artifact(contents, unavailable)


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


def cli_timeout_error(result: dict[str, object], *, what: str = "CLI command") -> list[TextContent]:
    """The refusal for a subprocess that hit the wall and was killed.

    Its own code, not ``cli_failed``: a command that ran and exited non-zero
    produced a verdict, and a command that was killed at ``_CLI_TIMEOUT_SECONDS``
    produced nothing. Reporting both the same way is the shape this codebase
    calls Class A — a check that could not run answering as one that ran.
    """
    seconds = result.get("timeout_seconds", _CLI_TIMEOUT_SECONDS)
    return error_text(
        f"{what} exceeded its {seconds}s timeout and was killed; no result was produced.",
        code="cli_timeout",
        timed_out=True,
        timeout_seconds=seconds,
    )


def parse_cli_json(result: dict[str, object]) -> tuple[dict | None, list[TextContent] | None]:
    """Parse JSON stdout from a CLI subprocess, even when exit code is non-zero.

    A timed-out call is refused before its stdout is read. ``run_cli_command``
    hands back whatever the process had flushed before it was killed, and that
    is a partial view of the answer, not the answer: a ``tasks`` array cut off
    at the wall still parses as JSON, so the old order returned it as a
    finished result with nothing marking it incomplete.
    """
    if result.get("timed_out"):
        return None, cli_timeout_error(result)
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


def _cli_stream_text(value: str | bytes | None, *, truncate: bool) -> tuple[str, bool]:
    """Optionally truncate a CLI stream for external raw-text boundaries."""
    if truncate:
        return truncate_text(value)
    if value is None:
        return "", False
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace"), False
    return value, False


def run_cli_command(args: list[str], root: Path, *, truncate: bool = False) -> dict[str, object]:
    """Run ``devcouncil`` CLI and return a structured subprocess payload.

    By default stdout/stderr are kept intact so structured JSON handlers can
    parse large payloads. Pass ``truncate=True`` only for external raw-text
    surfaces such as ``devcouncil_cli`` and report previews.

    **Blocking: an async handler must call this through**
    ``await asyncio.to_thread(run_cli_command, ...)``. ``subprocess.run`` is
    synchronous and bounded only by ``_CLI_TIMEOUT_SECONDS``, so calling it
    directly from a coroutine parks the whole event loop for up to that long:
    the server stops answering ``ping``, stops serving every other tool call,
    and — because the read loop is parked too — cannot even receive the
    ``notifications/cancelled`` that would abandon the call. The SDK applies
    that notification as a scope cancel (``mcp/shared/jsonrpc_dispatcher.py``
    ``PeerCancelMode`` ``"interrupt"``), and a scope cancel needs an await
    point to land on; a blocking handler offers none. It stays synchronous
    here so the existing monkeypatch seams in the handler modules keep working.
    """
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
        stdout, stdout_truncated = _cli_stream_text(result.stdout, truncate=truncate)
        stderr, stderr_truncated = _cli_stream_text(result.stderr, truncate=truncate)
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
        stdout, stdout_truncated = _cli_stream_text(exc.output, truncate=truncate)
        stderr, stderr_truncated = _cli_stream_text(exc.stderr, truncate=truncate)
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


def run_cli_json(args: list[str], root: Path) -> tuple[dict | None, list[TextContent] | None]:
    """Run CLI without truncating stdout, then parse structured JSON."""
    return parse_cli_json(run_cli_command(args, root, truncate=False))


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
