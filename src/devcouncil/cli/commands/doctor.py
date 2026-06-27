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


def _probe_ollama(base_url: str) -> tuple[bool, str]:
    """Best-effort reachability check for a local Ollama server.

    ``base_url`` carries the OpenAI-compatible ``/v1`` suffix; the native
    ``/api/version`` endpoint lives one level up. Returns (reachable, detail);
    any failure is reported, never raised.
    """
    root = base_url.rstrip("/")
    if root.endswith("/v1"):
        root = root[: -len("/v1")].rstrip("/")
    try:
        import httpx

        resp = httpx.get(f"{root}/api/version", timeout=3.0)
        if resp.status_code < 400:
            version = ""
            try:
                version = (resp.json() or {}).get("version", "")
            except Exception:
                version = ""
            return True, f"Reachable at {root}" + (f" (v{version})." if version else ".")
        return False, f"Server at {root} returned HTTP {resp.status_code}."
    except Exception:
        return False, f"No Ollama server reachable at {root}."


def _probe_ollama_models(base_url: str) -> tuple[bool, set[str]]:
    """Best-effort list of locally-pulled Ollama model tags via native ``/api/tags``.

    Returns (queried_ok, names). ``names`` holds the reported tags (e.g.
    ``qwen2.5-coder:7b``). Any failure returns (False, set()) and is never raised — a
    green liveness probe with no pulled model is the most common "configured but doesn't
    work" trap, so this turns it into an actionable row."""
    root = base_url.rstrip("/")
    if root.endswith("/v1"):
        root = root[: -len("/v1")].rstrip("/")
    try:
        import httpx

        resp = httpx.get(f"{root}/api/tags", timeout=3.0)
        if resp.status_code >= 400:
            return False, set()
        models = (resp.json() or {}).get("models", []) or []
        names = {str(m.get("name", "")).strip() for m in models if m.get("name")}
        return True, {n for n in names if n}
    except Exception:
        return False, set()


def _ollama_model_present(model: str, pulled: set[str]) -> bool:
    """Whether ``model`` is among the pulled tags, tolerant of the implicit ``:latest``
    tag Ollama adds to untagged models."""
    if model in pulled:
        return True
    base = model.split(":", 1)[0]
    # configured "qwen2.5-coder" matches a pulled "qwen2.5-coder:latest", and vice versa.
    candidates = {model, f"{model}:latest", base, f"{base}:latest"}
    return any(tag in candidates or tag.split(":", 1)[0] == base and ":" not in model for tag in pulled)


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

        # Is the local Ollama server actually up? A native /api/version probe is
        # cheap and turns the most common failure ("provider configured but
        # `ollama serve` not running") into an actionable row instead of a
        # connection traceback on the first model call.
        reachable, detail = _probe_ollama(base_url)
        if reachable:
            table.add_row("OLLAMA server", "[green]OK[/green]", detail)
        else:
            table.add_row(
                "OLLAMA server",
                "[yellow]WARN[/yellow]",
                f"{detail} Start it with 'ollama serve' (or 'brew services start ollama').",
            )

        # A reachable server with the configured model NOT pulled is the most common
        # "all-green doctor, 404 on first call" trap. Verify the role models exist locally.
        if reachable:
            try:
                configured_models = sorted(
                    {role.model for role in load_config(project_root).models.roles.values() if role.model}
                )
            except Exception:
                configured_models = []
            queried_ok, pulled = _probe_ollama_models(base_url)
            if not queried_ok:
                table.add_row(
                    "OLLAMA models",
                    "[yellow]WARN[/yellow]",
                    "Could not list pulled models (/api/tags). Ensure each configured model is pulled.",
                )
            elif not configured_models:
                table.add_row(
                    "OLLAMA models",
                    "[yellow]WARN[/yellow]",
                    "No role models configured; run 'dev setup --provider ollama --model <model>'.",
                )
            else:
                missing = [m for m in configured_models if not _ollama_model_present(m, pulled)]
                if missing:
                    pulls = "; ".join(f"ollama pull {m}" for m in missing)
                    table.add_row(
                        "OLLAMA models",
                        "[yellow]WARN[/yellow]",
                        f"Configured model(s) not pulled: {', '.join(missing)}. Pull first: {pulls}.",
                    )
                else:
                    table.add_row(
                        "OLLAMA models",
                        "[green]OK[/green]",
                        f"All configured models present locally ({', '.join(configured_models)}).",
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

        # Local model size is bounded by host memory — unified RAM on Apple Silicon,
        # VRAM on a discrete-GPU box, system RAM otherwise. Surface a model that will
        # actually fit on this host (any OS), not a one-size default.
        from devcouncil import hardware

        host = hardware.describe_host()
        table.add_row(
            host.platform_label,
            "[green]OK[/green]",
            f"{host.chip_label}, {host.memory_label}. "
            f"Recommended local model: {host.recommended_ollama_model} "
            f"(dev setup --provider ollama --model {host.recommended_ollama_model}).",
        )

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
