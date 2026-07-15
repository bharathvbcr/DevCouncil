"""``dev debug`` — persistent DAP control and runtime traces."""

from __future__ import annotations

import json
from pathlib import Path
from typing import List, Optional

import typer
from rich.console import Console

from devcouncil.codeintel.debug.broker_client import DebugBrokerClient
from devcouncil.codeintel.debug.consent import require_debug_consent, set_debug_consent
from devcouncil.codeintel.debug.discovery import adapter_by_id, discover_adapters

app = typer.Typer(name="debug", help="Control DAP adapters and capture runtime evidence.", add_completion=False)
console = Console()
status = Console(stderr=True)


def _root(path: Path) -> Path:
    return path.expanduser().resolve()


def _require_consent(root: Path) -> None:
    try:
        require_debug_consent(root)
    except PermissionError as exc:
        status.print(f"[yellow]{exc}[/yellow]")
        raise typer.Exit(code=2) from None


def _config(value: str) -> dict:
    parsed = json.loads(value or "{}")
    if not isinstance(parsed, dict):
        raise typer.BadParameter("--config-json must contain a JSON object")
    return parsed


def _adapter_command(adapter_id: str, command: Optional[List[str]]) -> list[str]:
    if command:
        return list(command)
    adapter = adapter_by_id(adapter_id)
    if adapter is None:
        raise typer.BadParameter(f"Adapter {adapter_id!r} was not discovered; use repeatable --adapter-command")
    return list(adapter.command)


def _initial_breakpoints(values: Optional[List[str]]) -> dict[str, list[int]]:
    result: dict[str, list[int]] = {}
    for value in values or []:
        source, separator, raw_line = value.rpartition(":")
        if not separator or not source:
            raise typer.BadParameter("--breakpoint must use PATH:LINE")
        try:
            line = int(raw_line)
        except ValueError:
            raise typer.BadParameter("--breakpoint line must be an integer") from None
        result.setdefault(str(Path(source).expanduser().resolve()), []).append(line)
    return result


@app.command("discover")
def discover(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    consent: bool = typer.Option(False, "--consent", help="Persist the one-time debugger opt-in."),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    root = _root(project_root)
    if consent:
        set_debug_consent(root, True)
    _require_consent(root)
    rows = [adapter.as_dict() for adapter in discover_adapters()]
    if json_output:
        typer.echo(json.dumps({"consent": True, "adapters": rows}, indent=2))
        return
    for row in rows:
        requests = "/".join(row["requests"])
        version = row["version"] or "unknown version"
        console.print(f"{row['id']}: {row['path']}  {version}  requests={requests}  sha256={row['executable_hash']}")
    if not rows:
        console.print("No supported DAP adapters discovered.")


def _start(
    request: str,
    adapter: str,
    adapter_command: Optional[List[str]],
    config_json: str,
    breakpoints: Optional[List[str]],
    project_root: Path,
) -> None:
    root = _root(project_root)
    _require_consent(root)
    client = DebugBrokerClient(root)
    client.ensure_started()
    result = client.call("start", {
        "adapter_command": _adapter_command(adapter, adapter_command),
        "request": request,
        "arguments": _config(config_json),
        "initial_breakpoints": _initial_breakpoints(breakpoints),
    })
    typer.echo(json.dumps(result, indent=2))


@app.command("start")
def start(
    adapter: str = typer.Option("debugpy", "--adapter"),
    adapter_command: Optional[List[str]] = typer.Option(None, "--adapter-command"),
    config_json: str = typer.Option("{}", "--config-json"),
    breakpoint: Optional[List[str]] = typer.Option(None, "--breakpoint", help="Set PATH:LINE before launch."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Launch a debuggee through a persistent local DAP broker."""
    _start("launch", adapter, adapter_command, config_json, breakpoint, project_root)


@app.command("attach")
def attach(
    adapter: str = typer.Option("debugpy", "--adapter"),
    adapter_command: Optional[List[str]] = typer.Option(None, "--adapter-command"),
    config_json: str = typer.Option("{}", "--config-json"),
    breakpoint: Optional[List[str]] = typer.Option(None, "--breakpoint", help="Set PATH:LINE before attach."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Attach to a debuggee through a persistent local DAP broker."""
    _start("attach", adapter, adapter_command, config_json, breakpoint, project_root)


@app.command("break")
def set_breakpoint(
    session_id: str = typer.Argument(...),
    source: Path = typer.Argument(...),
    lines: List[int] = typer.Argument(...),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    _require_consent(_root(project_root))
    result = DebugBrokerClient(_root(project_root)).call("breakpoints", {
        "session_id": session_id, "source": str(source), "lines": lines,
    })
    typer.echo(json.dumps(result, indent=2))


def _control(action: str, session_id: str, thread_id: int | None, project_root: Path) -> None:
    _require_consent(_root(project_root))
    result = DebugBrokerClient(_root(project_root)).call("control", {
        "session_id": session_id, "debug_action": action, "thread_id": thread_id,
    })
    typer.echo(json.dumps(result, indent=2))


@app.command("continue")
def continue_(session_id: str, thread_id: Optional[int] = typer.Option(None, "--thread"), project_root: Path = typer.Option(Path("."), "--project-root")) -> None:
    _control("continue", session_id, thread_id, project_root)


@app.command("pause")
def pause(session_id: str, thread_id: Optional[int] = typer.Option(None, "--thread"), project_root: Path = typer.Option(Path("."), "--project-root")) -> None:
    _control("pause", session_id, thread_id, project_root)


@app.command("step")
def step(session_id: str, kind: str = typer.Option("next", "--kind", help="next|stepIn|stepOut"), thread_id: int = typer.Option(..., "--thread"), project_root: Path = typer.Option(Path("."), "--project-root")) -> None:
    _control(kind, session_id, thread_id, project_root)


def _inspect(operation: str, session_id: str, arguments: dict, project_root: Path) -> None:
    _require_consent(_root(project_root))
    result = DebugBrokerClient(_root(project_root)).call("inspect", {
        "session_id": session_id, "operation": operation, "arguments": arguments,
    })
    typer.echo(json.dumps(result, indent=2))


@app.command("threads")
def threads(session_id: str, project_root: Path = typer.Option(Path("."), "--project-root")) -> None:
    _inspect("threads", session_id, {}, project_root)


@app.command("stack")
def stack(session_id: str, thread_id: int = typer.Option(..., "--thread"), project_root: Path = typer.Option(Path("."), "--project-root")) -> None:
    _inspect("stackTrace", session_id, {"threadId": thread_id}, project_root)


@app.command("scopes")
def scopes(session_id: str, frame_id: int = typer.Option(..., "--frame"), project_root: Path = typer.Option(Path("."), "--project-root")) -> None:
    _inspect("scopes", session_id, {"frameId": frame_id}, project_root)


@app.command("variables")
def variables(session_id: str, reference: int = typer.Option(..., "--reference"), project_root: Path = typer.Option(Path("."), "--project-root")) -> None:
    _inspect("variables", session_id, {"variablesReference": reference}, project_root)


@app.command("evaluate")
def evaluate(
    session_id: str,
    expression: str,
    frame_id: Optional[int] = typer.Option(None, "--frame"),
    allow_side_effects: bool = typer.Option(False, "--allow-side-effects"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    if not allow_side_effects:
        raise typer.BadParameter("evaluate can execute code; pass --allow-side-effects explicitly")
    _require_consent(_root(project_root))
    result = DebugBrokerClient(_root(project_root)).call("evaluate", {
        "session_id": session_id,
        "expression": expression,
        "frame_id": frame_id,
        "allow_side_effects": True,
    })
    typer.echo(json.dumps(result, indent=2))


@app.command("trace")
def trace(
    script: Optional[Path] = typer.Option(None, "--python-script"),
    node_script: Optional[Path] = typer.Option(None, "--node-script"),
    args: Optional[List[str]] = typer.Option(None, "--arg"),
    import_path: Optional[Path] = typer.Option(None, "--import"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    from devcouncil.codeintel.debug.tracing import NodeCpuProfileProvider, PythonTraceProvider, import_runtime_trace

    root = _root(project_root)
    _require_consent(root)
    if script is not None:
        result = PythonTraceProvider(root).run(script, args or [])
    elif node_script is not None:
        result = NodeCpuProfileProvider(root).run(node_script, args or [])
    elif import_path is not None:
        result = import_runtime_trace(root, import_path)
    else:
        raise typer.BadParameter("provide --python-script, --node-script, or --import")
    typer.echo(json.dumps(result, indent=2))


@app.command("stop")
def stop(
    session_id: str,
    keep_debuggee: bool = typer.Option(False, "--keep-debuggee"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    root = _root(project_root)
    _require_consent(root)
    result = DebugBrokerClient(root).call("stop", {
        "session_id": session_id, "terminate_debuggee": not keep_debuggee,
    })
    typer.echo(json.dumps(result, indent=2))
