import asyncio
import json
import os
import subprocess
import sys
from pathlib import Path
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, ArtifactGraphRepository, StateRepository, RequirementRepository
from devcouncil.reporting.report_builder import ReportBuilder
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.telemetry.traces import read_trace_events
from devcouncil.indexing.ast_matcher import AstMatcher
from devcouncil.indexing.lsp import LspInspector
from devcouncil.app.project_status import compute_phase
from devcouncil.live.cards import filter_cards, get_card, load_cards
from devcouncil.live.repair_prompt import build_bulk_live_repair_prompt, build_live_repair_prompt
from devcouncil.live.summary import live_review_summary

app = Server("devcouncil")
_DB_REQUIRED_TOOLS = {
    "devcouncil_status",
    "devcouncil_report",
    "devcouncil_get_task",
    "devcouncil_list_tasks",
    "devcouncil_get_prompt",
    "devcouncil_tail_trace",
    "devcouncil_policy_check_write",
    "devcouncil_graph_context",
    "devcouncil_prepare_execution",
}
_CLI_ALLOWED_ROOTS = {"status", "tasks", "report", "map", "prompt", "show", "trace", "lsp", "ast", "verify"}
_CLI_FORBIDDEN_FLAGS = {"--project-root", "--github", "--github-pr-comment", "--gitlab-pr-comment"}
_CLI_TIMEOUT_SECONDS = 120
_CLI_OUTPUT_LIMIT = 20_000


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
            description="List DevCouncil tasks with status and requirement mappings.",
            inputSchema={"type": "object", "properties": {}},
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
    ]

@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    arguments = _normalize_arguments(arguments)
    root = _project_root()
    db = get_db(root)
    if name in _DB_REQUIRED_TOOLS and not db:
        return _error_text("DevCouncil not initialized in this directory.", code="not_initialized")

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
        filtered, error, argument = filter_cards(
            load_cards(root),
            task_id=task_id,
            status=status,
            verdict=verdict,
            client=client,
        )
        if error:
            return _error_text(error, code="invalid_arguments", argument=argument)

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
        card_id, error = _required_string_argument(arguments, "card_id")
        if error:
            return error
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
        cards = [card for card in cards if card is not None]
        return [TextContent(
            type="text",
            text=json.dumps({
                "scope_task_id": summary["scope_task_id"],
                "cards": [card.model_dump() for card in cards],
                "prompt": build_bulk_live_repair_prompt(root, cards),
            }, indent=2),
        )]
            
    elif name == "devcouncil_get_task":
        assert db is not None
        task_id, error = _required_string_argument(arguments, "task_id")
        if error:
            return error
            
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if not task:
                return _error_text(f"Task {task_id} not found.", code="not_found", task_id=str(task_id))
            
            return [TextContent(type="text", text=task.model_dump_json(indent=2))]

    elif name == "devcouncil_list_tasks":
        assert db is not None
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            tasks = [task.model_dump() for task in task_repo.get_all()]

            return [TextContent(type="text", text=json.dumps({"tasks": tasks}, indent=2))]

    elif name == "devcouncil_get_prompt":
        assert db is not None
        task_id, error = _required_string_argument(arguments, "task_id")
        if error:
            return error

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
        path, error = _required_string_argument(arguments, "path")
        if error:
            return error
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
        task_id, error = _required_string_argument(arguments, "task_id")
        if error:
            return error
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

    return _error_text(f"Unknown tool: {name}", code="unknown_tool", tool=name)

async def run():
    # Use stdio to communicate
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(run())
