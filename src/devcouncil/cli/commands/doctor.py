import typer
import subprocess
import os
import shutil
from pathlib import Path
from rich.console import Console
from rich.table import Table

from devcouncil.app.config import get_gcloud_access_token, load_config, load_local_secrets, provider_api_key_env_var
from devcouncil.executors.agent_registry import (
    CODING_CLI_INTEGRATION_INFO,
    CODING_CLI_PROBE_ORDER,
    CODING_CLI_VERSION_COMMANDS,
    detect_available_coding_cli,
    resolve_automated_executor,
)
from devcouncil.llm.provider import SUPPORTED_MODEL_PROVIDERS, validate_model_provider

app = typer.Typer()
console = Console()

def render_doctor_check(project_root: Path = Path(".")):
    def _command_version(command: list[str]) -> str | None:
        executable = shutil.which(command[0])
        if not executable:
            return None

        resolved_command = [executable, *command[1:]]
        use_shell = os.name == "nt" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
        invocation = subprocess.list2cmdline(resolved_command) if use_shell else resolved_command
        try:
            return subprocess.check_output(
                invocation,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
                shell=use_shell,
                timeout=10,
            ).splitlines()[0].strip()
        except Exception:
            return None

    table = Table(title="DevCouncil Doctor Check")
    table.add_column("Component", style="cyan")
    table.add_column("Status", style="magenta")
    table.add_column("Notes", style="green")

    # Check Git
    git_ver = _command_version(["git", "--version"])
    if git_ver:
        table.add_row("Git", "[green]OK[/green]", git_ver)
    else:
        table.add_row("Git", "[red]Missing[/red]", "Git is required for repo mapping and checkpoints.")

    # Check uv
    uv_ver = _command_version(["uv", "--version"])
    if uv_ver:
        table.add_row("uv", "[green]OK[/green]", uv_ver)
    else:
        table.add_row("uv", "[red]Missing[/red]", "Install uv to run or install DevCouncil.")

    # Check CLI shims
    if shutil.which("devcouncil"):
        table.add_row("devcouncil CLI", "[green]OK[/green]", "Found on PATH.")
    else:
        table.add_row("devcouncil CLI", "[yellow]Missing[/yellow]", "Run via 'uv run devcouncil' or install with 'uv tool install --force .'.")

    # Check ripgrep
    rg_ver = _command_version(["rg", "--version"])
    if rg_ver:
        table.add_row("ripgrep (rg)", "[green]OK[/green]", rg_ver)
    else:
        table.add_row("ripgrep (rg)", "[yellow]Missing[/yellow]", "ripgrep is highly recommended for fast repo mapping.")

    # Check supported coding CLIs (driven by the agent registry, so new
    # built-in agents show up here without doctor edits).
    for name in CODING_CLI_PROBE_ORDER:
        info = CODING_CLI_INTEGRATION_INFO.get(name)
        if info is None:
            continue
        version = None
        for probe in CODING_CLI_VERSION_COMMANDS.get(name, ()):
            version = _command_version(list(probe))
            if version:
                break
        if version:
            table.add_row(info.label, "[green]OK[/green]", f"{version}. Setup: {info.notes}.")
        else:
            table.add_row(
                info.label,
                "[yellow]Missing[/yellow]",
                f"Optional. Install {info.label}, then use: {info.notes}.",
            )

    detected = detect_available_coding_cli(project_root)
    resolved = resolve_automated_executor(project_root, None)
    if detected:
        table.add_row(
            "Recommended coding CLI",
            "[green]OK[/green]",
            f"Use --executor {resolved} for dev go / dev run (detected on PATH).",
        )
    else:
        table.add_row(
            "Recommended coding CLI",
            "[yellow]Missing[/yellow]",
            "No built-in coding CLI on PATH. Run dev integrate recommend after installing one.",
        )

    try:
        provider = load_config(project_root).models.provider
    except Exception:
        provider = "openrouter"
    try:
        provider = validate_model_provider(provider)
    except ValueError:
        supported = ", ".join(SUPPORTED_MODEL_PROVIDERS)
        table.add_row(
            "models.provider",
            "[red]Unsupported[/red]",
            f"{provider} is configured, but this runtime supports: {supported}.",
        )
        console.print(table)
        return
    if provider == "ollama":
        # Use the provider's own resolver so the displayed URL reflects OLLAMA_HOST
        # (with scheme/-/v1 normalization), not just OLLAMA_BASE_URL.
        from devcouncil.execution.prompt_builder import MAX_PROMPT_CHARS
        from devcouncil.llm.provider import OllamaProvider

        base_url = OllamaProvider._resolve_base_url()
        num_ctx = OllamaProvider._resolve_num_ctx()
        # DevCouncil's planning prompts reach ~MAX_PROMPT_CHARS chars (~4 chars/token);
        # recommend a context window that covers the prompt plus headroom for output.
        recommended_ctx = 16384
        min_ctx = max(8192, (MAX_PROMPT_CHARS // 4))
        table.add_row(
            "OLLAMA",
            "[green]OK[/green]",
            f"Local provider; no API key required (server: {base_url}).",
        )
        if num_ctx is None:
            table.add_row(
                "OLLAMA num_ctx",
                "[yellow]WARN[/yellow]",
                f"OLLAMA_NUM_CTX not set — Ollama's small default (~2048-4096) will "
                f"truncate DevCouncil's large planning prompts. Set OLLAMA_NUM_CTX={recommended_ctx}.",
            )
        elif num_ctx < min_ctx:
            table.add_row(
                "OLLAMA num_ctx",
                "[yellow]WARN[/yellow]",
                f"OLLAMA_NUM_CTX={num_ctx} may be too small for planning prompts "
                f"(~{MAX_PROMPT_CHARS // 4} tokens); recommend >= {recommended_ctx}.",
            )
        else:
            table.add_row("OLLAMA num_ctx", "[green]OK[/green]", f"context window = {num_ctx} tokens.")
        console.print(table)
        return
    env_var = provider_api_key_env_var(provider)
    local_secrets = load_local_secrets(project_root)
    if os.environ.get(env_var):
        table.add_row(env_var, "[green]OK[/green]", f"Found in environment for {provider}.")
    elif local_secrets.get(env_var):
        table.add_row(env_var, "[green]OK[/green]", f"Found in .devcouncil/secrets.env for {provider}.")
    elif provider == "vertexai" and get_gcloud_access_token():
        table.add_row(env_var, "[green]OK[/green]", "Resolvable via gcloud auth print-access-token.")
        table.caption = "Resolvable via gcloud auth print-access-token."
    else:
        table.add_row(env_var, "[yellow]Missing[/yellow]", f"Required if using {provider} provider. Run 'dev setup'.")

    if provider == "vertexai":
        project = (
            os.environ.get("VERTEXAI_PROJECT")
            or os.environ.get("GOOGLE_CLOUD_PROJECT")
            or local_secrets.get("VERTEXAI_PROJECT")
            or local_secrets.get("GOOGLE_CLOUD_PROJECT")
        )
        if project:
            source = "environment" if os.environ.get("VERTEXAI_PROJECT") or os.environ.get("GOOGLE_CLOUD_PROJECT") else ".devcouncil/secrets.env"
            table.add_row("VERTEXAI_PROJECT", "[green]OK[/green]", f"Found in {source}.")
        else:
            table.add_row(
                "VERTEXAI_PROJECT",
                "[yellow]Missing[/yellow]",
                "Required for vertexai. Run 'dev setup --provider vertexai --vertex-project PROJECT_ID'.",
            )

        location = os.environ.get("VERTEXAI_LOCATION") or local_secrets.get("VERTEXAI_LOCATION", "global")
        table.add_row("VERTEXAI_LOCATION", "[green]OK[/green]", location)

    console.print(table)


@app.callback(invoke_without_command=True)
def doctor(
    ctx: typer.Context,
    project_root: Path = typer.Option(
        Path("."),
        "--project-root",
        help="Repository root containing .devcouncil/config.yaml.",
    ),
):
    """
    Check the environment for DevCouncil prerequisites.
    """
    if ctx.invoked_subcommand is not None:
        return

    render_doctor_check(project_root.expanduser().resolve())
