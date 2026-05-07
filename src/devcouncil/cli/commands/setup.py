from pathlib import Path
import os
import shutil
import sys

import typer
import yaml
from rich.console import Console
from rich.panel import Panel

from devcouncil.app.config import get_gcloud_access_token, load_config, load_local_secrets, provider_api_key_env_var
from devcouncil.cli.commands.doctor import render_doctor_check
from devcouncil.cli.commands.init import initialize_project, parse_role_model_overrides
from devcouncil.cli.commands.integrate import (
    _claude_command,
    _codex_command,
    _configure_native_hooks,
    _configure_warp,
    _configure,
    _cursor_command,
    _gemini_command,
)
from devcouncil.llm.provider import apply_provider_default_role_models, build_role_model_config, validate_model_provider

app = typer.Typer()
console = Console()


def _set_model_provider(project_root: Path, provider: str) -> None:
    normalized = validate_model_provider(provider)
    config_path = project_root / ".devcouncil" / "config.yaml"
    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8")) or {}
    raw_config.setdefault("models", {})
    previous = raw_config["models"].get("provider", "openrouter")
    raw_config["models"]["provider"] = normalized
    updated_role_defaults = apply_provider_default_role_models(raw_config, previous, normalized)
    config_path.write_text(yaml.dump(raw_config, default_flow_style=False), encoding="utf-8")
    if previous != normalized:
        console.print(f"[green]Updated model provider from {previous} to {normalized}.[/green]")
    if updated_role_defaults:
        console.print(f"[green]Updated default role models for {normalized}.[/green]")


def _set_model_roles(
    project_root: Path,
    model: str | None = None,
    role_models: dict[str, str] | None = None,
) -> None:
    if not model and not role_models:
        return

    config_path = project_root / ".devcouncil" / "config.yaml"
    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8")) or {}
    raw_config.setdefault("models", {})
    provider = validate_model_provider(raw_config["models"].get("provider", "openrouter"))
    raw_config["models"]["roles"] = build_role_model_config(
        provider,
        model=model,
        role_models=role_models,
    )
    config_path.write_text(yaml.dump(raw_config, default_flow_style=False), encoding="utf-8")
    if model:
        console.print(f"[green]Updated all model roles to use {model}.[/green]")
    for role, selected_model in (role_models or {}).items():
        console.print(f"[green]Updated {role} to use {selected_model}.[/green]")


def _write_local_secret(project_root: Path, env_var: str, value: str) -> Path:
    if "\n" in value or "\r" in value:
        raise ValueError("API keys cannot contain newlines.")
    secrets = load_local_secrets(project_root)
    secrets[env_var] = value
    path = project_root / ".devcouncil" / "secrets.env"
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        "# Local DevCouncil secrets. This file is ignored by git.",
        "# Process environment variables with the same name take precedence.",
        *[f"{key}={val}" for key, val in sorted(secrets.items())],
        "",
    ]
    path.write_text("\n".join(lines), encoding="utf-8")
    return path


def _configure_vertexai_settings(
    project_root: Path,
    provider: str,
    vertex_project: str | None,
    vertex_location: str | None,
) -> None:
    if provider != "vertexai":
        return

    local_secrets = load_local_secrets(project_root)
    if vertex_project:
        _write_local_secret(project_root, "VERTEXAI_PROJECT", vertex_project)
        console.print("[green]Saved VERTEXAI_PROJECT to .devcouncil/secrets.env.[/green]")
    elif not (
        os.environ.get("VERTEXAI_PROJECT")
        or os.environ.get("GOOGLE_CLOUD_PROJECT")
        or local_secrets.get("VERTEXAI_PROJECT")
        or local_secrets.get("GOOGLE_CLOUD_PROJECT")
    ):
        console.print(
            "[yellow]VERTEXAI_PROJECT is not set. "
            "Set it in your shell or rerun setup with --vertex-project PROJECT_ID.[/yellow]"
        )

    if vertex_location:
        _write_local_secret(project_root, "VERTEXAI_LOCATION", vertex_location)
        console.print("[green]Saved VERTEXAI_LOCATION to .devcouncil/secrets.env.[/green]")


def _configure_api_key(project_root: Path, api_key: str | None, skip_api_key: bool) -> None:
    config = load_config(project_root)
    provider = config.models.provider
    env_var = provider_api_key_env_var(provider)
    local_secrets = load_local_secrets(project_root)

    if os.environ.get(env_var):
        console.print(f"[green]{env_var} is already set in the environment.[/green]")
        return
    if local_secrets.get(env_var):
        console.print(f"[green]{env_var} is already set in .devcouncil/secrets.env.[/green]")
        return
    if api_key:
        _write_local_secret(project_root, env_var, api_key)
        console.print(f"[green]Saved {env_var} to .devcouncil/secrets.env.[/green]")
        return
    if provider == "vertexai" and get_gcloud_access_token():
        console.print("[green]Vertex AI access token is available from gcloud auth print-access-token.[/green]")
        return
    if skip_api_key:
        console.print(f"[yellow]Skipped {env_var} setup. Model-backed commands will ask again if it is missing.[/yellow]")
        return
    if not sys.stdin.isatty():
        console.print(
            f"[yellow]{env_var} is not set.[/yellow] "
            f"Run [bold]dev setup --api-key YOUR_KEY[/bold] or set it in your shell before model-backed commands."
        )
        return

    console.print()
    console.print(Panel.fit(
        "\n".join([
            f"Provider: {provider}",
            f"Required key: {env_var}",
            "Press Enter without a value to skip for now.",
        ]),
        title="Model API Key",
        border_style="cyan",
    ))
    entered = typer.prompt(f"{env_var}", default="", hide_input=True, show_default=False)
    if not entered:
        console.print(f"[yellow]Skipped {env_var} setup.[/yellow]")
        return
    _write_local_secret(project_root, env_var, entered)
    console.print(f"[green]Saved {env_var} to .devcouncil/secrets.env.[/green]")


def _is_interactive_terminal() -> bool:
    return sys.stdin.isatty()


def _configure_coding_cli_integrations(project_root: Path, apply: bool, gemini_scope: str) -> None:
    console.print()
    console.print("[bold]Coding CLI integration[/bold]")
    commands = [
        ("Codex CLI", _codex_command(project_root)),
        ("Gemini CLI", _gemini_command(project_root, gemini_scope)),
        ("Claude Code", _claude_command(project_root, "local")),
        ("Cursor", _cursor_command(project_root)),
    ]
    results = []
    for tool, command in commands:
        if apply and not shutil.which(command[0]):
            console.print(f"[yellow]{tool} CLI not found on PATH. Skipping optional integration.[/yellow]")
            continue
        results.append(_configure(tool, command, apply))
    results.append(_configure_warp(project_root, apply))
    _configure_native_hooks(project_root, "all", apply)
    if apply and any(not ok for ok in results):
        raise typer.Exit(code=1)


def _prompt_for_first_run_integrations(project_root: Path, apply: bool, gemini_scope: str) -> bool:
    if not _is_interactive_terminal():
        return False

    console.print()
    console.print(Panel.fit(
        "\n".join([
            "DevCouncil can configure supported coding CLIs now.",
            "This adds MCP setup and native hook config for detected clients.",
            "Missing optional clients are skipped.",
        ]),
        title="Coding CLI Setup",
        border_style="cyan",
    ))
    if not typer.confirm("Set up coding CLI integrations now?", default=True):
        console.print("[yellow]Skipped coding CLI integration setup.[/yellow]")
        return False

    _configure_coding_cli_integrations(project_root, apply=apply, gemini_scope=gemini_scope)
    return True


@app.callback(invoke_without_command=True)
def setup(
    ctx: typer.Context,
    project_root: Path = typer.Option(
        Path("."),
        "--project-root",
        help="Target project repository root. Defaults to the terminal's current directory.",
    ),
    name: str | None = typer.Option(None, "--name", "-n", help="Project name for .devcouncil/config.yaml."),
    integrate: bool = typer.Option(False, "--integrate", help="Configure supported coding CLI MCP integrations and native hooks."),
    apply: bool = typer.Option(False, "--apply", help="Apply integration config instead of previewing commands."),
    gemini_scope: str = typer.Option("project", "--gemini-scope", help="Gemini MCP config scope: project or user."),
    provider: str | None = typer.Option(None, "--provider", help="Set models.provider before configuring the API key."),
    model: str | None = typer.Option(None, "--model", "-m", help="Model id to use for every default role."),
    role_model: list[str] | None = typer.Option(
        None,
        "--role-model",
        help="Per-role model override in ROLE=MODEL form. Can be repeated.",
    ),
    api_key: str | None = typer.Option(None, "--api-key", help="Store the configured provider API key in local .devcouncil/secrets.env."),
    vertex_project: str | None = typer.Option(None, "--vertex-project", help="Store VERTEXAI_PROJECT for the vertexai provider."),
    vertex_location: str | None = typer.Option(None, "--vertex-location", help="Store VERTEXAI_LOCATION for the vertexai provider. Defaults to global."),
    skip_api_key: bool = typer.Option(False, "--skip-api-key", help="Skip the first-run model API key prompt."),
    skip_integrations: bool = typer.Option(False, "--skip-integrations", help="Skip the first-run coding CLI integration prompt."),
):
    """
    Initialize DevCouncil from a normal terminal in the target repository root.

    Use the coding CLI later only for the generated dev prompt output.
    """
    if ctx.invoked_subcommand is not None:
        return

    if gemini_scope not in {"project", "user"}:
        console.print("[red]--gemini-scope must be 'project' or 'user'.[/red]")
        raise typer.Exit(code=2)

    root = project_root.expanduser().resolve()
    try:
        role_models = parse_role_model_overrides(role_model)
        initial_provider = validate_model_provider(provider) if provider else "openrouter"
    except ValueError as e:
        console.print(f"[red]{e}[/red]")
        raise typer.Exit(code=2) from e

    created = initialize_project(
        root,
        project_name=name,
        model_provider=initial_provider,
        model=model,
        role_models=role_models,
    )
    if not created:
        console.print(f"[yellow]DevCouncil is already initialized at {root / '.devcouncil'}.[/yellow]")

    if provider:
        try:
            _set_model_provider(root, provider)
        except ValueError as e:
            console.print(f"[red]{e}[/red]")
            raise typer.Exit(code=2) from e

    _set_model_roles(root, model=model, role_models=role_models)

    configured_provider = load_config(root).models.provider
    _configure_vertexai_settings(root, configured_provider, vertex_project, vertex_location)
    _configure_api_key(root, api_key, skip_api_key)

    console.print()
    render_doctor_check(root)

    if integrate:
        _configure_coding_cli_integrations(root, apply=apply, gemini_scope=gemini_scope)
    elif created and not skip_integrations:
        _prompt_for_first_run_integrations(root, apply=True, gemini_scope=gemini_scope)

    console.print()
    console.print(Panel.fit(
        "\n".join([
            "[bold]Next commands[/bold]",
            f"Keep running DevCouncil commands in this terminal at: {root}",
            "One-command agent path:",
            "dev e2e \"Describe the implementation goal\"",
            "",
            "Manual sidecar path:",
            "dev plan \"Describe the implementation goal\"",
            "dev tasks",
            "dev run TASK-001 --executor manual",
            "dev prompt TASK-001",
            "Paste only the dev prompt output into your coding CLI.",
            "Paste only the dev prompt output into your coding CLI, or run directly:",
            "dev run TASK-001 --executor codex",
            "dev run TASK-001 --executor gemini",
            "dev run TASK-001 --executor claude",
            "dev verify TASK-001",
            "",
            "Use [bold]dev setup --integrate[/bold] to preview coding CLI MCP and native hook setup.",
            "Use [bold]dev setup --integrate --apply[/bold] to configure detected clients.",
        ]),
        title="DevCouncil is ready",
        border_style="green",
    ))
