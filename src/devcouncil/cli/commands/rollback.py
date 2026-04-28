import typer
import subprocess
from pathlib import Path
from rich.console import Console

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def rollback(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="ID of the task to rollback"),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Revert changes using a task's git checkpoint.
    """
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    checkpoint_file = root / ".devcouncil" / "checkpoints" / f"{task_id}-before.patch"
    after_patch = root / ".devcouncil" / "checkpoints" / f"{task_id}-after.patch"

    if not checkpoint_file.exists() and not after_patch.exists():
        console.print(
            f"[red]No checkpoint found for task {task_id}. Expected {after_patch} "
            f"or {checkpoint_file}.[/red]"
        )
        raise typer.Exit(code=1)

    console.print(f"Rolling back task [bold]{task_id}[/bold]...")
    
    try:
        if after_patch.exists():
            # Reverse-apply the task's changes only
            console.print(f"Applying reverse patch from [bold]{after_patch}[/bold]...")
            subprocess.check_call(
                ["git", "apply", "-R", str(after_patch)],
                cwd=root,
            )
            console.print(f"[green]Successfully rolled back task {task_id} changes.[/green]")
        else:
            # No after-patch, but we have the before-patch — warn and offer manual reset
            console.print(
                f"[yellow]No after-patch found at {after_patch}.[/yellow]\n"
                f"The before-patch at {checkpoint_file} captured the state before the task ran.\n"
                f"To manually reset:\n"
                f"  1. [bold]git stash[/bold] (if you want to keep current changes)\n"
                f"  2. [bold]git checkout -- .[/bold] (discard working tree changes)\n"
                f"  3. [bold]git apply {checkpoint_file}[/bold] (restore pre-task state)"
            )
    except subprocess.CalledProcessError as e:
        console.print(f"[red]Failed to apply reverse patch: {e}[/red]")
        console.print("[yellow]The patch may conflict with current changes. Try resolving manually:[/yellow]")
        console.print(f"  git apply -R --3way {after_patch}")
        raise typer.Exit(code=1)
    except Exception as e:
        console.print(f"[red]Failed to rollback: {e}[/red]")
        raise typer.Exit(code=1)
