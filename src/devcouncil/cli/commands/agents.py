from __future__ import annotations

import asyncio
import subprocess
import sys
from pathlib import Path

from pydantic import BaseModel, Field
import typer
from rich.console import Console
from rich.table import Table

from devcouncil.cli.commands import run as run_command
from devcouncil.cli.commands.integrate import _load_raw_config, _project_root, _save_raw_config
from devcouncil.executors.agent_registry import (
    VALID_INPUT_MODES,
    agent_config_entry,
    detect_available_coding_cli,
    is_reserved_agent_name,
    load_agent_profiles,
    load_cli_agent_specs,
    normalize_agent_name,
    resolve_automated_executor,
    resolve_cursor_agent_executable,
)

app = typer.Typer(help="Manage DevCouncil CLI agents.")
console = Console()


@app.callback(invoke_without_command=True)
def list_agents(
    ctx: typer.Context,
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """List built-in and configured CLI agents."""
    if ctx.invoked_subcommand is not None:
        return

    root = _project_root(project_root)
    table = Table(title="DevCouncil Agents")
    table.add_column("Agent", style="cyan")
    table.add_column("Type")
    table.add_column("Command")
    table.add_column("Profile")
    table.add_column("MCP")
    table.add_column("Diff Review")

    for name, spec in sorted(load_cli_agent_specs(root).items()):
        table.add_row(
            name,
            "built-in" if spec.built_in else spec.kind,
            " ".join(spec.base_command()),
            spec.default_profile,
            "yes" if spec.supports_mcp else "no",
            "yes" if spec.supports_diff_review else "no",
        )
    console.print(table)


@app.command("add")
def add_agent(
    name: str = typer.Argument(..., help="Agent name, for example opencode or aider."),
    command: str = typer.Option(..., "--command", help="Executable to launch."),
    arg: list[str] | None = typer.Option(None, "--arg", help="Argument to pass to the CLI. Repeat for multiple args."),
    input_mode: str = typer.Option("stdin", "--input-mode", help="Prompt input mode: stdin, argument, or prompt-file."),
    prompt_arg: str | None = typer.Option(None, "--prompt-arg", help="Flag used before the prompt or prompt file."),
    timeout_seconds: int | None = typer.Option(None, "--timeout-seconds", help="Agent-specific timeout override."),
    display_name: str | None = typer.Option(None, "--display-name", help="Human-readable agent name."),
    kind: str = typer.Option("custom", "--kind", help="Agent kind, for example coding-cli or review-cli."),
    supports_mcp: bool = typer.Option(False, "--supports-mcp", help="Mark this agent as MCP-capable."),
    supports_diff_review: bool = typer.Option(False, "--supports-diff-review", help="Mark this agent as able to review diffs."),
    default_profile: str = typer.Option("default", "--default-profile", help="Default execution profile for this agent."),
    help_arg: list[str] | None = typer.Option(None, "--help-arg", help="Argument for the agent help command. Repeat for multiple args."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Register an arbitrary prompt-taking CLI as a DevCouncil agent."""
    if input_mode not in VALID_INPUT_MODES:
        console.print("[red]--input-mode must be one of: stdin, argument, prompt-file.[/red]")
        raise typer.Exit(code=2)
    if not name.strip():
        console.print("[red]Agent name cannot be empty.[/red]")
        raise typer.Exit(code=2)
    if not command.strip():
        console.print("[red]--command cannot be empty.[/red]")
        raise typer.Exit(code=2)

    root = _project_root(project_root)
    if is_reserved_agent_name(name):
        console.print(f"[red]'{name}' is reserved for a built-in DevCouncil agent.[/red]")
        raise typer.Exit(code=2)
    if default_profile not in load_agent_profiles(root):
        console.print(f"[red]Unknown --default-profile '{default_profile}'.[/red]")
        raise typer.Exit(code=2)

    normalized = normalize_agent_name(name)
    entry = agent_config_entry(
        command=command,
        args=arg or [],
        input_mode=input_mode,
        prompt_arg=prompt_arg,
        timeout_seconds=timeout_seconds,
        display_name=display_name,
        kind=kind,
        supports_mcp=supports_mcp,
        supports_diff_review=supports_diff_review,
        default_profile=default_profile,
        help_command=[command, *(help_arg or [])] if help_arg else [],
    )
    config = _load_raw_config(root)
    agents = config.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})
    agents[normalized] = entry
    _save_raw_config(root, config)
    console.print(f"[green]Registered CLI agent '{normalized}' in .devcouncil/config.yaml.[/green]")


@app.command("doctor")
def doctor(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Check configured CLI agents and execution profiles."""
    root = _project_root(project_root)
    profiles = load_agent_profiles(root)
    table = Table(title="DevCouncil Agent Doctor")
    table.add_column("Agent", style="cyan")
    table.add_column("Status")
    table.add_column("Details", no_wrap=True)

    for name, spec in sorted(load_cli_agent_specs(root).items()):
        if spec.name == "cursor":
            executable = resolve_cursor_agent_executable()
        else:
            executable = _which(spec.executable)
        mode_ok = spec.input_mode in VALID_INPUT_MODES
        profile_ok = spec.default_profile in profiles
        help_ok, help_detail = _check_help(spec.help_command or [spec.executable, "--help"])

        if executable and mode_ok and profile_ok:
            status = "[green]OK[/green]"
        elif executable:
            status = "[red]Invalid[/red]"
        else:
            status = "[yellow]Missing[/yellow]"

        details = []
        details.append(executable or f"{spec.executable} not found on PATH")
        if not mode_ok:
            details.append(f"invalid input_mode={spec.input_mode}")
        if not profile_ok:
            details.append(f"missing profile={spec.default_profile}")
        if help_ok:
            details.append("help command OK")
        elif not spec.built_in:
            details.append(help_detail)
        table.add_row(name, status, "; ".join(details))

    console.print(table)
    detected = detect_available_coding_cli(root)
    if detected:
        resolved = resolve_automated_executor(root, None)
        console.print(
            f"\n[dim]Auto-pick for dev go / dev run:[/dim] [cyan]{resolved}[/cyan] "
            f"(first built-in CLI on PATH in probe order)"
        )
    else:
        console.print("\n[dim]No built-in coding CLI on PATH for auto-pick.[/dim]")


@app.command("run")
def run_agent(
    task_id: str = typer.Argument(..., help="ID of the task to run."),
    agent: str = typer.Option(..., "--agent", "-a", help="Agent name to execute."),
    profile: str | None = typer.Option(None, "--profile", help="Execution profile: default, yolo, prod, or configured."),
    stream: bool = typer.Option(
        False,
        "--stream",
        help="Stream coding CLI stdout/stderr live (also enabled by execution.stream_cli_output).",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Run a DevCouncil task with a named CLI agent and profile."""
    run_command.run(task_id, executor=agent, profile=profile, stream=stream, project_root=project_root)


def _which(command: str) -> str | None:
    from shutil import which

    return which(command)


def _check_help(command: list[str]) -> tuple[bool, str]:
    executable = _which(command[0]) if command else None
    if not command or not executable:
        return False, "help command unavailable"
    resolved = [executable, *command[1:]]
    use_shell = sys.platform == "win32" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
    try:
        result = subprocess.run(
            subprocess.list2cmdline(resolved) if use_shell else resolved,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=5,
            shell=use_shell,
        )
    except subprocess.TimeoutExpired:
        return False, "help command timed out"
    return result.returncode == 0, f"help command exited {result.returncode}"


class GepaMutation(BaseModel):
    reflection: str = Field(description="Explanation of why the previous run failed and what needs to be improved in the prompt preamble.")
    optimized_preamble: str = Field(description="The updated prompt preamble containing instructions for the agent.")


async def run_optimize_flow(
    task_id: str,
    agent: str,
    profile_name: str,
    iterations: int,
    project_root: Path,
) -> None:
    from devcouncil.app.config import get_api_key, load_config
    from devcouncil.executors.agent_registry import get_cli_agent_spec
    from devcouncil.executors.coding_cli import CodingCliExecutor
    from devcouncil.llm.provider import create_provider, validate_model_provider
    from devcouncil.llm.router import ModelRouter
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import RequirementRepository, TaskRepository
    from devcouncil.verification.verifier import Verifier

    root = project_root.expanduser().resolve()
    db = get_db(root)
    if not db:
        console.print("[red]DevCouncil state is unavailable in this directory.[/red]")
        raise typer.Exit(code=1)

    config = load_config(root)
    try:
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, root)
    except ValueError as e:
        console.print(f"[red]LLM provider configuration error: {e}[/red]")
        raise typer.Exit(code=1)

    provider = create_provider(config.models.provider, api_key, project_root=root)
    role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
    router = ModelRouter(provider, role_config, project_root=root)

    with db.get_session() as session:
        task_repo = TaskRepository(session)
        task = task_repo.get_by_id(task_id)
        if not task:
            console.print(f"[red]Task {task_id} not found.[/red]")
            raise typer.Exit(code=1)

        req_repo = RequirementRepository(session)
        reqs = req_repo.get_all()

    normalized_agent = normalize_agent_name(agent)
    spec = get_cli_agent_spec(root, normalized_agent)
    if not spec:
        console.print(f"[red]Agent '{agent}' is not registered or supported.[/red]")
        raise typer.Exit(code=1)

    profiles = load_agent_profiles(root)
    profile_cfg = profiles.get(profile_name)
    if not profile_cfg:
        console.print(f"[red]Profile '{profile_name}' not found for agent '{normalized_agent}'.[/red]")
        raise typer.Exit(code=1)

    # Check if git is dirty and stash
    is_dirty = False
    try:
        status_out = subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True).strip()
        if status_out:
            is_dirty = True
    except Exception:
        pass

    if is_dirty:
        console.print("[yellow]Working directory is dirty. Stashing changes to ensure clean baseline...[/yellow]")
        subprocess.run(["git", "stash", "-u"], cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    current_preamble = profile_cfg.prompt_preamble or ""
    best_preamble = current_preamble
    best_score = -1
    best_success = False

    console.print(f"\n[bold]Starting GEPA optimization loop for agent {normalized_agent.upper()} ({profile_name} profile)...[/bold]")
    console.print(f"Seed Prompt Preamble: {repr(current_preamble)}\n")

    try:
        for iter_idx in range(1, iterations + 1):
            console.print(f"[bold cyan]--- Iteration {iter_idx} of {iterations} ---[/bold cyan]")
            console.print(f"Preamble: {repr(current_preamble)}")

            # Update prompt_preamble temporarily in config.yaml
            temp_raw_config = _load_raw_config(root)
            integrations = temp_raw_config.setdefault("integrations", {})
            cli_agents = integrations.setdefault("cli_agents", {})
            profiles_dict = cli_agents.setdefault("profiles", {})
            profile_data = profiles_dict.setdefault(profile_name, {})
            profile_data["prompt_preamble"] = current_preamble
            _save_raw_config(root, temp_raw_config)

            # Rollout
            console.print("Running agent rollout...")
            cli_executor = CodingCliExecutor(root, normalized_agent, profile=profile_name)
            exec_result = cli_executor.run_task(task, reqs)

            # Verification
            console.print("Running verification...")
            verifier = Verifier(root, router=router)
            gaps, evidence = await verifier.verify_task(task, reqs)

            blocking_gaps = [g for g in gaps if g.blocking]
            non_blocking_gaps = [g for g in gaps if not g.blocking]

            exit_code = 0 if exec_result.success else -1
            success = exec_result.success and len(blocking_gaps) == 0

            if not exec_result.success:
                score = 0
            else:
                score = 100 - (len(blocking_gaps) * 20) - (len(non_blocking_gaps) * 5)
                score = max(0, score)

            console.print(f"Rollout Success: [bold]{exec_result.success}[/bold], Exit Code: {exit_code}")
            console.print(f"Verification Success: [bold]{len(blocking_gaps) == 0}[/bold], Gaps Found: {len(gaps)} ({len(blocking_gaps)} blocking)")
            console.print(f"Iteration Score: [bold green]{score}/100[/bold green]\n")

            if score > best_score or (score == best_score and success and not best_success):
                best_score = score
                best_preamble = current_preamble
                best_success = success

            # Clean up working tree for next iteration
            console.print("Cleaning repository state for next run...")
            subprocess.run(["git", "reset", "--hard", "HEAD"], cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["git", "clean", "-fd"], cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

            # If we achieved 100% success and 0 gaps, we can stop early!
            if best_success and best_score == 100:
                console.print("[bold green]Success! Achieved perfect score (100/100). Stopping early.[/bold green]")
                break

            if iter_idx == iterations:
                break

            # Reflection and Mutation using critic LLM
            gaps_desc = ""
            for idx, g in enumerate(gaps, 1):
                gaps_desc += f"{idx}. [{g.severity.upper()}] {g.description} (Recommended Fix: {g.recommended_fix})\n"
            if not gaps_desc:
                gaps_desc = "No verification gaps reported, but agent CLI run failed."

            log_content = ""
            log_path = root / ".devcouncil" / "logs" / f"{task.id}-{normalized_agent}.log"
            if log_path.exists():
                try:
                    log_content = log_path.read_text(encoding="utf-8")
                    if len(log_content) > 1500:
                        log_content = "...[truncated]...\n" + log_content[-1500:]
                except Exception:
                    pass

            reqs_str = "\n".join([f"- {r.title}" for r in reqs])
            messages = [
                {
                    "role": "system",
                    "content": (
                        "You are the GEPA (Generative Evaluation and Prompt Adaptation) prompt optimization assistant. "
                        "You analyze agent run failures and optimize prompt preambles."
                    )
                },
                {
                    "role": "user",
                    "content": f"""Optimize the prompt preamble for the agent CLI: {normalized_agent}.

Task Goal:
{task.description}

Active Requirements:
{reqs_str}

Current Preamble:
---
{current_preamble}
---

Rollout Outcome:
Exit Code: {exit_code}

Execution Log Preview:
---
{log_content}
---

Verification Gaps:
{gaps_desc}

Please reflect on why the agent failed. Then, propose a mutated and optimized prompt preamble.
Focus on instructions that would guide the agent to correct the specific issues mentioned in the verification gaps and logs.
Ensure the updated preamble is concise and directly actionable by the agent. Keep any general profile constraints from the current preamble if they are still relevant.
"""
                }
            ]

            try:
                mutation = await router.complete_structured(
                    role="critic_a",
                    messages=messages,
                    schema=GepaMutation,
                    temperature=0.7
                )
                current_preamble = mutation.optimized_preamble
                console.print(f"[bold green]LLM Reflection:[/bold green] {mutation.reflection}")
                console.print(f"[bold green]Mutated Preamble:[/bold green] {repr(current_preamble)}\n")
            except Exception as e:
                console.print(f"[red]Failed to run LLM reflection: {e}[/red]")
                break

    finally:
        # Restore configuration with the best preamble found
        console.print("\n[bold green]Optimization complete![/bold green]")
        console.print(f"Best Preamble Found (Score {best_score}/100): {repr(best_preamble)}")

        final_raw_config = _load_raw_config(root)
        integrations = final_raw_config.setdefault("integrations", {})
        cli_agents = integrations.setdefault("cli_agents", {})
        profiles_dict = cli_agents.setdefault("profiles", {})
        profile_data = profiles_dict.setdefault(profile_name, {})
        profile_data["prompt_preamble"] = best_preamble
        _save_raw_config(root, final_raw_config)
        console.print(f"[green]Saved best preamble to profile '{profile_name}' in .devcouncil/config.yaml.[/green]")

        if is_dirty:
            console.print("[yellow]Restoring stashed changes...[/yellow]")
            subprocess.run(["git", "stash", "pop"], cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


@app.command("optimize")
def optimize_agent(
    task_id: str = typer.Argument(..., help="ID of the task to run for optimization."),
    agent: str = typer.Option(..., "--agent", "-a", help="Agent name to execute/optimize."),
    profile_name: str = typer.Option("default", "--profile", help="The profile name to optimize (e.g. default, yolo, prod)."),
    iterations: int = typer.Option(3, "--iterations", "-i", help="Number of optimization cycles/iterations."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Optimize an agent's prompt preamble using GEPA (Genetic-Pareto / reflective evolution).
    """
    asyncio.run(
        run_optimize_flow(
            task_id=task_id,
            agent=agent,
            profile_name=profile_name,
            iterations=iterations,
            project_root=project_root,
        )
    )
