"""Filesystem change attribution with polling fallback."""

from __future__ import annotations

import time
import uuid
from pathlib import Path
from typing import Callable

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.execution.policy_engine import TaskPolicyEngine
from devcouncil.storage.db import get_db
from devcouncil.storage.native import FileChangeRepository, TaskLeaseRepository
from devcouncil.storage.repositories import GapRepository, TaskRepository
from devcouncil.verification.verifier import Verifier

_IGNORED_PREFIXES = (
    ".git/",
    ".devcouncil/cache/",
    ".devcouncil/logs/",
    "__pycache__/",
    ".pytest_cache/",
    ".mypy_cache/",
    ".ruff_cache/",
    "dist/",
    "build/",
    "target/",
    "node_modules/",
)

# Event mode must additionally ignore everything DevCouncil writes while
# recording events (state DB, traces, run artifacts) — otherwise recording a
# file-change event triggers another filesystem event, feeding back forever.
_EVENT_IGNORED_PREFIXES = _IGNORED_PREFIXES + (".devcouncil/", ".gitignore")

_TASK_CACHE_TTL_SECONDS = 10.0
_EVENT_DEBOUNCE_SECONDS = 0.5


class FilesystemWatcher:
    def __init__(
        self,
        project_root: Path,
        task_id: str,
        *,
        poll_interval: float = 1.0,
        on_event: Callable[[dict], None] | None = None,
    ):
        self.project_root = project_root.resolve()
        self.task_id = task_id
        self.poll_interval = poll_interval
        self.on_event = on_event
        self.policy = TaskPolicyEngine(self.project_root)
        self._seen: dict[str, float] = {}
        self._task_cache: tuple[float, Task | None] | None = None

    def should_ignore(self, path: str) -> bool:
        normalized = path.replace("\\", "/")
        return any(normalized.startswith(prefix) for prefix in _IGNORED_PREFIXES)

    def scan_once(self) -> list[dict]:
        task = self._load_task()
        changed = Verifier(self.project_root).get_changed_files()
        events: list[dict] = []
        for path in changed:
            if self.should_ignore(path):
                continue
            events.append(self._record_path(path, task, operation="modify"))
        return events

    def watch(self) -> None:
        observer = self._start_event_observer()
        if observer is None:
            # Polling fallback when watchdog is unavailable.
            while True:
                for event in self.scan_once():
                    self._notify(event)
                time.sleep(self.poll_interval)
        try:
            while True:
                time.sleep(self.poll_interval)
        finally:
            observer.stop()
            observer.join(timeout=5)

    def _start_event_observer(self):
        try:
            from watchdog.events import FileSystemEventHandler
            from watchdog.observers import Observer
        except ImportError:
            return None

        watcher = self

        class _Handler(FileSystemEventHandler):
            def on_any_event(self, event):
                if getattr(event, "is_directory", False):
                    return
                operation = {"created": "create", "deleted": "delete"}.get(event.event_type, "modify")
                # For moves, attribute the destination path.
                path = getattr(event, "dest_path", "") or event.src_path
                watcher.handle_event(str(path), operation=operation)

        observer = Observer()
        observer.schedule(_Handler(), str(self.project_root), recursive=True)
        observer.start()
        return observer

    def handle_event(self, path: str, *, operation: str = "modify") -> dict | None:
        """Attribute one raw filesystem event (event-driven mode)."""
        try:
            rel = Path(path).resolve().relative_to(self.project_root).as_posix()
        except (ValueError, OSError):
            return None
        if any(rel.startswith(prefix) for prefix in _EVENT_IGNORED_PREFIXES):
            return None
        if self._debounced(rel):
            return None
        event = self._record_path(rel, self._task_cached(), operation=operation)
        self._notify(event)
        return event

    def _notify(self, event: dict) -> None:
        if self.on_event is not None:
            self.on_event(event)

    def _debounced(self, rel: str, *, window: float = _EVENT_DEBOUNCE_SECONDS) -> bool:
        now = time.monotonic()
        last = self._seen.get(rel)
        self._seen[rel] = now
        return last is not None and (now - last) < window

    def _task_cached(self) -> Task | None:
        now = time.monotonic()
        if self._task_cache is not None and now - self._task_cache[0] < _TASK_CACHE_TTL_SECONDS:
            return self._task_cache[1]
        task = self._load_task()
        self._task_cache = (now, task)
        return task

    def _load_task(self) -> Task | None:
        db = get_db(self.project_root)
        if not db:
            return None
        with db.get_session() as session:
            return TaskRepository(session).get_by_id(self.task_id)

    def _record_path(self, path: str, task: Task | None, *, operation: str) -> dict:
        decision = self.policy.evaluate_file_change(path, task, operation=operation)  # type: ignore[arg-type]
        allowed = decision.action in {"allow", "warn"}
        db = get_db(self.project_root)
        if db:
            # Resolve the lease and record the event in one session so the
            # recorded lease_id cannot go stale between lookups.
            with db.get_session() as session:
                active = TaskLeaseRepository(session).active_for_task(self.task_id)
                lease_id = active.id if active else None
                FileChangeRepository(session).record(
                    path,
                    operation,
                    allowed,
                    task_id=self.task_id,
                    lease_id=lease_id,
                    reason=decision.reason,
                )
                if not allowed:
                    gap_repo = GapRepository(session)
                    gap_repo.save(
                        Gap(
                            id=f"GAP-{self.task_id}-ORPHAN-{uuid.uuid4().hex[:12]}",
                            severity="high",
                            gap_type="orphan_diff",
                            task_id=self.task_id,
                            description=f"Unplanned file change: {path}",
                            recommended_fix="Revert change or update planned_files.",
                            blocking=True,
                        )
                    )
        return {"path": path, "operation": operation, "allowed": allowed, "reason": decision.reason}
