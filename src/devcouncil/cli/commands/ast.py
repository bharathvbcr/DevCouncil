import json
from pathlib import Path

import typer

from devcouncil.indexing.ast_matcher import AstMatcher

app = typer.Typer(help="Run structural AST searches.")


@app.command("match")
def match(
    query: str = typer.Argument("", help="Name or source text to match."),
    language: str | None = typer.Option(None, "--language", "-l", help="Language filter."),
    kind: str | None = typer.Option(None, "--kind", "-k", help="Symbol kind filter."),
    limit: int = typer.Option(100, "--limit", help="Maximum matches."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Find function/class/type symbols using tree-sitter when available and fallbacks otherwise."""
    root = project_root.expanduser().resolve()
    matches = AstMatcher(root).match(query=query, language=language, kind=kind, limit=max(1, limit))
    typer.echo(json.dumps({"matches": [item.model_dump() for item in matches]}, indent=2))
