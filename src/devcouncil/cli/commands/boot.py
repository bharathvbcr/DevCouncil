"""One-command onboarding: setup, integrate, and run the full DevCouncil loop."""

from __future__ import annotations

import logging
import sys
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.app.config import load_config
from devcouncil.cli.commands.doctor import render_doctor_check
from devcouncil.cli.commands.go import go as go_command
from devcouncil.cli.commands.init import initialize_project, parse_role_model_overrides
from devcouncil.cli.commands.setup import (
    _configure_api_key,
    _configure_coding_cli_integrations,
    _configure_vertexai_settings,
    _set_model_provider,
    _set_model_roles,
)
from devcouncil.llm.provider import validate_model_provider
from devcouncil.repo.ci_scaffold import EVIDENCE_WORKFLOW_RELPATH, WORKFLOW_RELPATH, scaffold_ci, scaffold_evidence_ci
from devcouncil.telemetry.logging_setup import set_log_dir
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


def _run_setup_path(
    root: Path,
    *,
    name: str | None,
    provider: str | None,
    model: str | None,
    role_models: dict[str, str],
    api_key: str | None,
    skip_api_key: bool,
    skip_integrations: bool,
    skip_map: bool,
    skip_skills: bool,
    scaffold_ci_flag: bool,
    scaffold_ci_evidence: bool,
    gemini_scope: str,
) -> None:
    created = initialize_project(
        root,
        project_name=name,
        model_provider=validate_model_provider(provider) if provider else "openrouter",
        model=model,
        role_models=role_models,
        with_map=not skip_map,
        with_skills=not skip_skills,
    )
    if not created:
        console.print(f"[yellow]DevCouncil is already initialized at {root / '.devcouncil'}.[/yellow]")

    if provider:
        _set_model_provider(root, provider)

    _set_model_roles(root, model=model, role_models=role_models)

    configured_provider = load_config(root).models.provider
    _configure_vertexai_settings(root, configured_provider, None, None)
    _configure_api_key(root, api_key, skip_api_key)

    console.print()
    render_doctor_check(root)

    if scaffold_ci_flag:
        written = scaffold_ci(root)
        if written is None:
            console.print(f"[yellow]{WORKFLOW_RELPATH.as_posix()} already exists; left unchanged.[/yellow]")
        else:
            console.print(f"[green]Wrote starter CI workflow {written.relative_to(root).as_posix()}.[/green]")

    if scaffold_ci_evidence:
        evidence_written = scaffold_evidence_ci(root)
        if evidence_written is None:
            console.print(
                f"[yellow]{EVIDENCE_WORKFLOW_RELPATH.as_posix()} already exists; left unchanged.[/yellow]"
            )
        else:
            console.print(
                f"[green]Wrote evidence CI workflow {evidence_written.relative_to(root).as_posix()}.[/green]"
            )

    if not skip_integrations:
        _configure_coding_cli_integrations(root, apply=True, gemini_scope=gemini_scope)


def boot(
    ctx: typer.Context,
    goal: str = typer.Argument(..., help="Implementation goal passed through to dev go."),
    executor: str | None = typer.Option(
        None,
        "--executor",
        "-e",
        help="Automated executor to use. Defaults to execution.default_executor in .devcouncil/config.yaml.",
    ),
    dry_run: bool = typer.Option(False, "--dry-run", help="Use mock planning responses for local smoke testing."),
    quick: bool = typer.Option(
        False,
        "--quick",
        help="Skip the planning council for a single spec + plan.",
    ),
    force: bool = typer.Option(
        False,
        "--force",
        "--yes",
        "-y",
        help="Proceed past unresolved planning gaps without manual approval.",
    ),
    continue_on_blocked: bool = typer.Option(
        False,
        "--continue-on-blocked",
        help="Continue later tasks even if an earlier task is blocked by verification.",
    ),
    json_report: bool = typer.Option(False, "--json-report", "--json", help="Print the final report as JSON."),
    report_file: Path | None = typer.Option(
        None,
        "--report-file",
        help="Write the final report to a file. Relative paths resolve from --project-root.",
    ),
    agent: bool = typer.Option(
        False,
        "--agent",
        help="Use coding-agent defaults: JSON report plus .devcouncil/reports/latest.json.",
    ),
    profile: str | None = typer.Option(None, "--profile", help="CLI-agent execution profile to pass to dev run."),
    stream: bool = typer.Option(
        False,
        "--stream",
        help="Stream coding CLI stdout/stderr live during execution.",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    name: str | None = typer.Option(None, "--name", "-n", help="Project name for .devcouncil/config.yaml."),
    provider: str | None = typer.Option(None, "--provider", help="Set models.provider before configuring the API key."),
    model: str | None = typer.Option(None, "--model", "-m", help="Model id to use for every default role."),
    role_model: list[str] | None = typer.Option(
        None,
        "--role-model",
        help="Per-role model override in ROLE=MODEL form. Can be repeated.",
    ),
    api_key: str | None = typer.Option(None, "--api-key", help="Store the configured provider API key in local secrets."),
    skip_api_key: bool = typer.Option(False, "--skip-api-key", help="Skip the first-run model API key prompt."),
    skip_integrations: bool = typer.Option(
        False,
        "--skip-integrations",
        help="Skip applying coding CLI integrations (default applies dev integrate --apply).",
    ),
    skip_map: bool = typer.Option(False, "--skip-map", help="Skip generating repo_map.json and agent guides on init."),
    skip_skills: bool = typer.Option(False, "--skip-skills", help="Skip scaffolding engineering skills on init."),
    scaffold_ci_flag: bool = typer.Option(False, "--scaffold-ci", help="Write a starter GitHub Actions workflow."),
    scaffold_ci_evidence: bool = typer.Option(
        False,
        "--scaffold-ci-evidence",
        help="Write .github/workflows/devcouncil-evidence.yml.",
    ),
    gemini_scope: str = typer.Option("project", "--gemini-scope", help="Gemini MCP config scope: project or user."),
):
    """
    Initialize the repo, apply integrations, and run the full DevCouncil loop.
    """
    if gemini_scope not in {"project", "user"}:
        console.print("[red]--gemini-scope must be 'project' or 'user'.[/red]")
        raise typer.Exit(code=2)

    root = project_root.expanduser().resolve()
    set_log_dir(root)
    logger.info("dev boot: goal=%r skip_integrations=%s", goal, skip_integrations)

    try:
        role_models = parse_role_model_overrides(role_model)
        if provider:
            validate_model_provider(provider)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=2) from exc

    effective_skip_api_key = skip_api_key or not sys.stdin.isatty()

    with log_stage("boot", project_root=root, goal=goal):
        log_step("boot/1: setup", project_root=root, trace=True)
        _run_setup_path(
            root,
            name=name,
            provider=provider,
            model=model,
            role_models=role_models,
            api_key=api_key,
            skip_api_key=effective_skip_api_key,
            skip_integrations=skip_integrations,
            skip_map=skip_map,
            skip_skills=skip_skills,
            scaffold_ci_flag=scaffold_ci_flag,
            scaffold_ci_evidence=scaffold_ci_evidence,
            gemini_scope=gemini_scope,
        )

        log_step("boot/2: go", project_root=root, trace=True)
        go_command(
            ctx,
            goal=goal,
            executor=executor,
            dry_run=dry_run,
            quick=quick,
            force=force,
            continue_on_blocked=continue_on_blocked,
            json_report=json_report,
            report_file=report_file,
            agent=agent,
            profile=profile,
            stream=stream,
            project_root=root,
        )
