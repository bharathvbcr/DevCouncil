"""CLI service layer for MCP-equivalent task gate operations."""

from __future__ import annotations

import logging
from pathlib import Path

import typer

from devcouncil.execution.task_gate_ops import (
    attach_committed_range_payload,
    append_evidence_payload,
    get_evidence_payload,
    handoff_agent_payload,
    next_task_payload,
    policy_check_write_payload,
    record_command_payload,
    run_command_payload,
    update_task_scope_payload,
    verify_task_payload,
)
from devcouncil.telemetry.logging_setup import set_log_dir
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.utils.json_persist import dump_json

logger = logging.getLogger(__name__)

scope_app = typer.Typer(help="Update task scope for a leased task.")


def next_task(
    status: str | None = typer.Option(None, "--status", help="Filter by task status."),
    client_id: str | None = typer.Option(None, "--client-id", help="Client id for fleet routing."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Select the next unblocked, unleased task."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = next_task_payload(root, status_filter=status, client_id=client_id)
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    task = payload.get("task")
    if not task:
        typer.echo(payload.get("reason", "No task available."))
        return
    typer.echo(f"{task['id']}  status={task.get('status')}")


@scope_app.command("update")
def scope_update(
    task_id: str = typer.Argument(...),
    lease_token: str = typer.Option(..., "--lease-token"),
    expected_test: list[str] = typer.Option([], "--expected-test", help="Repeatable expected test command."),
    allowed_command: list[str] = typer.Option([], "--allowed-command", help="Repeatable allowed command."),
    planned_file: list[str] = typer.Option(
        [],
        "--planned-file",
        help="Repeatable path to append as modify-op planned file (lease-gated).",
    ),
    planned_file_create: list[str] = typer.Option(
        [],
        "--planned-file-create",
        help="Repeatable path to append as create-op planned file (lease-gated).",
    ),
    json_format: bool = typer.Option(False, "--json"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Append expected tests, allowed commands, or planned files to a leased task's scope."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = update_task_scope_payload(
        root,
        task_id=task_id,
        lease_token=lease_token,
        expected_tests=expected_test,
        allowed_commands=allowed_command,
        planned_files=planned_file,
        create_planned_files=planned_file_create,
    )
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    if not payload.get("ok"):
        typer.echo(payload.get("error", "scope update failed"))
        raise typer.Exit(code=1)
    typer.echo(f"Updated scope for {task_id}")


def policy_check(
    path: str = typer.Argument(..., help="Relative path to evaluate."),
    task_id: str | None = typer.Option(None, "--task-id"),
    json_format: bool = typer.Option(False, "--json"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Evaluate whether a file write would be allowed by policy."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = policy_check_write_payload(root, path=path, task_id=task_id)
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        return
    typer.echo(f"{payload.get('action')}: {payload.get('reason')}")


def record_command(
    task_id: str = typer.Argument(...),
    lease_token: str = typer.Option(..., "--lease-token"),
    command: str = typer.Option(..., "--command"),
    status: str = typer.Option(..., "--status"),
    exit_code: int | None = typer.Option(None, "--exit-code"),
    reason: str = typer.Option("", "--reason"),
    json_format: bool = typer.Option(False, "--json"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Record a shell command lifecycle event for a leased task."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = record_command_payload(
        root,
        task_id=task_id,
        lease_token=lease_token,
        command=command,
        status=status,
        exit_code=exit_code,
        reason=reason,
    )
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    if not payload.get("ok"):
        typer.echo(payload.get("error", "record failed"))
        raise typer.Exit(code=1)
    typer.echo(f"Recorded {status} for {task_id}")


def run_cmd(
    task_id: str = typer.Argument(...),
    lease_token: str = typer.Option(..., "--lease-token"),
    command: str = typer.Option(..., "--command"),
    json_format: bool = typer.Option(False, "--json"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Run an allowlisted command for a leased task through the policy gate."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    with log_stage("run-cmd", project_root=root, task_id=task_id):
        payload = run_command_payload(root, task_id=task_id, lease_token=lease_token, command=command)
        if json_format:
            typer.echo(dump_json(payload, indent=2))
            log_step("run-cmd/complete", project_root=root, task_id=task_id, trace=True)
            if not payload.get("ok") and payload.get("code") != "command_not_allowed":
                raise typer.Exit(code=1)
            return
        if not payload.get("ok"):
            typer.echo(payload.get("error", "run failed"))
            raise typer.Exit(code=1)
        typer.echo(f"exit {payload.get('exit_code')}")
        log_step("run-cmd/complete", project_root=root, task_id=task_id, trace=True)


def attach_committed_range(
    task_id: str = typer.Argument(...),
    lease_token: str = typer.Option(..., "--lease-token"),
    base: str = typer.Option(..., "--base"),
    head: str = typer.Option("HEAD", "--head"),
    json_format: bool = typer.Option(False, "--json"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Associate post-commit verification with an existing commit range."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = attach_committed_range_payload(
        root,
        task_id=task_id,
        lease_token=lease_token,
        base=base,
        head=head,
    )
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    if not payload.get("ok"):
        typer.echo(payload.get("error", "committed range attachment failed"))
        raise typer.Exit(code=1)
    typer.echo(f"Attached {payload['range']} to {task_id}")


def verify_leased(
    task_id: str = typer.Argument(...),
    lease_token: str = typer.Option(..., "--lease-token"),
    sandbox: str = typer.Option("local", "--sandbox"),
    json_format: bool = typer.Option(True, "--json/--no-json", help="MCP-compatible JSON output (default on)."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Verify a leased task (MCP-compatible output)."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    with log_stage("verify-leased", project_root=root, task_id=task_id, sandbox=sandbox):
        payload = verify_task_payload(root, task_id=task_id, lease_token=lease_token, sandbox=sandbox)
        typer.echo(dump_json(payload, indent=2))
        log_step("verify-leased/complete", project_root=root, task_id=task_id, trace=True)
        if not payload.get("ok"):
            raise typer.Exit(code=1)


def evidence_append(
    task_id: str = typer.Argument(...),
    lease_token: str = typer.Option(..., "--lease-token"),
    command: str = typer.Option(..., "--command"),
    summary: str = typer.Option(..., "--summary"),
    exit_code: int = typer.Option(0, "--exit-code"),
    json_format: bool = typer.Option(False, "--json"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Append command evidence for a leased task."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = append_evidence_payload(
        root,
        task_id=task_id,
        lease_token=lease_token,
        command=command,
        summary=summary,
        exit_code=exit_code,
    )
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    if not payload.get("ok"):
        typer.echo(payload.get("error", "append failed"))
        raise typer.Exit(code=1)
    typer.echo(f"Recorded evidence for {task_id}")


def evidence_list(
    task_id: str = typer.Argument(...),
    command: str | None = typer.Option(None, "--command"),
    limit: int = typer.Option(20, "--limit", min=1, max=100),
    json_format: bool = typer.Option(False, "--json"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """List stored command evidence for a task."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = get_evidence_payload(root, task_id=task_id, command_filter=command, limit=limit)
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    for row in payload.get("evidence") or []:
        typer.echo(f"{row.get('exit_code')}  {row.get('command')}")


def handoff_leased(
    task_id: str = typer.Argument(...),
    lease_token: str = typer.Option(..., "--lease-token"),
    from_agent: str = typer.Option(..., "--from"),
    to_agent: str = typer.Option(..., "--to"),
    instruction: str = typer.Option("", "--instruction"),
    json_format: bool = typer.Option(False, "--json"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Hand off a leased task between coding CLI agents."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = handoff_agent_payload(
        root,
        task_id=task_id,
        lease_token=lease_token,
        from_agent=from_agent,
        to_agent=to_agent,
        instruction=instruction,
    )
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    if not payload.get("ok"):
        typer.echo(payload.get("error", "handoff failed"))
        raise typer.Exit(code=1)
    typer.echo(f"Handoff manifest: {payload.get('manifest_path')}")
