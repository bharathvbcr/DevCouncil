import typer
from rich.console import Console
from rich.markdown import Markdown
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository
from devcouncil.reporting.report_builder import ReportBuilder
from devcouncil.integrations.github import GitHubIntegration
from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.telemetry.traces import TraceLogger
import asyncio
import os
import subprocess
from pathlib import Path

app = typer.Typer()
console = Console()

async def run_github_report(graph: ArtifactGraph):
    token = os.environ.get("GITHUB_TOKEN")
    repo = os.environ.get("GITHUB_REPOSITORY") # e.g. owner/repo
    
    if not token or not repo:
        console.print("[red]GITHUB_TOKEN and GITHUB_REPOSITORY must be set for GitHub reporting.[/red]")
        return

    try:
        # Detect current SHA
        sha = subprocess.check_output(["git", "rev-parse", "HEAD"]).decode().strip()
        integration = GitHubIntegration(token, repo, sha)
        await integration.report_verification(graph)
        console.print(f"[green]Successfully reported to GitHub PR Checks for {repo} at {sha[:7]}[/green]")
    except Exception as e:
        console.print(f"[red]Failed to report to GitHub: {e}[/red]")

@app.callback(invoke_without_command=True)
def report(
    ctx: typer.Context,
    planning_only: bool = typer.Option(False, "--planning-only", help="Report only the planning phase status"),
    json_format: bool = typer.Option(False, "--json", help="Output report in JSON format"),
    github: bool = typer.Option(False, "--github", help="Post report to GitHub PR Checks"),
):
    """
    Produce final evidence report.
    """
    if ctx.invoked_subcommand is not None:
        return

    db = get_db()
    if not db:
        console.print("[red]DevCouncil not initialized. Run 'dev init' first.[/red]")
        raise typer.Exit(code=1)

    with db.get_session() as session:
        graph_repo = ArtifactGraphRepository(session)
        graph = graph_repo.load_graph()
        TraceLogger(Path(".")).log_event(
            "report_generated",
            {"json": json_format, "github": github, "planning_only": planning_only},
            summary="Generated DevCouncil report",
        )
        
        if github:
            asyncio.run(run_github_report(graph))
            return

        if json_format:
            output = ReportBuilder.build_json(graph)
            console.print(output)
        else:
            output = ReportBuilder.build_markdown(graph)
            console.print(Markdown(output))
