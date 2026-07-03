import copy
import logging
from typing import Any
import typer
import yaml
from rich.console import Console
from pathlib import Path
from devcouncil.storage.db import Database
from devcouncil.integrations.gitnexus import GitNexusIntegration
from devcouncil.integrations.graphify import GraphifyIntegration
from devcouncil.llm.provider import build_role_model_config, validate_model_provider
from devcouncil.repo.gitignore import ensure_gitignore

from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)

# Per-stack default verification commands. A fresh project gets ONLY the commands
# for the stack(s) actually detected in the repo, so the verifier never inherits a
# cross-stack gate (e.g. `npm test`/`eslint`/`tsc` on a Python repo) that it would run
# as a blocking fallback and fail for tooling/stack reasons instead of a real defect.
_STACK_COMMAND_DEFAULTS: dict[str, dict[str, list[str]]] = {
    "python": {"test": ["pytest"], "lint": ["ruff check ."], "typecheck": ["mypy ."]},
    "node": {"test": ["npm test"], "lint": ["eslint ."], "typecheck": ["tsc --noEmit"]},
}


def _stack_aware_commands(project_root: Path) -> dict[str, list[str]]:
    """Default test/lint/typecheck commands scoped to the repo's detected stack(s).

    Returns empty lists when no stack is detected — empty is safe (no speculative
    fallback gates) and far better than guessing wrong-stack tools."""
    from devcouncil.repo.ci_scaffold import detect_stacks

    commands: dict[str, list[str]] = {"test": [], "lint": [], "typecheck": []}
    try:
        stacks = detect_stacks(project_root)
    except Exception:
        return commands
    for stack in sorted(stacks):
        for key, cmds in _STACK_COMMAND_DEFAULTS.get(stack, {}).items():
            for command in cmds:
                if command not in commands[key]:
                    commands[key].append(command)
    return commands


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
        "test": [],
        "lint": [],
        "typecheck": [],
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
        "checkpoint_before_each_task": True,
        "stream_cli_output": False,
        "cursor_resume_mode": "off",
        "coding_cli_probe_order": [],
    },
    "verification": {
        "rigor": {
            "enabled": True,
            # never | hard | always — "hard" blocks stub/effort findings only on hard tasks
            "stub_detection": "hard",
            "effort_heuristics": "hard",
            "enforce_coverage_on_hard": True,
            "reviewer_required_on_hard": False,
            "extra_repair_attempts_on_hard": 1,
            "min_added_lines_per_planned_file": 5,
            "acceptance_samples_on_hard": 2,
        },
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


def _generate_initial_map(project_root: Path, quiet: bool) -> None:
    """Best-effort repo map + agent guide generation on fresh init.

    Imported lazily to avoid a circular import with the map command, and wrapped
    so a mapping failure never blocks initialization.
    """
    try:
        from devcouncil.cli.commands.map import generate_map_artifacts

        generate_map_artifacts(project_root, project_root / ".devcouncil" / "repo_map.json")
        if not quiet:
            console.print("[green]Generated .devcouncil/repo_map.json and agent guides (AGENTS.md, CLAUDE.md).[/green]")
    except Exception as exc:  # mapping is best-effort, never fatal
        if not quiet:
            console.print(f"[yellow]Skipped repo map generation: {exc}. Run 'dev map' later.[/yellow]")


def _scaffold_initial_skills(project_root: Path, quiet: bool) -> None:
    """Best-effort scaffolding of applicable engineering skills into .claude/skills/.

    Always writes the core-engineering skill; adds domain skills (android, ios, web,
    ...) whose file triggers match the repository. Never fatal.
    """
    try:
        from devcouncil.skills.registry import scaffold_skills, select_skills

        selected = select_skills(project_root=project_root)
        written = scaffold_skills(project_root, selected)
        if written and not quiet:
            names = ", ".join(sorted(skill.name for skill in selected))
            console.print(f"[green]Scaffolded {len(written)} skill(s) into .claude/skills/ ({names}).[/green]")
    except Exception as exc:  # skill scaffolding is best-effort, never fatal
        if not quiet:
            console.print(f"[yellow]Skipped skill scaffolding: {exc}. Run 'dev skills scaffold' later.[/yellow]")


def initialize_project(
    project_root: Path = Path("."),
    project_name: str | None = None,
    model_provider: str = "openrouter",
    model: str | None = None,
    role_models: dict[str, str] | None = None,
    with_gitnexus: bool = False,
    with_graphify: bool = False,
    with_map: bool = True,
    with_skills: bool = True,
    quiet: bool = False,
) -> bool:
    """Initialize DevCouncil project state.

    Returns True when a fresh .devcouncil directory was created.
    """
    project_root = project_root.resolve()
    dev_dir = project_root / ".devcouncil"
    created = False

    # Detect "already initialized" by the config file, NOT the .devcouncil/ directory:
    # logging creates .devcouncil/logs/ on every CLI startup, so the directory exists
    # before init runs. Keying on config.yaml ensures a first init still writes config.
    if not (dev_dir / "config.yaml").exists():
        if not quiet:
            console.print("Initializing DevCouncil...")
        dev_dir.mkdir(exist_ok=True)
        (dev_dir / "runs").mkdir(exist_ok=True)
        (dev_dir / "cache").mkdir(exist_ok=True)
        (dev_dir / "checkpoints").mkdir(exist_ok=True)
        (dev_dir / "logs").mkdir(exist_ok=True)

        config_path = dev_dir / "config.yaml"
        config: dict[str, Any] = copy.deepcopy(DEFAULT_CONFIG)
        # Scope default verification commands to the repo's actual stack(s).
        config["commands"] = _stack_aware_commands(project_root)
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

        if with_map:
            _generate_initial_map(project_root, quiet)
        if with_skills:
            _scaffold_initial_skills(project_root, quiet)

    if with_gitnexus:
        nexus = GitNexusIntegration(project_root)
        nexus.initialize()

    if with_graphify:
        graphify = GraphifyIntegration(project_root)
        graphify.initialize()

    ensure_gitignore(project_root)
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
    skip_map: bool = typer.Option(False, "--skip-map", help="Skip generating repo_map.json and agent guides on init."),
    skip_skills: bool = typer.Option(False, "--skip-skills", help="Skip scaffolding engineering skills into .claude/skills/ on init."),
):
    """
    Initialize DevCouncil in the current directory.
    """
    if ctx.invoked_subcommand is not None:
        return

    dev_dir = Path(".devcouncil")
    # "Already initialized" means a config.yaml exists — not merely the .devcouncil/
    # directory, which the logging setup pre-creates (.devcouncil/logs/) on startup.
    if (dev_dir / "config.yaml").exists() and not (with_gitnexus or with_graphify):
        console.print("[yellow]DevCouncil is already initialized in this directory.[/yellow]")
        console.print("Use --gitnexus or --graphify to add upgrade paths.")
        raise typer.Exit()

    try:
        role_models = parse_role_model_overrides(role_model)
        model_provider = validate_model_provider(provider)
    except ValueError as e:
        console.print(f"[red]{e}[/red]")
        raise typer.Exit(code=2) from e

    root = Path(".").expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev init: provider=%s gitnexus=%s graphify=%s", provider, with_gitnexus, with_graphify)
    with log_stage("init", project_root=root, provider=provider):
        log_step("init/1: scaffolding project", project_root=root, trace=True)
        initialize_project(
            root,
            project_name=project_name,
            model_provider=model_provider,
            model=model,
            role_models=role_models,
            with_gitnexus=with_gitnexus,
            with_graphify=with_graphify,
            with_map=not skip_map,
            with_skills=not skip_skills,
        )
        log_step("init/complete", project_root=root, trace=True)
