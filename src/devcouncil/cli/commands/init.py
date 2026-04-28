import copy
import typer
import yaml
from rich.console import Console
from pathlib import Path
from devcouncil.storage.db import Database
from devcouncil.integrations.gitnexus import GitNexusIntegration
from devcouncil.integrations.graphify import GraphifyIntegration

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
        "roles": {
            "spec_writer": {"model": "anthropic/claude-3.5-sonnet"},
            "planner_a": {"model": "anthropic/claude-3.5-sonnet"},
            "planner_b": {"model": "google/gemini-pro-1.5"},
            "critic_a": {"model": "openai/gpt-4o"},
            "critic_b": {"model": "anthropic/claude-3-opus"},
            "arbiter": {"model": "openai/gpt-4o"},
            "native_agent": {"model": "anthropic/claude-3.5-sonnet"},
            "implementation_reviewer": {"model": "openai/gpt-4o"},
        }
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
        "default_executor": "native",
        "max_repair_attempts": 3,
        "checkpoint_before_each_task": True
    },
    "privacy": {
        "redact_env_vars": True,
        "redact_secrets_in_logs": True,
        "store_prompts_locally": True
    }
}

@app.callback(invoke_without_command=True)
def init(
    ctx: typer.Context,
    project_name: str = typer.Option(None, "--name", "-n", help="Project name"),
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

    if not dev_dir.exists():
        console.print("Initializing DevCouncil...")
        dev_dir.mkdir(exist_ok=True)
        (dev_dir / "runs").mkdir(exist_ok=True)
        (dev_dir / "cache").mkdir(exist_ok=True)
        (dev_dir / "checkpoints").mkdir(exist_ok=True)
        (dev_dir / "logs").mkdir(exist_ok=True)

        config_path = dev_dir / "config.yaml"
        config = copy.deepcopy(DEFAULT_CONFIG)
        if project_name:
            config["project"]["name"] = project_name
        else:
            config["project"]["name"] = Path.cwd().name

        with open(config_path, "w") as f:
            yaml.dump(config, f, default_flow_style=False)

        # Initialize DB
        db = Database(dev_dir / "state.sqlite")
        db.create_db_and_tables()
        console.print(f"[green]Successfully initialized DevCouncil in {dev_dir}[/green]")

    if with_gitnexus:
        nexus = GitNexusIntegration(Path("."))
        nexus.initialize()

    if with_graphify:
        graphify = GraphifyIntegration(Path("."))
        graphify.initialize()
