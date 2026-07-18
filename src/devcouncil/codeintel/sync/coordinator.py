"""Debounced native watcher with reconciliation and a single writer."""

from __future__ import annotations

import logging
import subprocess
import sys
import threading
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable

from devcouncil.codeintel.service import CodeIntelService, get_codeintel_service
from devcouncil.codeintel.sync.lease import WriterLease
from devcouncil.codeintel.sync.scope import IndexScope

logger = logging.getLogger(__name__)


def _fsevents_preflight(root: Path) -> bool:
    """Probe FSEvents out of process because startup failure may abort Python.

    Retries once: under heavy reconcile/build load the first probe can time out
    even when FSEvents is healthy, which previously forced a permanent Kqueue
    fallback for the process lifetime.
    """
    script = (
        "import sys,time; "
        "from watchdog.events import FileSystemEventHandler; "
        "from watchdog.observers import Observer; "
        "o=Observer(); o.schedule(FileSystemEventHandler(),sys.argv[1],recursive=True); "
        "o.start(); time.sleep(0.35); "
        "emitters=list(o.emitters); "
        "ok=bool(emitters) and all(e.is_alive() for e in emitters); "
        "o.stop(); o.join(2); raise SystemExit(0 if ok else 3)"
    )
    last_error = ""
    for attempt in range(2):
        try:
            completed = subprocess.run(
                [sys.executable, "-c", script, str(root)],
                capture_output=True,
                timeout=5.0,
                check=False,
            )
        except subprocess.TimeoutExpired:
            last_error = "timeout"
            time.sleep(0.2)
            continue
        except OSError as exc:
            last_error = str(exc)
            break
        if completed.returncode == 0:
            return True
        stderr = getattr(completed, "stderr", None) or b""
        if isinstance(stderr, str):
            stderr_text = stderr.strip()
        else:
            stderr_text = stderr.decode("utf-8", errors="replace").strip()
        last_error = stderr_text or f"exit {completed.returncode}"
        time.sleep(0.2)
    if last_error:
        logger.info("FSEvents preflight failed: %s", last_error)
    return False


@dataclass
class SyncState:
    state: str = "disabled"
    backend: str = ""
    backend_kind: str = ""
    generation: int | None = None
    pending: list[str] = field(default_factory=list)
    last_sync_at: float | None = None
    last_reconcile_at: float | None = None
    degraded_reason: str = ""
    last_error: str = ""
    build_id: str = ""
    build_state: str = "idle"
    build_pid: int | None = None
    build_phase: str = ""
    build_completed: int = 0
    build_total: int = 0
    build_last_progress_at: float | None = None
    generation_before: int | None = None
    generation_after: int | None = None
    compatibility_export: str = "unknown"

    def as_dict(self) -> dict[str, object]:
        return asdict(self)


class SyncCoordinator:
    def __init__(
        self,
        service: CodeIntelService,
        *,
        debounce_seconds: float = 0.75,
        reconcile_seconds: float = 60.0,
        sync_callback: Callable[[list[str]], object] | None = None,
        allow_polling_fallback: bool = True,
    ):
        self.service = service
        self.root = service.project_root
        self.scope = IndexScope(self.root)
        self.debounce_seconds = max(0.1, min(60.0, debounce_seconds))
        self.reconcile_seconds = max(1.0, reconcile_seconds)
        self.sync_callback = sync_callback or self._default_sync
        self.allow_polling_fallback = allow_polling_fallback
        self._state = SyncState(generation=service.store.current_generation())
        self._pending: set[str] = set()
        self._pending_since: float | None = None
        self._condition = threading.Condition()
        self._stop = threading.Event()
        self._observer: Any = None
        self._worker: threading.Thread | None = None
        self._sync_lock = threading.Lock()
        self._lease = WriterLease(self.root / ".devcouncil" / "codeintel" / "writer.lock")

    def start(self) -> SyncState:
        if self._worker is not None and self._worker.is_alive():
            return self.status()
        self._stop.clear()
        self._start_observer()
        self._worker = threading.Thread(target=self._run, name="devcouncil-codeintel-sync", daemon=True)
        self._worker.start()
        return self.status()

    def stop(self, *, timeout: float = 5.0) -> None:
        self._stop.set()
        with self._condition:
            self._condition.notify_all()
        observer = self._observer
        if observer is not None:
            observer.stop()
            observer.join(timeout=timeout)
            self._observer = None
        if self._worker is not None:
            self._worker.join(timeout=timeout)
            if self._worker.is_alive():
                self._state.state = "stopping"
                self._state.degraded_reason = "sync worker did not stop before timeout"
                return
            self._worker = None
        if self._sync_lock.locked():
            self._state.state = "stopping"
            self._state.degraded_reason = "an in-flight sync is still stopping"
            return
        self._state.state = "disabled"

    def status(self) -> SyncState:
        from devcouncil.codeintel.build_control import read_build_status

        build = read_build_status(self.root)
        with self._condition:
            if (
                self._worker is not None
                and not self._worker.is_alive()
                and self._stop.is_set()
            ):
                self._worker = None
                if self._state.state == "stopping":
                    self._state.state = "disabled"
            elif (
                self._worker is None
                and self._stop.is_set()
                and not self._sync_lock.locked()
                and self._state.state == "stopping"
            ):
                self._state.state = "disabled"
            self._state.pending = sorted(self._pending)
            self._state.generation = self.service.store.current_generation()
            self._state.build_id = build.build_id
            self._state.build_state = build.state
            self._state.build_pid = build.pid
            self._state.build_phase = build.phase
            self._state.build_completed = build.completed
            self._state.build_total = build.total
            self._state.build_last_progress_at = build.last_progress_at
            self._state.generation_before = build.generation_before
            self._state.generation_after = build.generation_after
            self._state.compatibility_export = build.compatibility_export
            return SyncState(**asdict(self._state))

    def mark_pending(self, path: str | Path) -> None:
        try:
            rel = self.scope.relative(path)
        except ValueError:
            return
        # Deletions cannot pass an existence-based check, so extension + prefix
        # filtering is enough for paths that were present in the committed index.
        if not self.scope.includes(rel) and not self.service.store.has_indexed_path(rel):
            return
        with self._condition:
            self._pending.add(rel)
            self._pending_since = self._pending_since or time.monotonic()
            self._state.state = "pending"
            self._condition.notify_all()

    def reconcile(self) -> list[str]:
        indexed = self.service.store.file_metadata()
        current: dict[str, tuple[int, int]] = {}
        for rel in self.scope.files():
            try:
                stat = (self.root / rel).stat()
            except OSError:
                continue
            current[rel] = (stat.st_size, stat.st_mtime_ns)
        changed = {
            rel
            for rel, values in current.items()
            if rel not in indexed or values != indexed[rel][:2]
        }
        changed.update(set(indexed) - set(current))
        if changed:
            # Every item came from IndexScope.files() or the persisted index, so
            # repeating per-path git ignore probes through mark_pending() is both
            # redundant and expensive.
            with self._condition:
                self._pending.update(changed)
                self._pending_since = self._pending_since or time.monotonic()
                self._state.state = "pending"
                self._condition.notify_all()
        self._state.last_reconcile_at = time.time()
        return sorted(changed)

    def sync_now(self, paths: list[str] | None = None) -> bool:
        from devcouncil.codeintel.build_control import (
            GraphBuildBusy,
            _lease_timeouts,
            graph_build_session,
        )

        with self._sync_lock:
            if paths:
                for path in paths:
                    self.mark_pending(path)
            with self._condition:
                batch = sorted(self._pending)
            if not batch:
                return True
            try:
                # Short bounded wait: another watcher/MCP writer often holds the
                # lease briefly. Pending stays queued on timeout so the debounce
                # loop retries instead of clearing work as healthy.
                sync_timeout = _lease_timeouts(self.root)[1]
                with graph_build_session(
                    self.root, lease=self._lease, timeout=sync_timeout
                ):
                    self._state.state = "syncing"
                    self.sync_callback(batch)
                with self._condition:
                    self._pending.difference_update(batch)
                    self._pending_since = None if not self._pending else time.monotonic()
                if self._stop.is_set():
                    self._state.state = "stopping"
                else:
                    # Drop transient lease-busy reasons so a later successful sync
                    # does not stay permanently ``degraded`` / ``read_only``.
                    if "writer lease" in (self._state.degraded_reason or "").lower():
                        self._state.degraded_reason = ""
                    self._state.state = (
                        "healthy" if not self._state.degraded_reason else "degraded"
                    )
                self._state.last_sync_at = time.time()
                self._state.last_error = ""
                self._state.generation = self.service.store.current_generation()
                return True
            except GraphBuildBusy as exc:
                # Keep pending queued; debounce/reconcile will retry after backoff.
                self._state.state = "pending" if batch or self._pending else "read_only"
                self._state.degraded_reason = str(exc)
                return False
            except Exception as exc:
                logger.exception("code-intelligence sync failed")
                self._state.state = "degraded"
                self._state.last_error = f"{type(exc).__name__}: {exc}"
                return False

    def wait_until_fresh(self, *, timeout: float = 2.0) -> bool:
        deadline = time.monotonic() + max(0.0, timeout)
        while time.monotonic() < deadline:
            if not self.status().pending:
                return True
            self.sync_now()
            if not self.status().pending:
                return True
            time.sleep(0.05)
        return not self.status().pending

    def _run(self) -> None:
        # Initial reconciliation is intentionally asynchronous.  MCP clients must
        # be able to complete initialize/list_tools immediately while the index is
        # brought current in the background.
        try:
            self.reconcile()
        except Exception as exc:  # noqa: BLE001
            logger.exception("initial code-intelligence reconciliation failed")
            self._state.state = "degraded"
            self._state.last_error = f"{type(exc).__name__}: {exc}"
        next_reconcile = time.monotonic() + self.reconcile_seconds
        while not self._stop.is_set():
            with self._condition:
                self._condition.wait(timeout=min(self.debounce_seconds, 1.0))
                pending_since = self._pending_since
            now = time.monotonic()
            if pending_since is not None and now - pending_since >= self.debounce_seconds:
                self.sync_now()
            if now >= next_reconcile:
                self.reconcile()
                next_reconcile = now + self.reconcile_seconds

    def _start_observer(self) -> None:
        try:
            from watchdog.events import FileSystemEventHandler
            from watchdog.observers import Observer
            from watchdog.observers.polling import PollingObserver

            coordinator = self

            class Handler(FileSystemEventHandler):
                def on_any_event(self, event):  # noqa: ANN001
                    if getattr(event, "is_directory", False):
                        return
                    source = getattr(event, "src_path", "")
                    destination = getattr(event, "dest_path", "")
                    if source:
                        coordinator.mark_pending(source)
                    if destination:
                        coordinator.mark_pending(destination)

            candidates: list[tuple[str, Callable[[], Any]]] = [("native", Observer)]
            try:
                from watchdog.observers.kqueue import KqueueObserver

                if KqueueObserver is not Observer:
                    candidates.append(("native-fallback", KqueueObserver))
            except (ImportError, AttributeError):
                pass

            if self.allow_polling_fallback:
                candidates.append((
                    "polling",
                    lambda: PollingObserver(timeout=max(1.0, self.debounce_seconds)),
                ))

            observer = None
            failures: list[str] = []
            backend = ""
            selected_kind = ""
            for kind, factory in candidates:
                candidate = factory()
                try:
                    if candidate.__class__.__name__ == "FSEventsObserver" and not _fsevents_preflight(self.root):
                        raise RuntimeError("FSEvents preflight failed in isolated process")
                    candidate.schedule(Handler(), str(self.root), recursive=True)
                    candidate.start()
                    emitters = list(getattr(candidate, "emitters", []))
                    if emitters:
                        # FSEvents can report startup failure only from its
                        # emitter thread, after Observer.start() has returned.
                        time.sleep(0.1)
                        if not all(emitter.is_alive() for emitter in emitters):
                            raise RuntimeError("observer emitter terminated during startup")
                    observer = candidate
                    backend = candidate.__class__.__name__
                    selected_kind = kind
                    break
                except Exception as exc:
                    failures.append(f"{candidate.__class__.__name__}: {type(exc).__name__}: {exc}")
                    try:
                        candidate.stop()
                        candidate.join(timeout=1.0)
                    except Exception:
                        logger.debug("failed to stop rejected observer", exc_info=True)
            if observer is None:
                raise RuntimeError("; ".join(failures) or "no observer backend available")
            if failures or selected_kind == "polling":
                self._state.degraded_reason = "; ".join(failures) or "polling fallback selected"
                logger.warning(
                    "filesystem observer degraded to %s: %s",
                    backend,
                    self._state.degraded_reason,
                )
            self._observer = observer
            self._state.backend = backend
            self._state.backend_kind = "polling" if selected_kind == "polling" else "native"
            self._state.state = "degraded" if self._state.degraded_reason else "healthy"
        except Exception as exc:
            self._observer = None
            self._state.state = "degraded"
            self._state.degraded_reason = f"watcher unavailable: {type(exc).__name__}: {exc}"

    def _default_sync(self, paths: list[str]) -> None:
        # Keep graph watch and ``dev map --watch`` on the same path: incremental
        # graph sync plus a full repo-map rebuild (subsystems/dependents/guides).
        from devcouncil.indexing.graph.build import refresh_map_for_paths

        refresh_map_for_paths(self.service.project_root, paths, liveness=True)


_COORDINATORS: dict[Path, SyncCoordinator] = {}
_COORDINATORS_LOCK = threading.Lock()


def get_sync_coordinator(root: Path, **kwargs: object) -> SyncCoordinator:
    service = get_codeintel_service(root)
    with _COORDINATORS_LOCK:
        coordinator = _COORDINATORS.get(service.project_root)
        if coordinator is None:
            coordinator = SyncCoordinator(service, **kwargs)  # type: ignore[arg-type]
            _COORDINATORS[service.project_root] = coordinator
        else:
            for name, requested in kwargs.items():
                current = getattr(coordinator, name)
                if current != requested:
                    raise ValueError(
                        f"sync coordinator for {service.project_root} already uses "
                        f"a different {name}"
                    )
        return coordinator


def stop_all_coordinators() -> None:
    with _COORDINATORS_LOCK:
        coordinators = list(_COORDINATORS.values())
        _COORDINATORS.clear()
    for coordinator in coordinators:
        coordinator.stop()
