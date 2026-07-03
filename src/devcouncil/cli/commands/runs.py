"""`dev runs` — list and inspect per-run agent manifests.

Coding-CLI executors write a manifest at
``.devcouncil/runs/<run-id>/agent-run.json`` (prompt file, executor, profile,
resolved command, exit status, run metadata). These commands let a developer or a
supervisor list and inspect those runs without reading raw JSON, and flag a run
whose status is still ``running`` but whose manifest has gone stale (the executor
process likely crashed) as ``orphaned``.
"""

from __future__ import annotations

import json
import logging
import time
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from devcouncil.utils.redaction import redact_text
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(help="List and inspect coding-agent run manifests.")
console = Console()
logger = logging.getLogger(__name__)

# A run still marked ``running`` whose manifest has not been touched for longer
# than this is treated as orphaned (the executor process likely died). Used as a
# sane default; can be overridden per-call with --orphan-after.
_DEFAULT_ORPHAN_AFTER_SECONDS = 600

# Transcript/log files (in priority order) whose tail `dev runs show` surfaces.
_TRANSCRIPT_CANDIDATES = ("transcript.txt", "transcript.log", "output.log", "run.log")
_TRANSCRIPT_TAIL_LINES = 40


def _runs_dir(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "runs"


def _orphan_after_seconds(project_root: Path) -> int:
    """Threshold (seconds) after which a stale ``running`` manifest is orphaned.

    Config-driven when available (execution.lease_ttl_seconds is a reasonable
    proxy for "how long a live run can plausibly stay quiet"); falls back to a
    sane default so the command works before `dev init`."""
    try:
        from devcouncil.app.config import load_config

        ttl = int(load_config(project_root).execution.lease_ttl_seconds)
        if ttl > 0:
            return ttl
    except Exception:
        pass
    return _DEFAULT_ORPHAN_AFTER_SECONDS


def _load_manifest(manifest_path: Path) -> dict | None:
    try:
        data = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _is_orphaned(manifest: dict, manifest_path: Path, *, orphan_after: int, now: float) -> bool:
    """A run is orphaned when it is still ``running`` but its manifest file has
    not been updated within the threshold — i.e. no heartbeat, executor gone."""
    if manifest.get("status") != "running":
        return False
    try:
        mtime = manifest_path.stat().st_mtime
    except OSError:
        return False
    return (now - mtime) > max(0, orphan_after)


def _run_summary(manifest: dict, manifest_path: Path, *, orphan_after: int, now: float) -> dict:
    return {
        "run_id": manifest.get("run_id") or manifest_path.parent.name,
        "task_id": manifest.get("task_id"),
        "agent": manifest.get("agent"),
        "profile": manifest.get("profile"),
        "status": manifest.get("status"),
        "started_at": manifest.get("started_at") or manifest.get("timestamp"),
        "finished_at": manifest.get("finished_at"),
        "returncode": manifest.get("returncode"),
        "orphaned": _is_orphaned(manifest, manifest_path, orphan_after=orphan_after, now=now),
    }


def _collect_runs(project_root: Path, *, orphan_after: int) -> list[dict]:
    runs_dir = _runs_dir(project_root)
    if not runs_dir.is_dir():
        return []
    now = time.time()
    summaries: list[tuple[float, dict]] = []
    for manifest_path in runs_dir.glob("*/agent-run.json"):
        manifest = _load_manifest(manifest_path)
        if manifest is None:
            continue
        try:
            sort_key = manifest_path.stat().st_mtime
        except OSError:
            sort_key = 0.0
        summaries.append((sort_key, _run_summary(manifest, manifest_path, orphan_after=orphan_after, now=now)))
    # Newest first.
    summaries.sort(key=lambda item: item[0], reverse=True)
    return [summary for _, summary in summaries]


def _find_transcript(run_dir: Path, manifest: dict) -> Path | None:
    recorded = manifest.get("transcript")
    if isinstance(recorded, str) and recorded:
        candidate = Path(recorded)
        if not candidate.is_absolute():
            candidate = run_dir / recorded
        if candidate.is_file():
            return candidate
    for name in _TRANSCRIPT_CANDIDATES:
        candidate = run_dir / name
        if candidate.is_file():
            return candidate
    return None


def _transcript_tail(path: Path, *, lines: int = _TRANSCRIPT_TAIL_LINES) -> str:
    """Return the redacted tail of a transcript file (best-effort)."""
    try:
        content = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    tail = content.splitlines()[-lines:]
    return redact_text("\n".join(tail))


@app.command("list")
def list_runs(
    json_output: bool = typer.Option(False, "--json", help="Emit machine-readable JSON."),
    limit: int = typer.Option(20, "--limit", help="Maximum number of runs to show."),
    status: str | None = typer.Option(None, "--status", help="Filter by run status (e.g. running, finished, failed, timeout)."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """List recorded coding-agent runs, newest first."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev runs list: status=%s limit=%d", status, limit)
    with log_stage("runs", project_root=root, subcommand="list"):
        log_step("runs/1: collecting run manifests", project_root=root, trace=True)
        orphan_after = _orphan_after_seconds(root)
        runs = _collect_runs(root, orphan_after=orphan_after)
        if status:
            runs = [run for run in runs if run.get("status") == status]
        if limit > 0:
            runs = runs[:limit]

        if json_output:
            console.print_json(data={"runs": runs, "count": len(runs)})
            log_step("runs/complete", project_root=root, count=len(runs), trace=True)
            return

        if not runs:
            console.print("[dim]No agent runs found under .devcouncil/runs/.[/dim]")
            log_step("runs/complete", project_root=root, count=0, trace=True)
            return

        table = Table(title="Agent runs")
        table.add_column("Run ID", overflow="fold")
        table.add_column("Task")
        table.add_column("Agent")
        table.add_column("Profile")
        table.add_column("Status")
        table.add_column("Started")
        for run in runs:
            status_text = str(run.get("status") or "?")
            if run.get("orphaned"):
                status_text = f"[red]{status_text} (orphaned)[/red]"
            table.add_row(
                str(run.get("run_id") or ""),
                str(run.get("task_id") or ""),
                str(run.get("agent") or ""),
                str(run.get("profile") or ""),
                status_text,
                str(run.get("started_at") or ""),
            )
        console.print(table)
        log_step("runs/complete", project_root=root, count=len(runs), trace=True)


@app.command("show")
def show_run(
    run_id: str = typer.Argument(..., help="The run id to inspect."),
    json_output: bool = typer.Option(False, "--json", help="Emit machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
) -> None:
    """Show the full manifest for a run plus a redacted transcript tail."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev runs show: run_id=%s", run_id)
    with log_stage("runs", project_root=root, subcommand="show", run_id=run_id):
        log_step("runs/1: loading run manifest", project_root=root, run_id=run_id, trace=True)
        run_dir = _runs_dir(root) / run_id
        manifest_path = run_dir / "agent-run.json"
        manifest = _load_manifest(manifest_path)
        if manifest is None:
            if json_output:
                console.print_json(data={"ok": False, "error": f"Run {run_id} not found.", "run_id": run_id})
            else:
                console.print(f"[red]Run {run_id} not found under .devcouncil/runs/.[/red]")
            raise typer.Exit(code=1)

        orphan_after = _orphan_after_seconds(root)
        orphaned = _is_orphaned(manifest, manifest_path, orphan_after=orphan_after, now=time.time())
        transcript_path = _find_transcript(run_dir, manifest)
        transcript_tail = _transcript_tail(transcript_path) if transcript_path else ""

        if json_output:
            console.print_json(data={
                "ok": True,
                "run_id": run_id,
                "manifest": manifest,
                "orphaned": orphaned,
                "transcript_path": str(transcript_path) if transcript_path else None,
                "transcript_tail": transcript_tail,
            })
            log_step("runs/complete", project_root=root, run_id=run_id, trace=True)
            return

        console.print_json(data=manifest)
        if orphaned:
            console.print("[red]This run is orphaned: still marked running but its manifest is stale.[/red]")
        if transcript_path:
            console.print(f"\n[bold]Transcript tail[/bold] [dim]({transcript_path})[/dim]:")
            console.print(transcript_tail or "[dim](empty)[/dim]")
        else:
            console.print("\n[dim]No transcript file found for this run.[/dim]")
        log_step("runs/complete", project_root=root, run_id=run_id, trace=True)
