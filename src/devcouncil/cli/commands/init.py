import copy
from typing import Any
import typer
import yaml
from rich.console import Console
from pathlib import Path
from devcouncil.storage.db import Database
from devcouncil.integrations.gitnexus import GitNexusIntegration
from devcouncil.integrations.graphify import GraphifyIntegration
from devcouncil.llm.provider import build_role_model_config, validate_model_provider

app = typer.Typer()
console = Console()

DEFAULT_CONFIG = {
    "project": {
        "name": "devcouncil-project",
        "root": ".",
        "default_branch": "main",
    },
    "models": {
        "provider": "openrouter",
        "roles": build_role_model_config("openrouter"),
    },
    "commands": {
        "test": ["pytest", "npm test"],
        "lint": ["flake8", "eslint"],
        "typecheck": ["mypy", "tsc"]
    },
    "gates": {
        "require_clean_git_before_task": True,
        "block_orphan_diffs": True,
        "block_missing_tests_for_high_requirements": True,
        "block_dependency_changes_without_approval": True,
        "block_schema_change_without_migration": True,
        "block_failed_commands": True
    },
    "execution": {
        "default_executor": "manual",
        "max_repair_attempts": 3,
        "checkpoint_before_each_task": True
    },
    "privacy": {
        "redact_env_vars": True,
        "redact_secrets_in_logs": True,
        "store_prompts_locally": True
    },
    "integrations": {
        "agent_flow": {
            "enabled": False,
            "trace_path": ".devcouncil/logs/traces.jsonl",
            "mode": "jsonl",
        },
        "code_review_graph": {
            "enabled": False,
            "command": "code-review-graph",
            "optional": True,
        },
        "live_review": {
            "enabled": True,
            "cards_path": ".devcouncil/live/cards",
            "signals_path": ".devcouncil/live/signals",
            "default_client": "claude",
        },
        "cli_agents": {
            "enabled": True,
            "profiles": {
                "default": {
                    "description": "Balanced local execution with DevCouncil verification.",
                },
                "yolo": {
                    "description": "Faster local execution; DevCouncil still verifies the final diff.",
                    "timeout_seconds": 3600,
                    "prompt_preamble": "Profile: yolo. Move efficiently within the task scope.",
                },
                "prod": {
                    "description": "Restrictive execution for high-risk repositories.",
                    "timeout_seconds": 1800,
                    "prompt_preamble": "Profile: prod. Keep edits minimal and explicitly within task scope.",
                    "require_explicit_confirmation": True,
                },
            },
            "agents": {},
        },
    }
}


def parse_role_model_overrides(values: list[str] | None) -> dict[str, str]:
    overrides: dict[str, str] = {}
    for value in values or []:
        if "=" not in value:
            raise ValueError(f"Invalid --role-model value '{value}'. Use ROLE=MODEL.")
        role, model = value.split("=", 1)
        role = role.strip()
        model = model.strip()
        if not role or not model:
            raise ValueError(f"Invalid --role-model value '{value}'. Use ROLE=MODEL.")
        overrides[role] = model
    return overrides


def initialize_project(
    project_root: Path = Path("."),
    project_name: str | None = None,
    model_provider: str = "openrouter",
    model: str | None = None,
    role_models: dict[str, str] | None = None,
    with_gitnexus: bool = False,
    with_graphify: bool = False,
    quiet: bool = False,
) -> bool:
    """Initialize DevCouncil project state.

    Returns True when a fresh .devcouncil directory was created.
    """
    project_root = project_root.resolve()
    dev_dir = project_root / ".devcouncil"
    created = False

    if not dev_dir.exists():
        if not quiet:
            console.print("Initializing DevCouncil...")
        dev_dir.mkdir(exist_ok=True)
        (dev_dir / "runs").mkdir(exist_ok=True)
        (dev_dir / "cache").mkdir(exist_ok=True)
        (dev_dir / "checkpoints").mkdir(exist_ok=True)
        (dev_dir / "logs").mkdir(exist_ok=True)

        config_path = dev_dir / "config.yaml"
        config: dict[str, Any] = copy.deepcopy(DEFAULT_CONFIG)
        if project_name:
            config["project"]["name"] = project_name
        else:
            config["project"]["name"] = project_root.name
        provider = validate_model_provider(model_provider)
        config["models"]["provider"] = provider
        config["models"]["roles"] = build_role_model_config(
            provider,
            model=model,
            role_models=role_models,
        )

        with open(config_path, "w") as f:
            yaml.dump(config, f, default_flow_style=False)

        db = Database(dev_dir / "state.sqlite")
        db.create_db_and_tables()
        if not quiet:
            console.print(f"[green]Successfully initialized DevCouncil in {dev_dir}[/green]")
        created = True

    if with_gitnexus:
        nexus = GitNexusIntegration(project_root)
        nexus.initialize()

    if with_graphify:
        graphify = GraphifyIntegration(project_root)
        graphify.initialize()

    return created


@app.callback(invoke_without_command=True)
def init(
    ctx: typer.Context,
    project_name: str = typer.Option(None, "--name", "-n", help="Project name"),
    provider: str = typer.Option("openrouter", "--provider", help="Model provider for generated config."),
    model: str | None = typer.Option(None, "--model", "-m", help="Model id to use for every default role."),
    role_model: list[str] | None = typer.Option(
        None,
        "--role-model",
        help="Per-role model override in ROLE=MODEL form. Can be repeated.",
    ),
    with_gitnexus: bool = typer.Option(False, "--gitnexus", help="Initialize GitNexus structural awareness"),
    with_graphify: bool = typer.Option(False, "--graphify", help="Initialize Graphify knowledge graph engine"),
):
    """
    Initialize DevCouncil in the current directory.
    """
    if ctx.invoked_subcommand is not None:
        return

    dev_dir = Path(".devcouncil")
    if dev_dir.exists() and not (with_gitnexus or with_graphify):
        console.print("[yellow]DevCouncil is already initialized in this directory.[/yellow]")
        console.print("Use --gitnexus or --graphify to add upgrade paths.")
        raise typer.Exit()

    try:
        role_models = parse_role_model_overrides(role_model)
        model_provider = validate_model_provider(provider)
    except ValueError as e:
        console.print(f"[red]{e}[/red]")
        raise typer.Exit(code=2) from e

    initialize_project(
        Path("."),
        project_name=project_name,
        model_provider=model_provider,
        model=model,
        role_models=role_models,
        with_gitnexus=with_gitnexus,
        with_graphify=with_graphify,
    )
