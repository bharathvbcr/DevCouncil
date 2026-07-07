"""`dev checkout`, `dev release`, and `dev lease` — task lease CLI service layer."""

from __future__ import annotations

import logging
from pathlib import Path

import typer

from devcouncil.execution.lease_ops import (
    checkout_task_payload,
    list_leases_payload,
    release_task_payload,
    renew_lease_payload,
)
from devcouncil.telemetry.logging_setup import set_log_dir
from devcouncil.telemetry.stages import log_stage, log_step
from devcouncil.utils.json_persist import dump_json

lease_app = typer.Typer(help="List and renew task leases.")
logger = logging.getLogger(__name__)


def checkout(
    task_id: str = typer.Argument(..., help="Task ID to acquire."),
    client_id: str = typer.Option(..., "--client-id", help="Stable client id for the lease owner."),
    agent: str | None = typer.Option(None, "--agent", help="Optional agent profile name."),
    force: bool = typer.Option(False, "--force", help="Reclaim a stale lease."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Acquire a task lease and return scope for gated write tools."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    logger.info("dev checkout: task=%s client=%s", task_id, client_id)

    with log_stage("checkout", project_root=root, task_id=task_id):
        payload = checkout_task_payload(root, task_id=task_id, client_id=client_id, agent=agent, force=force)
        if json_format:
            typer.echo(dump_json(payload, indent=2))
            log_step("checkout/complete", project_root=root, task_id=task_id, trace=True)
            if not payload.get("ok"):
                raise typer.Exit(code=1)
            return
        if not payload.get("ok"):
            typer.echo(payload.get("error", "checkout failed"))
            raise typer.Exit(code=1)
        typer.echo(f"Checked out {task_id} (expires {payload.get('expires_at')})")
        log_step("checkout/complete", project_root=root, task_id=task_id, trace=True)


def release(
    task_id: str = typer.Argument(..., help="Task ID to release."),
    lease_token: str = typer.Option(..., "--lease-token", help="Lease token from checkout."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Release a held task lease."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    logger.info("dev release: task=%s", task_id)

    with log_stage("release", project_root=root, task_id=task_id):
        payload = release_task_payload(root, task_id=task_id, lease_token=lease_token)
        if json_format:
            typer.echo(dump_json(payload, indent=2))
            log_step("release/complete", project_root=root, task_id=task_id, trace=True)
            if not payload.get("ok"):
                raise typer.Exit(code=1)
            return
        if not payload.get("ok"):
            typer.echo(payload.get("error", "release failed"))
            raise typer.Exit(code=1)
        typer.echo(f"Released {task_id}")
        log_step("release/complete", project_root=root, task_id=task_id, trace=True)


@lease_app.command("list")
def lease_list(
    active_only: bool = typer.Option(True, "--active-only/--all", help="Only active leases."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """List task leases for fleet supervision."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = list_leases_payload(root, active_only=active_only)
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    if not payload.get("ok"):
        typer.echo(payload.get("error", "lease list failed"))
        raise typer.Exit(code=1)
    for item in payload.get("leases") or []:
        typer.echo(f"{item['task_id']}  {item['owner']}  expires={item['expires_at']}")


@lease_app.command("renew")
def lease_renew(
    task_id: str = typer.Argument(..., help="Task ID."),
    lease_token: str = typer.Option(..., "--lease-token", help="Lease token to renew."),
    ttl_seconds: int | None = typer.Option(None, "--ttl-seconds", help="New TTL in seconds."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Extend a held task lease's TTL."""
    root = project_root.expanduser().resolve()
    set_log_dir(root)
    payload = renew_lease_payload(root, task_id=task_id, lease_token=lease_token, ttl_seconds=ttl_seconds)
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        if not payload.get("ok"):
            raise typer.Exit(code=1)
        return
    if not payload.get("ok"):
        typer.echo(payload.get("error", "renew failed"))
        raise typer.Exit(code=1)
    typer.echo(f"Renewed {task_id} until {payload.get('expires_at')}")
