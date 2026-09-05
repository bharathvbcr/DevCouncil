import typer
import json
from devcouncil.utils.json_persist import dump_json
import logging
import os
import sys
from pathlib import Path
from rich.console import Console
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.telemetry.stages import log_step
from devcouncil.live.signals import write_signal
from devcouncil.live.tasks import active_task_id

app = typer.Typer()
console = Console()
logger = logging.getLogger(__name__)


def __getattr__(name: str) -> object:
    """Resolve `get_db` on first access, not at import.

    This hook runs on every tool call an agent makes, and every path through it
    returns before opening a database whenever no single task is active — which
    is every call under `hook_gate.mode=off`. A module-scope import made that
    common path load SQLAlchemy and SQLModel (~120 ms measured) to reach a
    database it never opens.

    Deferring it into each function would have saved the same time but removed
    the name, and `hook.get_db` is the seam five tests substitute to drive
    `_verify_active_task`. PEP 562 keeps it a real, patchable module attribute
    that costs nothing until something asks for it.
    """
    if name == "get_db":
        from devcouncil.storage.db import get_db as _get_db

        return _get_db
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def _project_root(project_root: Path | None = None) -> Path:
    if project_root:
        root = project_root.expanduser().resolve()
    else:
        configured = os.environ.get("DEVCOUNCIL_PROJECT_ROOT")
        root = Path(configured).expanduser().resolve() if configured else Path(".").resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir

    set_log_dir(root)
    return root


def _effective_root(project_root: Path | None, payload: object) -> Path:
    """The repo this hook should act on, preferring the session's live ``cwd``.

    ``--project-root`` is baked into the hook command when ``dev integrate`` runs,
    and ``${CLAUDE_PROJECT_DIR}`` behaves the same way: both name the directory the
    session *started* in and neither follows Claude Code into a git worktree.  The
    hook payload's ``cwd`` field does follow, so when it names a different directory
    that is itself an initialized DevCouncil project, that project owns the event.

    Without this, an edit inside ``<root>/.claude/worktrees/<name>/`` resolved
    relative to the parent root as ``.claude/worktrees/...``, which the indexer
    skips as a nested checkout — so the worktree's map silently never refreshed.
    Anything else (no ``cwd``, the same directory, a plain subdirectory, an
    unrelated tree) keeps the baked root.

    This is the *single* owner of that rule: every hook command routes its root
    through here, so DevCouncil's state (traces, compact snapshot, live-review
    signals, repo map) cannot split across two repos.  When only some hooks resolved
    the worktree, PreCompact wrote its snapshot to the parent while the SessionStart
    that reads it looked in the worktree, and the post-compaction briefing was empty
    every time.  ``pre_tool_use``, the write authorization gate, was
    the last holdout and now routes through here too; the security reasoning for
    that is at its own call site.
    """
    baked = project_root.expanduser().resolve() if project_root else None
    cwd = payload.get("cwd") if isinstance(payload, dict) else None
    if isinstance(cwd, str) and cwd.strip():
        try:
            candidate = Path(cwd).expanduser().resolve()
        except (OSError, ValueError):
            candidate = None
        if candidate is not None and candidate != baked and (candidate / ".devcouncil").is_dir():
            return _project_root(candidate)
    return _project_root(project_root)


def _git_watch_paths(root: Path) -> list[str]:
    """Absolute paths whose change means the map may be stale repo-wide.

    A branch switch, pull, rebase or reset rewrites many files at once without any
    tool call, so ``PostToolUse`` never observes it.  Git's ``HEAD`` and ``index``
    are a two-path proxy for exactly that class of change, which keeps the watch
    list bounded — seeding it with every source file would not be.

    In a linked worktree ``.git`` is a *file* holding ``gitdir: <path>``, so the
    real HEAD lives in the linked git directory rather than under ``root``.
    """
    git = root / ".git"
    git_dir: Path | None = None
    if git.is_dir():
        git_dir = git
    elif git.is_file():
        try:
            text = git.read_text(encoding="utf-8").strip()
        except OSError:
            return []
        if text.startswith("gitdir:"):
            candidate = Path(text.split(":", 1)[1].strip()).expanduser()
            if not candidate.is_absolute():
                candidate = (root / candidate).resolve()
            if candidate.is_dir():
                git_dir = candidate
    if git_dir is None:
        return []
    return [str(git_dir / "HEAD"), str(git_dir / "index")]


def _active_task(root: Path):
    # Resolve the *single* unambiguous running task. active_task_id returns None when
    # zero or multiple tasks are running, so we never authorize a write against the
    # wrong task; the policy engine then denies for task=None (fail-closed).
    active_id = active_task_id(root)
    if not active_id:
        return None
    # Resolved here, not at module scope: see `__getattr__` above. `get_db` is
    # read through the module object so a substituted attribute is the one used
    # — a `from … import` would bind the real function regardless.
    from devcouncil.storage.repositories import TaskRepository

    db = sys.modules[__name__].get_db(root)
    if not db:
        return None
    with db.get_session() as session:
        return TaskRepository(session).get_by_id(active_id)


def _emit_stop_result(client: str, result) -> None:
    """Emit the client's native Stop result schema.

    Both strings go through ``_cap_hook_output``.  This is the one hook output
    DevCouncil builds out of *foreign* text: a failing claim check embeds the failing
    command's output tail, which ``verification/claims/checks.py`` bounds to 50
    **lines** rather than characters, once per failing claim.  Past 10,000 characters
    Claude Code spills the value to a file and hands Claude a preview plus a path, so
    an oversized reason still blocks the stop but stops carrying the corrective
    instructions the gate exists to deliver.
    """
    from devcouncil.execution.stop_gate import StopGateResult

    if not isinstance(result, StopGateResult):
        return
    normalized = client.lower()
    reason = _cap_hook_output(result.reason) if result.reason else result.reason
    system_message = (
        _cap_hook_output(result.system_message) if result.system_message else result.system_message
    )
    if result.decision == "block" and reason:
        if normalized == "codex":
            payload: dict[str, object] = {
                "continue": False,
                "stopReason": reason,
            }
            if system_message:
                payload["systemMessage"] = system_message
            print(dump_json(payload, separators=(",", ":")))
            return
        payload = {"decision": "block", "reason": reason}
        if system_message:
            payload["systemMessage"] = system_message
        print(dump_json(payload, separators=(",", ":")))
        return
    if normalized == "gemini":
        payload = {"decision": "allow", "suppressOutput": True}
        if system_message:
            payload["systemMessage"] = system_message
        print(dump_json(payload, separators=(",", ":")))
        return
    if system_message:
        print(dump_json({"systemMessage": system_message}, separators=(",", ":")))


def _handle_unified_stop(
    event_json: str | None,
    *,
    client: str,
    project_root: Path | None,
    hook_kind: str,
) -> None:
    """Shared Stop / SubagentStop / post_task orchestrator."""
    payload_text = event_json if event_json is not None else sys.stdin.read()
    try:
        payload = json.loads(payload_text) if payload_text.strip() else {}
    except json.JSONDecodeError:
        payload = {"raw": payload_text}
    if not isinstance(payload, dict):
        payload = {}
    root = _effective_root(project_root, payload)

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
        if client == "claude":
            # Plain stdout from a PreToolUse hook goes to Claude Code's debug log
            # only — neither the user nor Claude ever sees it, so a bare
            # console.print made this warning a no-op.  ``systemMessage`` is the
            # documented way to surface a message to the user.
            print(dump_json(
                {"systemMessage": _cap_hook_output(f"DevCouncil Warning: {reason}")},
                separators=(",", ":"),
            ))
            return
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
        # SECURITY: this is the write authorization gate — the root chosen here
        # decides whose lease, allowlist and active task authorize the call.
        #
        # It used the baked ``--project-root`` while all thirteen other hooks
        # followed the payload's ``cwd``, so inside a git worktree the gate
        # evaluated a *different repository's* active task than the one being
        # worked on: able to block legitimate work, and able to allow work the
        # worktree's own task scope forbids.  Following ``cwd`` was an explicit
        # decision, not a drive-by, because it moves an access decision.
        #
        # What it widens: a worktree holding its own initialized ``.devcouncil/``
        # now governs its own writes, so a permissive policy there is no longer
        # overridden by the parent's.  ``_effective_root`` switches *only* when
        # ``cwd`` resolves to a directory that is itself an initialized
        # DevCouncil project and differs from the baked root — a plain
        # subdirectory, a missing ``cwd`` or an unrelated tree all keep the baked
        # root, which is what stops this being a general escape from the gate.
        # Those negative cases are asserted in
        # ``tests/unit/test_hooks_write_gate_root.py``.
        root = _effective_root(project_root, call_data)
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
    defer_batch: bool = typer.Option(
        False,
        "--defer-batch",
        help="Queue edited paths for the PostToolBatch hook instead of refreshing now.",
    ),
):
    """
    Coding CLI hook: Records a post-tool-use checkpoint for native hook clients.

    Best-effort: when ``indexing.auto_refresh`` is enabled, incrementally refreshes
    the repo map for files written by the tool call. Never blocks the agent on
    refresh failure.

    With ``--defer-batch`` (Claude Code only, where PostToolBatch exists) the paths
    are queued rather than built: this hook fires once per tool and concurrently
    across a parallel batch, so building here made N edits cost N kernel builds
    contending on one lock.
    """
    payload_text = tool_call_json if tool_call_json is not None else sys.stdin.read()
    root = _effective_root(project_root, _read_stdin_payload(payload_text))
    log_step(f"hook/post_tool_use: client={client}", project_root=root)
    try:
        if defer_batch:
            # Sample staleness *before* enqueueing: the enqueue rewrites the queue
            # file and would reset the very mtime this check reads.
            stranded = _refresh_queue_is_stale(root)
            paths = _payload_refresh_paths(payload_text) or []
            queued = _defer_refresh_paths(root, paths)
            if stranded:
                # No PostToolBatch hook is draining us; do the work here instead of
                # leaving the map silently stale behind an unattended queue.
                logger.warning(
                    "map refresh queue is stale; PostToolBatch does not appear to be "
                    "running. Refreshing inline."
                )
                _maybe_refresh_map(root, "", paths=[], drain=True)
            elif queued:
                log_step(
                    f"hook/post_tool_use: queued {queued} path(s) for PostToolBatch",
                    project_root=root,
                )
        else:
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


def _auto_refresh_limits(root: Path) -> tuple[bool, int]:
    """``(enabled, max_files)`` for map auto-refresh, from indexing config."""
    try:
        from devcouncil.app.config import load_config

        cfg = load_config(root).indexing
        return bool(getattr(cfg, "auto_refresh", True)), int(
            getattr(cfg, "auto_refresh_max_files", 40) or 40
        )
    except Exception:
        return True, 40


def _refreshable_rels(root: Path, paths: list[str]) -> list[str]:
    """Root-relative code paths worth re-indexing, from raw hook payload paths.

    The single owner of this filter: the PostToolUse, PostToolBatch and FileChanged
    paths all run through it, so "which edits reach the map" is decided once.
    """
    from devcouncil.codeintel.languages import code_extensions
    from devcouncil.indexing.walk import should_skip_path

    code_exts = code_extensions()
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
        # Edits from a nested checkout (.claude/worktrees/<name>/…) or an ignored
        # tree belong to that checkout's own index, not this root's graph —
        # ingesting them duplicates every edited symbol here.
        if should_skip_path(rel):
            continue
        if Path(rel).suffix.lower() in code_exts:
            rels.append(rel)
    return rels


def _payload_refresh_paths(payload_text: str) -> list[str] | None:
    """Written paths from a hook payload, or None when it is not parseable."""
    try:
        payload = json.loads(payload_text) if payload_text.strip() else {}
    except json.JSONDecodeError:
        return None
    return _extract_written_paths(payload)


# A PostToolBatch hook drains the queue within the same batch, so a queue still
# waiting this long is evidence no batch hook ran — an older Claude Code, or the
# hook removed by hand. The deferred PostToolUse path then refreshes inline rather
# than let the map drift forever behind a queue nothing drains.
DEFER_FALLBACK_S = 60.0


def _refresh_queue_is_stale(root: Path, *, now: float | None = None) -> bool:
    """True when queued paths have waited past the point a batch hook should have run."""
    import time

    queue_path = root / _MAP_REFRESH_QUEUE_REL
    try:
        age = (now if now is not None else time.time()) - queue_path.stat().st_mtime
    except OSError:
        return False
    return age > DEFER_FALLBACK_S


def _defer_refresh_paths(root: Path, paths: list[str]) -> int:
    """Queue paths for a later drain without building the map. Returns queued count.

    Used by the Claude Code PostToolUse hook when a PostToolBatch hook is installed:
    the per-tool hook stays cheap (no lock, no debounce sleep, no kernel spawn) and
    the batch hook does one build for the whole parallel batch.
    """
    enabled, _max_files = _auto_refresh_limits(root)
    if not enabled:
        return 0
    rels = _refreshable_rels(root, paths)
    if not rels:
        return 0
    _enqueue_refresh_paths(root / _MAP_REFRESH_QUEUE_REL, rels)
    return len(rels)


def _maybe_refresh_map(
    root: Path, payload_text: str, *, paths: list[str] | None = None, drain: bool = False
) -> None:
    """Config-gated, queue+drain map refresh with debounce and PID/TTL lock reclaim.

    ``paths`` bypasses payload extraction for callers that already resolved the
    edited files (PostToolBatch, FileChanged).  ``drain`` keeps going when this
    call contributes no paths of its own, so the PostToolBatch hook still flushes
    what the deferred PostToolUse hooks queued.
    """
    import time

    enabled, max_files = _auto_refresh_limits(root)
    if not enabled:
        return

    queue_path = root / _MAP_REFRESH_QUEUE_REL
    if paths is None:
        paths = _payload_refresh_paths(payload_text)
        if paths is None:
            if not drain:
                return
            paths = []
    if len(paths) > max_files:
        logger.debug(
            "Skipping map auto-refresh: %d paths > max %d", len(paths), max_files
        )
        if not drain:
            return
        paths = []

    # Only refresh code-ish paths under the project (LANGUAGE_SPECS extensions).
    rels = _refreshable_rels(root, paths)
    if not rels and not (drain and queue_path.is_file()):
        return

    # Also used below to re-filter queue entries written before this filter existed.
    from devcouncil.indexing.walk import should_skip_path

    cache_dir = root / ".devcouncil" / "cache"
    cache_dir.mkdir(parents=True, exist_ok=True)
    lock = root / _MAP_REFRESH_LOCK_REL

    if not _try_acquire_refresh_lock(lock):
        # Another refresher holds the lock — enqueue so the holder drains us.
        if rels:
            _enqueue_refresh_paths(queue_path, rels)
        logger.debug("map refresh in progress; queued %d path(s)", len(rels))
        return

    map_path = root / ".devcouncil" / "repo_map.json"
    # Set when the kernel could not build. The paths go back on the queue and
    # the leftover drain is skipped: retrying the same unavailable kernel in the
    # same hook invocation only pays the failure twice.
    deferred = False
    try:
        # Debounce burst edits so a multi-file edit lands as one refresh.
        time.sleep(MAP_REFRESH_DEBOUNCE_S)
        pending = set(rels)
        # Queue files may predate the nested-checkout filter — re-filter on drain.
        pending.update(p for p in _take_queued_paths(queue_path) if not should_skip_path(p))
        from devcouncil.devmap_engine import DevMapEngineError
        from devcouncil.indexing.map_artifacts import refresh_map_artifacts

        while pending:
            batch = sorted(pending)
            pending.clear()
            try:
                refresh_map_artifacts(root, map_path, quiet=True, paths=batch)
            except DevMapEngineError as exc:
                # A hook must never fail the tool call it runs after. The map
                # stays where the last successful build left it, the paths go
                # back on the queue so the next hook retries them instead of
                # dropping the edits, and the reason is logged rather than
                # swallowed.
                deferred = True
                logger.warning("map refresh deferred: %s", exc)
                _enqueue_refresh_paths(queue_path, batch)
                break
            log_step(
                f"hook/post_tool_use: refreshed map for {len(batch)} path(s)",
                project_root=root,
            )
            # Drain anything enqueued while we were refreshing.
            pending.update(p for p in _take_queued_paths(queue_path) if not should_skip_path(p))
    except Exception:
        # A hook must not fail the tool call, but an *unexpected* error here is a
        # defect, not a deferral: log it at the same level as a deferred refresh so
        # it cannot hide behind debug-only output the way a NameError once did.
        logger.warning("incremental map refresh failed", exc_info=True)
    finally:
        try:
            lock.unlink(missing_ok=True)
        except OSError:
            pass
        # If anything arrived after we released but before unlink races settle,
        # a subsequent PostToolUse will pick it up; also try a best-effort drain
        # by re-acquiring if the queue is non-empty.
        if not deferred and queue_path.is_file() and _try_acquire_refresh_lock(lock):
            try:
                leftover = [p for p in _take_queued_paths(queue_path) if not should_skip_path(p)]
                if leftover:
                    from devcouncil.devmap_engine import DevMapEngineError
                    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

                    try:
                        refresh_map_artifacts(root, map_path, quiet=True, paths=leftover)
                    except DevMapEngineError as exc:
                        logger.warning("map refresh deferred: %s", exc)
                        _enqueue_refresh_paths(queue_path, leftover)
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
        db = sys.modules[__name__].get_db(root)
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


# Claude Code caps every hook output string — additionalContext, systemMessage and
# plain stdout alike — at 10,000 characters.  Longer values are not rejected: they
# are spilled to a file and replaced with a preview plus a path, so an oversized
# briefing silently stops being readable inline.  Truncating here keeps the part
# that matters in the context window instead.
HOOK_OUTPUT_MAX_CHARS = 10_000
_TRUNCATION_MARKER = "\n[truncated by DevCouncil]"


def _cap_hook_output(text: str) -> str:
    """Bound a hook output string to Claude Code's 10,000-character limit."""
    if len(text) <= HOOK_OUTPUT_MAX_CHARS:
        return text
    return text[: HOOK_OUTPUT_MAX_CHARS - len(_TRUNCATION_MARKER)] + _TRUNCATION_MARKER


def _emit_hook_specific(
    event_name: str, *, system_message: str | None = None, **fields: object
) -> None:
    """Emit one hook-result object (exit 0), dropping empty fields.

    Claude Code requires stdout to hold a *single* JSON object, so an event that
    needs both event-specific fields and the universal top-level ``systemMessage``
    has to combine them here rather than print twice.  ``hookSpecificOutput``
    requires a non-empty ``hookEventName``; without it the object fails schema
    validation and Claude Code reports a hook error instead of honoring the fields.
    """
    # An empty *list* is meaningful (CwdChanged clears the watch list with []), an
    # empty string or None is not. Callers with nothing to say pass None.
    specific = {
        key: (_cap_hook_output(value) if isinstance(value, str) else value)
        for key, value in fields.items()
        if value is not None and value != ""
    }
    result: dict[str, object] = {}
    if system_message:
        result["systemMessage"] = _cap_hook_output(system_message)
    if specific and event_name:
        result["hookSpecificOutput"] = {"hookEventName": event_name, **specific}
    if not result:
        return
    print(dump_json(result, separators=(",", ":")))


def _emit_additional_context(event_name: str, context: str | None) -> None:
    """Emit a Claude-Code hook result that injects additionalContext (exit 0)."""
    if not context:
        return
    _emit_hook_specific(event_name, additionalContext=context)


def _emit_system_message(event_name: str, message: str | None = None) -> None:
    """Emit a Claude-Code hook result with a user-visible systemMessage (exit 0).

    ``systemMessage`` is a *universal top-level* field in the hook output schema,
    not a member of ``hookSpecificOutput``.  Nesting it there dropped the message
    entirely, and the one-argument form additionally emitted an empty
    ``hookEventName``, which fails schema validation.  ``event_name`` is kept in
    the signature for call-site readability but no longer shapes the payload.
    """
    if message is None:
        message = event_name
    if not message:
        return
    print(dump_json({"systemMessage": _cap_hook_output(message)}, separators=(",", ":")))



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
    """Claude Code SessionStart hook: inject a DevCouncil status snapshot as context.

    Also seeds ``watchPaths`` so the FileChanged hook can see repo-wide rewrites
    (branch switch, pull, rebase) that never pass through a tool call. Claude Code
    starts its file watcher only once something names a path to watch.
    """
    payload = _read_stdin_payload(event_json)
    root = _effective_root(project_root, payload)
    try:
        # session_id lets trace consumers pair start/end events; ends are not
        # guaranteed (Claude Code fires no SessionEnd on crash/kill — observed
        # 34 starts vs 19 ends), so durations must treat unpaired starts as open.
        details = {"client": client.lower(), "session_id": payload.get("session_id")}
        TraceLogger(root).log_event("session_start", details, summary="Claude session started.")
    except Exception as e:
        logger.debug("Failed to record session_start trace event: %s", e)
    _emit_hook_specific(
        "SessionStart",
        system_message=_compact_toast(root, payload),
        additionalContext=_session_start_context(root, payload),
        # Outside a git repo there is nothing to watch; say nothing rather than
        # send an empty list, which on SessionStart would only be noise.
        watchPaths=_git_watch_paths(root) or None,
    )


def _compact_toast(root: Path, payload: dict) -> str | None:
    """The compact-snapshot toast, for the SessionStart that follows compaction.

    ``execution.compact_snapshot_toast`` is raised by the PreCompact hook, but
    Claude Code discards a PreCompact hook's ``systemMessage``, so the toast could
    never reach the user there. SessionStart honors it, and ``source == "compact"``
    is the first moment after compaction that DevCouncil runs.
    """
    if payload.get("source") != "compact":
        return None
    try:
        from devcouncil.app.config import load_config

        if not load_config(root).execution.compact_snapshot_toast:
            return None
    except Exception:
        return None
    return "DevCouncil restored its compact snapshot (phase, task, gaps) after context compaction."


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
    payload = _read_stdin_payload(event_json)
    root = _effective_root(project_root, payload)
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
    root = _effective_root(project_root, payload)
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
    root = _effective_root(project_root, payload)
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
            # Claude Code discards a PreCompact hook's systemMessage, so this line
            # reaches the user only on clients that do honor it. The same knob also
            # raises the toast on the post-compaction SessionStart (``_compact_toast``),
            # which is where Claude Code actually shows it.
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
    root = _effective_root(project_root, payload)
    try:
        details = {"client": client.lower(), "session_id": payload.get("session_id")}
        TraceLogger(root).log_event("post_compact", details, summary="Claude context compaction finished.")
    except Exception as e:
        logger.debug("Failed to record post_compact trace event: %s", e)


@app.command()
def post_tool_batch(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code PostToolBatch hook: one map refresh for a whole parallel batch.

    PostToolUse fires once per tool and concurrently across parallel calls;
    PostToolBatch fires exactly once after the batch resolves and before Claude Code
    sends the next request to the model. Doing the build here turns N kernel builds
    contending on one lock into one, and still refreshes before the model reads.
    """
    payload = _read_stdin_payload(event_json)
    root = _effective_root(project_root, payload)
    log_step(f"hook/post_tool_batch: client={client}", project_root=root)
    try:
        calls = payload.get("tool_calls")
        paths: list[str] = []
        seen: set[str] = set()
        for call in calls if isinstance(calls, list) else []:
            for candidate in _extract_written_paths(call if isinstance(call, dict) else {}):
                if candidate not in seen:
                    seen.add(candidate)
                    paths.append(candidate)
        # drain=True so the paths deferred by this batch's PostToolUse hooks are
        # flushed even when the batch itself wrote nothing (e.g. an all-Bash batch).
        _maybe_refresh_map(root, "", paths=paths, drain=True)
    except Exception as exc:  # noqa: BLE001 — hooks must never break the session
        print(f"DevCouncil map refresh error (ignored): {exc}", file=sys.stderr)


def _paths_changed_since(root: Path, head: str) -> list[str] | None:
    """Tracked files that differ between ``head`` and the working tree, or None."""
    from devcouncil.utils.proc import git_output

    if not head:
        return None
    out = git_output(["diff", "--name-only", head], cwd=root, default="")
    return [line.strip() for line in out.splitlines() if line.strip()]


@app.command()
def file_changed(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code FileChanged hook: refresh the map after an off-tool-call rewrite.

    The watch list seeded by SessionStart/CwdChanged is git's HEAD and index, so this
    fires on a branch switch, pull, rebase or reset — changes that rewrite many files
    at once and that PostToolUse structurally cannot observe. The refreshed set is
    exactly what git reports as different from the commit the map was built at, and
    stays bounded by ``indexing.auto_refresh_max_files``.
    """
    payload = _read_stdin_payload(event_json)
    root = _effective_root(project_root, payload)
    changed = str(payload.get("file_path") or "")
    log_step(f"hook/file_changed: {Path(changed).name or 'unknown'}", project_root=root)
    try:
        if Path(changed).name in {"HEAD", "index"}:
            from devcouncil.utils.json_persist import read_json

            map_path = root / ".devcouncil" / "repo_map.json"
            if not map_path.is_file():
                return
            loaded = read_json(map_path)
            data = loaded if isinstance(loaded, dict) else {}
            paths = _paths_changed_since(root, str(data.get("generated_head") or ""))
            if not paths:
                return
        else:
            paths = [changed] if changed else []
        _maybe_refresh_map(root, "", paths=paths)
    except Exception as exc:  # noqa: BLE001 — hooks must never break the session
        print(f"DevCouncil map refresh error (ignored): {exc}", file=sys.stderr)


@app.command()
def cwd_changed(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code CwdChanged hook: re-point the file watch list at the current repo.

    The map is repo-scoped, so the watch list seeded for the old directory is wrong
    the moment Claude cd's into a different repo. Returning an empty array clears the
    dynamic list, which is the documented behaviour for leaving a repo entirely.
    """
    payload = _read_stdin_payload(event_json)
    new_cwd = payload.get("new_cwd")
    root: Path | None = None
    if isinstance(new_cwd, str) and new_cwd.strip():
        try:
            candidate = Path(new_cwd).expanduser().resolve()
        except (OSError, ValueError):
            candidate = None
        if candidate is not None and (candidate / ".devcouncil").is_dir():
            root = candidate
    # No DevCouncil project at the new cwd: clear the dynamic watch list rather than
    # keep watching the repo we just left.
    _emit_hook_specific("CwdChanged", watchPaths=_git_watch_paths(root) if root else [])


@app.command()
def directory_added(
    event_json: str | None = typer.Argument(None, help="The JSON hook payload from Claude Code."),
    client: str = typer.Option("claude", "--client", help="Hook client (claude)."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Claude Code DirectoryAdded hook: say whether the map covers the new directory.

    ``/add-dir`` widens the workspace, but the repo map stays scoped to the project it
    was built for. Without this, Claude reads the map as authoritative for files it
    never indexed. For ``slash_command`` adds Claude Code delivers this systemMessage
    to Claude as context on the next turn.
    """
    payload = _read_stdin_payload(event_json)
    root = _effective_root(project_root, payload)
    directory = str(payload.get("directory") or "").strip()
    if not directory:
        return
    try:
        TraceLogger(root).log_event(
            "directory_added",
            {"client": client.lower(), "directory": directory, "source": payload.get("source")},
            summary=f"Working directory added: {directory}",
        )
    except Exception as e:
        logger.debug("Failed to record directory_added trace event: %s", e)
    try:
        added = Path(directory).expanduser().resolve()
    except (OSError, ValueError):
        return
    if (added / ".devcouncil" / "repo_map.json").is_file():
        message = (
            f"{added} has its own DevCouncil repo map; this session's map does not "
            "index it. Query that project's map from its own root."
        )
    else:
        message = (
            f"{added} was added to the workspace but the DevCouncil repo map does not "
            "cover it. Run `dev map` there before relying on map or graph answers "
            "for its files."
        )
    _emit_system_message("DirectoryAdded", message)


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
    root = _effective_root(project_root, payload)
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
            TaskRepository,
        )
        from devcouncil.verification.next_actions import split_next_actions
        from devcouncil.verification.verifier import Verifier, verification_task_status

        active_id = active_task_id(root)
        db = sys.modules[__name__].get_db(root)
        if not active_id or not db:
            return "Run [bold]dev verify[/bold] to finalize implementation evidence."
        with db.get_session() as session:
            task = TaskRepository(session).get_by_id(active_id)
            if not task:
                return "Run [bold]dev verify[/bold] to finalize implementation evidence."
            reqs = RequirementRepository(session).get_all()
            verifier = Verifier(root)
            gaps, evidence = asyncio.run(verifier.verify_task(task, reqs))
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
            task.status = verification_task_status(gaps, verifier.last_outcome)
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
        if task.status == "done":
            return f"[green]{active_id} completed; quality verification is disabled.[/green]"
        return f"[green]{active_id} verified.[/green]"
    except Exception as exc:  # never let a hook crash the agent
        return f"[dim]post-task verification skipped: {exc}[/dim]"
