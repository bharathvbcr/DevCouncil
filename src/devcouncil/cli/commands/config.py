import logging
import typer
import yaml
from pathlib import Path
from rich.console import Console
from devcouncil.cli.commands.init import parse_role_model_overrides
from devcouncil.app.config import load_config
from devcouncil.llm.provider import (
    SUPPORTED_MODEL_PROVIDERS,
    apply_provider_default_role_models,
    build_role_model_config,
    validate_model_provider,
)
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(help="Manage DevCouncil configuration")
console = Console()
logger = logging.getLogger(__name__)

@app.command("models")
def models(
    role: str = typer.Option(None, "--role", "-r", help="Specific role to show/edit"),
    model: str = typer.Option(None, "--model", "-m", help="New model string to set for the role, or every role when --role is omitted."),
    provider: str = typer.Option(None, "--provider", help="Set the model provider."),
    role_model: list[str] | None = typer.Option(
        None,
        "--role-model",
        help="Per-role model override in ROLE=MODEL form. Can be repeated.",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """View or edit model role configuration."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev config models: role=%s provider=%s", role, provider)

    with log_stage("config", project_root=root, subcommand="models"):
        log_step("config/1: loading configuration", project_root=root, trace=True)
        try:
            load_config(root)
        except FileNotFoundError as e:
            console.print(f"[red]{e}[/red]")
            return

        config_path = root / ".devcouncil" / "config.yaml"

        with open(config_path) as f:
            raw_config = yaml.safe_load(f) or {}

        try:
            role_models = parse_role_model_overrides(role_model)
        except ValueError as e:
            console.print(f"[red]{e}[/red]")
            raise typer.Exit(code=2) from e

        if provider:
            try:
                normalized_provider = validate_model_provider(provider)
            except ValueError as e:
                console.print(f"[red]{e}[/red]")
                raise typer.Exit(code=2) from e
            raw_config.setdefault("models", {})
            previous = raw_config["models"].get("provider", "openrouter")
            raw_config["models"]["provider"] = normalized_provider
            updated_role_defaults = apply_provider_default_role_models(raw_config, previous, normalized_provider)
            with open(config_path, "w") as f:
                yaml.dump(raw_config, f, default_flow_style=False)
            if previous == normalized_provider:
                console.print(f"[green]Model provider remains '{normalized_provider}'.[/green]")
            else:
                console.print(f"[green]Updated model provider from '{previous}' to '{normalized_provider}'.[/green]")
            if updated_role_defaults:
                console.print(f"[green]Updated default role models for '{normalized_provider}'.[/green]")
            if not model and not role_models:
                log_step("config/complete", project_root=root, trace=True)
                return

        if not role and (model or role_models):
            raw_config.setdefault("models", {})
            configured_provider = validate_model_provider(raw_config["models"].get("provider", "openrouter"))
            raw_config["models"]["roles"] = build_role_model_config(
                configured_provider,
                model=model,
                role_models=role_models,
            )
            with open(config_path, "w") as f:
                yaml.dump(raw_config, f, default_flow_style=False)
            if model:
                console.print(f"[green]Updated all model roles to use '{model}'.[/green]")
            for selected_role, selected_model in role_models.items():
                console.print(f"[green]Updated '{selected_role}' to use model '{selected_model}'.[/green]")
            log_step("config/complete", project_root=root, trace=True)
            return

        if not role:
            console.print("[bold]Model Configuration[/bold]")
            configured_provider = raw_config.get("models", {}).get("provider", "openrouter")
            supported = ", ".join(SUPPORTED_MODEL_PROVIDERS)
            console.print(f"  [cyan]provider[/cyan]: {configured_provider} (supported: {supported})")
            for r, m in raw_config.get("models", {}).get("roles", {}).items():
                console.print(f"  [cyan]{r}[/cyan]: {m.get('model')}")
            log_step("config/complete", project_root=root, trace=True)
            return

        if not model:
            m = raw_config.get("models", {}).get("roles", {}).get(role)
            if m:
                console.print(f"[cyan]{role}[/cyan]: {m.get('model')}")
            else:
                console.print(f"[red]Role '{role}' not found.[/red]")
            log_step("config/complete", project_root=root, trace=True)
            return

        if "models" not in raw_config:
            raw_config["models"] = {"roles": {}}
        if "roles" not in raw_config["models"]:
            raw_config["models"]["roles"] = {}

        if role not in raw_config["models"]["roles"]:
            raw_config["models"]["roles"][role] = {}

        raw_config["models"]["roles"][role]["model"] = model

        with open(config_path, "w") as f:
            yaml.dump(raw_config, f, default_flow_style=False)

        console.print(f"[green]Updated '{role}' to use model '{model}'[/green]")
        log_step("config/complete", project_root=root, trace=True)
