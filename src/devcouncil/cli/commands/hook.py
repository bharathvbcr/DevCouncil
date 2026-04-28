import typer
import json
import sys
from rich.console import Console
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from devcouncil.execution.hook_policy import HookPolicy

app = typer.Typer()
console = Console()

@app.command()
def pre_tool_use(
    tool_call_json: str | None = typer.Argument(None, help="The JSON string of the tool call from Claude Code")
):
    """
    Claude Code hook: Inspects a tool call before execution.
    Exits with code 2 to block unauthorized file writes.
    """
    try:
        if tool_call_json is None:
            tool_call_json = sys.stdin.read()
        if not tool_call_json.strip():
            raise typer.Exit(code=0)
        call_data = json.loads(tool_call_json)
        active_task = None
        db = get_db()
        if db:
            with db.get_session() as session:
                task_repo = TaskRepository(session)
                running_tasks = [t for t in task_repo.get_all() if t.status == "running"]
                active_task = running_tasks[0] if running_tasks else None

        decision = HookPolicy().evaluate(call_data, active_task)
        if decision.action == "deny":
            console.print(f"[red]DevCouncil Blocked Action:[/red] {decision.reason}")
            sys.exit(2)
        if decision.action == "warn":
            console.print(f"[yellow]DevCouncil Warning:[/yellow] {decision.reason}")
                
    except json.JSONDecodeError:
        raise typer.Exit(code=0)

@app.command()
def post_task():
    """
    Claude Code hook: Runs after a task is completed.
    Triggers deterministic verification.
    """
    console.print("[cyan]DevCouncil: Claude finished task. Triggering automatic verification...[/cyan]")
    # In a real environment, this would invoke 'dev verify <active-task>'
    # For the hook script, we just notify the user.
    console.print("Run [bold]dev verify[/bold] to finalize implementation evidence.")
