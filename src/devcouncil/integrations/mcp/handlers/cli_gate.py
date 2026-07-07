"""Gated devcouncil_cli MCP tool handler."""

from __future__ import annotations

from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import error_text, json_text, run_cli_command

CLI_ALLOWED_ROOTS = frozenset({
    "status", "tasks", "report", "map", "prompt", "show", "trace", "lsp", "ast",
    "verify", "verify-leased", "rollback", "gaps", "doctor", "cost", "check", "export", "requirements",
    "runs", "logs", "watch", "go", "wiki", "okf", "graph-context", "provenance", "resource",
    "checkout", "release", "lease", "write", "apply-patch",
    "next-task", "scope", "policy-check", "record-command", "run-cmd",
    "evidence-append", "evidence-list", "handoff-leased",
})
CLI_FORBIDDEN_FLAGS = frozenset({"--project-root", "--github", "--github-pr-comment", "--gitlab-pr-comment"})


def forbidden_cli_flags(args: list[str]) -> list[str]:
    forbidden: set[str] = set()
    for arg in args:
        for flag in CLI_FORBIDDEN_FLAGS:
            if arg == flag or arg.startswith(f"{flag}="):
                forbidden.add(flag)
    return sorted(forbidden)


async def handle_cli(root: Path, arguments: dict) -> list[TextContent]:
    args = arguments.get("args")
    if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args) or not args:
        return error_text("args must be a non-empty string array", code="invalid_arguments")
    if args[0] not in CLI_ALLOWED_ROOTS:
        return error_text(f"command {args[0]} is not allowed through MCP", code="command_not_allowed", command=args[0])
    forbidden = forbidden_cli_flags(args)
    if forbidden:
        return error_text("forbidden flag(s) through MCP: " + ", ".join(forbidden), code="forbidden_flags", flags=forbidden)
    try:
        return json_text(run_cli_command(args, root))
    except Exception as exc:
        return error_text(str(exc), code="cli_execution_error")
