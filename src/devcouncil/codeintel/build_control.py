"""Single-writer graph build sessions and supervised full-build isolation."""

from __future__ import annotations

import json
import logging
import os
import queue
import signal
import subprocess
import sys
import threading
import time
import uuid
from collections import deque
from contextlib import contextmanager
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Iterator

from devcouncil.codeintel.service import get_codeintel_service
from devcouncil.indexing.graph.schema import CodeGraph
from devcouncil.utils.json_persist import read_json, write_json

# WriterLease is imported lazily inside graph_build_session to avoid
# sync.__init__ → incremental → build_control circular import at module load.
if TYPE_CHECKING:
    from devcouncil.codeintel.sync.lease import WriterLease

logger = logging.getLogger(__name__)

STATUS_REL = Path(".devcouncil") / "codeintel" / "build_status.json"


class GraphBuildBusy(RuntimeError):
    """Another thread or process owns the graph writer session."""


class GraphBuildTimeout(TimeoutError):
    """A supervised graph build exceeded its progress or total deadline."""


class GraphBuildFailed(RuntimeError):
    """A supervised graph worker exited without committing a graph."""


@dataclass
class BuildStatus:
    build_id: str = ""
    state: str = "idle"
    mode: str = ""
    pid: int | None = None
    phase: str = ""
    completed: int = 0
    total: int = 0
    started_at: float | None = None
    last_progress_at: float | None = None
    generation_before: int | None = None
    generation_after: int | None = None
    degraded_reason: str = ""
    compatibility_export: str = "unknown"
    stall_timeout_seconds: float = 90.0
    total_timeout_seconds: float = 900.0
    # CPU seconds the supervised worker (and its children) had consumed at the
    # last heartbeat. A worker whose phase counters are flat but whose CPU keeps
    # climbing is working, not hung — the supervisor uses this to decide.
    worker_cpu_seconds: float = 0.0
    last_cpu_progress_at: float | None = None


@dataclass
class IsolatedBuildResult:
    graph: CodeGraph
    status: BuildStatus


_LOCKS: dict[Path, threading.RLock] = {}
_LOCKS_GUARD = threading.Lock()
_LOCAL = threading.local()


def status_path(root: Path) -> Path:
    return root.expanduser().resolve() / STATUS_REL


def read_build_status(root: Path) -> BuildStatus:
    try:
        raw = read_json(status_path(root))
        status = BuildStatus(**{
            key: raw[key] for key in BuildStatus.__dataclass_fields__ if key in raw
        })
        if status.state == "building":
            if status.pid is not None and not _pid_alive(status.pid):
                status.state = "stale"
                status.degraded_reason = "recorded graph worker is no longer running"
            else:
                # A silent phase is not a stall. Only call it stalled when both
                # phase progress and worker CPU have been flat past the budget.
                marks = [
                    mark
                    for mark in (status.last_progress_at, status.last_cpu_progress_at)
                    if mark is not None
                ]
                if marks and time.time() - max(marks) > status.stall_timeout_seconds:
                    status.state = "stalled"
                    status.degraded_reason = (
                        f"no graph progress or worker CPU for "
                        f"{status.stall_timeout_seconds:.1f}s"
                    )
        return status
    except Exception:
        return BuildStatus()


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except (PermissionError, OSError):
        return True
    return True


def _write_status(root: Path, status: BuildStatus) -> None:
    path = status_path(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    write_json(path, asdict(status))


def record_inline_build_status(
    root: Path,
    *,
    state: str,
    mode: str,
    phase: str,
    generation_before: int | None,
    generation_after: int | None,
    reason: str = "",
    compatibility_export: str = "healthy",
    build_id: str | None = None,
    completed: int = 1,
    total: int = 1,
    started_at: float | None = None,
    stall_timeout_seconds: float | None = None,
    total_timeout_seconds: float | None = None,
    worker_cpu_seconds: float = 0.0,
    last_cpu_progress_at: float | None = None,
) -> BuildStatus:
    """Write (or refresh) inline build_status for in-process / incremental work.

    Unlike supervised full builds, incremental sync has no parent supervisor.
    Recording pid + phase heartbeats lets ``dev map unlock`` target a stuck
    watcher without introducing a second supervisor process.
    """
    root = root.expanduser().resolve()
    now = time.time()
    stall = 90.0 if stall_timeout_seconds is None else float(stall_timeout_seconds)
    total_to = 900.0 if total_timeout_seconds is None else float(total_timeout_seconds)
    if stall_timeout_seconds is None or total_timeout_seconds is None:
        try:
            from devcouncil.app.config import load_config

            indexing = load_config(root).indexing
            if stall_timeout_seconds is None:
                stall = float(indexing.build_stall_timeout_seconds)
            if total_timeout_seconds is None:
                total_to = float(indexing.build_total_timeout_seconds)
        except Exception:
            pass
    status = BuildStatus(
        build_id=build_id or uuid.uuid4().hex,
        state=state,
        mode=mode,
        pid=os.getpid(),
        phase=phase,
        completed=int(completed),
        total=int(total),
        started_at=now if started_at is None else float(started_at),
        last_progress_at=now,
        generation_before=generation_before,
        generation_after=generation_after,
        degraded_reason=reason,
        compatibility_export=compatibility_export,
        stall_timeout_seconds=stall,
        total_timeout_seconds=total_to,
        worker_cpu_seconds=float(worker_cpu_seconds),
        last_cpu_progress_at=last_cpu_progress_at,
    )
    _write_status(root, status)
    return status


def update_inline_build_progress(
    root: Path,
    *,
    phase: str,
    completed: int | None = None,
    total: int | None = None,
    state: str = "building",
    mode: str = "incremental",
    reason: str | None = None,
    worker_cpu_seconds: float | None = None,
) -> BuildStatus:
    """Heartbeat an in-process build without resetting ``build_id`` / ``started_at``.

    Also used by an orphaned supervised worker (parent gone) to keep the
    status file truthful; ``worker_cpu_seconds`` carries its CPU heartbeat so
    stall detection keeps working without a supervisor.
    """
    root = root.expanduser().resolve()
    current = read_build_status(root)
    same_inline = (
        current.pid == os.getpid()
        and current.mode in {"", mode, "incremental", "full"}
        and current.state in {"building", "stalled", "stale", "timed_out", "idle", state}
        and bool(current.build_id)
    )
    if reason is None:
        reason = "" if state == "building" else current.degraded_reason
    cpu = current.worker_cpu_seconds if worker_cpu_seconds is None else float(worker_cpu_seconds)
    cpu_mark = current.last_cpu_progress_at
    if worker_cpu_seconds is not None and cpu > current.worker_cpu_seconds:
        cpu_mark = time.time()
    return record_inline_build_status(
        root,
        state=state,
        mode=mode or current.mode or "incremental",
        phase=phase,
        generation_before=current.generation_before,
        generation_after=current.generation_after,
        reason=reason,
        compatibility_export=current.compatibility_export or "unknown",
        build_id=current.build_id if same_inline else None,
        completed=current.completed if completed is None else completed,
        total=current.total if total is None else total,
        started_at=current.started_at if same_inline else None,
        stall_timeout_seconds=current.stall_timeout_seconds,
        total_timeout_seconds=current.total_timeout_seconds,
        worker_cpu_seconds=cpu,
        last_cpu_progress_at=cpu_mark,
    )


def writer_busy_details(root: Path) -> dict[str, object]:
    """Recovery fields for ``graph_writer_busy`` CLI / MCP responses."""
    from devcouncil.codeintel.sync.lease import read_holder

    root = root.expanduser().resolve()
    lock_path = root / ".devcouncil" / "codeintel" / "writer.lock"
    holder = read_holder(lock_path)
    build = read_build_status(root)
    pid = holder.pid if holder.pid is not None else build.pid
    return {
        "hint": "dev map unlock",
        "build_pid": pid,
        "build_state": build.state,
        "build_phase": build.phase,
        "holder_pid": holder.pid,
        "holder_started_at": holder.started_at,
    }


def unlock_writer_lease(
    root: Path,
    *,
    force: bool = False,
    term_wait: float = 5.0,
) -> dict[str, object]:
    """Free a stuck graph writer lease.

    Default: free if the recorded holder is dead; if ``build_status`` is in
    ``{stalled, timed_out, stale}`` or the holder age exceeds the stall timeout,
    SIGTERM → wait → SIGKILL. ``force`` kills even when progress looks healthy.
    """
    from devcouncil.codeintel.sync.lease import read_holder

    root = root.expanduser().resolve()
    lock_path = root / ".devcouncil" / "codeintel" / "writer.lock"
    holder = read_holder(lock_path)
    build = read_build_status(root)
    stall = float(build.stall_timeout_seconds or 90.0)
    pid = holder.pid if holder.pid is not None else build.pid
    started = holder.started_at if holder.started_at is not None else build.started_at
    age = (time.time() - started) if started is not None else None
    details = {
        "hint": "dev map unlock",
        "holder_pid": holder.pid,
        "holder_started_at": holder.started_at,
        "build_pid": build.pid,
        "build_state": build.state,
        "build_phase": build.phase,
        "force": force,
        "holder_age_seconds": age,
        "stall_timeout_seconds": stall,
    }
    if pid is None:
        return {"ok": True, "action": "noop", "reason": "no writer holder recorded", **details}

    if not _pid_alive(pid):
        _clear_stale_holder_metadata(lock_path)
        if build.state in {"building", "stalled", "timed_out", "stale"}:
            build.state = "idle"
            build.degraded_reason = "writer holder process is no longer running"
            build.pid = None
            _write_status(root, build)
        return {
            "ok": True,
            "action": "freed",
            "reason": "holder process is dead; lease is free",
            "target_pid": pid,
            **details,
        }

    stuck_states = {"stalled", "timed_out", "stale"}
    age_stalled = age is not None and age > stall
    stuck = build.state in stuck_states or age_stalled
    if not force and not stuck:
        return {
            "ok": False,
            "action": "refused",
            "reason": (
                "holder appears to be progressing; "
                "re-run with --force to kill anyway"
            ),
            "hint": "dev map unlock --force",
            "target_pid": pid,
            **{k: v for k, v in details.items() if k != "hint"},
        }

    killed = _terminate_pid(pid, term_wait=term_wait)
    if killed or not _pid_alive(pid):
        _clear_stale_holder_metadata(lock_path)
        build.state = "idle"
        build.degraded_reason = (
            f"unlocked via {'--force' if force else 'stall recovery'} (pid={pid})"
        )
        build.pid = None
        build.phase = "unlocked"
        build.last_progress_at = time.time()
        _write_status(root, build)
        return {
            "ok": True,
            "action": "killed",
            "reason": "terminated stuck writer holder",
            "target_pid": pid,
            **details,
        }
    return {
        "ok": False,
        "action": "failed",
        "reason": "holder still alive after SIGTERM/SIGKILL",
        "target_pid": pid,
        **details,
    }


def _stale_holder_cleared(root: Path) -> bool:
    """Clear writer-lock metadata when the recorded holder process is gone.

    Returns True when a dead holder was found (and its metadata wiped), meaning
    a retry is worth attempting. Never touches a live holder.
    """
    try:
        from devcouncil.codeintel.sync.lease import read_holder

        lock_path = root / ".devcouncil" / "codeintel" / "writer.lock"
        holder = read_holder(lock_path)
        if holder.pid is None or _pid_alive(holder.pid):
            return False
        _clear_stale_holder_metadata(lock_path)
        return True
    except Exception:
        logger.debug("stale writer-holder probe failed", exc_info=True)
        return False


def _clear_stale_holder_metadata(lock_path: Path) -> None:
    """Best-effort wipe of advisory metadata when the flock holder is gone."""
    try:
        # Only rewrite when we can take the lease; otherwise a live holder still owns it.
        from devcouncil.codeintel.sync.lease import WriterLease

        lease = WriterLease(lock_path)
        if not lease.acquire():
            return
        try:
            handle = lease._handle
            if handle is not None:
                handle.seek(0)
                handle.truncate()
                handle.flush()
        finally:
            lease.release()
    except OSError:
        return


def _terminate_pid(pid: int, *, term_wait: float = 5.0) -> bool:
    """SIGTERM → wait → SIGKILL a pid (process group when possible)."""
    if not _pid_alive(pid):
        return True

    def _signal(sig: int) -> None:
        if os.name == "nt":
            os.kill(pid, sig)
            return
        try:
            os.killpg(pid, sig)
        except (ProcessLookupError, PermissionError, OSError):
            os.kill(pid, sig)

    try:
        _signal(signal.SIGTERM)
    except ProcessLookupError:
        return True
    except OSError:
        pass
    deadline = time.monotonic() + max(0.0, term_wait)
    while time.monotonic() < deadline:
        if not _pid_alive(pid):
            return True
        time.sleep(0.05)
    try:
        kill_sig = signal.SIGKILL if hasattr(signal, "SIGKILL") else signal.SIGTERM
        _signal(kill_sig)
    except ProcessLookupError:
        return True
    except OSError:
        pass
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        if not _pid_alive(pid):
            return True
        time.sleep(0.05)
    return not _pid_alive(pid)


def _root_lock(root: Path) -> threading.RLock:
    with _LOCKS_GUARD:
        return _LOCKS.setdefault(root, threading.RLock())


def _lease_timeouts(root: Path) -> tuple[float, float]:
    """Return ``(build_timeout, sync_timeout)`` seconds for writer-lease waits."""
    build_timeout = 30.0
    sync_timeout = 5.0
    try:
        from devcouncil.app.config import load_config

        cfg = load_config(root).code_intelligence
        build_timeout = float(
            getattr(cfg, "writer_lease_timeout_seconds", build_timeout) or build_timeout
        )
        sync_timeout = float(
            getattr(cfg, "writer_lease_sync_timeout_seconds", sync_timeout) or sync_timeout
        )
    except Exception:
        pass
    return max(0.1, build_timeout), max(0.1, sync_timeout)


@contextmanager
def graph_build_session(
    root: Path,
    *,
    lease: WriterLease | None = None,
    timeout: float | None = None,
) -> Iterator[None]:
    """Hold the process-local and cross-process writer locks at outermost depth.

    ``timeout`` bounds how long a contending writer waits for ``writer.lock``
    (exponential backoff). Defaults to ``code_intelligence.writer_lease_timeout_seconds``.
    """
    from devcouncil.codeintel.sync.lease import WriterLease

    root = root.expanduser().resolve()
    lock = _root_lock(root)
    with lock:
        depths = getattr(_LOCAL, "depths", None)
        if depths is None:
            depths = {}
            _LOCAL.depths = depths
        leases = getattr(_LOCAL, "leases", None)
        if leases is None:
            leases = {}
            _LOCAL.leases = leases
        depth = int(depths.get(root, 0))
        owned_lease = None
        if depth == 0:
            owned_lease = lease or WriterLease(
                root / ".devcouncil" / "codeintel" / "writer.lock"
            )
            wait = timeout if timeout is not None else _lease_timeouts(root)[0]
            if not owned_lease.acquire_with_retry(timeout=wait):
                # A worker killed mid-build leaves advisory holder metadata behind
                # even though the OS dropped its flock. Clear it and retry once so
                # recovery does not require a manual `dev map unlock`.
                if _stale_holder_cleared(root):
                    if not owned_lease.acquire_with_retry(timeout=min(wait, 5.0)):
                        raise GraphBuildBusy(
                            "another process owns the code-intelligence writer lease"
                        )
                else:
                    raise GraphBuildBusy(
                        "another process owns the code-intelligence writer lease"
                    )
            leases[root] = owned_lease
        depths[root] = depth + 1
        try:
            yield
        finally:
            remaining = int(depths.get(root, 1)) - 1
            if remaining:
                depths[root] = remaining
            else:
                depths.pop(root, None)
                held = leases.pop(root, owned_lease)
                if held is not None:
                    held.release()


@contextmanager
def yield_writer_lease_for_child(root: Path) -> Iterator[None]:
    """Temporarily release this process's writer lease so a supervised child can own it.

    Isolated graph workers must hold ``writer.lock`` themselves. When the parent already
    owns the lease (nested ``graph_build_session``), release it for the child lifetime and
    re-acquire afterward with bounded backoff so a concurrent watcher cannot permanently
    starve the parent. If the parent does not own the lease, this is a no-op.
    """
    root = root.expanduser().resolve()
    leases = getattr(_LOCAL, "leases", {}) or {}
    held = leases.get(root)
    if held is None:
        yield
        return
    held.release()
    yield_error: BaseException | None = None
    try:
        yield
    except BaseException as exc:
        yield_error = exc
        raise
    finally:
        # Another watcher may have claimed the lock in the yield window; wait out a
        # full build-lease budget before failing the otherwise-successful child commit.
        wait = _lease_timeouts(root)[0]
        if not held.acquire_with_retry(timeout=wait):
            # Drop thread-local lease ownership so outer graph_build_session.release()
            # does not pretend we still hold a lock we failed to re-acquire.
            leases.pop(root, None)
            busy = GraphBuildBusy(
                "could not re-acquire the code-intelligence writer lease after isolated build"
            )
            # Do not silently replace GraphBuildTimeout/Failed with Busy (masks the stall).
            if isinstance(yield_error, (GraphBuildTimeout, GraphBuildFailed)):
                raise yield_error from busy
            raise busy


def _kill_worker_group(pgid: int) -> None:
    """Best-effort SIGKILL to the worker's whole process group.

    Sent even after the leader is confirmed dead: multiprocessing resource
    trackers ignore SIGTERM and pool children whose queue pipes never EOF can
    outlive the leader indefinitely (observed: stragglers idle for 2h at ppid
    1). SIGKILL cannot be ignored; ESRCH means everyone is already gone.
    """
    if os.name == "nt":
        return
    kill_sig = signal.SIGKILL if hasattr(signal, "SIGKILL") else signal.SIGTERM
    try:
        os.killpg(pgid, kill_sig)
    except (ProcessLookupError, PermissionError, OSError):
        pass


def _terminate_worker(
    process: subprocess.Popen[str],
    status: BuildStatus | None = None,
) -> bool:
    """SIGTERM → wait → SIGKILL → process.kill() fallback → wait/verify dead.

    Returns True if the worker is still alive after all kill attempts. When still
    alive, sets ``status.degraded_reason`` to ``worker_still_alive_after_kill`` and
    keeps ``status.pid`` for unlock/recovery. Whenever the leader dies here, the
    whole group still gets a SIGKILL so SIGTERM-ignoring stragglers are reaped.
    """
    if process.poll() is not None:
        return False

    try:
        if os.name == "nt":
            process.terminate()
        else:
            os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5.0)
    except (OSError, subprocess.TimeoutExpired):
        pass
    if process.poll() is not None:
        _kill_worker_group(process.pid)
        return False

    try:
        if os.name == "nt":
            process.kill()
        else:
            os.killpg(process.pid, signal.SIGKILL)
    except OSError:
        pass
    try:
        process.wait(timeout=2.0)
    except (OSError, subprocess.TimeoutExpired):
        pass
    if process.poll() is not None:
        _kill_worker_group(process.pid)
        return False

    # Last resort: Popen.kill() (covers killpg failure / Windows tree asymmetry).
    try:
        process.kill()
    except OSError:
        pass
    try:
        process.wait(timeout=2.0)
    except (OSError, subprocess.TimeoutExpired):
        pass
    if process.poll() is not None:
        _kill_worker_group(process.pid)
        return False

    if status is not None:
        status.degraded_reason = "worker_still_alive_after_kill"
        # Keep status.pid so unlock/status can still target the holder.
    return True


_BUILD_ARTIFACT_MAX_AGE_SECONDS = 2 * 86400


def gc_build_artifacts(
    root: Path,
    *,
    max_age_seconds: float = _BUILD_ARTIFACT_MAX_AGE_SECONDS,
) -> int:
    """Delete leftover build handoff files a dead supervisor never cleaned up.

    Targets ``changed-*.txt`` change-set handoffs and orphaned atomic-write
    temp files under ``.devcouncil/codeintel/``. Only files older than
    ``max_age_seconds`` go — anything younger may belong to a live build.
    Returns the number of files removed.
    """
    root = root.expanduser().resolve()
    codeintel = root / ".devcouncil" / "codeintel"
    if not codeintel.is_dir():
        return 0
    cutoff = time.time() - max(0.0, max_age_seconds)
    removed = 0
    try:
        candidates = [
            *codeintel.glob("changed-*.txt"),
            *codeintel.glob(".build_status.json.*.tmp"),
        ]
    except OSError:
        return 0
    for candidate in candidates:
        try:
            if candidate.stat().st_mtime > cutoff:
                continue
            candidate.unlink()
            removed += 1
        except OSError:
            continue
    return removed


def _worker_command(
    root: Path,
    *,
    build_id: str,
    heartbeat_interval: float,
    liveness: bool,
    changed_file: Path | None,
    total_timeout: float,
) -> list[str]:
    """Argv for the supervised graph worker.

    ``--total-timeout`` gives the worker a self-enforced deadline so an
    orphaned worker (supervisor killed) can never grind unbounded while
    holding ``writer.lock``.
    """
    command = [
        sys.executable,
        "-m",
        "devcouncil.codeintel.build_worker",
        "--root",
        str(root),
        "--build-id",
        build_id,
        "--heartbeat-interval",
        str(heartbeat_interval),
        "--total-timeout",
        str(float(total_timeout)),
    ]
    if not liveness:
        command.append("--no-liveness")
    if changed_file is not None:
        command.extend(("--changed-paths-file", str(changed_file)))
    return command


def _write_changed_paths_file(
    root: Path,
    build_id: str,
    changed_paths: set[str] | None,
) -> Path | None:
    """Persist the change set for the worker; ``None`` when there is nothing to pass.

    Newline-delimited under ``.devcouncil/codeintel/`` so the worker reads it
    with the writer lease already held and the supervisor can clean it up.
    """
    paths = sorted(changed_paths or set())
    if not paths:
        return None
    target = root / ".devcouncil" / "codeintel" / f"changed-{build_id}.txt"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("\n".join(paths) + "\n", encoding="utf-8")
    return target


def run_isolated_full_build(
    root: Path,
    *,
    changed_paths: set[str] | None = None,
    liveness: bool = True,
) -> IsolatedBuildResult:
    """Build and persist a full graph in a heartbeat-supervised child process."""
    from devcouncil.app.config import IndexingConfig, load_config

    root = root.expanduser().resolve()
    try:
        config = load_config(root).indexing
    except FileNotFoundError:
        config = IndexingConfig()
    before = get_codeintel_service(root).store.current_generation()
    build_id = uuid.uuid4().hex
    # The worker self-terminates at total_timeout + 30s, so any handoff older
    # than that belongs to a dead build (SIGKILLed hook supervisors skip their
    # ``finally`` cleanup and would otherwise litter for the 2-day default).
    gc_build_artifacts(
        root,
        max_age_seconds=2.0 * (float(config.build_total_timeout_seconds) + 30.0),
    )
    heartbeat_interval = float(
        getattr(config, "build_heartbeat_interval_seconds", 5.0) or 5.0
    )
    min_cpu_delta = float(getattr(config, "build_heartbeat_min_cpu_seconds", 0.05) or 0.0)
    # Phase counters go silent for the whole of semantic enrichment, so the stall
    # budget never sits below that (belt-and-braces behind the CPU heartbeat).
    stall_budget = (
        config.effective_stall_timeout_seconds()
        if hasattr(config, "effective_stall_timeout_seconds")
        else float(config.build_stall_timeout_seconds)
    )
    # One ``--changed-path`` per entry overflows ARG_MAX on a repo-scale change
    # set (OSError: [Errno 7] Argument list too long). Hand the worker a file.
    changed_file = _write_changed_paths_file(root, build_id, changed_paths)
    command = _worker_command(
        root,
        build_id=build_id,
        heartbeat_interval=heartbeat_interval,
        liveness=liveness,
        changed_file=changed_file,
        total_timeout=float(config.build_total_timeout_seconds),
    )
    creationflags = 0
    if os.name == "nt":
        creationflags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    process = subprocess.Popen(
        command,
        cwd=root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
        start_new_session=os.name != "nt",
        creationflags=creationflags,
    )
    now = time.time()
    status = BuildStatus(
        build_id=build_id,
        state="building",
        mode="full",
        pid=process.pid,
        phase="starting",
        started_at=now,
        last_progress_at=now,
        generation_before=before,
        stall_timeout_seconds=stall_budget,
        total_timeout_seconds=float(config.build_total_timeout_seconds),
        last_cpu_progress_at=now,
    )
    _write_status(root, status)
    messages: queue.Queue[str] = queue.Queue()
    stderr_lines: deque[str] = deque(maxlen=200)

    def _read_stdout() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            messages.put(line)

    reader = threading.Thread(target=_read_stdout, name="devcouncil-graph-worker-output", daemon=True)
    reader.start()

    def _read_stderr() -> None:
        assert process.stderr is not None
        for line in process.stderr:
            stderr_lines.append(line)

    error_reader = threading.Thread(
        target=_read_stderr,
        name="devcouncil-graph-worker-errors",
        daemon=True,
    )
    error_reader.start()
    started = time.monotonic()
    last_progress = started
    last_cpu_progress = started
    worker_cpu = 0.0
    stall = stall_budget
    total = float(config.build_total_timeout_seconds)
    final_payload: dict[str, object] = {}
    after: int | None = None
    # Child must own writer.lock; release any parent-held lease for the supervision window.
    with yield_writer_lease_for_child(root):
        try:
            while True:
                try:
                    line = messages.get(timeout=0.2)
                except queue.Empty:
                    line = ""
                if line:
                    try:
                        payload = json.loads(line)
                    except json.JSONDecodeError:
                        payload = {}
                    if payload:
                        # A heartbeat proves the worker is alive but is not phase
                        # progress: it refreshes the CPU clock only, so a truly
                        # wedged (zero-CPU) worker still trips the stall detector.
                        cpu = payload.get("cpu_seconds")
                        if isinstance(cpu, (int, float)):
                            if float(cpu) - worker_cpu >= min_cpu_delta:
                                last_cpu_progress = time.monotonic()
                                status.last_cpu_progress_at = time.time()
                            worker_cpu = max(worker_cpu, float(cpu))
                            status.worker_cpu_seconds = worker_cpu
                        if payload.get("state") != "heartbeat":
                            last_progress = time.monotonic()
                            status.last_progress_at = time.time()
                        status.phase = str(payload.get("phase") or status.phase)
                        status.completed = int(payload.get("completed") or 0)
                        status.total = int(payload.get("total") or 0)
                        status.compatibility_export = str(
                            payload.get("compatibility_export") or status.compatibility_export
                        )
                        if payload.get("state") in {"complete", "degraded"}:
                            final_payload = payload
                        _write_status(root, status)
                elapsed = time.monotonic() - started
                idle = time.monotonic() - max(last_progress, last_cpu_progress)
                if elapsed > total or idle > stall:
                    reason = (
                        f"graph build exceeded {total:.1f}s total timeout"
                        if elapsed > total
                        else (
                            f"graph build made no progress and burned no CPU for "
                            f"{stall:.1f}s (phase={status.phase!r}, "
                            f"worker_cpu={worker_cpu:.1f}s)"
                        )
                    )
                    still_alive = _terminate_worker(process, status=status)
                    status.state = "timed_out"
                    if not still_alive:
                        status.degraded_reason = reason
                        # The dead worker cannot release writer.lock's advisory
                        # metadata itself. Clearing it here is what stops every
                        # timed-out run from leaving a holder that blocks the
                        # next `dev map` until someone runs `dev map unlock`.
                        _clear_stale_holder_metadata(
                            root / ".devcouncil" / "codeintel" / "writer.lock"
                        )
                    # else: keep worker_still_alive_after_kill; preserve status.pid
                    status.generation_after = get_codeintel_service(root).store.current_generation()
                    _write_status(root, status)
                    raise GraphBuildTimeout(reason)
                if process.poll() is not None:
                    reader.join(timeout=0.2)
                    if not reader.is_alive() and messages.empty():
                        break
            reader.join(timeout=1.0)
            error_reader.join(timeout=1.0)
            after = get_codeintel_service(root).store.current_generation()
            # Generation advanced is success even when returncode != 0 (post-commit kill).
            advanced = after is not None and (before is None or after > before)
            if not advanced:
                stderr = "".join(stderr_lines).strip()
                reason = stderr[-4000:] or f"graph worker exited with code {process.returncode}"
                status.state = "failed"
                status.degraded_reason = reason
                status.generation_after = after
                _write_status(root, status)
                raise GraphBuildFailed(reason)
            status.state = str(final_payload.get("state") or "complete")
            status.phase = "complete"
            status.completed = status.total or 1
            status.total = status.total or 1
            status.generation_after = after
            status.compatibility_export = str(
                final_payload.get("compatibility_export") or "healthy"
            )
            status.degraded_reason = str(final_payload.get("reason") or "")
            _write_status(root, status)
        finally:
            if process.poll() is None:
                _terminate_worker(process, status=status)
            if changed_file is not None:
                changed_file.unlink(missing_ok=True)
    # Reload under the re-acquired lease so a concurrent watcher that wrote during
    # the yield window cannot leave the parent holding a stale in-memory graph.
    after = get_codeintel_service(root).store.current_generation()
    status.generation_after = after
    graph = get_codeintel_service(root).load()
    if graph is None:
        raise GraphBuildFailed("graph worker committed a generation that could not be loaded")
    _write_status(root, status)
    return IsolatedBuildResult(graph=graph, status=status)
