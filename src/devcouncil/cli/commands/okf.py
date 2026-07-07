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
from devcouncil.telemetry.stages import log_stage, log_step
import logging

app = typer.Typer(help="Export, ingest, and validate Open Knowledge Format bundles.")
console = Console()
logger = logging.getLogger(__name__)


def _knowledge_okf_dir(root: Path) -> Path:
    from devcouncil.app.config import load_config

    directory = ".devcouncil/knowledge"
    try:
        directory = load_config(root).knowledge.directory
    except Exception as e:
        logger.debug("Failed to load knowledge directory from config, using default: %s", e)
    return root / directory / "okf"


def _knowledge_design_md(root: Path) -> Path | None:
    """Locate the project's design.md under the configured knowledge dir's design/ subdir.

    Honors the same config dir resolution as :func:`_knowledge_okf_dir`, falling back to the
    default ``.devcouncil/knowledge/design/design.md``. Returns ``None`` if no design.md exists.
    """
    from devcouncil.app.config import load_config

    directory = ".devcouncil/knowledge"
    try:
        directory = load_config(root).knowledge.directory
    except Exception as e:
        logger.debug("Failed to load knowledge directory from config, using default: %s", e)
    candidates = [
        root / directory / "design" / "design.md",
        root / ".devcouncil" / "knowledge" / "design" / "design.md",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return None


@app.command("export")
def export(
    output: Path = typer.Option(Path("okf_bundle"), "--output", "-o", help="Directory to write the OKF bundle into."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    skills: bool = typer.Option(
        True,
        "--skills/--no-skills",
        help="Include the engineering skills library as OKF documents in the bundle.",
    ),
    design: bool = typer.Option(
        True,
        "--design/--no-design",
        help="Include the project's design.md (if present) as an OKF document in the bundle.",
    ),
):
    """Export the DevCouncil artifact graph as an OKF bundle."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev okf export")
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    with log_stage("okf", project_root=root, subcommand="export"):
        log_step("okf/1: exporting OKF bundle", project_root=root, trace=True)
        _run_okf_export(root, db, output, skills, design)
        log_step("okf/complete", project_root=root, trace=True)


def _run_okf_export(root, db, output, skills, design):
    out_dir = output.expanduser().resolve()
    with db.get_session() as session:
        graph = ArtifactGraphRepository(session).load_graph()

    project_name = root.name or "DevCouncil Project"
    try:
        from devcouncil.app.config import load_config

        project_name = load_config(root).project.name or project_name
    except Exception as e:
        logger.debug("Failed to load project name from config, using directory name: %s", e)

    # Load the FULL skill set (packaged library + this repo's own skills) so the export is
    # complete; goal-driven selection would only emit the few skills that match a goal.
    skill_list: list = []
    if skills:
        from devcouncil.skills.registry import load_skills

        skill_list = load_skills(project_root=root)

    # Look for the project's design.md under the configured knowledge dir; silently skip if
    # absent so export stays useful for projects without a design system.
    design_obj = None
    if design:
        design_md = _knowledge_design_md(root)
        if design_md is not None:
            from devcouncil.knowledge.design import parse_design_md

            try:
                design_obj = parse_design_md(design_md)
            except Exception:
                design_obj = None

    timestamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    from devcouncil.reporting.okf_bundle_writer import OKFBundleWriter

    written = OKFBundleWriter.generate(
        graph,
        out_dir,
        project_name=project_name,
        timestamp=timestamp,
        include_skills=bool(skill_list),
        skills=skill_list,
        include_design=design_obj is not None,
        design=design_obj,
    )
    console.print(
        f"[green]Exported {len(written)} OKF documents to[/green] {out_dir} "
        f"([cyan]{out_dir / 'index.md'}[/cyan])."
    )
    if skill_list:
        console.print(f"[green]Included {len(skill_list)} engineering skill document(s).[/green]")
    if design_obj is not None:
        console.print("[green]Included 1 design system document(s).[/green]")


@app.command("ingest")
def ingest(
    bundle: str = typer.Argument(
        ...,
        help="OKF bundle source: a local directory, a .tar.gz/.tgz/.zip archive, or a git URL.",
    ),
    name: str = typer.Option("", "--name", help="Subfolder name under knowledge/okf (defaults to the bundle name)."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Ingest an external OKF bundle as durable planning/coding context.

    The bundle may be a local directory, a local archive (``.tar.gz``/``.tgz``/``.zip``,
    extracted behind a path-traversal guard), or a git URL (shallow-cloned). After the
    source is materialized to a local directory, the existing read/validate/copy logic runs
    unchanged; any temp directory is removed afterwards.
    """
    root = project_root.expanduser().resolve()

    from devcouncil.knowledge.fetch import fetch_bundle

    try:
        fetched = fetch_bundle(bundle)
    except Exception as exc:
        console.print(f"[red]Could not fetch bundle[/red] {bundle!r}: {exc}")
        raise typer.Exit(code=1)

    try:
        src = fetched.directory
        if not src.is_dir():
            console.print(f"[red]Not a directory:[/red] {src}")
            raise typer.Exit(code=1)

        initialize_project(root, quiet=True)
        parsed = read_bundle(src)
        if not parsed.documents:
            console.print(f"[yellow]No OKF documents (*.md) found in[/yellow] {bundle}")
            raise typer.Exit(code=1)

        problems = validate_bundle(parsed)
        if problems:
            console.print(f"[yellow]Ingesting a bundle with {len(problems)} validation issue(s):[/yellow]")
            for p in problems[:10]:
                console.print(f"  - {p}")

        dest = _knowledge_okf_dir(root) / (name or fetched.suggested_name or "bundle")
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
    finally:
        fetched.cleanup()


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


@app.command("select")
def select_knowledge(
    goal: str = typer.Option(..., "--goal", "-g", help="Task goal used to rank ingested knowledge sources."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    json_output: bool = typer.Option(False, "--json", help="Emit machine-readable JSON."),
) -> None:
    """Select OKF/design knowledge sources that match a goal."""
    from devcouncil.knowledge.knowledge_select import select_knowledge_payload

    root = project_root.expanduser().resolve()
    payload = select_knowledge_payload(root, goal)
    if json_output:
        console.print_json(data=payload)
        return
    if payload.get("sources"):
        for source in payload["sources"]:
            console.print(f"- [{source['kind']}] {source['name']}: {source['description']}")
    if payload.get("preamble"):
        console.print("\n" + str(payload["preamble"]))


@app.command("html")
def html(
    bundle: Path = typer.Argument(..., help="Path to an OKF bundle directory to render."),
    output: Path = typer.Option(Path("okf_site"), "--output", "-o", help="Directory to write the static HTML site into."),
):
    """Render an OKF bundle as a browsable, self-contained static HTML site."""
    src = bundle.expanduser().resolve()
    if not src.is_dir():
        console.print(f"[red]Not a directory:[/red] {src}")
        raise typer.Exit(code=1)

    parsed = read_bundle(src)
    if not parsed.documents:
        console.print(f"[yellow]No OKF documents (*.md) found in[/yellow] {src}")
        raise typer.Exit(code=1)

    from devcouncil.reporting.okf_html import write_bundle_html

    out_dir = output.expanduser().resolve()
    written = write_bundle_html(parsed, out_dir)
    console.print(
        f"[green]Rendered {len(written)} page(s) to[/green] {out_dir} "
        f"([cyan]{out_dir / 'index.html'}[/cyan])."
    )
