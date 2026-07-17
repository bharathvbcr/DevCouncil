import logging
import typer
from typing import cast
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

_CONFIG_SETTABLE_KEYS = {
    "execution.default_executor": ("execution", "default_executor", str),
    "execution.max_repair_attempts": ("execution", "max_repair_attempts", int),
    "execution.command_timeout": ("execution", "command_timeout", int),
    "execution.verify_on_post_task": ("execution", "verify_on_post_task", bool),
    "execution.stop_gate.mode": ("execution", "stop_gate", "mode", str),
    "execution.stop_gate.check_claims": ("execution", "stop_gate", "check_claims", bool),
    "execution.stop_gate.verify_active_task": ("execution", "stop_gate", "verify_active_task", bool),
    "execution.stop_gate.max_blocks": ("execution", "stop_gate", "max_blocks", int),
    "verification.diff_coverage.enforce": ("verification", "diff_coverage", "enforce", bool),
    "semantic_layer.enabled": ("semantic_layer", "enabled", bool),
    "semantic_layer.cache.enabled": ("semantic_layer", "cache", "enabled", bool),
    "semantic_layer.router.enabled": ("semantic_layer", "router", "enabled", bool),
    "semantic_layer.compressor.enabled": ("semantic_layer", "compressor", "enabled", bool),
}


def _parse_config_value(raw: str, typ: type) -> object:
    if typ is bool:
        normalized = raw.strip().lower()
        if normalized in {"true", "1", "yes", "on"}:
            return True
        if normalized in {"false", "0", "no", "off"}:
            return False
        raise ValueError(f"Expected boolean (true/false), got {raw!r}")
    if typ is int:
        return int(raw)
    return raw


def _nested_get(raw: dict, path: tuple[str, ...]) -> object | None:
    current: object = raw
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return current


def _nested_set(raw: dict, path: tuple[str, ...], value: object) -> None:
    current = raw
    for key in path[:-1]:
        if key not in current or not isinstance(current[key], dict):
            current[key] = {}
        current = current[key]
    current[path[-1]] = value


@app.command("show")
def show_config(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Display key DevCouncil settings."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)

    with log_stage("config", project_root=root, subcommand="show"):
        try:
            cfg = load_config(root)
        except FileNotFoundError as e:
            console.print(f"[red]{e}[/red]")
            raise typer.Exit(code=1) from e

        console.print("[bold]DevCouncil Settings[/bold]")
        console.print(f"  [cyan]execution.default_executor[/cyan]: {cfg.execution.default_executor}")
        console.print(f"  [cyan]execution.max_repair_attempts[/cyan]: {cfg.execution.max_repair_attempts}")
        console.print(f"  [cyan]execution.command_timeout[/cyan]: {cfg.execution.command_timeout}")
        console.print(f"  [cyan]execution.verify_on_post_task[/cyan]: {cfg.execution.verify_on_post_task} [dim](deprecated alias)[/dim]")
        sg = cfg.execution.stop_gate
        console.print(f"  [cyan]execution.stop_gate.mode[/cyan]: {sg.mode}")
        console.print(f"  [cyan]execution.stop_gate.check_claims[/cyan]: {sg.check_claims}")
        console.print(f"  [cyan]execution.stop_gate.verify_active_task[/cyan]: {sg.verify_active_task}")
        console.print(f"  [cyan]execution.stop_gate.max_blocks[/cyan]: {sg.max_blocks}")
        console.print(f"  [cyan]verification.diff_coverage.enforce[/cyan]: {cfg.verification.diff_coverage.enforce}")
        console.print(f"  [cyan]verification.rigor.enabled[/cyan]: {cfg.verification.rigor.enabled}")
        console.print(f"  [cyan]verification.rigor.stub_detection[/cyan]: {cfg.verification.rigor.stub_detection}")
        console.print(f"  [cyan]gates.block_orphan_diffs[/cyan]: {cfg.gates.block_orphan_diffs}")
        console.print(f"  [cyan]semantic_layer.enabled[/cyan]: {cfg.semantic_layer.enabled}")
        if cfg.semantic_layer.enabled:
            console.print(f"  [cyan]semantic_layer.cache.enabled[/cyan]: {cfg.semantic_layer.cache.enabled}")
            console.print(f"  [cyan]semantic_layer.router.enabled[/cyan]: {cfg.semantic_layer.router.enabled}")
            console.print(f"  [cyan]semantic_layer.compressor.enabled[/cyan]: {cfg.semantic_layer.compressor.enabled}")
            console.print(
                f"  [cyan]semantic_layer.embedding.model_name[/cyan]: {cfg.semantic_layer.embedding.model_name}"
            )
            console.print(
                "  [dim]Enable: dev config set semantic_layer.enabled true "
                "(requires uv sync --group semantic). "
                "Check: dev doctor.[/dim]"
            )


@app.command("set")
def set_config(
    key: str = typer.Argument(..., help="Dotted config key (e.g. execution.command_timeout)."),
    value: str = typer.Argument(..., help="New value."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Set a common DevCouncil config key."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)

    mapping = _CONFIG_SETTABLE_KEYS.get(key)
    if mapping is None:
        supported = ", ".join(sorted(_CONFIG_SETTABLE_KEYS))
        console.print(f"[red]Unsupported key {key!r}. Supported: {supported}[/red]")
        raise typer.Exit(code=2)

    config_path = root / ".devcouncil" / "config.yaml"
    if not config_path.exists():
        console.print(f"[red]Config not found at {config_path}[/red]")
        raise typer.Exit(code=1)

    typ = cast(type, mapping[-1])
    path = cast(tuple[str, ...], mapping[:-1])
    try:
        parsed = _parse_config_value(value, typ)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=2) from exc

    with log_stage("config", project_root=root, subcommand="set"):
        with open(config_path) as f:
            raw_config = yaml.safe_load(f) or {}
        _nested_set(raw_config, path, parsed)
        with open(config_path, "w") as f:
            yaml.dump(raw_config, f, default_flow_style=False)
        console.print(f"[green]Updated {key} = {parsed!r}[/green]")


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
