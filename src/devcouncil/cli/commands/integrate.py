import shutil
import subprocess
import sys
from pathlib import Path

import typer
import yaml
from rich.console import Console
from rich.table import Table

app = typer.Typer(help="Set up DevCouncil integrations with coding CLIs.")
setup_app = typer.Typer(help="Set up optional external companion integrations.")
app.add_typer(setup_app, name="setup")
console = Console()

SUPPORTED_TOOLS = ("codex", "gemini")


def _project_root(path: Path | None) -> Path:
    return (path or Path(".")).expanduser().resolve()


def _server_args(project_root: Path) -> list[str]:
    return ["devcouncil", "mcp-server"]


def _codex_command(project_root: Path) -> list[str]:
    return [
        "codex",
        "mcp",
        "add",
        "devcouncil",
        "--env",
        f"DEVCOUNCIL_PROJECT_ROOT={project_root}",
        "--",
        *_server_args(project_root),
    ]


def _gemini_command(project_root: Path, scope: str) -> list[str]:
    return [
        "gemini",
        "mcp",
        "add",
        "--scope",
        scope,
        "--env",
        f"DEVCOUNCIL_PROJECT_ROOT={project_root}",
        "devcouncil",
        *_server_args(project_root),
    ]


def _format_command(command: list[str]) -> str:
    return subprocess.list2cmdline(command)


def _run(command: list[str]) -> int:
    result = subprocess.run(command, text=True)
    return result.returncode


def _run_capture(command: list[str], timeout: int = 10) -> tuple[int, str]:
    executable = shutil.which(command[0])
    if not executable:
        return 127, f"{command[0]} not found on PATH"

    resolved = [executable, *command[1:]]
    use_shell = sys.platform == "win32" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
    invocation = subprocess.list2cmdline(resolved) if use_shell else resolved
    try:
        result = subprocess.run(
            invocation,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            shell=use_shell,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return 124, "timed out"
    return result.returncode, (result.stdout + result.stderr).strip()


def _config_path(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "config.yaml"


def _load_raw_config(project_root: Path) -> dict:
    path = _config_path(project_root)
    if not path.exists():
        return {}
    return yaml.safe_load(path.read_text(encoding="utf-8")) or {}


def _save_raw_config(project_root: Path, config: dict) -> None:
    path = _config_path(project_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(config, sort_keys=False), encoding="utf-8")


def _print_command(tool: str, command: list[str], apply: bool):
    if apply:
        console.print(f"[cyan]Configuring {tool} MCP integration...[/cyan]")
    else:
        console.print(f"[bold]{tool}[/bold]")
        console.print(_format_command(command), soft_wrap=True)


def _configure(tool: str, command: list[str], apply: bool) -> bool:
    executable = command[0]
    if not shutil.which(executable):
        console.print(f"[yellow]{tool} CLI not found on PATH. Install it first, then rerun this command.[/yellow]")
        console.print(_format_command(command), soft_wrap=True)
        return False

    _print_command(tool, command, apply)
    if not apply:
        return True

    code = _run(command)
    if code == 0:
        console.print(f"[green]{tool} integration configured.[/green]")
        return True

    console.print(f"[red]{tool} integration command failed with exit code {code}.[/red]")
    console.print("You can rerun it manually:")
    console.print(_format_command(command), soft_wrap=True)
    return False


@app.callback(invoke_without_command=True)
def overview(ctx: typer.Context):
    """
    Show integration options for supported coding CLIs.
    """
    if ctx.invoked_subcommand is not None:
        return

    table = Table(title="DevCouncil Coding CLI Integrations")
    table.add_column("Tool", style="cyan")
    table.add_column("Setup command", style="green")
    table.add_column("Notes")
    table.add_row("Codex CLI", "dev integrate codex --apply", "Adds DevCouncil as a stdio MCP server.")
    table.add_row("Gemini CLI", "dev integrate gemini --apply", "Adds DevCouncil as a project-scoped stdio MCP server.")
    table.add_row("Both", "dev integrate all --apply", "Runs both setup commands.")
    console.print(table)
    console.print("\nRun without [bold]--apply[/bold] to preview the exact commands first.")


@app.command("doctor")
def integrations_doctor():
    """Check optional integration tools and local client wiring prerequisites."""
    table = Table(title="DevCouncil Integration Doctor")
    table.add_column("Integration", style="cyan")
    table.add_column("Status")
    table.add_column("Notes")

    checks = [
        ("Agent Flow", "agent-flow-app", "Optional live/replay visualizer for trace JSONL."),
        ("code-review-graph", "code-review-graph", "Optional structural graph context adapter."),
        ("Claude Code", "claude", "Optional hook runtime for pre-tool-use enforcement."),
        ("Codex CLI", "codex", "Optional MCP client and headless executor companion."),
        ("Gemini CLI", "gemini", "Optional MCP client companion."),
    ]
    for label, executable, notes in checks:
        found = shutil.which(executable)
        table.add_row(label, "[green]OK[/green]" if found else "[yellow]Missing[/yellow]", found or notes)

    config = _config_path(Path("."))
    table.add_row(
        "DevCouncil config",
        "[green]OK[/green]" if config.exists() else "[red]Missing[/red]",
        str(config) if config.exists() else "Run dev init first.",
    )
    console.print(table)


@app.command("codex")
def codex(
    apply: bool = typer.Option(False, "--apply", help="Run the setup command instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Set up DevCouncil MCP tools for Codex CLI.
    """
    root = _project_root(project_root)
    command = _codex_command(root)
    ok = _configure("Codex CLI", command, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("gemini")
def gemini(
    apply: bool = typer.Option(False, "--apply", help="Run the setup command instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    scope: str = typer.Option("project", "--scope", help="Gemini MCP config scope: project or user."),
):
    """
    Set up DevCouncil MCP tools for Gemini CLI.
    """
    if scope not in {"project", "user"}:
        console.print("[red]--scope must be 'project' or 'user'.[/red]")
        raise typer.Exit(code=2)

    root = _project_root(project_root)
    command = _gemini_command(root, scope)
    ok = _configure("Gemini CLI", command, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("all")
def all_tools(
    apply: bool = typer.Option(False, "--apply", help="Run setup commands instead of printing them."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    gemini_scope: str = typer.Option("project", "--gemini-scope", help="Gemini MCP config scope: project or user."),
):
    """
    Set up DevCouncil MCP tools for every supported coding CLI found on PATH.
    """
    if gemini_scope not in {"project", "user"}:
        console.print("[red]--gemini-scope must be 'project' or 'user'.[/red]")
        raise typer.Exit(code=2)

    root = _project_root(project_root)
    results = [
        _configure("Codex CLI", _codex_command(root), apply),
        _configure("Gemini CLI", _gemini_command(root, gemini_scope), apply),
    ]
    if apply and not all(results):
        raise typer.Exit(code=1)


@app.command("check")
def check(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Check whether DevCouncil is ready to integrate with coding CLIs.
    """
    root = _project_root(project_root)
    table = Table(title="DevCouncil Integration Check")
    table.add_column("Check", style="cyan")
    table.add_column("Status", style="magenta")
    table.add_column("Details")

    failures = 0

    def add(ok: bool, name: str, details: str):
        nonlocal failures
        table.add_row(name, "[green]OK[/green]" if ok else "[red]FAIL[/red]", details)
        if not ok:
            failures += 1

    add((root / ".devcouncil").exists(), "Project state", str(root / ".devcouncil"))

    devcouncil_path = shutil.which("devcouncil")
    add(devcouncil_path is not None, "devcouncil CLI", devcouncil_path or "Install DevCouncil first.")

    code, output = _run_capture(["devcouncil", "--help"])
    add(code == 0, "devcouncil command", output.splitlines()[0] if output else "No output")

    code, output = _run_capture(["codex", "--version"])
    add(code == 0, "Codex CLI", output.splitlines()[0] if output else "Optional; install Codex to use this integration.")

    code, output = _run_capture(["gemini", "--version"])
    add(code == 0, "Gemini CLI", output.splitlines()[0] if output else "Optional; install Gemini CLI to use this integration.")

    try:
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client

        async def _list_tools() -> list[str]:
            import os

            env = os.environ.copy()
            env["DEVCOUNCIL_PROJECT_ROOT"] = str(root)
            params = StdioServerParameters(
                command=sys.executable,
                args=["-m", "devcouncil", "mcp-server"],
                cwd=str(root),
                env=env,
            )
            async with stdio_client(params) as (read, write):
                async with ClientSession(read, write) as session:
                    await session.initialize()
                    tools = await session.list_tools()
                    return [tool.name for tool in tools.tools]

        import asyncio

        tools = asyncio.run(_list_tools())
        expected = {"devcouncil_status", "devcouncil_report", "devcouncil_get_task"}
        add(expected.issubset(set(tools)), "MCP server", ", ".join(tools))
    except Exception as exc:
        add(False, "MCP server", str(exc))

    console.print(table)
    if failures:
        console.print("\n[yellow]Fix failed checks, then run:[/yellow] dev integrate all --apply")
        raise typer.Exit(code=1)

    console.print("\n[green]Ready.[/green] Run: dev integrate all --apply")


@setup_app.command("agent-flow")
def setup_agent_flow(
    apply: bool = typer.Option(False, "--apply", help="Write DevCouncil config instead of previewing."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Configure DevCouncil trace output for Agent Flow-style JSONL replay."""
    root = _project_root(project_root)
    trace_path = root / ".devcouncil" / "logs" / "traces.jsonl"
    console.print("[bold]Agent Flow setup[/bold]")
    console.print(f"Trace JSONL: {trace_path}")
    console.print("Replay/tail locally with: dev trace tail --follow")
    console.print("External visualizers can watch the trace JSONL path above.")

    if not apply:
        console.print("[yellow]Preview only. Rerun with --apply to record this integration in config.[/yellow]")
        return

    config = _load_raw_config(root)
    integrations = config.setdefault("integrations", {})
    integrations["agent_flow"] = {
        "enabled": True,
        "trace_path": str(trace_path),
        "mode": "jsonl",
    }
    _save_raw_config(root, config)
    docs_dir = root / ".devcouncil" / "integrations"
    docs_dir.mkdir(parents=True, exist_ok=True)
    (docs_dir / "agent-flow.md").write_text(
        "\n".join([
            "# Agent Flow",
            "",
            f"DevCouncil writes trace events to `{trace_path}`.",
            "",
            "Local replay:",
            "",
            "```bash",
            "dev trace tail --follow",
            "```",
            "",
            "External visualizers can watch the JSONL file directly. DevCouncil does not modify global editor or Claude Code settings from this setup command.",
            "",
        ]),
        encoding="utf-8",
    )
    console.print("[green]Agent Flow trace integration recorded in .devcouncil/config.yaml.[/green]")


@setup_app.command("code-review-graph")
def setup_code_review_graph(
    apply: bool = typer.Option(False, "--apply", help="Write DevCouncil config and ignore file."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Configure optional code-review-graph context enrichment."""
    root = _project_root(project_root)
    executable = shutil.which("code-review-graph")
    ignore_path = root / ".code-review-graphignore"
    console.print("[bold]code-review-graph setup[/bold]")
    console.print(f"Binary: {executable or 'not found on PATH'}")
    console.print("Install separately with: pipx install code-review-graph")
    console.print("Build graph separately with: code-review-graph build")

    if not apply:
        console.print("[yellow]Preview only. Rerun with --apply to record this integration.[/yellow]")
        return

    if not ignore_path.exists():
        ignore_path.write_text(
            "\n".join([
                ".devcouncil/**",
                ".git/**",
                ".venv/**",
                "dist/**",
                "node_modules/**",
                "",
            ]),
            encoding="utf-8",
        )
        console.print(f"[green]Created {ignore_path}.[/green]")

    config = _load_raw_config(root)
    integrations = config.setdefault("integrations", {})
    integrations["code_review_graph"] = {
        "enabled": True,
        "command": "code-review-graph",
        "optional": True,
    }
    _save_raw_config(root, config)
    docs_dir = root / ".devcouncil" / "integrations"
    docs_dir.mkdir(parents=True, exist_ok=True)
    (docs_dir / "code-review-graph.md").write_text(
        "\n".join([
            "# code-review-graph",
            "",
            "Install and build the graph outside DevCouncil:",
            "",
            "```bash",
            "pipx install code-review-graph",
            "code-review-graph build",
            "```",
            "",
            "DevCouncil uses this as an optional context adapter for mapping, prompts, verification traces, and MCP graph context.",
            "",
        ]),
        encoding="utf-8",
    )
    console.print("[green]code-review-graph adapter recorded in .devcouncil/config.yaml.[/green]")
