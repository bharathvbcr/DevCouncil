import typer
from rich.console import Console
from rich.markdown import Markdown
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository
from devcouncil.reporting.report_builder import ReportBuilder
from devcouncil.integrations.github import GitHubIntegration
from devcouncil.integrations.pr_comments import GitHubPRCommenter, GitLabMRCommenter, build_pr_comment_body
from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.cli.commands.init import initialize_project
from devcouncil.live.summary import live_review_summary
import asyncio
import os
import subprocess
from pathlib import Path

app = typer.Typer()
console = Console()

async def run_github_report(graph: ArtifactGraph, project_root: Path):
    token = os.environ.get("GITHUB_TOKEN")
    repo = os.environ.get("GITHUB_REPOSITORY") # e.g. owner/repo
    
    if not token or not repo:
        console.print("[red]GITHUB_TOKEN and GITHUB_REPOSITORY must be set for GitHub reporting.[/red]")
        return

    try:
        # Detect current SHA
        sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=project_root).decode().strip()
        integration = GitHubIntegration(token, repo, sha)
        await integration.report_verification(graph)
        console.print(f"[green]Successfully reported to GitHub PR Checks for {repo} at {sha[:7]}[/green]")
    except Exception as e:
        console.print(f"[red]Failed to report to GitHub: {e}[/red]")


async def run_github_pr_comment(graph: ArtifactGraph, live_review: dict | None = None):
    token = os.environ.get("GITHUB_TOKEN")
    repo = os.environ.get("GITHUB_REPOSITORY")
    pull_number = os.environ.get("GITHUB_PR_NUMBER") or os.environ.get("PR_NUMBER")
    if not token or not repo or not pull_number:
        console.print("[red]GITHUB_TOKEN, GITHUB_REPOSITORY, and GITHUB_PR_NUMBER must be set for GitHub PR comments.[/red]")
        return
    try:
        pull_number_int = int(pull_number)
    except ValueError:
        console.print("[red]GITHUB_PR_NUMBER must be an integer.[/red]")
        return
    commenter = GitHubPRCommenter(token, repo, pull_number_int)
    await commenter.post_comment(build_pr_comment_body(graph, live_review=live_review))
    console.print(f"[green]Posted DevCouncil PR comment to GitHub PR #{pull_number}.[/green]")


async def run_gitlab_mr_comment(graph: ArtifactGraph, live_review: dict | None = None):
    token = os.environ.get("GITLAB_TOKEN")
    project_id = os.environ.get("GITLAB_PROJECT_ID")
    mr_iid = os.environ.get("GITLAB_MR_IID") or os.environ.get("CI_MERGE_REQUEST_IID")
    base_url = os.environ.get("GITLAB_API_URL", "https://gitlab.com/api/v4")
    if not token or not project_id or not mr_iid:
        console.print("[red]GITLAB_TOKEN, GITLAB_PROJECT_ID, and GITLAB_MR_IID must be set for GitLab MR comments.[/red]")
        return
    try:
        mr_iid_int = int(mr_iid)
    except ValueError:
        console.print("[red]GITLAB_MR_IID must be an integer.[/red]")
        return
    commenter = GitLabMRCommenter(token, project_id, mr_iid_int, base_url=base_url)
    await commenter.post_comment(build_pr_comment_body(graph, live_review=live_review))
    console.print(f"[green]Posted DevCouncil MR comment to GitLab MR !{mr_iid}.[/green]")

@app.callback(invoke_without_command=True)
def report(
    ctx: typer.Context,
    planning_only: bool = typer.Option(False, "--planning-only", help="Report only the planning phase status"),
    json_format: bool = typer.Option(False, "--json", help="Output report in JSON format"),
    github: bool = typer.Option(False, "--github", help="Post report to GitHub PR Checks"),
    github_pr_comment: bool = typer.Option(False, "--github-pr-comment", help="Post report as a GitHub PR comment"),
    gitlab_pr_comment: bool = typer.Option(False, "--gitlab-pr-comment", help="Post report as a GitLab merge request comment"),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Produce final evidence report.
    """
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    with db.get_session() as session:
        graph_repo = ArtifactGraphRepository(session)
        graph = graph_repo.load_graph()
        live_review = live_review_summary(root)
        TraceLogger(root).log_event(
            "report_generated",
            {
                "json": json_format,
                "github": github,
                "github_pr_comment": github_pr_comment,
                "gitlab_pr_comment": gitlab_pr_comment,
                "planning_only": planning_only,
            },
            summary="Generated DevCouncil report",
        )
        
        if github:
            asyncio.run(run_github_report(graph, root))
            return

        if github_pr_comment:
            asyncio.run(run_github_pr_comment(graph, live_review=live_review))
            return

        if gitlab_pr_comment:
            asyncio.run(run_gitlab_mr_comment(graph, live_review=live_review))
            return

        if json_format:
            output = ReportBuilder.build_json(graph, live_review=live_review)
            typer.echo(output)
        else:
            output = ReportBuilder.build_markdown(graph, live_review=live_review)
            console.print(Markdown(output))
