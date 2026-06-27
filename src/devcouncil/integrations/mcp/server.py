import asyncio
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent, Resource
from pydantic import AnyUrl
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import (
    TaskRepository,
    ArtifactGraphRepository,
    StateRepository,
    RequirementRepository,
    EvidenceRepository,
    GapRepository,
)
from devcouncil.storage.native import (
    TaskLeaseRepository,
    ShellCommandRepository,
    FileChangeRepository,
    VerificationRunRepository,
    CorrectionManifestRepository,
)
from devcouncil.domain.evidence import CommandResult, DiffEvidence, DiffCoverageEvidence, TestEvidence
from devcouncil.verification.next_actions import split_next_actions
from devcouncil.reporting.report_builder import ReportBuilder
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.utils.subprocess_env import clean_subprocess_env
from devcouncil.telemetry.traces import read_trace_events
from devcouncil.indexing.ast_matcher import AstMatcher
from devcouncil.indexing.lsp import LspInspector
from devcouncil.app.project_status import compute_phase
from devcouncil.live.cards import filter_cards, get_card, load_cards
from devcouncil.live.repair_prompt import build_bulk_live_repair_prompt, build_live_repair_prompt
from devcouncil.integrations.check import integration_status_summary
from devcouncil.live.summary import live_review_summary

app = Server("devcouncil")
_DB_REQUIRED_TOOLS = {
    "devcouncil_status",
    "devcouncil_report",
    "devcouncil_get_task",
    "devcouncil_list_tasks",
    "devcouncil_get_gaps",
    "devcouncil_get_next_actions",
    "devcouncil_get_task_provenance",
    "devcouncil_list_leases",
    "devcouncil_renew_lease",
    "devcouncil_get_prompt",
    "devcouncil_tail_trace",
    "devcouncil_policy_check_write",
    "devcouncil_graph_context",
    "devcouncil_prepare_execution",
    "devcouncil_checkout_task",
    "devcouncil_release_task",
    "devcouncil_update_task_scope",
    "devcouncil_append_evidence",
    "devcouncil_record_command",
    "devcouncil_write_file",
    "devcouncil_apply_patch",
    "devcouncil_verify_task",
    "devcouncil_handoff_agent",
    "devcouncil_get_evidence",
    "devcouncil_run_command",
    "devcouncil_next_task",
}
_CLI_ALLOWED_ROOTS = {"status", "tasks", "report", "map", "prompt", "show", "trace", "lsp", "ast", "verify"}
_CLI_FORBIDDEN_FLAGS = {"--project-root", "--github", "--github-pr-comment", "--gitlab-pr-comment"}
_CLI_TIMEOUT_SECONDS = 120
_CLI_OUTPUT_LIMIT = 20_000
# Allowed values for devcouncil_record_command.status (validated, not free-text).
_RECORD_COMMAND_STATUSES = {"started", "finished", "failed", "blocked"}


def _forbidden_cli_flags(args: list[str]) -> list[str]:
    forbidden: set[str] = set()
    for arg in args:
        for flag in _CLI_FORBIDDEN_FLAGS:
            if arg == flag or arg.startswith(f"{flag}="):
                forbidden.add(flag)
    return sorted(forbidden)


def _truncate_text(value: str | bytes | None, limit: int = _CLI_OUTPUT_LIMIT) -> tuple[str, bool]:
    if value is None:
        return "", False
    if isinstance(value, bytes):
        value = value.decode("utf-8", errors="replace")
    if len(value) <= limit:
        return value, False
    marker = f"\n...[truncated to {limit} characters]"
    return value[:limit] + marker, True


def _json_text(payload: dict[str, object]) -> list[TextContent]:
    return [TextContent(type="text", text=json.dumps(payload, indent=2))]


def _is_git_repo(root: Path) -> bool:
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--is-inside-work-tree"],
            cwd=root, capture_output=True, text=True,
        )
        return result.returncode == 0 and result.stdout.strip() == "true"
    except Exception:
        return False


def _read_log_file(path: str | None) -> str:
    """Best-effort read of a persisted stdout/stderr log; tolerate a missing file."""
    if not path:
        return ""
    try:
        return Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def _git_diff(root: Path, paths: list[str], staged: bool) -> dict[str, object]:
    """Compute a (optionally path-scoped, optionally staged) git diff.

    Returns {ok, files:[{path,status,additions,deletions}], unified_diff (truncated),
    truncated}. Never raises — a git failure is reported as an empty diff with the
    stderr in an error field so the agent can act on it."""
    diff_args = ["git", "diff"]
    numstat_args = ["git", "diff", "--numstat"]
    namestatus_args = ["git", "diff", "--name-status"]
    if staged:
        for args in (diff_args, numstat_args, namestatus_args):
            args.append("--cached")
    if paths:
        for args in (diff_args, numstat_args, namestatus_args):
            args.append("--")
            args.extend(paths)

    def _run(args: list[str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            args, cwd=root, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=_CLI_TIMEOUT_SECONDS,
        )

    try:
        diff_proc = _run(diff_args)
        numstat_proc = _run(numstat_args)
        namestatus_proc = _run(namestatus_args)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"ok": False, "files": [], "unified_diff": "", "truncated": False, "error": str(exc)}

    status_by_path: dict[str, str] = {}
    for line in namestatus_proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) >= 2:
            status_by_path[parts[-1].replace("\\", "/")] = parts[0]

    files: list[dict[str, object]] = []
    for line in numstat_proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        added_str, deleted_str, file_path = parts[0], parts[1], parts[-1]
        file_path = file_path.replace("\\", "/")
        files.append({
            "path": file_path,
            "status": status_by_path.get(file_path, "M"),
            "additions": int(added_str) if added_str.isdigit() else 0,
            "deletions": int(deleted_str) if deleted_str.isdigit() else 0,
        })

    unified_diff, truncated = _truncate_text(diff_proc.stdout)
    return {"ok": True, "files": files, "unified_diff": unified_diff, "truncated": truncated, "staged": staged}


def _within_root(root: Path, rel_or_abs: str) -> Path | None:
    """Resolve a path against the project root and confirm it stays inside it.

    Returns the absolute resolved path, or None when the path escapes the project
    (a containment violation — a write tool must refuse it)."""
    raw = rel_or_abs.strip().strip('"').replace("\\", "/")
    try:
        candidate = Path(raw)
        resolved = candidate.resolve() if candidate.is_absolute() else (root / raw).resolve()
        resolved.relative_to(root.resolve())
        return resolved
    except (OSError, ValueError):
        return None


def _diff_target_paths(unified_diff: str) -> list[str]:
    """Extract EVERY repo-relative file a unified diff touches — both sides.

    Captures pre- and post-image paths from ``---``/``+++`` hunk headers AND the
    ``rename from/to`` / ``copy from/to`` lines (a pure rename has no hunk headers, so
    its source would otherwise escape the policy check — letting a protected file be
    moved out of scope). Handles paths with spaces and git's C-quoting. Every target is
    then policy-checked before the patch is applied."""
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
            # Fallback for metadata-only changes (e.g. mode change) that have no hunk or
            # rename lines; best-effort split handles the common no-space case.
            parts = line[len("diff --git "):].split()
            if len(parts) == 2:
                _add(_clean(parts[0]))
                _add(_clean(parts[1]))
    return targets


def _lease_ttl_seconds(root: Path) -> int:
    """Default MCP lease TTL from config (so a crashed agent's lease auto-expires)."""
    try:
        from devcouncil.app.config import load_config

        return max(0, int(load_config(root).execution.lease_ttl_seconds))
    except Exception:
        return 1800


def _load_router(root: Path):
    """Build a ModelRouter from project config, or return None when no provider key
    is configured. When present, the verifier runs DevCouncil's strong compiled
    per-criterion acceptance checks; when None, it falls back to coarse mode (which
    the verify response now reports explicitly so the agent is never misled)."""
    try:
        from devcouncil.app.config import load_config, get_api_key
        from devcouncil.llm.provider import create_provider, validate_model_provider
        from devcouncil.llm.router import ModelRouter

        config = load_config(root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, root)
        provider = create_provider(config.models.provider, api_key, project_root=root)
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        return ModelRouter(provider, role_config, project_root=root)
    except Exception:
        return None


def _error_text(message: str, *, code: str = "error", **details: object) -> list[TextContent]:
    return _json_text({"ok": False, "error": message, "code": code, **details})


def _normalize_arguments(arguments: object) -> dict:
    return arguments if isinstance(arguments, dict) else {}


def _int_argument(arguments: dict, name: str, default: int, *, minimum: int, maximum: int) -> int:
    value = arguments.get(name, default)
    if not isinstance(value, int) or isinstance(value, bool):
        value = default
    return max(minimum, min(value, maximum))


def _optional_string_argument(arguments: dict, name: str) -> str | None:
    value = arguments.get(name)
    if value is None:
        return None
    return value if isinstance(value, str) else ""


def _optional_string_list_argument(arguments: dict, name: str) -> tuple[list[str], list[TextContent] | None]:
    value = arguments.get(name)
    if value is None:
        return [], None
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        return [], _error_text(f"{name} must be a string array", code="invalid_arguments", argument=name)
    return value, None


def _required_string_argument(arguments: dict, name: str) -> tuple[str | None, list[TextContent] | None]:
    value = arguments.get(name)
    if value is None or value == "":
        return None, _error_text(f"Missing {name}", code="missing_argument", argument=name)
    if not isinstance(value, str):
        return None, _error_text(f"{name} must be a string", code="invalid_arguments", argument=name)
    return value, None


def _run_cli_command(args: list[str], root: Path) -> dict[str, object]:
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
        stdout, stdout_truncated = _truncate_text(result.stdout)
        stderr, stderr_truncated = _truncate_text(result.stderr)
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
        stdout, stdout_truncated = _truncate_text(exc.output)
        stderr, stderr_truncated = _truncate_text(exc.stderr)
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


def _project_root() -> Path:
    configured = os.environ.get("DEVCOUNCIL_PROJECT_ROOT")
    return Path(configured).expanduser().resolve() if configured else Path(".")


def _knowledge_source_uri(kind: str, name: str) -> str:
    """Stable, parseable resource URI for one ingested knowledge source.

    The name is percent-encoded so OKF/design source names with spaces or slashes
    still yield a valid AnyUrl and round-trip cleanly through read_resource."""
    from urllib.parse import quote

    return f"devcouncil://knowledge/{kind}/{quote(name, safe='')}"


def _discover_knowledge_sources(root: Path) -> list:
    """Best-effort enumeration of ingested OKF/design knowledge for the project.

    A broken or absent knowledge layer must never break resource listing, so any
    failure degrades to an empty list (mirrors the other optional handlers here).

    Honors the project's ``knowledge`` config (enabled / directory / design_always) so MCP
    exposes exactly what the planning and task prompts do — otherwise a project that
    disabled or relocated its knowledge would still leak it through MCP resources."""
    try:
        from devcouncil.knowledge.sources import discover_knowledge_sources

        directory, design_always = _knowledge_settings(root)
        if directory is None:  # explicitly disabled in config
            return []
        return discover_knowledge_sources(root, directory=directory, design_always=design_always)
    except Exception:
        return []


def _knowledge_settings(root: Path) -> tuple[str | None, bool]:
    """Resolve (directory, design_always) for knowledge exposure from project config.

    Returns ``(None, _)`` when the project explicitly disables knowledge so callers can
    suppress it. Falls back to defaults when no/invalid config is present (the MCP server
    must keep working for projects without a full ``.devcouncil/config.yaml``)."""
    try:
        from devcouncil.app.config import load_config

        cfg = load_config(root).knowledge
        return (None if not cfg.enabled else cfg.directory), cfg.design_always
    except Exception:
        return ".devcouncil/knowledge", True


def _is_secret_path(root: Path, rel_or_abs: str) -> bool:
    """True when a path matches a protected secret/credential glob.

    Reuses the shared SECRET_PATH_PATTERNS (the single source of truth in the policy
    engine) so read tools refuse exactly the same files the write gate refuses — an
    MCP agent must never be able to exfiltrate a credential through a read tool."""
    from devcouncil.execution.policy_engine import SECRET_PATH_PATTERNS

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
    import fnmatch as _fnmatch

    return any(_fnmatch.fnmatch(normalized, pattern) for pattern in SECRET_PATH_PATTERNS)


def _allowed_next_tools(status: str, has_blocking_gaps: bool) -> list[str]:
    """Compute the self-describing next-tool contract from task state.

    Replaces a hardcoded list so an agent is steered by what the task actually
    needs: a verified task only needs releasing; a blocked/running task gets the
    full read->edit->test loop; a planned task should check out first."""
    if status == "verified":
        return ["devcouncil_release_task"]
    if status == "done":
        return []
    if status in {"running", "blocked"} or has_blocking_gaps:
        return [
            "devcouncil_read_file",
            "devcouncil_get_evidence",
            "devcouncil_get_diff",
            "devcouncil_run_command",
            "devcouncil_apply_patch",
            "devcouncil_write_file",
            "devcouncil_verify_task",
        ]
    # planned / ready and not yet leased: bootstrap by checking out.
    return [
        "devcouncil_checkout_task",
        "devcouncil_read_file",
        "devcouncil_get_diff",
    ]


@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="devcouncil_status",
            description="Get the current status of the DevCouncil project, including phase, tasks, and gaps.",
            inputSchema={
                "type": "object",
                "properties": {}
            }
        ),
        Tool(
            name="devcouncil_integration_status",
            description="Get read-only coding CLI integration status, capability rows, detected clients, and recommended executor.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="devcouncil_report",
            description="Get the full coverage report and a list of all requirements and blocking gaps.",
            inputSchema={
                "type": "object",
                "properties": {}
            }
        ),
        Tool(
            name="devcouncil_get_task",
            description="Get details, constraints, and requirements for a specific implementation task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the task, e.g. TASK-001"
                    }
                },
                "required": ["task_id"]
            }
        ),
        Tool(
            name="devcouncil_get_gaps",
            description=(
                "Read the persisted verification gaps for a task WITHOUT re-running "
                "verification. Cheap and idempotent — use it to resume after a "
                "reconnect or to inspect outstanding work before deciding to repair."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "blocking_only": {"type": "boolean", "default": False},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_get_next_actions",
            description=(
                "Get the typed, machine-routable next-actions contract for a task from "
                "its persisted gaps, WITHOUT re-verifying. Returns blocking next_actions "
                "plus advisory_actions and the tools allowed next."
            ),
            inputSchema={
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_get_task_provenance",
            description=(
                "Inspect the recorded audit trail for a task: gated file changes "
                "(write_file/apply_patch and hook events), verification runs, diff-coverage "
                "evidence (was the changed code actually exercised), and the latest "
                "correction manifest. Read-only — lets a developer or agent trust what "
                "actually happened on disk."
            ),
            inputSchema={
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_live_review",
            description="Get live coding-agent review status, pending signals, critique-card counts, and blockers.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional task scope for live-review blocker calculation.",
                    }
                },
            },
        ),
        Tool(
            name="devcouncil_live_cards",
            description="List live-review critique cards with optional task, status, verdict, and client filters.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional task scope for critique cards.",
                    },
                    "status": {
                        "type": "string",
                        "enum": ["open", "resolved", "ignored"],
                        "description": "Optional card status filter.",
                    },
                    "verdict": {
                        "type": "string",
                        "enum": ["approved", "concerns", "critical"],
                        "description": "Optional card verdict filter.",
                    },
                    "client": {
                        "type": "string",
                        "description": "Optional coding-agent client filter.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "default": 20,
                    },
                },
            },
        ),
        Tool(
            name="devcouncil_live_repair_prompt",
            description="Generate a ready-to-paste repair prompt for a live-review critique card.",
            inputSchema={
                "type": "object",
                "properties": {
                    "card_id": {
                        "type": "string",
                        "description": "The critique card ID, e.g. CARD-abc123.",
                    }
                },
                "required": ["card_id"],
            },
        ),
        Tool(
            name="devcouncil_live_repair_all",
            description="Generate one repair prompt for all blocking live-review critique cards in scope.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "Optional task scope for blocking live-review cards.",
                    }
                },
            },
        ),
        Tool(
            name="devcouncil_list_tasks",
            description="List DevCouncil tasks with status and requirement mappings. Supports a status filter and limit/offset paging so large projects don't blow the agent's context.",
            inputSchema={
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "Optional status filter (e.g. planned, running, blocked, verified, done)."},
                    "limit": {"type": "integer", "description": "Max tasks to return (default 100, max 500)."},
                    "offset": {"type": "integer", "description": "Number of tasks to skip (default 0)."},
                },
            },
        ),
        Tool(
            name="devcouncil_get_prompt",
            description="Get the raw implementation prompt for a DevCouncil task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "The ID of the task, e.g. TASK-001"},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_tail_trace",
            description="Return recent DevCouncil trace events as JSON.",
            inputSchema={
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200, "default": 20},
                },
            },
        ),
        Tool(
            name="devcouncil_policy_check_write",
            description="Check whether a file write is allowed for a task or the active running task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Repository-relative or absolute path to check."},
                    "task_id": {"type": "string", "description": "Optional task ID. Defaults to the running task."},
                },
                "required": ["path"],
            },
        ),
        Tool(
            name="devcouncil_graph_context",
            description="Get optional code-review-graph structural context for changed or planned files.",
            inputSchema={
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Repository-relative files to contextualize.",
                    }
                },
            },
        ),
        Tool(
            name="devcouncil_lsp_status",
            description="Return detected language servers and starter LSP initialize payloads.",
            inputSchema={"type": "object", "properties": {}},
        ),
        Tool(
            name="devcouncil_ast_match",
            description="Search code symbols structurally using optional tree-sitter support and deterministic fallbacks.",
            inputSchema={
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "language": {"type": "string"},
                    "kind": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 100},
                },
            },
        ),
        Tool(
            name="devcouncil_cli",
            description="Run a safe DevCouncil CLI command for status, tasks, report, map, prompt, show, trace, lsp, or ast.",
            inputSchema={
                "type": "object",
                "properties": {
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Arguments after the dev command, for example ['status','--json'].",
                    }
                },
                "required": ["args"],
            },
        ),
        Tool(
            name="devcouncil_prepare_execution",
            description="Return a task prompt plus planned files and allowed commands for external execution tooling.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "The ID of the task, e.g. TASK-001"},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_checkout_task",
            description="Acquire a task lease and return scope for MCP write tools.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "client_id": {"type": "string"},
                    "agent": {"type": "string"},
                    "force": {"type": "boolean", "default": False},
                },
                "required": ["task_id", "client_id"],
            },
        ),
        Tool(
            name="devcouncil_release_task",
            description="Release a task lease using its token.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_renew_lease",
            description=(
                "Extend a held task lease's TTL so a long-running agent does not lose it "
                "to expiry. Returns the new expires_at."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "ttl_seconds": {"type": "integer"},
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_list_leases",
            description=(
                "List task leases for fleet supervision — task_id, owner, agent, "
                "expires_at, and whether each is expired. Defaults to active leases."
            ),
            inputSchema={
                "type": "object",
                "properties": {"active_only": {"type": "boolean", "default": True}},
            },
        ),
        Tool(
            name="devcouncil_update_task_scope",
            description="Append unique expected tests or allowed commands for a leased task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "expected_tests": {"type": "array", "items": {"type": "string"}},
                    "allowed_commands": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_append_evidence",
            description="Append command evidence for a leased task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "command": {"type": "string"},
                    "exit_code": {"type": "integer"},
                    "summary": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "command", "exit_code", "summary"],
            },
        ),
        Tool(
            name="devcouncil_record_command",
            description="Record a shell command event for a leased task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "command": {"type": "string"},
                    "status": {"type": "string", "enum": ["started", "finished", "failed", "blocked"]},
                    "exit_code": {"type": "integer"},
                    "reason": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "command", "status"],
            },
        ),
        Tool(
            name="devcouncil_write_file",
            description=(
                "Write a file for a leased task through DevCouncil's policy gate. The write "
                "is checked against the task's scope BEFORE it lands (out-of-scope or "
                "protected paths are rejected), applied atomically, and recorded as a "
                "FileChangeEvent. Returns applied_files and rejected_files."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "path", "content"],
            },
        ),
        Tool(
            name="devcouncil_apply_patch",
            description=(
                "Apply a unified diff for a leased task through DevCouncil's policy gate. "
                "EVERY target file is policy-checked first; if any is out of scope the whole "
                "patch is rejected (never partially applied). Applied atomically via git and "
                "each file recorded as a FileChangeEvent. Returns applied_files/rejected_files."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "unified_diff": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "unified_diff"],
            },
        ),
        Tool(
            name="devcouncil_verify_task",
            description="Run verification for a leased task (local sandbox).",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "sandbox": {"type": "string", "enum": ["local"], "default": "local", "description": "Only 'local' is supported in this build."},
                },
                "required": ["task_id", "lease_token"],
            },
        ),
        Tool(
            name="devcouncil_handoff_agent",
            description="Hand off a task between coding CLI agents.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "from_agent": {"type": "string"},
                    "to_agent": {"type": "string"},
                    "instruction": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "from_agent", "to_agent"],
            },
        ),
        Tool(
            name="devcouncil_read_file",
            description=(
                "Read a repository file (read-only, no lease required) so an MCP-only "
                "agent can inspect content before constructing a diff or overwriting it. "
                "Containment-checked against the project root and refuses secret/credential "
                "paths. Supports offset/limit or line_range windowing. Returns content "
                "(truncated), sha256, and line_count."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Repository-relative or absolute path inside the project."},
                    "offset": {"type": "integer", "minimum": 0, "description": "0-based line offset to start from."},
                    "limit": {"type": "integer", "minimum": 1, "description": "Max number of lines to return."},
                    "line_range": {
                        "type": "string",
                        "description": "Inclusive 1-based line range like '10-40' (overrides offset/limit).",
                    },
                },
                "required": ["path"],
            },
        ),
        Tool(
            name="devcouncil_get_diff",
            description=(
                "Return the working-tree diff for the project (requires a git repo). When "
                "task_id is given the diff is scoped to that task's planned/changed files. "
                "Set staged=true to include the staged (git diff --cached) changes. Returns "
                "per-file status with additions/deletions and the truncated unified diff."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Optional task to scope the diff to its files."},
                    "paths": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional explicit repo-relative paths to scope the diff to.",
                    },
                    "staged": {"type": "boolean", "default": False, "description": "Include staged changes."},
                },
            },
        ),
        Tool(
            name="devcouncil_get_evidence",
            description=(
                "Read persisted CommandResult evidence for a task and inline the truncated "
                "stdout/stderr from the stored log files (best-effort; tolerates missing "
                "files). Pairs with verification to close the diagnose leg of the loop."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "command": {"type": "string", "description": "Optional substring filter on the recorded command."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
                },
                "required": ["task_id"],
            },
        ),
        Tool(
            name="devcouncil_run_command",
            description=(
                "Run a command for a leased task through DevCouncil's allowlist gate. The "
                "command must pass the task's allowed_commands policy (same gate as the "
                "hooks); otherwise it is refused and nothing runs. Executed with a clean "
                "subprocess env and a timeout, recorded as a ShellCommandEvent. Returns "
                "exit_code and truncated stdout/stderr."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "lease_token": {"type": "string"},
                    "command": {"type": "string"},
                },
                "required": ["task_id", "lease_token", "command"],
            },
        ),
        Tool(
            name="devcouncil_list_agent_runs",
            description=(
                "List recorded coding-agent runs (from .devcouncil/runs/*/agent-run.json), "
                "newest first. Each entry includes run_id, task, agent, profile, status, "
                "started time, and an orphaned flag for runs still marked running whose "
                "manifest has gone stale (executor likely crashed). Read-only."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "Optional status filter (e.g. running, finished, failed, timeout)."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 20},
                },
            },
        ),
        Tool(
            name="devcouncil_get_run",
            description=(
                "Get the full manifest for a single coding-agent run plus a redacted "
                "transcript tail when a transcript/log file exists in the run directory. "
                "Includes the resolved CLI invocation and an orphaned flag. Read-only."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "run_id": {"type": "string", "description": "The run id to inspect."},
                },
                "required": ["run_id"],
            },
        ),
        Tool(
            name="devcouncil_next_task",
            description=(
                "Return the highest-priority task that is unblocked (its depends_on are "
                "satisfied) and has no active lease, so an autonomous agent can bootstrap "
                "deterministically instead of racing list_tasks. Includes a blocking-gap "
                "summary and a ready_to_checkout flag."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "client_id": {"type": "string", "description": "Optional client id (informational)."},
                    "status": {"type": "string", "description": "Optional status filter (default planned/ready)."},
                },
            },
        ),
        Tool(
            name="devcouncil_select_knowledge",
            description=(
                "Select the ingested project knowledge (OKF documents and the design "
                "system) that applies to a goal and return it as a ready-to-inject "
                "markdown preamble, so a coding agent can ask 'what project knowledge "
                "applies to <goal>?'. Always-on design knowledge is included; OKF "
                "documents are matched on goal keywords. Returns the matched sources "
                "and the rendered preamble."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "goal": {"type": "string", "description": "The task or goal to find applicable knowledge for."},
                },
                "required": ["goal"],
            },
        ),
    ]

@app.list_resources()
async def list_resources() -> list[Resource]:
    """Expose the DevCouncil corpus as browsable/subscribable MCP resources, so a host
    can read the report, task graph, gaps, and live-review state without a tool call."""
    root = _project_root()
    resources: list[Resource] = [
        Resource(uri=AnyUrl("devcouncil://report"), name="DevCouncil report",
                 description="Coverage report, requirement/task mapping, and blocking gaps.",
                 mimeType="text/markdown"),
        Resource(uri=AnyUrl("devcouncil://tasks"), name="Tasks",
                 description="All planned tasks with scope and status.", mimeType="application/json"),
        Resource(uri=AnyUrl("devcouncil://gaps"), name="Gaps",
                 description="All open verification gaps.", mimeType="application/json"),
        Resource(uri=AnyUrl("devcouncil://cards"), name="Live review",
                 description="Live-review summary: cards, signals, and blockers.",
                 mimeType="application/json"),
    ]
    db = get_db(root)
    if db:
        with db.get_session() as session:
            for task in TaskRepository(session).get_all():
                resources.append(Resource(
                    uri=AnyUrl(f"devcouncil://task/{task.id}"),
                    name=f"Task {task.id}: {task.title}",
                    description=f"Scope, status, and gaps for {task.id}.",
                    mimeType="application/json",
                ))
    # Project knowledge (ingested OKF + design.md) — surfaced only when something has
    # actually been ingested, so hosts without a knowledge layer see no empty entries.
    knowledge_sources = _discover_knowledge_sources(root)
    if knowledge_sources:
        resources.append(Resource(
            uri=AnyUrl("devcouncil://knowledge"),
            name="Project knowledge",
            description="Index of ingested OKF and design knowledge for this project.",
            mimeType="text/markdown",
        ))
        for source in knowledge_sources:
            resources.append(Resource(
                uri=AnyUrl(_knowledge_source_uri(source.kind, source.name)),
                name=f"Knowledge ({source.kind}): {source.description or source.name}",
                description=source.description or source.name,
                mimeType="text/markdown",
            ))
    return resources


@app.read_resource()
async def read_resource(uri: AnyUrl) -> str:
    root = _project_root()
    db = get_db(root)
    key = str(uri).rstrip("/")

    if key == "devcouncil://report":
        if not db:
            return "DevCouncil is not initialized in this directory."
        with db.get_session() as session:
            graph = ArtifactGraphRepository(session).load_graph()
        return ReportBuilder.build_markdown(graph, live_review=live_review_summary(root))
    if key == "devcouncil://tasks":
        if not db:
            return json.dumps({"tasks": []})
        with db.get_session() as session:
            tasks = [t.model_dump() for t in TaskRepository(session).get_all()]
        return json.dumps({"tasks": tasks}, indent=2)
    if key == "devcouncil://gaps":
        if not db:
            return json.dumps({"gaps": []})
        with db.get_session() as session:
            gaps = [g.model_dump() for g in GapRepository(session).get_all()]
        return json.dumps({"gaps": gaps}, indent=2)
    if key == "devcouncil://cards":
        return json.dumps(live_review_summary(root), indent=2)
    if key.startswith("devcouncil://task/"):
        task_id = key.rsplit("/", 1)[-1]
        if not db:
            return json.dumps({"ok": False, "error": "not initialized"})
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(task_id)
            if not task:
                return json.dumps({"ok": False, "error": f"Task {task_id} not found."})
            gaps = [g.model_dump() for g in GapRepository(session).get_all() if g.task_id == task_id]
        return json.dumps({"task": task.model_dump(), "gaps": gaps}, indent=2)

    if key == "devcouncil://knowledge":
        # Markdown index linking each ingested source to its per-source resource URI.
        sources = _discover_knowledge_sources(root)
        if not sources:
            return "# Project knowledge\n\nNo OKF or design knowledge has been ingested for this project."
        lines = ["# Project knowledge", "", "Ingested OKF and design knowledge for this project.", ""]
        for kind in ("design", "okf"):
            kind_sources = [s for s in sources if s.kind == kind]
            if not kind_sources:
                continue
            lines.append(f"## {kind.upper() if kind == 'okf' else kind.capitalize()}")
            lines.append("")
            for source in kind_sources:
                uri = _knowledge_source_uri(source.kind, source.name)
                desc = source.description or source.name
                lines.append(f"- [{desc}]({uri})")
            lines.append("")
        return "\n".join(lines).strip()
    if key.startswith("devcouncil://knowledge/"):
        # Match the requested URI back to a discovered source and render its markdown.
        for source in _discover_knowledge_sources(root):
            if _knowledge_source_uri(source.kind, source.name) == key:
                return source.render() or source.body
        return f"Knowledge source not found: {key}"

    raise ValueError(f"Unknown resource: {uri}")


@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    arguments = _normalize_arguments(arguments)
    root = _project_root()
    db = get_db(root)
    if name in _DB_REQUIRED_TOOLS and not db:
        return _error_text("DevCouncil not initialized in this directory.", code="not_initialized")

    if name == "devcouncil_integration_status":
        return _json_text(integration_status_summary(root))

    if name == "devcouncil_status":
        assert db is not None
        with db.get_session() as session:
            graph_repo = ArtifactGraphRepository(session)
            graph = graph_repo.load_graph()
            summary = graph.coverage_summary()
            state = StateRepository(session).get_state()
            phase = compute_phase(graph, state.current_phase if state else None)
            
            status_str = f"Phase: {phase}\n"
            status_str += f"Requirements: {summary['total_requirements']} ({summary['requirements_without_tasks']} unmapped)\n"
            status_str += f"Tasks: {summary['total_tasks']} ({summary['tasks_without_requirements']} orphaned)\n"
            status_str += f"Gaps: {summary['total_gaps']} ({summary['blocking_gaps']} blocking)\n"
            
            return [TextContent(type="text", text=status_str)]

    elif name == "devcouncil_report":
        assert db is not None
        with db.get_session() as session:
            graph_repo = ArtifactGraphRepository(session)
            graph = graph_repo.load_graph()
            markdown_report = ReportBuilder.build_markdown(graph, live_review=live_review_summary(root))
            return [TextContent(type="text", text=markdown_report)]

    elif name == "devcouncil_live_review":
        task_id = _optional_string_argument(arguments, "task_id")
        if task_id == "":
            return _error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
        return [TextContent(
            type="text",
            text=json.dumps(live_review_summary(root, task_id=task_id), indent=2),
        )]

    elif name == "devcouncil_live_cards":
        task_id = _optional_string_argument(arguments, "task_id")
        status = _optional_string_argument(arguments, "status")
        verdict = _optional_string_argument(arguments, "verdict")
        client = _optional_string_argument(arguments, "client")
        for arg_name, value in [
            ("task_id", task_id),
            ("status", status),
            ("verdict", verdict),
            ("client", client),
        ]:
            if value == "":
                return _error_text(f"{arg_name} must be a string", code="invalid_arguments", argument=arg_name)

        limit = _int_argument(arguments, "limit", 20, minimum=1, maximum=200)
        filtered, filter_error, argument = filter_cards(
            load_cards(root),
            task_id=task_id,
            status=status,
            verdict=verdict,
            client=client,
        )
        if filter_error:
            return _error_text(filter_error, code="invalid_arguments", argument=argument)

        total = len(filtered)
        return [TextContent(
            type="text",
            text=json.dumps({
                "cards": [card.model_dump() for card in filtered[:limit]],
                "filters": {
                    "task_id": task_id,
                    "status": status,
                    "verdict": verdict,
                    "client": client,
                },
                "limit": limit,
                "total": total,
            }, indent=2),
        )]

    elif name == "devcouncil_live_repair_prompt":
        card_id, arg_error = _required_string_argument(arguments, "card_id")
        if arg_error:
            return arg_error
        assert card_id is not None
        card = get_card(root, card_id)
        if not card:
            return _error_text(f"Critique card {card_id} not found.", code="not_found", card_id=card_id)
        return [TextContent(
            type="text",
            text=json.dumps({
                "card": card.model_dump(),
                "prompt": build_live_repair_prompt(root, card),
            }, indent=2),
        )]

    elif name == "devcouncil_live_repair_all":
        task_id = _optional_string_argument(arguments, "task_id")
        if task_id == "":
            return _error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
        summary = live_review_summary(root, task_id=task_id)
        cards = [
            get_card(root, item["id"])
            for item in summary["blocking_cards"]
            if isinstance(item.get("id"), str)
        ]
        resolved_cards = [card for card in cards if card is not None]
        return [TextContent(
            type="text",
            text=json.dumps({
                "scope_task_id": summary["scope_task_id"],
                "cards": [card.model_dump() for card in resolved_cards],
                "prompt": build_bulk_live_repair_prompt(root, resolved_cards),
            }, indent=2),
        )]
            
    elif name == "devcouncil_get_task":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        assert task_id is not None
            
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=str(task_id))
            
            return [TextContent(type="text", text=task.model_dump_json(indent=2))]

    elif name == "devcouncil_get_gaps":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        blocking_only = bool(arguments.get("blocking_only", False))
        with db.get_session() as session:
            gaps = [g for g in GapRepository(session).get_all() if g.task_id == task_id]
        if blocking_only:
            gaps = [g for g in gaps if g.blocking]
        return _json_text({
            "ok": True,
            "task_id": task_id,
            "gaps": [g.model_dump() for g in gaps],
            "blocking_count": sum(1 for g in gaps if g.blocking),
        })

    elif name == "devcouncil_get_next_actions":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        assert task_id is not None
        with db.get_session() as session:
            gaps = [g for g in GapRepository(session).get_all() if g.task_id == task_id]
            task = TaskRepository(session).get_by_id(task_id)
        blocking_actions, advisory_actions = split_next_actions(gaps)
        has_blocking = any(g.blocking for g in gaps)
        return _json_text({
            "ok": True,
            "task_id": task_id,
            "next_actions": [a.model_dump() for a in blocking_actions],
            "advisory_actions": [a.model_dump() for a in advisory_actions],
            # Self-describing loop: computed from the task's status + blocking gaps so
            # the agent is steered toward what this task actually needs next.
            "allowed_next_tools": _allowed_next_tools(task.status if task else "planned", has_blocking),
        })

    elif name == "devcouncil_get_task_provenance":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        assert task_id is not None
        with db.get_session() as session:
            file_changes = [r.model_dump() for r in FileChangeRepository(session).list_for_task(task_id)]
            verification_runs = [r.model_dump() for r in VerificationRunRepository(session).list_for_task(task_id)]
            coverage = [
                ev.model_dump()
                for ev in EvidenceRepository(session).get_all()
                if isinstance(ev, DiffCoverageEvidence) and ev.task_id == task_id
            ]
            correction_manifest = CorrectionManifestRepository(session).latest_for_task(task_id)
        return _json_text({
            "ok": True,
            "task_id": task_id,
            "file_changes": file_changes,
            "verification_runs": verification_runs,
            "diff_coverage": coverage,
            "latest_correction_manifest": correction_manifest.model_dump() if correction_manifest else None,
        })

    elif name == "devcouncil_list_tasks":
        assert db is not None
        status_filter = _optional_string_argument(arguments, "status")
        if status_filter == "":
            return _error_text("status must be a string", code="invalid_arguments", argument="status")
        limit = _int_argument(arguments, "limit", 100, minimum=1, maximum=500)
        offset = _int_argument(arguments, "offset", 0, minimum=0, maximum=1_000_000)
        with db.get_session() as session:
            all_tasks = TaskRepository(session).get_all()
        if status_filter:
            all_tasks = [t for t in all_tasks if t.status == status_filter]
        total = len(all_tasks)
        window = all_tasks[offset:offset + limit]
        return _json_text({
            "tasks": [task.model_dump() for task in window],
            "total": total,
            "offset": offset,
            "limit": limit,
            "returned": len(window),
        })

    elif name == "devcouncil_get_prompt":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        assert task_id is not None

        with db.get_session() as session:
            task_repo = TaskRepository(session)
            req_repo = RequirementRepository(session)
            task = task_repo.get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=str(task_id))
            prompt = PromptBuilder(root).build_task_prompt(task, req_repo.get_all())
            return [TextContent(type="text", text=prompt)]

    elif name == "devcouncil_tail_trace":
        limit = _int_argument(arguments, "limit", 20, minimum=1, maximum=200)
        events = list(read_trace_events(root))[-limit:]

        return [TextContent(
            type="text",
            text=json.dumps({"events": [event.model_dump(by_alias=True) for event in events]}, indent=2),
        )]

    elif name == "devcouncil_policy_check_write":
        assert db is not None
        path, arg_error = _required_string_argument(arguments, "path")
        if arg_error:
            return arg_error
        assert path is not None
        task_id = _optional_string_argument(arguments, "task_id")
        if task_id == "":
            return _error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            if task_id:
                task = task_repo.get_by_id(task_id)
            else:
                running = [task for task in task_repo.get_all() if task.status == "running"]
                task = running[0] if running else None
            decision = HookPolicy(project_root=root).evaluate_file_write(path, task)
            return [TextContent(type="text", text=json.dumps({
                "action": decision.action,
                "allowed": decision.allowed,
                "reason": decision.reason,
                "target": decision.target,
                "task_id": task.id if task else None,
            }, indent=2))]

    elif name == "devcouncil_graph_context":
        files = arguments.get("files", [])
        if not isinstance(files, list):
            files = []
        context = CodeReviewGraphAdapter(root).get_context([file for file in files if isinstance(file, str)])
        return [TextContent(type="text", text=context.model_dump_json(indent=2))]

    elif name == "devcouncil_lsp_status":
        return [TextContent(type="text", text=LspInspector(root).summary_json())]

    elif name == "devcouncil_ast_match":
        query = _optional_string_argument(arguments, "query")
        language = _optional_string_argument(arguments, "language")
        kind = _optional_string_argument(arguments, "kind")
        for arg_name, value in [("query", query), ("language", language), ("kind", kind)]:
            if value == "":
                return _error_text(f"{arg_name} must be a string", code="invalid_arguments", argument=arg_name)
        limit = _int_argument(arguments, "limit", 100, minimum=1, maximum=500)
        matches = AstMatcher(root).match(
            query=query or "",
            language=language,
            kind=kind,
            limit=limit,
        )
        return [TextContent(type="text", text=json.dumps({"matches": [item.model_dump() for item in matches]}, indent=2))]

    elif name == "devcouncil_cli":
        args = arguments.get("args")
        if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args) or not args:
            return _error_text("args must be a non-empty string array", code="invalid_arguments")
        if args[0] not in _CLI_ALLOWED_ROOTS:
            return _error_text(f"command {args[0]} is not allowed through MCP", code="command_not_allowed", command=args[0])
        forbidden = _forbidden_cli_flags(args)
        if forbidden:
            return _error_text("forbidden flag(s) through MCP: " + ", ".join(forbidden), code="forbidden_flags", flags=forbidden)
        try:
            return _json_text(_run_cli_command(args, root))
        except Exception as exc:
            return _error_text(str(exc), code="cli_execution_error")

    elif name == "devcouncil_prepare_execution":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        assert task_id is not None
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            req_repo = RequirementRepository(session)
            task = task_repo.get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=str(task_id))
            prompt = PromptBuilder(root).build_task_prompt(task, req_repo.get_all())
            return [TextContent(type="text", text=json.dumps({
                "task_id": task.id,
                "prompt": prompt,
                "planned_files": [file.model_dump() for file in task.planned_files],
                "allowed_commands": task.allowed_commands,
                "expected_tests": task.expected_tests,
            }, indent=2))]

    elif name == "devcouncil_checkout_task":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        client_id, arg_error = _required_string_argument(arguments, "client_id")
        if arg_error:
            return arg_error
        assert task_id is not None and client_id is not None
        agent = _optional_string_argument(arguments, "agent")
        if agent == "":
            return _error_text("agent must be a string", code="invalid_arguments", argument="agent")
        force_value = arguments.get("force", False)
        if not isinstance(force_value, bool):
            return _error_text("force must be a boolean", code="invalid_arguments", argument="force")
        force = force_value
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)
            lease_repo = TaskLeaseRepository(session)
            try:
                lease = lease_repo.acquire(
                    task_id,
                    owner=f"mcp:{client_id}",
                    agent=agent,
                    client_id=client_id,
                    ttl_seconds=_lease_ttl_seconds(root),
                    force=force,
                )
            except ValueError as exc:
                return _error_text(str(exc), code="lease_conflict", task_id=task_id)
            prompt = PromptBuilder(root).build_task_prompt(task, RequirementRepository(session).get_all())
            semantic = None
            semantic_path = root / ".devcouncil" / "semantic" / task_id / "before.json"
            if semantic_path.exists():
                semantic = json.loads(semantic_path.read_text(encoding="utf-8"))
            return _json_text({
                "ok": True,
                "lease_token": lease.lease_token,
                "task_id": task.id,
                "status": task.status,
                "expires_at": lease.expires_at,
                "prompt": prompt,
                "planned_files": [f.model_dump() for f in task.planned_files],
                "allowed_commands": task.allowed_commands,
                "expected_tests": task.expected_tests,
                "semantic_context": semantic,
                # The task is now leased and running-ready: surface the inner-loop tools.
                "allowed_next_tools": _allowed_next_tools(
                    "running",
                    any(g.blocking for g in GapRepository(session).get_all() if g.task_id == task_id),
                ),
            })

    elif name == "devcouncil_release_task":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        assert task_id is not None and lease_token is not None
        with db.get_session() as session:
            released = TaskLeaseRepository(session).release(task_id, lease_token)
            if not released:
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
            return _json_text({"ok": True, "task_id": task_id, "released": True})

    elif name == "devcouncil_renew_lease":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        assert task_id is not None and lease_token is not None
        ttl_value = arguments.get("ttl_seconds")
        if ttl_value is not None and (not isinstance(ttl_value, int) or isinstance(ttl_value, bool)):
            return _error_text("ttl_seconds must be an integer", code="invalid_arguments", argument="ttl_seconds")
        ttl_seconds = ttl_value if isinstance(ttl_value, int) and not isinstance(ttl_value, bool) else _lease_ttl_seconds(root)
        with db.get_session() as session:
            renewed_lease = TaskLeaseRepository(session).renew(task_id, lease_token, ttl_seconds)
            if renewed_lease is None:
                return _error_text("Invalid or expired lease.", code="invalid_lease", task_id=task_id)
            return _json_text({
                "ok": True,
                "task_id": task_id,
                "expires_at": renewed_lease.expires_at,
                "ttl_seconds": ttl_seconds,
            })

    elif name == "devcouncil_list_leases":
        assert db is not None
        active_only = arguments.get("active_only", True)
        if not isinstance(active_only, bool):
            return _error_text("active_only must be a boolean", code="invalid_arguments", argument="active_only")
        with db.get_session() as session:
            pairs = TaskLeaseRepository(session).list_leases(active_only=active_only)
        leases = [
            {
                "task_id": lease.task_id,
                "owner": lease.owner,
                "agent": lease.agent,
                "status": lease.status,
                "expires_at": lease.expires_at,
                "expired": expired,
            }
            for lease, expired in pairs
        ]
        return _json_text({"ok": True, "leases": leases, "count": len(leases)})

    elif name == "devcouncil_update_task_scope":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        assert task_id is not None and lease_token is not None
        expected_tests, arg_error = _optional_string_list_argument(arguments, "expected_tests")
        if arg_error:
            return arg_error
        allowed_commands, arg_error = _optional_string_list_argument(arguments, "allowed_commands")
        if arg_error:
            return arg_error
        with db.get_session() as session:
            lease_repo = TaskLeaseRepository(session)
            if not lease_repo.validate(task_id, lease_token):
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)
            for cmd in allowed_commands:
                if cmd not in task.allowed_commands:
                    task.allowed_commands.append(cmd)
            for test in expected_tests:
                if test not in task.expected_tests:
                    task.expected_tests.append(test)
            task_repo.save(task)
            return _json_text({
                "ok": True,
                "task_id": task_id,
                "allowed_commands": task.allowed_commands,
                "expected_tests": task.expected_tests,
            })

    elif name == "devcouncil_append_evidence":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        command, arg_error = _required_string_argument(arguments, "command")
        if arg_error:
            return arg_error
        summary_text, arg_error = _required_string_argument(arguments, "summary")
        if arg_error:
            return arg_error
        assert task_id is not None and lease_token is not None
        exit_code = arguments.get("exit_code", 0)
        if not isinstance(exit_code, int) or isinstance(exit_code, bool):
            return _error_text("exit_code must be an integer", code="invalid_arguments")
        with db.get_session() as session:
            if not TaskLeaseRepository(session).validate(task_id, lease_token):
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
            EvidenceRepository(session).save_command_result(
                task_id,
                CommandResult(
                    command=command or "",
                    exit_code=exit_code,
                    stdout_path="",
                    stderr_path="",
                    summary=summary_text or "",
                ),
            )
            return _json_text({"ok": True, "task_id": task_id, "recorded": True})

    elif name == "devcouncil_record_command":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        command, arg_error = _required_string_argument(arguments, "command")
        if arg_error:
            return arg_error
        status, arg_error = _required_string_argument(arguments, "status")
        if arg_error:
            return arg_error
        assert task_id is not None and lease_token is not None
        if status not in _RECORD_COMMAND_STATUSES:
            return _error_text(
                f"status must be one of {sorted(_RECORD_COMMAND_STATUSES)}",
                code="invalid_arguments", argument="status",
            )
        with db.get_session() as session:
            if not TaskLeaseRepository(session).validate(task_id, lease_token):
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
            exit_code = arguments.get("exit_code")
            if exit_code is not None and (not isinstance(exit_code, int) or isinstance(exit_code, bool)):
                return _error_text("exit_code must be an integer", code="invalid_arguments")
            ShellCommandRepository(session).record(
                task_id,
                command or "",
                status or "finished",
                exit_code=exit_code if isinstance(exit_code, int) else None,
                reason=str(arguments.get("reason") or ""),
            )
            return _json_text({"ok": True, "task_id": task_id, "recorded": True})

    elif name == "devcouncil_write_file":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        rel_path, arg_error = _required_string_argument(arguments, "path")
        if arg_error:
            return arg_error
        content = arguments.get("content")
        if not isinstance(content, str):
            return _error_text("content must be a string", code="invalid_arguments", argument="content")
        assert task_id is not None and lease_token is not None and rel_path is not None
        with db.get_session() as session:
            lease_record = TaskLeaseRepository(session).active_for_task(task_id)
            if lease_record is None or lease_record.lease_token != lease_token:
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
            task = TaskRepository(session).get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)

            decision = HookPolicy(project_root=root).evaluate_file_write(rel_path, task, content=content)
            target = _within_root(root, rel_path)
            if target is None:
                FileChangeRepository(session).record(
                    rel_path, "write", False, task_id=task_id, lease_id=lease_record.id,
                    reason="path escapes the project root",
                )
                return _json_text({
                    "ok": False, "task_id": task_id, "applied_files": [],
                    "rejected_files": [{"path": rel_path, "reason": "path escapes the project root"}],
                })
            if not decision.allowed:
                FileChangeRepository(session).record(
                    rel_path, "write", False, task_id=task_id, lease_id=lease_record.id, reason=decision.reason,
                )
                return _json_text({
                    "ok": False, "task_id": task_id, "applied_files": [],
                    "rejected_files": [{"path": rel_path, "reason": decision.reason}],
                })
            # Atomic write: stage to a sibling temp file, then replace.
            try:
                target.parent.mkdir(parents=True, exist_ok=True)
                tmp = target.with_name(target.name + ".devcouncil-tmp")
                tmp.write_text(content, encoding="utf-8")
                os.replace(tmp, target)
            except OSError as exc:
                return _error_text(f"Write failed: {exc}", code="write_failed", task_id=task_id)
            FileChangeRepository(session).record(
                rel_path, "write", True, task_id=task_id, lease_id=lease_record.id, reason=decision.reason,
            )
            return _json_text({
                "ok": True, "task_id": task_id, "applied_files": [rel_path], "rejected_files": [],
            })

    elif name == "devcouncil_apply_patch":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        unified_diff = arguments.get("unified_diff")
        if not isinstance(unified_diff, str) or not unified_diff.strip():
            return _error_text("unified_diff must be a non-empty string", code="invalid_arguments", argument="unified_diff")
        assert task_id is not None and lease_token is not None
        if not _is_git_repo(root):
            return _error_text(
                "apply_patch requires a git repository. Use devcouncil_write_file instead.",
                code="not_a_git_repo", task_id=task_id,
            )
        targets = _diff_target_paths(unified_diff)
        if not targets:
            return _error_text("No target files found in the diff.", code="empty_patch", task_id=task_id)
        with db.get_session() as session:
            lease_record = TaskLeaseRepository(session).active_for_task(task_id)
            if lease_record is None or lease_record.lease_token != lease_token:
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
            task = TaskRepository(session).get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)

            # Policy-check EVERY target before touching the tree. Any rejection aborts
            # the whole patch — never a partial apply.
            policy = HookPolicy(project_root=root)
            rejected: list[dict[str, str]] = []
            for path in targets:
                if _within_root(root, path) is None:
                    rejected.append({"path": path, "reason": "path escapes the project root"})
                    continue
                d = policy.evaluate_file_write(path, task)
                if not d.allowed:
                    rejected.append({"path": path, "reason": d.reason})
            if rejected:
                for item in rejected:
                    FileChangeRepository(session).record(
                        item["path"], "apply_patch", False, task_id=task_id, lease_id=lease_record.id, reason=item["reason"],
                    )
                return _json_text({
                    "ok": False, "task_id": task_id, "applied_files": [], "rejected_files": rejected,
                })

            # Validate then apply atomically (git apply is all-or-nothing).
            patch_path = root / ".devcouncil" / f"mcp-apply-{lease_record.id}.patch"
            patch_path.parent.mkdir(parents=True, exist_ok=True)
            patch_path.write_text(unified_diff, encoding="utf-8")
            try:
                check = subprocess.run(
                    ["git", "apply", "--check", "--ignore-whitespace", str(patch_path)],
                    cwd=root, capture_output=True, text=True,
                )
                if check.returncode != 0:
                    return _error_text(
                        f"Patch does not apply cleanly: {check.stderr.strip()}",
                        code="patch_rejected", task_id=task_id,
                    )
                applied = subprocess.run(
                    ["git", "apply", "--ignore-whitespace", str(patch_path)],
                    cwd=root, capture_output=True, text=True,
                )
                if applied.returncode != 0:
                    return _error_text(
                        f"Patch apply failed: {applied.stderr.strip()}",
                        code="patch_failed", task_id=task_id,
                    )
            finally:
                try:
                    patch_path.unlink()
                except OSError:
                    pass
            for path in targets:
                FileChangeRepository(session).record(
                    path, "apply_patch", True, task_id=task_id, lease_id=lease_record.id, reason="policy allowed",
                )
            return _json_text({
                "ok": True, "task_id": task_id, "applied_files": targets, "rejected_files": [],
            })

    elif name == "devcouncil_verify_task":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        assert task_id is not None and lease_token is not None
        sandbox = _optional_string_argument(arguments, "sandbox") or "local"
        if sandbox in {"docker", "nix"}:
            return _json_text({
                "ok": False,
                "code": "unsupported_sandbox",
                "reason": f"Sandbox {sandbox} is not available in this build.",
                "sandbox": sandbox,
            })
        with db.get_session() as session:
            if not TaskLeaseRepository(session).validate(task_id, lease_token):
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)
            from devcouncil.verification.verifier import Verifier

            GapRepository(session).delete_for_task(task_id)
            EvidenceRepository(session).delete_for_task(task_id)
            # Run the STRONG gate when a provider key is configured (compiled
            # per-criterion checks); otherwise fall back to coarse mode and report it.
            verifier = Verifier(root, router=_load_router(root))
            evidence_gaps, evidence = await verifier.verify_task(
                task, RequirementRepository(session).get_all()
            )
            gaps = evidence_gaps
            for gap in gaps:
                GapRepository(session).save(gap)
            for ev in evidence:
                if isinstance(ev, CommandResult):
                    EvidenceRepository(session).save_command_result(task_id, ev)
                elif isinstance(ev, DiffCoverageEvidence):
                    EvidenceRepository(session).save_diff_coverage_evidence(ev)
                elif isinstance(ev, DiffEvidence):
                    EvidenceRepository(session).save_diff_evidence(ev)
                elif isinstance(ev, TestEvidence):
                    EvidenceRepository(session).save_test_evidence(ev, task_id)
            task.status = "blocked" if any(g.blocking for g in gaps) else "verified"
            task_repo.save(task)
            blocking = [g.model_dump() for g in gaps if g.blocking]
            # The typed next-actions contract: structured, routable steps the agent
            # can act on to self-repair and re-verify without a human pasting prose.
            # blocking_actions must be cleared to pass; advisory_actions (e.g. the
            # diff↔coverage "tests passed but new code never ran" signal) are quality
            # signals the agent can act on without confusing them with the gate.
            blocking_actions, advisory_actions = split_next_actions(gaps)
            outcome = verifier.last_outcome
            return _json_text({
                "ok": True,
                "task_id": task_id,
                "status": task.status,
                "sandbox": sandbox,
                "blocking_gaps": blocking,
                "next_actions": [a.model_dump() for a in blocking_actions],
                "advisory_actions": [a.model_dump() for a in advisory_actions],
                # Computed from the post-verify status: verified -> release only,
                # blocked -> the read/edit/test repair loop.
                "allowed_next_tools": _allowed_next_tools(task.status, len(blocking) > 0),
                "passed": len(blocking) == 0,
                # Rigor of this run so the agent never reads passed==True as proven
                # when the gate could not actually check.
                "verification_mode": outcome.mode if outcome else "unknown",
                "compiler_active": outcome.compiler_active if outcome else False,
                "diff_empty": outcome.diff_empty if outcome else False,
                "coverage_measured": outcome.coverage_measured if outcome else False,
                "coverage_skipped_reason": outcome.coverage_skipped_reason if outcome else None,
            })

    elif name == "devcouncil_handoff_agent":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        from_agent, arg_error = _required_string_argument(arguments, "from_agent")
        if arg_error:
            return arg_error
        to_agent, arg_error = _required_string_argument(arguments, "to_agent")
        if arg_error:
            return arg_error
        assert task_id is not None and lease_token is not None
        with db.get_session() as session:
            if not TaskLeaseRepository(session).validate(task_id, lease_token):
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
        try:
            from devcouncil.execution.handoff import HandoffService

            manifest, handoff_path, run_id = HandoffService(root).create(
                task_id,
                from_agent or "",
                to_agent or "",
                instruction=str(arguments.get("instruction") or ""),
            )
            return _json_text({
                "ok": True,
                "task_id": task_id,
                "manifest_path": str(handoff_path),
                "run_id": run_id,
                "manifest": manifest.model_dump(),
            })
        except ValueError as exc:
            return _error_text(str(exc), code="handoff_failed", task_id=task_id)

    elif name == "devcouncil_read_file":
        rel_path, arg_error = _required_string_argument(arguments, "path")
        if arg_error:
            return arg_error
        assert rel_path is not None
        if _is_secret_path(root, rel_path):
            return _error_text(
                "Refusing to read a secret/credential path.",
                code="secret_path", path=rel_path,
            )
        target = _within_root(root, rel_path)
        if target is None:
            return _error_text("path escapes the project root", code="path_escape", path=rel_path)
        if not target.exists() or not target.is_file():
            return _error_text(f"File not found: {rel_path}", code="not_found", path=rel_path)
        try:
            raw = target.read_bytes()
        except OSError as exc:
            return _error_text(f"Read failed: {exc}", code="read_failed", path=rel_path)
        sha256 = hashlib.sha256(raw).hexdigest()
        text = raw.decode("utf-8", errors="replace")
        all_lines = text.splitlines()
        line_count = len(all_lines)
        # Optional windowing: line_range ('10-40', 1-based inclusive) wins over offset/limit.
        line_range = _optional_string_argument(arguments, "line_range")
        if line_range == "":
            return _error_text("line_range must be a string", code="invalid_arguments", argument="line_range")
        selected = all_lines
        if line_range:
            try:
                start_str, _, end_str = line_range.partition("-")
                start = max(1, int(start_str))
                end = int(end_str) if end_str else line_count
            except ValueError:
                return _error_text("line_range must look like '10-40'", code="invalid_arguments", argument="line_range")
            selected = all_lines[start - 1:end]
        else:
            offset = _int_argument(arguments, "offset", 0, minimum=0, maximum=10_000_000)
            limit_value = arguments.get("limit")
            if isinstance(limit_value, int) and not isinstance(limit_value, bool):
                limit = max(1, limit_value)
                selected = all_lines[offset:offset + limit]
            elif offset:
                selected = all_lines[offset:]
        windowed = "\n".join(selected)
        content, truncated = _truncate_text(windowed)
        return _json_text({
            "ok": True,
            "path": rel_path.replace("\\", "/"),
            "content": content,
            "sha256": sha256,
            "line_count": line_count,
            "truncated": truncated,
        })

    elif name == "devcouncil_get_diff":
        if not _is_git_repo(root):
            return _error_text("get_diff requires a git repository.", code="not_a_git_repo")
        task_id = _optional_string_argument(arguments, "task_id")
        if task_id == "":
            return _error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
        explicit_paths, arg_error = _optional_string_list_argument(arguments, "paths")
        if arg_error:
            return arg_error
        staged_value = arguments.get("staged", False)
        if not isinstance(staged_value, bool):
            return _error_text("staged must be a boolean", code="invalid_arguments", argument="staged")
        scope_paths: list[str] = list(explicit_paths)
        if task_id and db:
            with db.get_session() as session:
                task = TaskRepository(session).get_by_id(task_id)
            if task is None:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)
            for planned in task.planned_files:
                p = planned.path.replace("\\", "/")
                if p not in scope_paths:
                    scope_paths.append(p)
        return _json_text(_git_diff(root, scope_paths, staged_value))

    elif name == "devcouncil_get_evidence":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        assert task_id is not None
        command_filter = _optional_string_argument(arguments, "command")
        if command_filter == "":
            return _error_text("command must be a string", code="invalid_arguments", argument="command")
        limit = _int_argument(arguments, "limit", 20, minimum=1, maximum=100)
        with db.get_session() as session:
            results = EvidenceRepository(session).get_command_results_for_task(task_id)
        evidence_rows: list[dict[str, object]] = []
        for result in results:
            if command_filter and command_filter not in result.command:
                continue
            stdout, stdout_truncated = _truncate_text(_read_log_file(result.stdout_path))
            stderr, stderr_truncated = _truncate_text(_read_log_file(result.stderr_path))
            evidence_rows.append({
                "command": result.command,
                "exit_code": result.exit_code,
                "summary": result.summary,
                "stdout": stdout,
                "stderr": stderr,
                "truncated": stdout_truncated or stderr_truncated,
            })
            if len(evidence_rows) >= limit:
                break
        return _json_text({"ok": True, "task_id": task_id, "evidence": evidence_rows})

    elif name == "devcouncil_run_command":
        assert db is not None
        task_id, arg_error = _required_string_argument(arguments, "task_id")
        if arg_error:
            return arg_error
        lease_token, arg_error = _required_string_argument(arguments, "lease_token")
        if arg_error:
            return arg_error
        command, arg_error = _required_string_argument(arguments, "command")
        if arg_error:
            return arg_error
        assert task_id is not None and lease_token is not None and command is not None
        normalized = " ".join(command.split())
        with db.get_session() as session:
            if not TaskLeaseRepository(session).validate(task_id, lease_token):
                return _error_text("Invalid lease token.", code="invalid_lease", task_id=task_id)
            task = TaskRepository(session).get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=task_id)
            from devcouncil.execution.policy_engine import TaskPolicyEngine

            policy_decision = TaskPolicyEngine(root).evaluate_command(normalized, task)
            if policy_decision.action == "deny":
                # Record nothing executed; the gate refused before any side effect.
                ShellCommandRepository(session).record(
                    task_id, normalized, "blocked", reason=policy_decision.reason,
                )
                return _error_text(
                    policy_decision.reason or "Command is not in the task allowlist.",
                    code="command_not_allowed", task_id=task_id, command=normalized,
                )
            try:
                import shlex

                args = shlex.split(normalized, posix=(os.name != "nt"))
                completed = subprocess.run(
                    args,
                    cwd=root,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    env=clean_subprocess_env(),
                    timeout=_CLI_TIMEOUT_SECONDS,
                )
                exit_code = completed.returncode
                stdout, stdout_truncated = _truncate_text(completed.stdout)
                stderr, stderr_truncated = _truncate_text(completed.stderr)
                timed_out = False
            except subprocess.TimeoutExpired as exc:
                exit_code = None
                stdout, stdout_truncated = _truncate_text(exc.output)
                stderr, stderr_truncated = _truncate_text(exc.stderr)
                timed_out = True
            except (FileNotFoundError, OSError, ValueError) as exc:
                ShellCommandRepository(session).record(
                    task_id, normalized, "failed", reason=str(exc),
                )
                return _error_text(f"Could not run command: {exc}", code="run_failed", task_id=task_id)
            ShellCommandRepository(session).record(
                task_id,
                normalized,
                "finished" if exit_code == 0 else "failed",
                exit_code=exit_code,
            )
            return _json_text({
                "ok": exit_code == 0,
                "task_id": task_id,
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr,
                "truncated": stdout_truncated or stderr_truncated,
                "timed_out": timed_out,
            })

    elif name == "devcouncil_next_task":
        assert db is not None
        status_filter = _optional_string_argument(arguments, "status")
        if status_filter == "":
            return _error_text("status must be a string", code="invalid_arguments", argument="status")
        client_id = _optional_string_argument(arguments, "client_id")
        if client_id == "":
            return _error_text("client_id must be a string", code="invalid_arguments", argument="client_id")
        with db.get_session() as session:
            tasks = TaskRepository(session).get_all()
            leased_task_ids = {
                lease.task_id
                for lease, expired in TaskLeaseRepository(session).list_leases(active_only=True)
                if not expired
            }
            blocking_by_task: dict[str, int] = {}
            for gap in GapRepository(session).get_all():
                if gap.blocking and gap.task_id:
                    blocking_by_task[gap.task_id] = blocking_by_task.get(gap.task_id, 0) + 1
        done_ids = {t.id for t in tasks if t.status in {"verified", "done"}}
        # Candidate set: not finished, not actively leased, deps satisfied, matching the
        # optional status filter (default to planned/ready bootstrap states).
        wanted_statuses = {status_filter} if status_filter else {"planned", "ready"}
        candidates = []
        for task in tasks:
            if task.status not in wanted_statuses:
                continue
            if task.id in leased_task_ids:
                continue
            if any(dep not in done_ids for dep in task.depends_on):
                continue
            candidates.append(task)
        if not candidates:
            return _json_text({
                "ok": True,
                "task": None,
                "reason": "No unblocked, unleased task is available.",
            })
        # Deterministic "highest priority": fewest unmet deps, then task id order, so the
        # same task is chosen on every call (no race with list_tasks ordering).
        candidates.sort(key=lambda t: (len(t.depends_on), t.id))
        chosen = candidates[0]
        blocking_count = blocking_by_task.get(chosen.id, 0)
        return _json_text({
            "ok": True,
            "task": chosen.model_dump(),
            "blocking_gap_count": blocking_count,
            "ready_to_checkout": blocking_count == 0,
            "allowed_next_tools": _allowed_next_tools(chosen.status, blocking_count > 0),
        })

    elif name == "devcouncil_list_agent_runs":
        from devcouncil.cli.commands.runs import _collect_runs, _orphan_after_seconds

        status_filter = _optional_string_argument(arguments, "status")
        if status_filter == "":
            return _error_text("status must be a string", code="invalid_arguments", argument="status")
        limit = _int_argument(arguments, "limit", 20, minimum=1, maximum=500)
        run_rows = _collect_runs(root, orphan_after=_orphan_after_seconds(root))
        if status_filter:
            run_rows = [row for row in run_rows if row.get("status") == status_filter]
        total = len(run_rows)
        run_window = run_rows[:limit]
        return _json_text({"ok": True, "runs": run_window, "total": total, "returned": len(run_window)})

    elif name == "devcouncil_get_run":
        from devcouncil.cli.commands.runs import (
            _find_transcript,
            _is_orphaned,
            _load_manifest,
            _orphan_after_seconds,
            _runs_dir,
            _transcript_tail,
        )
        import time as _time

        target_run_id, arg_error = _required_string_argument(arguments, "run_id")
        if arg_error:
            return arg_error
        assert target_run_id is not None
        run_dir = _runs_dir(root) / target_run_id
        manifest_path = run_dir / "agent-run.json"
        run_manifest = _load_manifest(manifest_path)
        if run_manifest is None:
            return _error_text(f"Run {target_run_id} not found.", code="not_found", run_id=target_run_id)
        orphaned = _is_orphaned(
            run_manifest, manifest_path, orphan_after=_orphan_after_seconds(root), now=_time.time()
        )
        transcript_path = _find_transcript(run_dir, run_manifest)
        transcript_tail = _transcript_tail(transcript_path) if transcript_path else ""
        tail, truncated = _truncate_text(transcript_tail)
        return _json_text({
            "ok": True,
            "run_id": target_run_id,
            "manifest": run_manifest,
            "orphaned": orphaned,
            "transcript_path": str(transcript_path) if transcript_path else None,
            "transcript_tail": tail,
            "transcript_truncated": truncated,
        })

    elif name == "devcouncil_select_knowledge":
        # No DB needed: knowledge lives on disk. Best-effort — a knowledge failure
        # degrades to an empty preamble rather than crashing the server.
        goal, arg_error = _required_string_argument(arguments, "goal")
        if arg_error:
            return arg_error
        assert goal is not None
        try:
            from devcouncil.knowledge.sources import (
                render_knowledge_preamble,
                select_knowledge_sources,
            )

            # Honor the project's knowledge config so MCP selection matches the prompts.
            directory, design_always = _knowledge_settings(root)
            if directory is None:  # explicitly disabled
                sources = []
            else:
                sources = select_knowledge_sources(
                    goal, root, directory=directory, design_always=design_always
                )
            preamble = render_knowledge_preamble(sources)
            return _json_text({
                "ok": True,
                "goal": goal,
                "sources": [
                    {"name": s.name, "kind": s.kind, "description": s.description}
                    for s in sources
                ],
                "preamble": preamble,
            })
        except Exception as exc:
            return _json_text({
                "ok": True,
                "goal": goal,
                "sources": [],
                "preamble": "",
                "note": f"knowledge unavailable: {exc}",
            })

    return _error_text(f"Unknown tool: {name}", code="unknown_tool", tool=name)

async def run():
    # Use stdio to communicate
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(run())
