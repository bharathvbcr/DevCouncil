from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from devcouncil.skills.registry import get_skill, load_skills, scaffold_skills, select_skills

app = typer.Typer(help="Inspect and scaffold DevCouncil engineering skills for coding agents.")
console = Console()


def _is_repo_skill(skill, project_root: Path) -> bool:
    if skill.source_path is None:
        return False
    try:
        skill.source_path.resolve().relative_to(project_root.resolve())
        return True
    except ValueError:
        return False


@app.callback(invoke_without_command=True)
def skills(
    ctx: typer.Context,
    goal: str = typer.Option("", "--goal", help="Optional goal text; highlights the skills that would apply."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root used for file-based skill triggers."),
):
    """List available skills and show which apply to this repository/goal."""
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    all_skills = load_skills(project_root=root)
    if not all_skills:
        console.print("[yellow]No skills found in the DevCouncil skills library.[/yellow]")
        raise typer.Exit()

    selected = {skill.name for skill in select_skills(goal, root)}
    table = Table(title="DevCouncil Skills")
    table.add_column("Skill", style="cyan")
    table.add_column("Source", justify="center")
    table.add_column("Applies", justify="center")
    table.add_column("Description")
    for skill in all_skills:
        applies = "always" if skill.always else ("yes" if skill.name in selected else "-")
        style = "green" if skill.name in selected else "dim"
        source = "repo" if _is_repo_skill(skill, root) else "library"
        table.add_row(skill.name, source, f"[{style}]{applies}[/{style}]", skill.description)
    console.print(table)
    console.print(
        "\nScaffold the applicable skills into this repo with: "
        "[bold]dev skills scaffold[/bold] (add a goal to widen selection, or --all)."
    )


@app.command("show")
def show(
    name: str = typer.Argument(..., help="Skill name, e.g. core-engineering or android."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root, so repo-local skills are found too."),
):
    """Print the full body of a single skill."""
    skill = get_skill(name, project_root=project_root.expanduser().resolve())
    if skill is None:
        console.print(f"[red]No skill named '{name}'. Run 'dev skills' to list available skills.[/red]")
        raise typer.Exit(code=1)
    console.print(skill.to_skill_md())


@app.command("scaffold")
def scaffold(
    goal: str = typer.Argument("", help="Optional goal text used to widen domain-skill selection."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root to scaffold skills into."),
    all_skills: bool = typer.Option(False, "--all", help="Scaffold every skill, not just the ones that apply."),
):
    """Write the applicable skills into <repo>/.claude/skills/<name>/SKILL.md."""
    root = project_root.expanduser().resolve()
    chosen = load_skills(project_root=root) if all_skills else select_skills(goal, root)
    written = scaffold_skills(root, chosen)
    if not written:
        console.print(
            f"[green]Skills already up to date in {root / '.claude' / 'skills'} "
            f"({len(chosen)} applicable).[/green]"
        )
        return
    console.print(f"[green]Wrote {len(written)} skill file(s):[/green]")
    for path in written:
        console.print(f"  {path.relative_to(root).as_posix()}")
