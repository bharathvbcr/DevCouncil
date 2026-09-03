"""Single-writer graph build sessions and writer-lease recovery.

The supervised full-build isolation that used to live here (``build_worker``,
``run_isolated_full_build`` and its kill/GC/heartbeat helpers) supervised the
retired Python graph engine. The Rust kernel builds in its own process and owns
the same ``writer.lock``, so what remains is the lease itself plus the status
file ``dev map status`` / ``dev map unlock`` read.
"""

from __future__ import annotations

import logging
import os
import signal
import threading
import time
from contextlib import contextmanager
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterator

from devcouncil.codeintel.sync.lease import WriterLease, read_holder
from devcouncil.utils.json_persist import read_json, write_json

logger = logging.getLogger(__name__)

STATUS_REL = Path(".devcouncil") / "codeintel" / "build_status.json"


class GraphBuildBusy(RuntimeError):
    """Another thread or process owns the graph writer session."""


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
