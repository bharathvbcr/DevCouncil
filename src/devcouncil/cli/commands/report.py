import typer
from rich.console import Console
from rich.markdown import Markdown
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository
from devcouncil.reporting.report_builder import ReportBuilder

app = typer.Typer()
console = Console()

@app.callback(invoke_without_command=True)
def report(
    ctx: typer.Context,
    planning_only: bool = typer.Option(False, "--planning-only", help="Report only the planning phase status"),
    json_format: bool = typer.Option(False, "--json", help="Output report in JSON format"),
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
        
        if json_format:
            output = ReportBuilder.build_json(graph)
            console.print(output)
        else:
            output = ReportBuilder.build_markdown(graph)
            console.print(Markdown(output))
