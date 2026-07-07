from devcouncil.utils.json_persist import dump_json
import logging
from pathlib import Path

import typer

from devcouncil.indexing.ast_matcher import AstMatcher
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(help="Run structural AST searches.")
logger = logging.getLogger(__name__)


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
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev ast match: query=%r language=%s kind=%s", query, language, kind)
    with log_stage("ast", project_root=root):
        log_step("ast/1: running structural match", project_root=root, trace=True)
        matches = AstMatcher(root).match(query=query, language=language, kind=kind, limit=max(1, limit))
        typer.echo(dump_json({"matches": [item.model_dump() for item in matches]}, indent=2))
        log_step("ast/complete", project_root=root, count=len(matches), trace=True)
