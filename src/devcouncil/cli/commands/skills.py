import asyncio
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from devcouncil.knowledge.frontmatter import build_frontmatter_markdown
from devcouncil.skills.registry import Skill, get_skill, load_skills, scaffold_skills, select_skills

app = typer.Typer(help="Inspect and scaffold DevCouncil engineering skills for coding agents.")
console = Console()


def _is_repo_skill(skill, project_root: Path) -> bool:
    if skill.source_path is None:
        return False
    try:
        skill.source_path.resolve().relative_to(project_root.resolve())
        return True
    except ValueError:
        return False


@app.callback(invoke_without_command=True)
def skills(
    ctx: typer.Context,
    goal: str = typer.Option("", "--goal", help="Optional goal text; highlights the skills that would apply."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root used for file-based skill triggers."),
):
    """List available skills and show which apply to this repository/goal."""
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    all_skills = load_skills(project_root=root)
    if not all_skills:
        console.print("[yellow]No skills found in the DevCouncil skills library.[/yellow]")
        raise typer.Exit()

    selected = {skill.name for skill in select_skills(goal, root)}
    table = Table(title="DevCouncil Skills")
    table.add_column("Skill", style="cyan")
    table.add_column("Source", justify="center")
    table.add_column("Applies", justify="center")
    table.add_column("Description")
    for skill in all_skills:
        applies = "always" if skill.always else ("yes" if skill.name in selected else "-")
        style = "green" if skill.name in selected else "dim"
        source = "repo" if _is_repo_skill(skill, root) else "library"
        table.add_row(skill.name, source, f"[{style}]{applies}[/{style}]", skill.description)
    console.print(table)
    console.print(
        "\nScaffold the applicable skills into this repo with: "
        "[bold]dev skills scaffold[/bold] (add a goal to widen selection, or --all)."
    )


@app.command("show")
def show(
    name: str = typer.Argument(..., help="Skill name, e.g. core-engineering or android."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root, so repo-local skills are found too."),
):
    """Print the full body of a single skill."""
    skill = get_skill(name, project_root=project_root.expanduser().resolve())
    if skill is None:
        console.print(f"[red]No skill named '{name}'. Run 'dev skills' to list available skills.[/red]")
        raise typer.Exit(code=1)
    console.print(skill.to_skill_md())


@app.command("scaffold")
def scaffold(
    goal: str = typer.Argument("", help="Optional goal text used to widen domain-skill selection."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root to scaffold skills into."),
    all_skills: bool = typer.Option(False, "--all", help="Scaffold every skill, not just the ones that apply."),
):
    """Write the applicable skills into <repo>/.claude/skills/<name>/SKILL.md."""
    root = project_root.expanduser().resolve()
    chosen = load_skills(project_root=root) if all_skills else select_skills(goal, root)
    written = scaffold_skills(root, chosen)
    if not written:
        console.print(
            f"[green]Skills already up to date in {root / '.claude' / 'skills'} "
            f"({len(chosen)} applicable).[/green]"
        )
        return
    console.print(f"[green]Wrote {len(written)} skill file(s):[/green]")
    for path in written:
        console.print(f"  {path.relative_to(root).as_posix()}")


def _skill_to_markdown(skill: Skill, body: str) -> str:
    """Render a skill back to markdown, preserving its selection frontmatter."""
    meta: dict[str, object] = {"name": skill.name}
    if skill.title:
        meta["title"] = skill.title
    if skill.description:
        meta["description"] = skill.description
    if skill.always:
        meta["always"] = True
    triggers = {
        k: v
        for k, v in {"keywords": skill.triggers.keywords, "globs": skill.triggers.globs}.items()
        if v
    }
    if triggers:
        meta["triggers"] = triggers
    return build_frontmatter_markdown(meta, body)


def _write_skill_body(project_root: Path, skill: Skill, body: str) -> Path:
    """Persist an optimized skill body, overwriting a repo-local skill in place or
    materializing a packaged-library skill under ``.devcouncil/skills/<name>.md``."""
    content = _skill_to_markdown(skill, body)
    if skill.source_path is not None:
        try:
            skill.source_path.resolve().relative_to(project_root.resolve())
            skill.source_path.write_text(content, encoding="utf-8")
            return skill.source_path
        except ValueError:
            pass
    target = project_root / ".devcouncil" / "skills" / f"{skill.name}.md"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")
    return target


def _build_router(project_root: Path):
    """Build a ModelRouter from project config, adding SkillOpt roles when absent."""
    from devcouncil.app.config import get_api_key, load_config
    from devcouncil.llm.provider import create_provider
    from devcouncil.llm.router import ModelRouter

    config = load_config(project_root)
    api_key = get_api_key(config.models.provider, project_root)
    provider = create_provider(config.models.provider, api_key, project_root=project_root)
    role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
    if not role_config:
        raise RuntimeError(
            "No model roles configured in .devcouncil/config.yaml. "
            "Run 'dev init' or add a 'models.roles' entry before optimizing."
        )
    # SkillOpt's rollout/optimizer roles fall back to a capable existing role when the
    # project config doesn't define dedicated ones. Copy the dict so the three roles
    # don't alias one config object.
    capable = role_config.get("arbiter") or role_config.get("planner_a") or next(iter(role_config.values()))
    role_config.setdefault("skill_target", dict(capable))
    role_config.setdefault("skill_optimizer", dict(capable))
    return ModelRouter(provider, role_config, project_root=project_root)


@app.command("optimize")
def optimize(
    name: str = typer.Argument(..., help="Skill name to optimize, e.g. core-engineering."),
    evals_path: Path = typer.Option(..., "--evals", help="JSON or JSONL dataset of evaluation tasks."),
    profile_name: str = typer.Option(
        "default", "--profile", help="Agent profile whose prompt preamble (guidance) is co-optimized."
    ),
    epochs: int = typer.Option(5, "--epochs", min=1, help="Optimization epochs."),
    max_edits: int = typer.Option(3, "--max-edits", min=1, help="Edit budget per epoch (textual learning rate)."),
    val_fraction: float = typer.Option(0.5, "--val-fraction", min=0.0, max=1.0, help="Held-out validation fraction."),
    seed: int = typer.Option(0, "--seed", help="Seed for the deterministic train/validation split."),
    apply: bool = typer.Option(
        False,
        "--apply/--dry-run",
        help="Write the optimized skill body and guidance preamble back to disk. Defaults to dry-run.",
    ),
    output_path: Path | None = typer.Option(None, "--output", help="Write the optimization artifact to this path."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Co-optimize a skill document and its agent guidance preamble with the SkillOpt loop.

    Each epoch runs the skill+guidance on training tasks, scores the rollouts, and lets an
    optimizer model propose bounded edits to **both** documents at once; a candidate is kept
    only if it strictly improves the held-out validation score.
    """
    from devcouncil.executors.agent_registry import load_agent_profiles
    from devcouncil.optimization.gepa_agent import load_agent_eval_dataset
    from devcouncil.optimization.skillopt import (
        GUIDANCE,
        SKILL,
        DEFAULT_OBJECTIVE,
        SkillOptConfig,
        default_artifact_path,
        make_llm_optimizer,
        make_llm_rollout,
        optimize_skill,
        write_result_artifact,
    )
    from devcouncil.optimization.gepa_agent import _apply_profile_preamble

    root = project_root.expanduser().resolve()
    skill = get_skill(name, project_root=root)
    if skill is None:
        console.print(f"[red]No skill named '{name}'. Run 'dev skills' to list available skills.[/red]")
        raise typer.Exit(code=1)

    resolved_evals = evals_path.expanduser()
    if not resolved_evals.is_absolute():
        resolved_evals = root / resolved_evals
    try:
        dataset = load_agent_eval_dataset(resolved_evals)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=2) from exc

    profiles = load_agent_profiles(root)
    profile = profiles.get(profile_name)
    if profile is None:
        known = ", ".join(sorted(profiles)) or "(none)"
        console.print(
            f"[red]No agent profile named '{profile_name}'. Known profiles: {known}.[/red]"
        )
        raise typer.Exit(code=2)
    guidance = profile.prompt_preamble or ""

    try:
        router = _build_router(root)
    except (RuntimeError, ValueError, FileNotFoundError) as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc

    rollout = make_llm_rollout(router)
    optimizer = make_llm_optimizer(router)
    result = asyncio.run(
        optimize_skill(
            skill_name=skill.name,
            docs={GUIDANCE: guidance, SKILL: skill.body},
            dataset=dataset,
            rollout=rollout,
            optimizer=optimizer,
            config=SkillOptConfig(
                epochs=epochs, max_edits_per_epoch=max_edits, val_fraction=val_fraction, seed=seed
            ),
        )
    )

    artifact_path = (output_path or default_artifact_path(root, skill.name)).expanduser()
    if not artifact_path.is_absolute():
        artifact_path = root / artifact_path
    result.artifact_path = artifact_path
    result.applied = apply
    write_result_artifact(
        artifact_path, result, objective=DEFAULT_OBJECTIVE, dataset_path=str(resolved_evals)
    )

    if apply and result.improved:
        # Only write a document that actually changed, so a guidance-only improvement
        # doesn't churn the skill file (and vice versa).
        if result.best_skill_body != skill.body:
            skill_path = _write_skill_body(root, skill, result.best_skill_body)
            console.print(f"[green]Updated skill body:[/green] {skill_path.relative_to(root).as_posix()}")
        if result.best_guidance_body != guidance:
            _apply_profile_preamble(root, profile_name, result.best_guidance_body)
            console.print(f"[green]Updated guidance preamble for profile '{profile_name}'.[/green]")

    mode = "applied" if (apply and result.improved) else "dry-run"
    console.print(
        f"[green]SkillOpt complete ({mode}) for '{skill.name}'.[/green] "
        f"validation {result.seed_val_score:.3f} -> {result.best_val_score:.3f} "
        f"over {len(result.epochs)} epoch(s), "
        f"{result.accepted_edit_count} edit(s) accepted, {result.rejected_edit_count} rejected."
    )
    console.print(f"Artifact: [dim]{artifact_path}[/dim]")
    if apply and not result.improved:
        console.print("[yellow]No validated improvement — nothing written. Re-run with more epochs or data.[/yellow]")
