import typer
import json
from devcouncil.utils.json_persist import dump_json
import logging
import os
import sys
from pathlib import Path
from rich.console import Console
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.telemetry.stages import log_step
from devcouncil.live.signals import write_signal
from devcouncil.live.tasks import active_task_id

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)


def _project_root(project_root: Path | None = None) -> Path:
    if project_root:
        root = project_root.expanduser().resolve()
    else:
        configured = os.environ.get("DEVCOUNCIL_PROJECT_ROOT")
        root = Path(configured).expanduser().resolve() if configured else Path(".").resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir

    set_log_dir(root)
    return root


def _active_task(root: Path):
    # Resolve the *single* unambiguous running task. active_task_id returns None when
    # zero or multiple tasks are running, so we never authorize a write against the
    # wrong task; the policy engine then denies for task=None (fail-closed).
    active_id = active_task_id(root)
    if not active_id:
        return None
    db = get_db(root)
    if not db:
        return None
    with db.get_session() as session:
        return TaskRepository(session).get_by_id(active_id)


def _emit_stop_result(client: str, result) -> None:
    """Emit the client's native Stop result schema."""
    from devcouncil.execution.stop_gate import StopGateResult

    if not isinstance(result, StopGateResult):
        return
    normalized = client.lower()
    if result.decision == "block" and result.reason:
        if normalized == "codex":
            payload: dict[str, object] = {
                "continue": False,
                "stopReason": result.reason,
            }
            if result.system_message:
                payload["systemMessage"] = result.system_message
            print(dump_json(payload, separators=(",", ":")))
            return
        payload = {"decision": "block", "reason": result.reason}
        if result.system_message:
            payload["systemMessage"] = result.system_message
        print(dump_json(payload, separators=(",", ":")))
        return
    if normalized == "gemini":
        payload = {"decision": "allow", "suppressOutput": True}
        if result.system_message:
            payload["systemMessage"] = result.system_message
        print(dump_json(payload, separators=(",", ":")))
        return
    if result.system_message:
        print(dump_json({"systemMessage": result.system_message}, separators=(",", ":")))


def _handle_unified_stop(
    event_json: str | None,
    *,
    client: str,
    project_root: Path | None,
    hook_kind: str,
) -> None:
    """Shared Stop / SubagentStop / post_task orchestrator."""
    payload_text = event_json if event_json is not None else sys.stdin.read()
    root = _project_root(project_root)
    try:
        payload = json.loads(payload_text) if payload_text.strip() else {}
    except json.JSONDecodeError:
        payload = {"raw": payload_text}
    if not isinstance(payload, dict):
        payload = {}

    if not any(key in payload for key in ("task_id", "taskId", "task")):
        try:
            active_id = active_task_id(root)
        except Exception:
            active_id = None
        if active_id:
            payload["task_id"] = active_id

    try:
        signal_path = write_signal(root, client.lower(), payload)
        trace_type = "agent_response_ready" if hook_kind == "stop" else "subagent_stop"
        summary = (
            f"{client} response ready for critique-card review."
            if hook_kind == "stop"
            else f"{client} subagent finished; signal recorded."
        )
        TraceLogger(root).log_event(
            trace_type,
            {"client": client.lower(), "signal": str(signal_path), "hook": hook_kind},
            summary=summary,
        )
    except Exception as exc:  # noqa: BLE001
        print(f"DevCouncil stop hook signal error (ignored): {exc}", file=sys.stderr)

    try:
        from devcouncil.execution.stop_gate import evaluate_stop

        result = evaluate_stop(root, payload)
        _emit_stop_result(client, result)
    except Exception as exc:  # noqa: BLE001
        print(f"DevCouncil stop hook gate error (fail-open): {exc}", file=sys.stderr)
        if client.lower() == "gemini":
            print(dump_json({"decision": "allow", "suppressOutput": True}, separators=(",", ":")))


def _emit_decision(client: str, action: str, reason: str) -> None:
    if action == "deny":
        print(reason, file=sys.stderr)
        raise typer.Exit(code=2)

    if client == "codex":
        # Codex PreToolUse currently accepts only systemMessage.  Emitting the
        # legacy decision/suppressOutput fields marks the hook failed and the tool
        # proceeds, so successful checks stay silent and warnings use the one
        # supported field.
        if action == "warn":
            print(dump_json({"systemMessage": f"DevCouncil Warning: {reason}"}, separators=(",", ":")))
        return

    if client == "gemini":
        payload = {"decision": "allow", "reason": reason, "suppressOutput": True}
        if action == "warn":
            payload["systemMessage"] = f"DevCouncil Warning: {reason}"
        print(dump_json(payload, separators=(",", ":")))
        return

    if action == "warn":
        console.print(f"[yellow]DevCouncil Warning:[/yellow] {reason}")


def _emit_unevaluable(client: str, reason: str, strict: bool, *, action: str = "warn") -> None:
    """Decide what to do when a tool call cannot be evaluated (empty/malformed/error).

    Fail-closed in strict mode (block), otherwise surface a warning but allow — and
    never leak an undefined exit code, which would silently disable the only pre-action
    gate."""
    _emit_decision(client, "deny" if strict else action, f"{reason}{' (strict mode: blocking)' if strict else ''}")


@app.command()
def pre_tool_use(
    tool_call_json: str | None = typer.Argument(None, help="The JSON string of the tool call from the coding CLI."),
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    strict: bool = typer.Option(
        False,
        "--strict",
        envvar="DEVCOUNCIL_HOOK_STRICT",
        help="Fail closed (block) when a tool call cannot be parsed or evaluated.",
    ),
):
    """
    Coding CLI hook: Inspects a tool call before execution.
    Exits with code 2 to block unauthorized file writes.
    """
    normalized_client = client.lower()
    try:
        if tool_call_json is None:
            tool_call_json = sys.stdin.read()
        # Empty payload: nothing to evaluate. Benign in normal use, so allow — but make
        # it observable, and block under --strict.
        if not tool_call_json.strip():
            return _emit_unevaluable(normalized_client, "Empty tool-call payload; nothing to evaluate.", strict, action="allow")
        try:
            call_data = json.loads(tool_call_json)
        except json.JSONDecodeError:
            # A real tool call we cannot parse must not silently pass the gate.
            return _emit_unevaluable(normalized_client, "Tool-call payload was not valid JSON; could not enforce policy.", strict)
        root = _project_root(project_root)
        log_step(f"hook/pre_tool_use: client={normalized_client}", project_root=root)
        active_task = _active_task(root)

        decision = HookPolicy(project_root=root).evaluate(call_data, active_task)
        _emit_decision(normalized_client, decision.action, decision.reason)
    except typer.Exit:
        raise
    except Exception as exc:  # never emit an undefined exit code from a crashing hook
        return _emit_unevaluable(normalized_client, f"Hook error: {exc}", strict)

@app.command()
def post_tool_use(
    tool_call_json: str | None = typer.Argument(None, help="The JSON string of the completed tool call."),
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Coding CLI hook: Records a post-tool-use checkpoint for native hook clients.

    Best-effort: when ``indexing.auto_refresh`` is enabled, incrementally refreshes
    the repo map for files written by the tool call. Never blocks the agent on
    refresh failure.
    """
    payload_text = tool_call_json if tool_call_json is not None else sys.stdin.read()
    root = _project_root(project_root)
    log_step(f"hook/post_tool_use: client={client}", project_root=root)
    try:
        _maybe_refresh_map(root, payload_text)
    except Exception as exc:  # noqa: BLE001 — hooks must never break the session
        print(f"DevCouncil map refresh error (ignored): {exc}", file=sys.stderr)
    if client.lower() == "gemini":
        print(dump_json({"decision": "allow", "suppressOutput": True}, separators=(",", ":")))


def _extract_written_paths(payload: object) -> list[str]:
    """Best-effort path extraction from Write/Edit/MultiEdit-style hook payloads."""
    paths: list[str] = []
    if not isinstance(payload, dict):
        return paths

    def _add(value: object) -> None:
        if isinstance(value, str) and value.strip():
            paths.append(value.strip())
        elif isinstance(value, list):
            for item in value:
                _add(item)

    for key in (
        "file_path",
        "filePath",
        "path",
        "file",
        "target_file",
        "targetFile",
    ):
        if key in payload:
            _add(payload[key])

    tool_input = payload.get("tool_input") or payload.get("toolInput") or payload.get("input")
    if isinstance(tool_input, dict):
        paths.extend(_extract_written_paths(tool_input))
    elif isinstance(tool_input, str):
        try:
            parsed = json.loads(tool_input)
            paths.extend(_extract_written_paths(parsed))
        except json.JSONDecodeError:
            pass

    # Claude-style: tool_name + tool_input
    tool_name = str(payload.get("tool_name") or payload.get("toolName") or payload.get("name") or "").lower()
    if tool_name in {"write", "edit", "multiedit", "create", "notebookedit"} or "edit" in tool_name or "write" in tool_name:
        for key in ("file_path", "filePath", "path", "target_file"):
            if key in payload:
                _add(payload[key])

    edits = payload.get("edits")
    if isinstance(edits, list):
        for edit in edits:
            paths.extend(_extract_written_paths(edit))

    # Dedupe preserving order
    seen: set[str] = set()
    out: list[str] = []
    for p in paths:
        norm = p.replace("\\", "/")
        if norm.startswith("./"):
            norm = norm[2:]
        if norm and norm not in seen:
            seen.add(norm)
            out.append(norm)
    return out


# Debounce burst edits; reclaim a lock left by a crashed refresher after TTL.
MAP_REFRESH_DEBOUNCE_S = 0.3
MAP_REFRESH_LOCK_TTL_S = 120.0
_MAP_REFRESH_QUEUE_REL = Path(".devcouncil") / "cache" / "map_refresh_queue.json"
_MAP_REFRESH_LOCK_REL = Path(".devcouncil") / "cache" / "map_refresh.lock"


def _pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        # Process exists but we cannot signal it — treat as alive.
        return True
    except OSError:
        return False
    return True


def _read_lock_meta(lock: Path) -> tuple[int | None, float | None]:
    """Return (pid, started_at) from lockfile contents, or (None, None) if unreadable."""
    try:
        raw = lock.read_text(encoding="utf-8").strip()
    except OSError:
        return None, None
    if not raw:
        return None, None
    try:
        data = json.loads(raw)
        if isinstance(data, dict):
            pid = data.get("pid")
            started = data.get("started_at")
            return (
                int(pid) if pid is not None else None,
                float(started) if started is not None else None,
            )
    except (json.JSONDecodeError, TypeError, ValueError):
        pass
    # Legacy / plain text: first line pid, optional second line timestamp
    lines = raw.splitlines()
    try:
        pid = int(lines[0].strip())
    except ValueError:
        return None, None
    started = None
    if len(lines) > 1:
        try:
            started = float(lines[1].strip())
        except ValueError:
            started = None
    return pid, started


def _lock_is_reclaimable(lock: Path, *, now: float | None = None) -> bool:
    """True when the lock owner is gone or the lock is older than the TTL."""
    import time

    if not lock.exists():
        return True
    pid, started = _read_lock_meta(lock)
    if pid is not None and not _pid_alive(pid):
        return True
    ts = started
    if ts is None:
        try:
            ts = lock.stat().st_mtime
        except OSError:
            return True
    clock = now if now is not None else time.time()
    return (clock - ts) > MAP_REFRESH_LOCK_TTL_S


def _try_acquire_refresh_lock(lock: Path) -> bool:
    """Create the lockfile exclusively, reclaiming a dead/stale owner first."""
    import time

    lock.parent.mkdir(parents=True, exist_ok=True)
    payload = dump_json({"pid": os.getpid(), "started_at": time.time()}, separators=(",", ":"))
    try:
        fd = os.open(str(lock), os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        try:
            os.write(fd, payload.encode("utf-8"))
        finally:
            os.close(fd)
        return True
    except FileExistsError:
        if not _lock_is_reclaimable(lock):
            return False
        try:
            lock.unlink(missing_ok=True)
        except OSError:
            return False
        try:
            fd = os.open(str(lock), os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            try:
                os.write(fd, payload.encode("utf-8"))
            finally:
                os.close(fd)
            return True
        except (FileExistsError, OSError):
            return False
    except OSError:
        return False


def _enqueue_refresh_paths(queue_path: Path, paths: list[str]) -> None:
    """Append paths to the durable refresh queue (never drop a pending refresh).

    Uses temp+replace. Safe concurrent with rename-to-drain: if the holder
    renames the queue away mid-read, we create a fresh queue the holder
    re-checks after drain.
    """
    queue_path.parent.mkdir(parents=True, exist_ok=True)
    existing: list[str] = []
    if queue_path.is_file():
        try:
            data = json.loads(queue_path.read_text(encoding="utf-8"))
            if isinstance(data, dict):
                existing = [str(p) for p in (data.get("paths") or []) if p]
            elif isinstance(data, list):
                existing = [str(p) for p in data if p]
        except (json.JSONDecodeError, OSError):
            existing = []
    seen = set(existing)
    for p in paths:
        if p not in seen:
            existing.append(p)
            seen.add(p)
    tmp = queue_path.with_suffix(".tmp")
    tmp.write_text(dump_json({"paths": existing}, indent=2) + "\n", encoding="utf-8")
    tmp.replace(queue_path)


def _parse_queue_file(path: Path) -> list[str]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(data, dict):
            return [str(p) for p in (data.get("paths") or []) if p]
        if isinstance(data, list):
            return [str(p) for p in data if p]
    except (json.JSONDecodeError, OSError):
        pass
    return []


def _take_queued_paths(queue_path: Path) -> list[str]:
    """Rename-to-drain the queue; merge any file recreated during drain.

    Avoids the lose-on-drain race where unlink after read drops paths that
    were enqueued between the read and the unlink.
    """
    collected: list[str] = []
    seen: set[str] = set()

    def _absorb(paths: list[str]) -> None:
        for p in paths:
            if p not in seen:
                seen.add(p)
                collected.append(p)

    # Bound retries against pathological enqueue churn during drain.
    for _ in range(8):
        if not queue_path.is_file():
            break
        drained = queue_path.with_name(queue_path.name + ".draining")
        try:
            os.replace(str(queue_path), str(drained))
        except OSError:
            break
        _absorb(_parse_queue_file(drained))
        try:
            drained.unlink(missing_ok=True)
        except OSError:
            pass
    return collected


def _maybe_refresh_map(root: Path, payload_text: str) -> None:
    """Config-gated, queue+drain map refresh with debounce and PID/TTL lock reclaim."""
    import time

    try:
        from devcouncil.app.config import load_config

        cfg = load_config(root).indexing
        if not getattr(cfg, "auto_refresh", True):
            return
        max_files = int(getattr(cfg, "auto_refresh_max_files", 40) or 40)
    except Exception:
        max_files = 40

    try:
        payload = json.loads(payload_text) if payload_text.strip() else {}
    except json.JSONDecodeError:
        return
    paths = _extract_written_paths(payload)
    if not paths:
        return
    if len(paths) > max_files:
        logger.debug(
            "Skipping map auto-refresh: %d paths > max %d", len(paths), max_files
        )
        return

    # Only refresh code-ish paths under the project
    code_exts = {".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".go", ".rs"}
    rels: list[str] = []
    for p in paths:
        candidate = Path(p)
        if candidate.is_absolute():
            try:
                rel = candidate.relative_to(root).as_posix()
            except ValueError:
                continue
        else:
            rel = p.replace("\\", "/")
            if rel.startswith("./"):
                rel = rel[2:]
        if Path(rel).suffix.lower() in code_exts:
            rels.append(rel)
    if not rels:
        return

    cache_dir = root / ".devcouncil" / "cache"
    cache_dir.mkdir(parents=True, exist_ok=True)
    lock = root / _MAP_REFRESH_LOCK_REL
    queue_path = root / _MAP_REFRESH_QUEUE_REL

    if not _try_acquire_refresh_lock(lock):
        # Another refresher holds the lock — enqueue so the holder drains us.
        _enqueue_refresh_paths(queue_path, rels)
        logger.debug("map refresh in progress; queued %d path(s)", len(rels))
        return

    try:
        # Debounce burst edits so a multi-file edit lands as one refresh.
        time.sleep(MAP_REFRESH_DEBOUNCE_S)
        pending = set(rels)
        pending.update(_take_queued_paths(queue_path))
        from devcouncil.indexing.graph.build import refresh_map_for_paths

        while pending:
            batch = sorted(pending)
            pending.clear()
            refresh_map_for_paths(root, batch)
            log_step(
                f"hook/post_tool_use: refreshed map for {len(batch)} path(s)",
                project_root=root,
            )
            # Drain anything enqueued while we were refreshing.
            pending.update(_take_queued_paths(queue_path))
    except Exception:
        logger.debug("incremental map refresh failed", exc_info=True)
    finally:
        try:
            lock.unlink(missing_ok=True)
        except OSError:
            pass
        # If anything arrived after we released but before unlink races settle,
        # a subsequent PostToolUse will pick it up; also try a best-effort drain
        # by re-acquiring if the queue is non-empty.
        if queue_path.is_file() and _try_acquire_refresh_lock(lock):
            try:
                leftover = _take_queued_paths(queue_path)
                if leftover:
                    from devcouncil.indexing.graph.build import refresh_map_for_paths

                    refresh_map_for_paths(root, leftover)
            except Exception:
                logger.debug("map refresh leftover drain failed", exc_info=True)
            finally:
                try:
                    lock.unlink(missing_ok=True)
                except OSError:
                    pass


@app.command()
def agent_response(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from the coding CLI."),
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Coding CLI Stop hook: live-review signal + unified stop gate (claims + verify).
    """
    _handle_unified_stop(event_json, client=client, project_root=project_root, hook_kind="stop")

def _status_line(root: Path) -> str | None:
    """A one-line DevCouncil status snapshot, or None when uninitialized/unavailable.

    Used by the SessionStart and UserPromptSubmit hooks to inject lightweight project
    context into Claude Code. Best-effort: any failure returns None so a hook never
    breaks the session."""
    try:
        db = get_db(root)
        if not db:
            return None
        from devcouncil.storage.repositories import ArtifactGraphRepository, StateRepository
        from devcouncil.app.project_status import compute_phase

        with db.get_session() as session:
            graph = ArtifactGraphRepository(session).load_graph()
            summary = graph.coverage_summary()
            state = StateRepository(session).get_state()
            phase = compute_phase(graph, state.current_phase if state else None)
        base = (
            f"DevCouncil — phase: {phase}; tasks: {summary['total_tasks']}; "
            f"gaps: {summary['total_gaps']} ({summary['blocking_gaps']} blocking). "
            "Use the devcouncil_* MCP tools and `dev` CLI to stay inside the verify loop."
        )
        hints: list[str] = []
        try:
            from devcouncil.indexing.repo_mapper import RepoMapper
            from devcouncil.utils.json_persist import read_json

            map_path = root / ".devcouncil" / "repo_map.json"
            if map_path.is_file():
                loaded = read_json(map_path)
                data = loaded if isinstance(loaded, dict) else {}
                if RepoMapper(root).map_is_stale(data):
                    hints.append("repo map stale — run `dev map` (or MCP graph_ingest)")
            else:
                hints.append("no repo map — run `dev map`")
        except Exception:
            pass
        try:
            from devcouncil.integrations.check import _cursor_config_status

            status, fixable, _ = _cursor_config_status(root)
            if status != "ok" and fixable:
                hints.append(
                    f"Cursor MCP {status} — run `dev integrate cursor --apply`"
                )
        except Exception:
            pass
        if hints:
            return f"{base} Continuity — {'; '.join(hints)}."
        return base
    except Exception:
        return None


def _emit_additional_context(event_name: str, context: str | None) -> None:
    """Emit a Claude-Code hook result that injects additionalContext (exit 0)."""
    if not context:
        return
    print(dump_json({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "additionalContext": context,
        }
    }, separators=(",", ":")))


def _emit_system_message(event_name: str, message: str | None = None) -> None:
    """Emit a Claude-Code hook result with a user-visible systemMessage (exit 0)."""
    if message is None:
        message = event_name
        event_name = ""
    if not message:
        return
    import json
    print(json.dumps({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "systemMessage": message,
        }
    }, separators=(",", ":")))



def _read_stdin_payload(event_json: str | None) -> dict:
    text = event_json if event_json is not None else sys.stdin.read()
    if not text or not text.strip():
        return {}
    try:
        parsed = json.loads(text)
        return parsed if isinstance(parsed, dict) else {"raw": text}
    except json.JSONDecodeError:
        return {"raw": text}


@app.command()
def session_start(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code SessionStart hook: inject a DevCouncil status snapshot as context."""
    payload = _read_stdin_payload(event_json)
    root = _project_root(project_root)
    try:
        # session_id lets trace consumers pair start/end events; ends are not
        # guaranteed (Claude Code fires no SessionEnd on crash/kill — observed
        # 34 starts vs 19 ends), so durations must treat unpaired starts as open.
        details = {"client": client.lower(), "session_id": payload.get("session_id")}
        TraceLogger(root).log_event("session_start", details, summary="Claude session started.")
    except Exception as e:
        logger.debug("Failed to record session_start trace event: %s", e)
    _emit_additional_context("SessionStart", _session_start_context(root, payload))


def _session_start_context(root: Path, payload: dict) -> str | None:
    if payload.get("source") == "compact":
        from devcouncil.execution.stop_gate import compact_briefing, record_compact_brief

        session_id = payload.get("session_id")
        record_compact_brief(root, str(session_id) if session_id else None)
        return compact_briefing(root, payload)
    base = _status_line(root)
    try:
        from devcouncil.execution.stop_gate import session_briefing

        extra = session_briefing(root, payload)
    except Exception:
        extra = None
    if base and extra:
        return f"{base}\n{extra}"
    return base or extra


@app.command()
def user_prompt_submit(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code UserPromptSubmit hook: surface the current DevCouncil status as context."""
    _read_stdin_payload(event_json)
    root = _project_root(project_root)
    try:
        from devcouncil.app.config import load_config
        from devcouncil.execution.stop_gate import recent_compact_brief

        skip_secs = load_config(root).execution.skip_prompt_status_after_compact_seconds
        if recent_compact_brief(root, skip_secs):
            return
    except Exception:
        pass
    _emit_additional_context("UserPromptSubmit", _status_line(root))


@app.command()
def session_end(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code SessionEnd hook: record session teardown in the DevCouncil trace."""
    payload = _read_stdin_payload(event_json)
    root = _project_root(project_root)
    try:
        details = {
            "client": client.lower(),
            "session_id": payload.get("session_id"),
            "reason": payload.get("reason"),
        }
        TraceLogger(root).log_event("session_end", details, summary="Claude session ended.")
    except Exception as e:
        logger.debug("Failed to record session_end trace event: %s", e)


@app.command()
def pre_compact(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code PreCompact hook: write continuity snapshot + trace (no model context)."""
    payload = _read_stdin_payload(event_json)
    root = _project_root(project_root)
    try:
        from devcouncil.execution.stop_gate import write_compact_snapshot

        write_compact_snapshot(root, payload)
    except Exception as e:
        logger.debug("Failed to write compact snapshot: %s", e)
    try:
        TraceLogger(root).log_event(
            "pre_compact",
            {"client": client.lower(), "session_id": payload.get("session_id")},
            summary="Claude context compaction starting.",
        )
    except Exception as e:
        logger.debug("Failed to record pre_compact trace event: %s", e)
    try:
        from devcouncil.app.config import load_config

        if load_config(root).execution.compact_snapshot_toast:
            _emit_system_message(
                "PreCompact",
                "DevCouncil saved a compact snapshot (phase, task, gaps) before context compaction.",
            )
    except Exception:
        pass


@app.command()
def post_compact(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code PostCompact hook: trace only — never injects additionalContext."""
    payload = _read_stdin_payload(event_json)
    root = _project_root(project_root)
    try:
        details = {"client": client.lower(), "session_id": payload.get("session_id")}
        TraceLogger(root).log_event("post_compact", details, summary="Claude context compaction finished.")
    except Exception as e:
        logger.debug("Failed to record post_compact trace event: %s", e)


@app.command()
def subagent_stop(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code SubagentStop hook: same unified stop gate as Stop."""
    _handle_unified_stop(event_json, client=client, project_root=project_root, hook_kind="subagent")


@app.command()
def notification(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code Notification hook: record a Claude notification in the DevCouncil trace."""
    payload = _read_stdin_payload(event_json)
    root = _project_root(project_root)
    try:
        message = str(payload.get("message", ""))[:200]
        TraceLogger(root).log_event(
            "claude_notification",
            {"client": client.lower(), "message": message},
            # Put the message in the summary too — that's the field trace
            # viewers render, and 18 consecutive "Claude notification." lines
            # tell a reader nothing.
            summary=f"Claude notification: {message}" if message else "Claude notification.",
        )
    except Exception as e:
        logger.debug("Failed to record claude_notification trace event: %s", e)


@app.command()
def claude_statusline(
    event_json: str | None = typer.Argument(None, help="The JSON status payload from Claude Code."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code statusLine command: print a compact DevCouncil status line.

    Reads Claude's status JSON on stdin (cwd, model, ...) and prints one line. Falls back
    to a minimal marker when the project isn't initialized so the status bar never breaks."""
    payload = _read_stdin_payload(event_json)
    # Prefer the cwd Claude reports so the line reflects the active workspace.
    cwd = payload.get("cwd") if isinstance(payload, dict) else None
    root = _project_root(Path(cwd) if isinstance(cwd, str) and cwd else project_root)
    session_id = payload.get("session_id") if isinstance(payload, dict) else None
    line1 = _status_line(root)
    if not line1:
        print("DevCouncil: not initialized")
        return
    line1 = line1.split(". Use the")[0]
    try:
        from devcouncil.execution.stop_gate import statusline_tally

        tally = statusline_tally(root, str(session_id) if session_id else None)
    except Exception:
        tally = None
    model = payload.get("model") if isinstance(payload, dict) else None
    extras: list[str] = []
    if tally:
        extras.append(tally)
    if isinstance(model, str) and model.strip():
        extras.append(model.strip())
    if extras:
        print(line1)
        print(" | ".join(extras))
    else:
        print(line1)


@app.command()
def post_task(
    event_json: str | None = typer.Argument(None, help="Optional JSON hook payload."),
    client: str = typer.Option("claude", "--client", help="Hook client: claude, codex, gemini, cursor, or generic."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Deprecated alias for the unified Stop handler (``execution.stop_gate``).

    ``execution.verify_on_post_task`` is a deprecated alias for
    ``stop_gate.mode != off`` with ``verify_active_task``.
    """
    _handle_unified_stop(event_json, client=client, project_root=project_root, hook_kind="post_task")


def _emit_post_task_allow(client: str) -> None:
    if client.lower() == "gemini":
        print(dump_json({"decision": "allow", "suppressOutput": True}, separators=(",", ":")))


def _verify_active_task(root: Path) -> str:
    """Run deterministic verification of the active task and persist gaps/evidence.
    Returns a human summary line. Best-effort: never raises out of a hook."""
    try:
        import asyncio

        from devcouncil.domain.evidence import CommandResult, DiffCoverageEvidence, DiffEvidence, TestEvidence
        from devcouncil.storage.repositories import (
            EvidenceRepository,
            GapRepository,
            RequirementRepository,
        )
        from devcouncil.verification.next_actions import split_next_actions
        from devcouncil.verification.verifier import Verifier

        active_id = active_task_id(root)
        db = get_db(root)
        if not active_id or not db:
            return "Run [bold]dev verify[/bold] to finalize implementation evidence."
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(active_id)
            if not task:
                return "Run [bold]dev verify[/bold] to finalize implementation evidence."
            reqs = RequirementRepository(session).get_all()
            gaps, evidence = asyncio.run(Verifier(root).verify_task(task, reqs))
            gap_repo = GapRepository(session)
            ev_repo = EvidenceRepository(session)
            gap_repo.delete_for_task(task.id)
            ev_repo.delete_for_task(task.id)
            for gap in gaps:
                gap_repo.save(gap)
            for ev in evidence:
                if isinstance(ev, CommandResult):
                    ev_repo.save_command_result(task.id, ev)
                elif isinstance(ev, DiffCoverageEvidence):
                    ev_repo.save_diff_coverage_evidence(ev)
                elif isinstance(ev, DiffEvidence):
                    ev_repo.save_diff_evidence(ev)
                elif isinstance(ev, TestEvidence):
                    ev_repo.save_test_evidence(ev, task.id)
            blocking = [g for g in gaps if g.blocking]
            task.status = "blocked" if blocking else "verified"
            TaskRepository(session).save(task)
        blocking_actions, _ = split_next_actions(gaps)
        TraceLogger(root).log_event(
            "post_task_verified",
            {"task_id": active_id, "blocking": len(blocking)},
            task_id=active_id,
            summary=f"post_task verification: {task.status}",
        )
        if blocking:
            return (
                f"[yellow]{active_id} is blocked by {len(blocking)} gap(s); "
                f"{len(blocking_actions)} next action(s). Run [bold]dev repair[/bold].[/yellow]"
            )
        return f"[green]{active_id} verified.[/green]"
    except Exception as exc:  # never let a hook crash the agent
        return f"[dim]post-task verification skipped: {exc}[/dim]"
