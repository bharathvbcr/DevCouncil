import typer
import json
import os
import sys
from pathlib import Path
from rich.console import Console
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.live.signals import write_signal
from devcouncil.live.tasks import active_task_id

app = typer.Typer()
console = Console()


def _project_root(project_root: Path | None = None) -> Path:
    if project_root:
        return project_root.expanduser().resolve()
    configured = os.environ.get("DEVCOUNCIL_PROJECT_ROOT")
    return Path(configured).expanduser().resolve() if configured else Path(".").resolve()


def _active_task(root: Path):
    # Resolve the *single* unambiguous running task. active_task_id returns None when
    # zero or multiple tasks are running, so we never authorize a write against the
    # wrong task; the policy engine then denies for task=None (fail-closed).
    active_id = active_task_id(root)
    if not active_id:
        return None
    db = get_db(root)
    if not db:
        return None
    with db.get_session() as session:
        return TaskRepository(session).get_by_id(active_id)


def _emit_decision(client: str, action: str, reason: str) -> None:
    if action == "deny":
        print(reason, file=sys.stderr)
        raise typer.Exit(code=2)

    if client in {"codex", "gemini"}:
        payload = {"decision": "allow", "reason": reason, "suppressOutput": True}
        if action == "warn":
            payload["systemMessage"] = f"DevCouncil Warning: {reason}"
        print(json.dumps(payload, separators=(",", ":")))
        return

    if action == "warn":
        console.print(f"[yellow]DevCouncil Warning:[/yellow] {reason}")


def _emit_unevaluable(client: str, reason: str, strict: bool, *, action: str = "warn") -> None:
    """Decide what to do when a tool call cannot be evaluated (empty/malformed/error).

    Fail-closed in strict mode (block), otherwise surface a warning but allow — and
    never leak an undefined exit code, which would silently disable the only pre-action
    gate."""
    _emit_decision(client, "deny" if strict else action, f"{reason}{' (strict mode: blocking)' if strict else ''}")


@app.command()
def pre_tool_use(
    tool_call_json: str | None = typer.Argument(None, help="The JSON string of the tool call from the coding CLI."),
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    strict: bool = typer.Option(
        False,
        "--strict",
        envvar="DEVCOUNCIL_HOOK_STRICT",
        help="Fail closed (block) when a tool call cannot be parsed or evaluated.",
    ),
):
    """
    Coding CLI hook: Inspects a tool call before execution.
    Exits with code 2 to block unauthorized file writes.
    """
    normalized_client = client.lower()
    try:
        if tool_call_json is None:
            tool_call_json = sys.stdin.read()
        # Empty payload: nothing to evaluate. Benign in normal use, so allow — but make
        # it observable, and block under --strict.
        if not tool_call_json.strip():
            return _emit_unevaluable(normalized_client, "Empty tool-call payload; nothing to evaluate.", strict, action="allow")
        try:
            call_data = json.loads(tool_call_json)
        except json.JSONDecodeError:
            # A real tool call we cannot parse must not silently pass the gate.
            return _emit_unevaluable(normalized_client, "Tool-call payload was not valid JSON; could not enforce policy.", strict)
        root = _project_root(project_root)
        active_task = _active_task(root)

        decision = HookPolicy(project_root=root).evaluate(call_data, active_task)
        _emit_decision(normalized_client, decision.action, decision.reason)
    except typer.Exit:
        raise
    except Exception as exc:  # never emit an undefined exit code from a crashing hook
        return _emit_unevaluable(normalized_client, f"Hook error: {exc}", strict)

@app.command()
def post_tool_use(
    tool_call_json: str | None = typer.Argument(None, help="The JSON string of the completed tool call."),
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Coding CLI hook: Records a post-tool-use checkpoint for native hook clients.
    """
    _ = tool_call_json if tool_call_json is not None else sys.stdin.read()
    _ = _project_root(project_root)
    if client.lower() in {"codex", "gemini"}:
        print(json.dumps({"decision": "allow", "suppressOutput": True}, separators=(",", ":")))

@app.command()
def agent_response(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from the coding CLI."),
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Coding CLI hook: records that an agent response is ready for DevCouncil watch review.
    """
    payload_text = event_json if event_json is not None else sys.stdin.read()
    root = _project_root(project_root)
    try:
        payload = json.loads(payload_text) if payload_text.strip() else {}
    except json.JSONDecodeError:
        payload = {"raw": payload_text}
    if isinstance(payload, dict) and not any(key in payload for key in ("task_id", "taskId", "task")):
        active_id = active_task_id(root)
        if active_id:
            payload["task_id"] = active_id
    signal_path = write_signal(root, client.lower(), payload)
    TraceLogger(root).log_event(
        "agent_response_ready",
        {"client": client.lower(), "signal": str(signal_path)},
        summary=f"{client} response ready for critique-card review.",
    )
    if client.lower() in {"codex", "gemini"}:
        print(json.dumps({"decision": "allow", "suppressOutput": True}, separators=(",", ":")))

@app.command()
def post_task(
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Coding CLI hook: Runs after a task is completed.

    When ``execution.verify_on_post_task`` is enabled, this runs deterministic
    verification of the active task and records gaps; otherwise it just reminds the
    user to run ``dev verify`` (the default, to keep hooks fast/cheap).
    """
    root = _project_root(project_root)
    try:
        from devcouncil.app.config import load_config
        verify_enabled = load_config(root).execution.verify_on_post_task
    except Exception:
        verify_enabled = False

    if not verify_enabled:
        console.print("[cyan]DevCouncil: coding agent finished task.[/cyan]")
        console.print("Run [bold]dev verify[/bold] to finalize implementation evidence.")
        _emit_post_task_allow(client)
        return

    summary = _verify_active_task(root)
    console.print(summary)
    _emit_post_task_allow(client)


def _emit_post_task_allow(client: str) -> None:
    if client.lower() in {"codex", "gemini"}:
        print(json.dumps({"decision": "allow", "suppressOutput": True}, separators=(",", ":")))


def _verify_active_task(root: Path) -> str:
    """Run deterministic verification of the active task and persist gaps/evidence.
    Returns a human summary line. Best-effort: never raises out of a hook."""
    try:
        import asyncio

        from devcouncil.domain.evidence import CommandResult, DiffCoverageEvidence, DiffEvidence, TestEvidence
        from devcouncil.storage.repositories import (
            EvidenceRepository,
            GapRepository,
            RequirementRepository,
        )
        from devcouncil.verification.next_actions import split_next_actions
        from devcouncil.verification.verifier import Verifier

        active_id = active_task_id(root)
        db = get_db(root)
        if not active_id or not db:
            return "Run [bold]dev verify[/bold] to finalize implementation evidence."
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(active_id)
            if not task:
                return "Run [bold]dev verify[/bold] to finalize implementation evidence."
            reqs = RequirementRepository(session).get_all()
            gaps, evidence = asyncio.run(Verifier(root).verify_task(task, reqs))
            gap_repo = GapRepository(session)
            ev_repo = EvidenceRepository(session)
            gap_repo.delete_for_task(task.id)
            ev_repo.delete_for_task(task.id)
            for gap in gaps:
                gap_repo.save(gap)
            for ev in evidence:
                if isinstance(ev, CommandResult):
                    ev_repo.save_command_result(task.id, ev)
                elif isinstance(ev, DiffCoverageEvidence):
                    ev_repo.save_diff_coverage_evidence(ev)
                elif isinstance(ev, DiffEvidence):
                    ev_repo.save_diff_evidence(ev)
                elif isinstance(ev, TestEvidence):
                    ev_repo.save_test_evidence(ev, task.id)
            blocking = [g for g in gaps if g.blocking]
            task.status = "blocked" if blocking else "verified"
            TaskRepository(session).save(task)
        blocking_actions, _ = split_next_actions(gaps)
        TraceLogger(root).log_event(
            "post_task_verified",
            {"task_id": active_id, "blocking": len(blocking)},
            task_id=active_id,
            summary=f"post_task verification: {task.status}",
        )
        if blocking:
            return (
                f"[yellow]{active_id} is blocked by {len(blocking)} gap(s); "
                f"{len(blocking_actions)} next action(s). Run [bold]dev repair[/bold].[/yellow]"
            )
        return f"[green]{active_id} verified.[/green]"
    except Exception as exc:  # never let a hook crash the agent
        return f"[dim]post-task verification skipped: {exc}[/dim]"
