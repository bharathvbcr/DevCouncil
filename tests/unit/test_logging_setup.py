import json
import logging

import pytest

from devcouncil.telemetry.logging_setup import (
    LOG_RELATIVE_PATH,
    configure_logging,
    run_log,
    set_log_dir,
)
from devcouncil.telemetry.stages import log_stage, log_step


@pytest.fixture(autouse=True)
def _no_log_dir_override(monkeypatch):
    """These tests assert the DEFAULT path resolution (root / LOG_RELATIVE_PATH).

    The session-wide DEVCOUNCIL_LOG_DIR isolation fixture would otherwise win.
    Each test here binds to its own tmp_path, so no repo pollution either way.
    """
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)


@pytest.fixture(autouse=True)
def _isolated_root_logger():
    """Give each test a clean root logger.

    configure_logging mutates the process-global root logger and is idempotent, so a
    file handler installed by an earlier test (e.g. via a CLI invocation) would make
    this test's configure_logging reuse it instead of binding to this test's tmp_path.
    Detach DevCouncil handlers before the test and restore the original set after.
    """
    root = logging.getLogger()
    saved_handlers = root.handlers[:]
    saved_level = root.level
    root.handlers[:] = [
        h for h in root.handlers if getattr(h, "_devcouncil_tag", None) is None
    ]
    yield
    root.handlers[:] = saved_handlers
    root.setLevel(saved_level)


def _devcouncil_handlers(root):
    return [h for h in root.handlers if getattr(h, "_devcouncil_tag", None)]


def test_configure_logging_writes_debug_to_file(tmp_path):
    log_path = configure_logging(tmp_path, verbosity=0)
    assert log_path == tmp_path / LOG_RELATIVE_PATH

    logging.getLogger("devcouncil.test").debug("a debug breadcrumb")
    for h in logging.getLogger().handlers:
        h.flush()

    contents = log_path.read_text(encoding="utf-8")
    # File handler captures DEBUG even though the console default is WARNING.
    assert "a debug breadcrumb" in contents


def test_configure_logging_is_idempotent(tmp_path):
    configure_logging(tmp_path)
    root = logging.getLogger()
    first = _devcouncil_handlers(root)
    configure_logging(tmp_path)
    second = _devcouncil_handlers(root)
    # No duplicate handlers stacked on repeat calls.
    assert len(first) == len(second) == 2


def test_console_level_follows_verbosity(tmp_path):
    configure_logging(tmp_path, verbosity=2)
    root = logging.getLogger()
    console = next(
        h for h in root.handlers if getattr(h, "_devcouncil_tag", None) == "devcouncil.console"
    )
    assert console.level == logging.DEBUG

    configure_logging(tmp_path, quiet=True)
    assert console.level == logging.ERROR


def test_log_level_arg_overrides_verbosity(tmp_path):
    configure_logging(tmp_path, verbosity=2, log_level="WARNING")
    root = logging.getLogger()
    console = next(
        h for h in root.handlers if getattr(h, "_devcouncil_tag", None) == "devcouncil.console"
    )
    assert console.level == logging.WARNING


def test_log_stage_emits_boundaries_and_trace(tmp_path):
    configure_logging(tmp_path)
    with log_stage("demo", project_root=tmp_path, run_id="run-1", foo="bar"):
        log_step("inside", project_root=tmp_path, run_id="run-1", trace=True)

    events = [
        json.loads(line)
        for line in (tmp_path / ".devcouncil" / "logs" / "traces.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
        if line.strip()
    ]
    types = [e["type"] for e in events]
    assert "stage_started" in types
    assert "stage_completed" in types
    assert "step" in types


def test_set_log_dir_repoints_file_handler(tmp_path):
    # configure under dir A, then re-point at dir B (simulating --project-root).
    dir_a = tmp_path / "a"
    dir_b = tmp_path / "b"
    configure_logging(dir_a)
    new_path = set_log_dir(dir_b)
    assert new_path == dir_b / LOG_RELATIVE_PATH

    log = logging.getLogger("devcouncil.repointtest")
    log.warning("lands in B")
    for h in logging.getLogger().handlers:
        h.flush()

    assert "lands in B" in (dir_b / LOG_RELATIVE_PATH).read_text(encoding="utf-8")
    # Exactly one devcouncil file handler remains (the old one was closed/removed).
    file_handlers = [
        h for h in logging.getLogger().handlers
        if getattr(h, "_devcouncil_tag", None) == "devcouncil.file"
    ]
    assert len(file_handlers) == 1


def test_run_log_captures_only_within_block(tmp_path):
    configure_logging(tmp_path)
    log = logging.getLogger("devcouncil.runlogtest")

    log.info("before the run")
    run_file = tmp_path / "runs" / "abc" / "run.log"
    with run_log(run_file) as p:
        assert p == run_file
        log.info("inside the run")
    log.info("after the run")

    for h in logging.getLogger().handlers:
        h.flush()
    contents = run_file.read_text(encoding="utf-8")
    assert "inside the run" in contents
    # The run-scoped file only captures what happened during the block.
    assert "before the run" not in contents
    assert "after the run" not in contents


def test_run_log_detaches_handler_on_exit(tmp_path):
    configure_logging(tmp_path)
    root = logging.getLogger()
    before = len(root.handlers)
    with run_log(tmp_path / "runs" / "x" / "run.log"):
        assert any(getattr(h, "_devcouncil_tag", None) == "devcouncil.run" for h in root.handlers)
    # Handler removed after the block — no leak across runs.
    assert len(root.handlers) == before
    assert not any(getattr(h, "_devcouncil_tag", None) == "devcouncil.run" for h in root.handlers)


def test_excepthook_logs_uncaught_exception(tmp_path):
    import sys

    saved_hook = sys.excepthook
    try:
        log_path = configure_logging(tmp_path)
        assert getattr(sys.excepthook, "_devcouncil_hook", False)
        # Simulate the interpreter calling the hook on an uncaught exception.
        try:
            raise RuntimeError("boom in the wild")
        except RuntimeError:
            sys.excepthook(*sys.exc_info())
        for h in logging.getLogger().handlers:
            h.flush()
        contents = log_path.read_text(encoding="utf-8")
        assert "Uncaught exception" in contents
        assert "boom in the wild" in contents  # traceback included via exc_info
    finally:
        sys.excepthook = saved_hook


def test_log_stage_records_failure_and_reraises(tmp_path):
    configure_logging(tmp_path)
    with pytest.raises(ValueError):
        with log_stage("boom", project_root=tmp_path):
            raise ValueError("kaboom")

    events = [
        json.loads(line)
        for line in (tmp_path / ".devcouncil" / "logs" / "traces.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
        if line.strip()
    ]
    assert any(e["type"] == "stage_failed" for e in events)
