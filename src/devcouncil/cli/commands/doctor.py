import logging
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
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


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


def _knowledge_dir(project_root: Path, config=None) -> str:
    """Configured knowledge directory (honors ``knowledge.directory``), best-effort.

    Mirrors ``cli.commands.okf._knowledge_okf_dir`` so doctor inspects the same location
    ingest writes to. Any config failure falls back to the documented default rather than
    raising — doctor must keep running even with a broken config.

    ``config`` is an optional pre-loaded config (default ``None`` loads as before), so a
    single doctor invocation can reuse one ``load_config`` call across checks.
    """
    directory = ".devcouncil/knowledge"
    try:
        cfg = config if config is not None else load_config(project_root)
        directory = cfg.knowledge.directory
    except Exception:
        pass
    return directory


def check_ingested_knowledge(project_root: Path, config=None) -> list[tuple[str, str, str]]:
    """Best-effort health rows for ingested knowledge under ``<knowledge dir>/{okf,design}``.

    Returns ``(component, status_markup, notes)`` rows for the doctor table. This is the
    most common "ingested but silently broken" surface: an OKF bundle with a dangling
    cross-link, or a design.md with broken token references, both validate clean to the
    eye but degrade the prompt context. We therefore read+validate every ingested bundle
    and lint the design system, reporting counts and any problems.

    Never raises: any knowledge-layer failure becomes a ``WARN`` row, and an empty
    knowledge area yields a neutral ``INFO`` row — a project that never ingested knowledge
    is not a misconfiguration.
    """
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    info = "[cyan]INFO[/cyan]"
    rows: list[tuple[str, str, str]] = []

    directory = _knowledge_dir(project_root, config=config)
    base = project_root / directory
    okf_area = base / "okf"
    design_md = base / "design" / "design.md"

    # Identify ingested OKF bundles. `dev okf ingest` writes each bundle into its own
    # subfolder under okf/; treat each such subdir as a bundle. If documents sit loose
    # directly under okf/ (and there are no subfolder bundles), treat okf/ as one bundle.
    # Choosing one or the other avoids double-counting documents via rglob.
    bundle_dirs: list[Path] = []
    if okf_area.is_dir():
        try:
            subdir_bundles = [
                child
                for child in sorted(okf_area.iterdir())
                if child.is_dir() and any(child.rglob("*.md"))
            ]
        except Exception:
            subdir_bundles = []
        loose_docs = any(p.is_file() for p in okf_area.glob("*.md"))
        if subdir_bundles:
            bundle_dirs = subdir_bundles
        elif loose_docs:
            bundle_dirs = [okf_area]

    has_design = design_md.is_file()

    # Nothing ingested at all → neutral info line, not a failure.
    if not bundle_dirs and not has_design:
        rows.append(
            (
                "Ingested knowledge",
                info,
                f"No ingested knowledge under {directory}/ (okf/, design/). "
                "Add some with 'dev okf ingest <bundle>'.",
            )
        )
        return rows

    # --- OKF bundles -----------------------------------------------------------------
    if bundle_dirs:
        from devcouncil.knowledge.okf import read_bundle, validate_bundle

        total_docs = 0
        problems: list[str] = []
        for bdir in bundle_dirs:
            try:
                bundle = read_bundle(bdir)
                total_docs += len(bundle.documents)
                problems.extend(validate_bundle(bundle))
            except Exception as exc:  # never let a malformed bundle crash doctor
                problems.append(f"{bdir.name}: failed to read bundle ({exc})")
        summary = f"{len(bundle_dirs)} bundle(s), {total_docs} document(s)"
        if problems:
            preview = "; ".join(problems[:5])
            extra = "" if len(problems) <= 5 else f" (+{len(problems) - 5} more)"
            rows.append(
                (
                    "Ingested OKF",
                    warn,
                    f"{summary}: {len(problems)} validation problem(s): {preview}{extra}.",
                )
            )
        else:
            rows.append(("Ingested OKF", ok, f"{summary}; no validation problems."))

    # --- Design system ----------------------------------------------------------------
    if has_design:
        from devcouncil.knowledge.design import lint, parse_design_md

        try:
            findings = lint(parse_design_md(design_md))
        except Exception as exc:  # never let a malformed design.md crash doctor
            rows.append(("Ingested design.md", warn, f"present, but lint failed: {exc}."))
        else:
            if findings:
                preview = "; ".join(f.format() for f in findings[:5])
                extra = "" if len(findings) <= 5 else f" (+{len(findings) - 5} more)"
                rows.append(
                    (
                        "Ingested design.md",
                        warn,
                        f"present; {len(findings)} lint finding(s): {preview}{extra}.",
                    )
                )
            else:
                rows.append(("Ingested design.md", ok, "present; 0 lint findings."))

    return rows


def _add_logging_row(table, project_root: Path) -> None:
    """Append a logging-health row: where the durable run log lives and how big it
    is, so a user chasing a recurring failure knows exactly where to look."""
    from devcouncil.telemetry.logging_setup import LOG_RELATIVE_PATH

    log_path = project_root / LOG_RELATIVE_PATH
    if log_path.exists():
        size_kb = log_path.stat().st_size / 1024
        detail = f"{log_path} ({size_kb:.0f} KB). View: dev logs tail"
    else:
        detail = f"Will write to {log_path} on first command. View: dev logs tail"
    table.add_row("logging", "[green]OK[/green]", detail)


def render_doctor_check(project_root: Path = Path(".")):
    # Load config once for the whole invocation; the diagnostic checks below reuse
    # this instead of re-reading config.yaml. None falls back to per-check loading.
    try:
        config = load_config(project_root)
    except Exception:
        config = None

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

    # Ingested-knowledge health (added before the provider branch so it appears on every
    # code path, including the early returns for ollama / unsupported providers).
    for component, status, notes in check_ingested_knowledge(project_root, config=config):
        table.add_row(component, status, notes)

    try:
        provider = config.models.provider if config is not None else "openrouter"
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
        _add_logging_row(table, project_root)
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
        if reachable and config is not None:
            try:
                configured_models = sorted(
                    {role.model for role in config.models.roles.values() if role.model}
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
            # Only reachable via the explicit OLLAMA_NUM_CTX=0 opt-out: unset now
            # resolves to the provider's raised DEFAULT_NUM_CTX, never the server default.
            table.add_row(
                "OLLAMA num_ctx",
                "[yellow]WARN[/yellow]",
                f"OLLAMA_NUM_CTX=0 opts into Ollama's small server default (~2048-4096), "
                f"which will truncate DevCouncil's large planning prompts. Unset it (default "
                f"{OllamaProvider.DEFAULT_NUM_CTX}) or set OLLAMA_NUM_CTX={recommended_ctx}.",
            )
        elif num_ctx < min_ctx:
            table.add_row(
                "OLLAMA num_ctx",
                "[yellow]WARN[/yellow]",
                f"OLLAMA_NUM_CTX={num_ctx} may be too small for planning prompts "
                f"(~{MAX_PROMPT_CHARS // 4} tokens); recommend >= {recommended_ctx}.",
            )
        else:
            table.add_row(
                "OLLAMA num_ctx",
                "[green]OK[/green]",
                f"context window = {num_ctx} tokens (auto-grows per request up to "
                f"{OllamaProvider._resolve_max_num_ctx()} for oversized prompts; cap with OLLAMA_MAX_NUM_CTX).",
            )

        think = OllamaProvider._resolve_think()
        if think is None:
            table.add_row(
                "OLLAMA think",
                "[green]OK[/green]",
                "server default. On thinking models (qwen3/deepseek-r1/...) the reasoning "
                "channel can dominate review latency; OLLAMA_THINK=false trades some check "
                "quality for much faster verification calls.",
            )
        else:
            table.add_row("OLLAMA think", "[green]OK[/green]", f"OLLAMA_THINK={'true' if think else 'false'}.")

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

        _add_logging_row(table, project_root)
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

    _add_logging_row(table, project_root)
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

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev doctor: project_root=%s", root)
    with log_stage("doctor", project_root=root):
        log_step("doctor/1: running environment checks", project_root=root, trace=True)
        render_doctor_check(root)
        log_step("doctor/complete", project_root=root, trace=True)
