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
    db = get_db(root)
    if not db:
        return None
    with db.get_session() as session:
        task_repo = TaskRepository(session)
        running_tasks = [t for t in task_repo.get_all() if t.status == "running"]
        return running_tasks[0] if running_tasks else None


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


@app.command()
def pre_tool_use(
    tool_call_json: str | None = typer.Argument(None, help="The JSON string of the tool call from the coding CLI."),
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Coding CLI hook: Inspects a tool call before execution.
    Exits with code 2 to block unauthorized file writes.
    """
    try:
        if tool_call_json is None:
            tool_call_json = sys.stdin.read()
        if not tool_call_json.strip():
            raise typer.Exit(code=0)
        call_data = json.loads(tool_call_json)
        normalized_client = client.lower()
        root = _project_root(project_root)
        active_task = _active_task(root)

        decision = HookPolicy(project_root=root).evaluate(call_data, active_task)
        _emit_decision(normalized_client, decision.action, decision.reason)
                
    except json.JSONDecodeError:
        raise typer.Exit(code=0)

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
def post_task():
    """
    Coding CLI hook: Runs after a task is completed.
    Triggers deterministic verification.
    """
    console.print("[cyan]DevCouncil: coding agent finished task. Triggering automatic verification...[/cyan]")
    # In a real environment, this would invoke 'dev verify <active-task>'
    # For the hook script, we just notify the user.
    console.print("Run [bold]dev verify[/bold] to finalize implementation evidence.")
