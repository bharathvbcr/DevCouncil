"""Filesystem change attribution with polling fallback."""

from __future__ import annotations

import logging
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
from devcouncil.verification.stub_detector import detect_stubs, task_allows_scaffolding
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
_STUB_SCAN_INTERVAL_SECONDS = 3.0

logger = logging.getLogger(__name__)


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
        self._seen_last_cleanup: float = 0.0
        self._task_cache: tuple[float, Task | None] | None = None
        self._last_stub_scan: float = 0.0
        self._reported_stub_keys: set[tuple[str, int, str]] = set()
        # Reuse one Verifier across polls (avoids rebuilding its scanners each tick) but
        # still run get_changed_files() fresh every poll — caching the git result would
        # make the watcher miss changes for the cache TTL, defeating the poll interval.
        self._verifier: Verifier | None = None

    def should_ignore(self, path: str) -> bool:
        normalized = path.replace("\\", "/")
        return any(normalized.startswith(prefix) for prefix in _IGNORED_PREFIXES)

    def scan_once(self) -> list[dict]:
        task = self._task_cached()
        changed = self._changed_files()
        events: list[dict] = []
        for path in changed:
            if self.should_ignore(path):
                continue
            events.append(self._record_path(path, task, operation="modify"))
        self._scan_stubs_live(task)
        return events

    def watch(self) -> None:
        observer = self._start_event_observer()
        mode = "polling" if observer is None else "event-driven (watchdog)"
        logger.info("Filesystem watcher started for %s in %s mode", self.task_id, mode)
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
        self._scan_stubs_live(self._task_cached())
        return event

    def _notify(self, event: dict) -> None:
        if self.on_event is not None:
            self.on_event(event)

    def _debounced(self, rel: str, *, window: float = _EVENT_DEBOUNCE_SECONDS) -> bool:
        now = time.monotonic()
        last = self._seen.get(rel)
        self._seen[rel] = now
        # Periodically evict stale entries so _seen doesn't grow unbounded in long watch
        # sessions. Anything older than the debounce window can never suppress a future
        # event, so dropping it leaves dedup behavior unchanged. Run at most once/window.
        if now - self._seen_last_cleanup >= window:
            self._seen = {key: ts for key, ts in self._seen.items() if now - ts < window}
            self._seen_last_cleanup = now
        return last is not None and (now - last) < window

    def _task_cached(self) -> Task | None:
        now = time.monotonic()
        if self._task_cache is not None and now - self._task_cache[0] < _TASK_CACHE_TTL_SECONDS:
            return self._task_cache[1]
        task = self._load_task()
        self._task_cache = (now, task)
        return task

    def _changed_files(self) -> list[str]:
        if self._verifier is None:
            self._verifier = Verifier(self.project_root)
        return self._verifier.get_changed_files()

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
                    logger.warning(
                        "Orphan diff for %s: %s %s (%s)",
                        self.task_id, operation, path, decision.reason,
                    )
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

    def _scan_stubs_live(self, task: Task | None) -> None:
        """Run stub detection on the current working-tree diff during execution.

        Surfaces non-blocking ``stub_detected`` gaps mid-session — cheaper to fix
        than waiting for ``dev verify``. Throttled so rapid saves do not spam gaps.
        """
        now = time.monotonic()
        if now - self._last_stub_scan < _STUB_SCAN_INTERVAL_SECONDS:
            return
        self._last_stub_scan = now
        if task is None:
            return
        if self._verifier is None:
            self._verifier = Verifier(self.project_root)
        try:
            diff = self._verifier.get_diff()
        except Exception:
            logger.debug("Live stub scan skipped: could not load diff", exc_info=True)
            return
        if not diff.strip():
            return
        scaffolding_ok = task_allows_scaffolding(task)
        findings = detect_stubs(self.project_root, diff, honor_allow_stub=scaffolding_ok)
        if not findings:
            return
        db = get_db(self.project_root)
        if not db:
            return
        with db.get_session() as session:
            gap_repo = GapRepository(session)
            for finding in findings:
                key = (finding.file, finding.line, finding.reason)
                if key in self._reported_stub_keys:
                    continue
                self._reported_stub_keys.add(key)
                gap_repo.save(
                    Gap(
                        id=f"GAP-{self.task_id}-LIVESTUB-{uuid.uuid4().hex[:10]}",
                        severity="medium",
                        gap_type="stub_detected",
                        task_id=self.task_id,
                        description=(
                            f"Live stub/placeholder at {finding.file}:{finding.line}: {finding.reason}."
                        ),
                        evidence=[f"{finding.file}:{finding.line}", finding.snippet],
                        recommended_fix=(
                            f"Replace the placeholder at {finding.file}:{finding.line} before continuing."
                        ),
                        blocking=False,
                        file=finding.file,
                        line=finding.line,
                    )
                )
                logger.info(
                    "Live stub detected for %s at %s:%s (%s)",
                    self.task_id, finding.file, finding.line, finding.reason,
                )
