from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.fs_watcher import FilesystemWatcher
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import GapRepository, TaskRepository


def _setup(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="T",
                description="D",
                planned_files=[PlannedFile(path="src/app.py", reason="x", allowed_change="modify")],
            )
        )
    return db


def test_ignored_paths_do_not_record_events(tmp_path, monkeypatch):
    _setup(tmp_path)
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    assert watcher.should_ignore(".git/index")
    assert watcher.should_ignore("node_modules/pkg/index.js")


def test_unplanned_write_records_denied_event_and_gap(tmp_path, monkeypatch):
    _setup(tmp_path)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "other.py").write_text("x", encoding="utf-8")
    monkeypatch.setattr(
        "devcouncil.execution.fs_watcher.Verifier.get_changed_files",
        lambda self: ["src/other.py"],
    )
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    event = watcher.scan_once()[0]
    assert event["allowed"] is False
    with Database(tmp_path / ".devcouncil" / "state.sqlite").get_session() as session:
        gaps = GapRepository(session).get_all()
        assert any(g.gap_type == "orphan_diff" for g in gaps)


def test_handle_event_records_change_and_debounces(tmp_path):
    _setup(tmp_path)
    (tmp_path / "src").mkdir()
    target = tmp_path / "src" / "app.py"
    target.write_text("x", encoding="utf-8")
    watcher = FilesystemWatcher(tmp_path, "TASK-001")

    event = watcher.handle_event(str(target), operation="modify")
    assert event is not None
    assert event["path"] == "src/app.py"
    assert event["allowed"] is True

    # An immediate duplicate event for the same path is debounced.
    assert watcher.handle_event(str(target), operation="modify") is None


def test_handle_event_ignores_devcouncil_runtime_state(tmp_path):
    _setup(tmp_path)
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    state_db = tmp_path / ".devcouncil" / "state.sqlite"
    assert watcher.handle_event(str(state_db), operation="modify") is None


def test_handle_event_ignores_paths_outside_project_root(tmp_path):
    _setup(tmp_path)
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    assert watcher.handle_event(str(tmp_path.parent / "outside.txt"), operation="modify") is None


def test_start_event_observer_returns_running_observer(tmp_path):
    _setup(tmp_path)
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    observer = watcher._start_event_observer()
    assert observer is not None
    try:
        assert observer.is_alive()
    finally:
        observer.stop()
        observer.join(timeout=5)


def test_live_stub_scan_records_advisory_gap(tmp_path, monkeypatch):
    _setup(tmp_path)
    src = tmp_path / "src"
    src.mkdir()
    (src / "app.py").write_text("def f():\n    pass\n", encoding="utf-8")
    diff = (
        "--- a/src/app.py\n"
        "+++ b/src/app.py\n"
        "@@ -0,0 +1,2 @@\n"
        "+def f():\n"
        "+    pass\n"
    )
    monkeypatch.setattr(
        "devcouncil.execution.fs_watcher.Verifier.get_changed_files",
        lambda self: ["src/app.py"],
    )
    monkeypatch.setattr(
        "devcouncil.execution.fs_watcher.Verifier.get_diff",
        lambda self: diff,
    )
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    watcher.scan_once()
    with Database(tmp_path / ".devcouncil" / "state.sqlite").get_session() as session:
        gaps = GapRepository(session).get_all()
        assert any(g.gap_type == "stub_detected" and not g.blocking for g in gaps)


def test_notify_callback_invoked(tmp_path):
    _setup(tmp_path)
    (tmp_path / "src").mkdir()
    target = tmp_path / "src" / "app.py"
    target.write_text("x", encoding="utf-8")
    events = []
    watcher = FilesystemWatcher(tmp_path, "TASK-001", on_event=events.append)
    watcher.handle_event(str(target), operation="modify")
    assert len(events) == 1


def test_debounced_evicts_stale_entries(tmp_path, monkeypatch):
    _setup(tmp_path)
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    import devcouncil.execution.fs_watcher as fw

    now = {"t": 100.0}
    monkeypatch.setattr(fw.time, "monotonic", lambda: now["t"])

    assert watcher._debounced("src/a.py") is False
    now["t"] = 100.1
    assert watcher._debounced("src/a.py") is True
    now["t"] = 101.0
    watcher._debounced("src/b.py")
    assert "src/a.py" not in watcher._seen or watcher._seen["src/a.py"] == 101.0


def test_task_cached_reuses_within_ttl(tmp_path):
    _setup(tmp_path)
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    first = watcher._task_cached()
    second = watcher._task_cached()
    assert first is not None
    assert second is first


def test_scan_stubs_live_throttled_and_no_task(tmp_path, monkeypatch):
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    watcher._last_stub_scan = 9999999999.0
    watcher._scan_stubs_live(None)

    _setup(tmp_path)
    watcher = FilesystemWatcher(tmp_path, "TASK-001")
    monkeypatch.setattr(
        "devcouncil.execution.fs_watcher.Verifier.get_diff",
        lambda self: "",
    )
    watcher._scan_stubs_live(watcher._task_cached())


def test_scan_stubs_live_get_diff_exception(tmp_path, monkeypatch):
    _setup(tmp_path)
    watcher = FilesystemWatcher(tmp_path, "TASK-001")

    def _boom(self):
        raise RuntimeError("no git")

    monkeypatch.setattr(
        "devcouncil.execution.fs_watcher.Verifier.get_diff",
        _boom,
    )
    watcher._scan_stubs_live(watcher._task_cached())
