"""`dev okf` — export, ingest, and validate Open Knowledge Format bundles.

* ``dev okf export`` renders DevCouncil's artifact graph as a portable OKF bundle.
* ``dev okf ingest`` imports an external OKF bundle as planning/coding context under
  ``.devcouncil/knowledge/okf/`` (selected into prompts like a domain skill).
* ``dev okf validate`` checks a bundle for the OKF invariants (typed docs, resolved links).
"""

from __future__ import annotations

import shutil
from datetime import datetime, timezone
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.knowledge.okf import read_bundle, validate_bundle
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository

app = typer.Typer(help="Export, ingest, and validate Open Knowledge Format bundles.")
console = Console()


def _knowledge_okf_dir(root: Path) -> Path:
    from devcouncil.app.config import load_config

    directory = ".devcouncil/knowledge"
    try:
        directory = load_config(root).knowledge.directory
    except Exception:
        pass
    return root / directory / "okf"


@app.command("export")
def export(
    output: Path = typer.Option(Path("okf_bundle"), "--output", "-o", help="Directory to write the OKF bundle into."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    skills: bool = typer.Option(
        True,
        "--skills/--no-skills",
        help="Include the engineering skills library as OKF documents in the bundle.",
    ),
):
    """Export the DevCouncil artifact graph as an OKF bundle."""
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    out_dir = output.expanduser().resolve()
    with db.get_session() as session:
        graph = ArtifactGraphRepository(session).load_graph()

    project_name = root.name or "DevCouncil Project"
    try:
        from devcouncil.app.config import load_config

        project_name = load_config(root).project.name or project_name
    except Exception:
        pass

    # Load the FULL skill set (packaged library + this repo's own skills) so the export is
    # complete; goal-driven selection would only emit the few skills that match a goal.
    skill_list: list = []
    if skills:
        from devcouncil.skills.registry import load_skills

        skill_list = load_skills(project_root=root)

    timestamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    from devcouncil.reporting.okf_bundle_writer import OKFBundleWriter

    written = OKFBundleWriter.generate(
        graph,
        out_dir,
        project_name=project_name,
        timestamp=timestamp,
        include_skills=bool(skill_list),
        skills=skill_list,
    )
    console.print(
        f"[green]Exported {len(written)} OKF documents to[/green] {out_dir} "
        f"([cyan]{out_dir / 'index.md'}[/cyan])."
    )
    if skill_list:
        console.print(f"[green]Included {len(skill_list)} engineering skill document(s).[/green]")


@app.command("ingest")
def ingest(
    bundle: Path = typer.Argument(..., help="Path to an OKF bundle directory to ingest."),
    name: str = typer.Option("", "--name", help="Subfolder name under knowledge/okf (defaults to the bundle dir name)."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Ingest an external OKF bundle as durable planning/coding context."""
    root = project_root.expanduser().resolve()
    src = bundle.expanduser().resolve()
    if not src.is_dir():
        console.print(f"[red]Not a directory:[/red] {src}")
        raise typer.Exit(code=1)

    initialize_project(root, quiet=True)
    parsed = read_bundle(src)
    if not parsed.documents:
        console.print(f"[yellow]No OKF documents (*.md) found in[/yellow] {src}")
        raise typer.Exit(code=1)

    problems = validate_bundle(parsed)
    if problems:
        console.print(f"[yellow]Ingesting a bundle with {len(problems)} validation issue(s):[/yellow]")
        for p in problems[:10]:
            console.print(f"  - {p}")

    dest = _knowledge_okf_dir(root) / (name or src.name)
    dest.mkdir(parents=True, exist_ok=True)
    count = 0
    for md in src.rglob("*.md"):
        rel = md.relative_to(src)
        target = dest / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(md, target)
        count += 1
    console.print(
        f"[green]Ingested {count} OKF document(s) into[/green] {dest}. "
        "They are now available as planning/coding context."
    )


@app.command("validate")
def validate(
    bundle: Path = typer.Argument(..., help="Path to an OKF bundle directory to validate."),
):
    """Validate an OKF bundle (every doc typed; every intra-bundle link resolves)."""
    src = bundle.expanduser().resolve()
    if not src.is_dir():
        console.print(f"[red]Not a directory:[/red] {src}")
        raise typer.Exit(code=1)

    parsed = read_bundle(src)
    problems = validate_bundle(parsed)
    if not problems:
        console.print(f"[green]✓ Valid OKF bundle[/green] — {len(parsed.documents)} document(s), all links resolve.")
        return
    console.print(f"[red]✗ {len(problems)} problem(s) in[/red] {src}:")
    for p in problems:
        console.print(f"  - {p}")
    raise typer.Exit(code=1)
