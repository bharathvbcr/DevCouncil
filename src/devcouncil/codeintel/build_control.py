"""Single-writer graph build sessions and supervised full-build isolation."""

from __future__ import annotations

import json
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
            elif (
                status.last_progress_at is not None
                and time.time() - status.last_progress_at > status.stall_timeout_seconds
            ):
                status.state = "stalled"
                status.degraded_reason = (
                    f"no recorded graph progress for {status.stall_timeout_seconds:.1f}s"
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
) -> BuildStatus:
    now = time.time()
    status = BuildStatus(
        build_id=uuid.uuid4().hex,
        state=state,
        mode=mode,
        pid=os.getpid(),
        phase=phase,
        completed=1,
        total=1,
        started_at=now,
        last_progress_at=now,
        generation_before=generation_before,
        generation_after=generation_after,
        degraded_reason=reason,
        compatibility_export=compatibility_export,
        stall_timeout_seconds=90.0,
        total_timeout_seconds=900.0,
    )
    _write_status(root.expanduser().resolve(), status)
    return status


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
                raise GraphBuildBusy("another process owns the code-intelligence writer lease")
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
    try:
        yield
    finally:
        # Another watcher may have claimed the lock in the yield window; wait out a
        # full build-lease budget before failing the otherwise-successful child commit.
        wait = _lease_timeouts(root)[0]
        if not held.acquire_with_retry(timeout=wait):
            raise GraphBuildBusy(
                "could not re-acquire the code-intelligence writer lease after isolated build"
            )


def _terminate_worker(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    try:
        if os.name == "nt":
            process.terminate()
        else:
            os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5.0)
    except (OSError, subprocess.TimeoutExpired):
        try:
            if os.name == "nt":
                process.kill()
            else:
                os.killpg(process.pid, signal.SIGKILL)
        except OSError:
            pass


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
    command = [
        sys.executable,
        "-m",
        "devcouncil.codeintel.build_worker",
        "--root",
        str(root),
        "--build-id",
        build_id,
    ]
    if not liveness:
        command.append("--no-liveness")
    for path in sorted(changed_paths or set()):
        command.extend(("--changed-path", path))
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
        stall_timeout_seconds=float(config.build_stall_timeout_seconds),
        total_timeout_seconds=float(config.build_total_timeout_seconds),
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
    stall = float(config.build_stall_timeout_seconds)
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
                        last_progress = time.monotonic()
                        status.phase = str(payload.get("phase") or status.phase)
                        status.completed = int(payload.get("completed") or 0)
                        status.total = int(payload.get("total") or 0)
                        status.last_progress_at = time.time()
                        status.compatibility_export = str(
                            payload.get("compatibility_export") or status.compatibility_export
                        )
                        if payload.get("state") in {"complete", "degraded"}:
                            final_payload = payload
                        _write_status(root, status)
                elapsed = time.monotonic() - started
                idle = time.monotonic() - last_progress
                if elapsed > total or idle > stall:
                    reason = (
                        f"graph build exceeded {total:.1f}s total timeout"
                        if elapsed > total
                        else f"graph build made no progress for {stall:.1f}s"
                    )
                    _terminate_worker(process)
                    status.state = "timed_out"
                    status.degraded_reason = reason
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
            if process.returncode != 0 or after is None or after == before:
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
                _terminate_worker(process)
    # Reload under the re-acquired lease so a concurrent watcher that wrote during
    # the yield window cannot leave the parent holding a stale in-memory graph.
    after = get_codeintel_service(root).store.current_generation()
    status.generation_after = after
    graph = get_codeintel_service(root).load()
    if graph is None:
        raise GraphBuildFailed("graph worker committed a generation that could not be loaded")
    _write_status(root, status)
    return IsolatedBuildResult(graph=graph, status=status)
