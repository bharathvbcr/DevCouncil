"""Task lookup and prompt MCP tool handlers."""

from __future__ import annotations

import json
from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    json_text,
    required_string_argument,
    run_cli_command,
)
from devcouncil.utils.json_persist import dump_json


def _cli_json(root: Path, args: list[str]) -> tuple[dict | None, list[TextContent] | None]:
    result = run_cli_command(args, root)
    if not result.get("ok"):
        stderr = str(result.get("stderr") or "CLI command failed")
        return None, error_text(stderr, code="cli_failed")
    try:
        return json.loads(str(result.get("stdout") or "{}")), None
    except json.JSONDecodeError:
        return None, error_text("CLI command returned invalid JSON", code="cli_parse_error")


async def handle_get_task(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    payload, cli_error = _cli_json(root, ["show", task_id, "--json"])
    if cli_error:
        return cli_error
    assert payload is not None
    task = payload.get("task")
    if not isinstance(task, dict):
        return error_text(f"Task {task_id} not found.", code="not_found", task_id=str(task_id))
    return [TextContent(type="text", text=dump_json(task, indent=2))]


async def handle_get_prompt(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    payload, cli_error = _cli_json(root, ["prompt", task_id, "--json"])
    if cli_error:
        return cli_error
    assert payload is not None
    if not payload.get("ok"):
        return error_text(
            str(payload.get("error") or f"Task {task_id} not found."),
            code="not_found",
            task_id=str(task_id),
        )
    prompt = payload.get("prompt")
    if not isinstance(prompt, str):
        return error_text("prompt command returned no prompt text", code="cli_parse_error")
    return [TextContent(type="text", text=prompt)]


async def handle_prepare_execution(root: Path, db: object, arguments: dict) -> list[TextContent]:
    del db  # routed through CLI service layer
    task_id, arg_error = required_string_argument(arguments, "task_id")
    if arg_error:
        return arg_error
    assert task_id is not None
    show_payload, show_error = _cli_json(root, ["show", task_id, "--json"])
    if show_error:
        return show_error
    assert show_payload is not None
    task = show_payload.get("task")
    if not isinstance(task, dict):
        return error_text(f"Task {task_id} not found.", code="not_found", task_id=str(task_id))
    prompt_payload, prompt_error = _cli_json(root, ["prompt", task_id, "--json"])
    if prompt_error:
        return prompt_error
    assert prompt_payload is not None
    if not prompt_payload.get("ok"):
        return error_text(
            str(prompt_payload.get("error") or f"Task {task_id} not found."),
            code="not_found",
            task_id=str(task_id),
        )
    prompt = prompt_payload.get("prompt")
    if not isinstance(prompt, str):
        return error_text("prompt command returned no prompt text", code="cli_parse_error")
    return json_text({
        "task_id": task.get("id", task_id),
        "prompt": prompt,
        "planned_files": task.get("planned_files") or [],
        "allowed_commands": task.get("allowed_commands") or [],
        "expected_tests": task.get("expected_tests") or [],
    })
