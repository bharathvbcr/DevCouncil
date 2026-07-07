"""`dev write` and `dev apply-patch` — policy-gated file writes through the CLI."""

from __future__ import annotations

import logging
import sys
from pathlib import Path

import typer

from devcouncil.execution.gated_write import apply_patch_payload, write_file_payload
from devcouncil.telemetry.logging_setup import set_log_dir
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.utils.json_persist import dump_json

logger = logging.getLogger(__name__)


def write(
    task_id: str = typer.Argument(..., help="Task ID."),
    lease_token: str = typer.Option(..., "--lease-token", help="Lease token from checkout."),
    path: str = typer.Option(..., "--path", help="Repository-relative file path."),
    content: str | None = typer.Option(None, "--content", help="File content (or read from stdin when omitted)."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Write a file for a leased task through DevCouncil's policy gate."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    body = content if content is not None else sys.stdin.read()
    logger.info("dev write: task=%s path=%s", task_id, path)

    with log_stage("write", project_root=root, task_id=task_id):
        payload = write_file_payload(root, task_id=task_id, lease_token=lease_token, rel_path=path, content=body)
        if json_format:
            typer.echo(dump_json(payload, indent=2))
            log_step("write/complete", project_root=root, task_id=task_id, trace=True)
            if not payload.get("ok"):
                raise typer.Exit(code=1)
            return
        if not payload.get("ok"):
            typer.echo(payload.get("error") or payload.get("rejected_files"))
            raise typer.Exit(code=1)
        typer.echo(f"Wrote {path}")
        log_step("write/complete", project_root=root, task_id=task_id, trace=True)


def apply_patch(
    task_id: str = typer.Argument(..., help="Task ID."),
    lease_token: str = typer.Option(..., "--lease-token", help="Lease token from checkout."),
    unified_diff: str | None = typer.Option(None, "--unified-diff", help="Unified diff (or read from stdin)."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Apply a unified diff for a leased task through DevCouncil's policy gate."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    diff_body = unified_diff if unified_diff is not None else sys.stdin.read()
    if not diff_body.strip():
        typer.echo("unified_diff must be a non-empty string")
        raise typer.Exit(code=1)
    logger.info("dev apply-patch: task=%s", task_id)

    with log_stage("apply-patch", project_root=root, task_id=task_id):
        payload = apply_patch_payload(root, task_id=task_id, lease_token=lease_token, unified_diff=diff_body)
        if json_format:
            typer.echo(dump_json(payload, indent=2))
            log_step("apply-patch/complete", project_root=root, task_id=task_id, trace=True)
            if not payload.get("ok"):
                raise typer.Exit(code=1)
            return
        if not payload.get("ok"):
            typer.echo(payload.get("error") or payload.get("rejected_files"))
            raise typer.Exit(code=1)
        typer.echo(f"Applied patch to {payload.get('applied_files')}")
        log_step("apply-patch/complete", project_root=root, task_id=task_id, trace=True)
