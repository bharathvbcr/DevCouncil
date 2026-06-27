"""`dev design` — lint, export, and inspect a project design.md design system.

Mirrors the upstream ``@google/design.md`` CLI's ``lint`` and ``export`` subcommands so a
DevCouncil project can validate its design tokens and convert them to CSS / Tailwind / W3C
Design Tokens. The same design.md is injected into coding-agent prompts (see
``.devcouncil/knowledge/design``) so agents honor the system while they build.
"""

from __future__ import annotations

import os
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.knowledge.design import export as export_design
from devcouncil.knowledge.design import lint as lint_design
from devcouncil.knowledge.design import parse_design_md
from devcouncil.knowledge.design_conformance import (
    STYLE_EXTENSIONS,
    scan_files,
)

app = typer.Typer(help="Lint, export, and inspect a design.md design system.")
console = Console()

# Where a project's design system is looked for, in order.
_DEFAULT_PATHS = (
    ".devcouncil/knowledge/design/design.md",
    "DESIGN.md",
    "design.md",
)

# Directories pruned while auto-discovering style files (heavy / generated / vendored).
_PRUNE_DIRS = frozenset({
    ".git", ".hg", ".svn", "node_modules", ".venv", "venv", "env",
    "dist", "build", "out", ".next", ".nuxt", ".svelte-kit", "coverage",
    "__pycache__", ".mypy_cache", ".pytest_cache", ".cache", "vendor",
})
# Cap on auto-discovered files so an enormous repo can't make `check` run unbounded.
_MAX_DISCOVERED_FILES = 5000


def _discover_style_files(root: Path) -> list[Path]:
    """Walk ``root`` for style-ish files, pruning heavy dirs and bounding the count."""
    found: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in _PRUNE_DIRS and not d.startswith(".")]
        for name in filenames:
            if Path(name).suffix.lower() in STYLE_EXTENSIONS:
                found.append(Path(dirpath) / name)
                if len(found) >= _MAX_DISCOVERED_FILES:
                    return found
    return found


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


@app.command("check")
def check(
    files: list[Path] = typer.Argument(
        None, help="Files to check (defaults to the repo's style-ish files)."),
    design: Path = typer.Option(
        None, "--design", help="Path to a design.md (defaults to the project's design system)."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Flag hardcoded style literals (hex colors, px sizes) that bypass design tokens.

    Exits non-zero when any violation is found so it can gate CI / a pre-commit hook.
    """
    root = project_root.expanduser().resolve()
    target = _resolve_path(design, root)
    if target is None:
        console.print("[red]No design.md found.[/red] Looked for: " + ", ".join(_DEFAULT_PATHS))
        raise typer.Exit(code=1)

    ds = parse_design_md(target)
    paths = [f.expanduser() for f in files] if files else _discover_style_files(root)
    violations = scan_files(paths, ds)

    if not violations:
        scanned = len([p for p in paths if p.suffix.lower() in STYLE_EXTENSIONS])
        console.print(
            f"[green]✓ No design-token violations in {scanned} file(s) "
            f"(tokens from {target}).[/green]")
        return

    by_file: dict[str, list] = {}
    for v in violations:
        by_file.setdefault(v.file or "<text>", []).append(v)

    console.print(
        f"[bold red]{len(violations)} design-token violation(s) "
        f"in {len(by_file)} file(s):[/bold red]")
    for fname in sorted(by_file):
        console.print(f"[bold]{fname}[/bold]")
        for v in sorted(by_file[fname], key=lambda x: (x.line, x.kind)):
            console.print(
                f"  [yellow]{v.line}[/yellow] [{v.kind}] {v.message}\n"
                f"      [dim]{v.snippet}[/dim]")
    raise typer.Exit(code=1)


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
