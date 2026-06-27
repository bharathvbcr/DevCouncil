"""`dev design` — lint, export, and inspect a project design.md design system.

Mirrors the upstream ``@google/design.md`` CLI's ``lint`` and ``export`` subcommands so a
DevCouncil project can validate its design tokens and convert them to CSS / Tailwind / W3C
Design Tokens. The same design.md is injected into coding-agent prompts (see
``.devcouncil/knowledge/design``) so agents honor the system while they build.
"""

from __future__ import annotations

from pathlib import Path

import typer
from rich.console import Console

from devcouncil.knowledge.design import export as export_design
from devcouncil.knowledge.design import lint as lint_design
from devcouncil.knowledge.design import parse_design_md

app = typer.Typer(help="Lint, export, and inspect a design.md design system.")
console = Console()

# Where a project's design system is looked for, in order.
_DEFAULT_PATHS = (
    ".devcouncil/knowledge/design/design.md",
    "DESIGN.md",
    "design.md",
)


def _resolve_path(explicit: Path | None, project_root: Path) -> Path | None:
    if explicit is not None:
        candidate = explicit.expanduser()
        return candidate if candidate.is_file() else None
    for rel in _DEFAULT_PATHS:
        candidate = project_root / rel
        if candidate.is_file():
            return candidate
    return None


@app.command("lint")
def lint(
    path: Path = typer.Argument(None, help="Path to a design.md (defaults to the project's design system)."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Validate a design system: broken token refs, contrast, ordering, orphans."""
    root = project_root.expanduser().resolve()
    target = _resolve_path(path, root)
    if target is None:
        console.print("[red]No design.md found.[/red] Looked for: " + ", ".join(_DEFAULT_PATHS))
        raise typer.Exit(code=1)

    findings = lint_design(parse_design_md(target))
    if not findings:
        console.print(f"[green]✓ {target} passed all design.md lint rules.[/green]")
        return

    errors = [f for f in findings if f.severity == "error"]
    color = {"error": "red", "warning": "yellow", "info": "cyan"}
    console.print(f"[bold]{len(findings)} finding(s) in {target}:[/bold]")
    for f in findings:
        console.print(f"  [{color.get(f.severity, 'white')}]{f.format()}[/{color.get(f.severity, 'white')}]")
    if errors:
        raise typer.Exit(code=1)


@app.command("export")
def export(
    path: Path = typer.Argument(None, help="Path to a design.md (defaults to the project's design system)."),
    fmt: str = typer.Option("css", "--format", "-f", help="Output format: css | tailwind | w3c."),
    output: Path = typer.Option(None, "--output", "-o", help="Write to this file instead of stdout."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Export design tokens to CSS custom properties, a Tailwind config, or W3C tokens."""
    root = project_root.expanduser().resolve()
    if fmt not in ("css", "tailwind", "w3c"):
        console.print(f"[red]Unknown format '{fmt}'.[/red] Use one of: css, tailwind, w3c.")
        raise typer.Exit(code=2)
    target = _resolve_path(path, root)
    if target is None:
        console.print("[red]No design.md found.[/red] Looked for: " + ", ".join(_DEFAULT_PATHS))
        raise typer.Exit(code=1)

    rendered = export_design(parse_design_md(target), fmt)  # type: ignore[arg-type]
    if output is not None:
        out = output.expanduser().resolve()
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(rendered, encoding="utf-8")
        console.print(f"[green]Wrote {fmt} tokens to[/green] {out}")
    else:
        typer.echo(rendered, nl=not rendered.endswith("\n"))


@app.command("show")
def show(
    path: Path = typer.Argument(None, help="Path to a design.md (defaults to the project's design system)."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Summarize a design system: token counts and document sections."""
    root = project_root.expanduser().resolve()
    target = _resolve_path(path, root)
    if target is None:
        console.print("[red]No design.md found.[/red] Looked for: " + ", ".join(_DEFAULT_PATHS))
        raise typer.Exit(code=1)

    ds = parse_design_md(target)
    console.print(f"[bold]{ds.name or target.name}[/bold] ({target})")
    console.print(
        f"  colors={len(ds.colors)}  typography={len(ds.typography)}  "
        f"rounded={len(ds.rounded)}  spacing={len(ds.spacing)}  components={len(ds.components)}"
    )
    if ds.sections:
        console.print("  sections: " + ", ".join(h for h, _ in ds.sections))
