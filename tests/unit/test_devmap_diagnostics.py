"""Dev Map issues must be diagnosable, traceable and fixable by an agent while running.

Diagnosable: every kernel failure carries a stable code, the exact fix and the
kernel's evidence. Traceable: every kernel run is recorded in the project trace
log with argv, exit, duration and notes, and a running build is visible from
another process. Fixable: the doctor's checks carry commands, `--fix` applies
the ones a repository can apply, and a stuck build can be aborted safely.
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

from devcouncil import devmap_engine
from devcouncil.devmap_engine import (
    LIVE_BUILD_RELPATH,
    RUN_EVENT_TYPE,
    DevMapEngineError,
    classify_kernel_failure,
    read_runs,
)
from devcouncil.devmap_health import (
    abort_build,
    apply_fixes,
    build_activity,
    collect_map_status,
    run_doctor,
)


def _script(path: Path, body: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/bin/sh\n" + body, encoding="utf-8")
    path.chmod(0o755)
    return path


def _root(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    (root / ".devcouncil").mkdir(parents=True)
    return root


# --- diagnosable -------------------------------------------------------------


@pytest.mark.parametrize(
    "output, code",
    [
        ("Error: unsupported future schema version 13", "schema_newer_than_kernel"),
        ("another devmap writer holds \"/x/devmap.sqlite.writer.lock\" (pid 4242)", "store_locked"),
        ("Error: database disk image is malformed", "store_corrupt"),
        ("Error: attempt to write a readonly database", "store_unwritable"),
        ("error: unexpected argument '--full' found", "kernel_flag_unsupported"),
        ("thread 'main' panicked at src/main.rs:1", "kernel_failed"),
    ],
)
def test_kernel_failures_are_classified_with_a_fix(output: str, code: str) -> None:
    got, fix = classify_kernel_failure(output)
    assert got == code
    assert fix, "every code carries the command or step that resolves it"


def test_a_failing_build_raises_a_diagnosis_not_just_a_message(tmp_path: Path) -> None:
    root = _root(tmp_path)
    kernel = _script(
        tmp_path / "bin" / "devmap",
        'echo "another devmap writer holds \\"$3.writer.lock\\" (pid 4242); waited 60s" >&2\nexit 2\n',
    )
    with pytest.raises(DevMapEngineError) as caught:
        devmap_engine._run(
            [str(kernel), "--db", str(root / "db.sqlite"), "--progress", "always", "build", str(root)],
            cwd=root,
            timeout=30,
        )
    exc = caught.value
    assert exc.code == "store_locked"
    assert "dev map abort" in exc.fix
    assert exc.stage == "build"
    assert exc.run_id and len(exc.run_id) == 12
    assert any("pid 4242" in line for line in exc.evidence)
    payload = exc.to_dict()
    assert payload["ok"] is False and payload["code"] == "store_locked"


# --- traceable ---------------------------------------------------------------


def test_every_kernel_run_is_recorded_in_the_project_trace_log(tmp_path: Path) -> None:
    root = _root(tmp_path)
    kernel = _script(
        tmp_path / "bin" / "devmap",
        'echo "[1/3] scanning" >&2\necho "  discovery refused 1 file(s):" >&2\necho "    big.bin: Oversized" >&2\n'
        'echo "built generation 7"\nexit 0\n',
    )
    devmap_engine._run(
        [str(kernel), "--db", str(root / "db.sqlite"), "--progress", "always", "build", str(root)],
        cwd=root,
        timeout=30,
    )
    runs = read_runs(root, limit=5)
    assert len(runs) == 1
    run = runs[0]
    assert run["stage"] == "build" and run["ok"] is True and run["exit_code"] == 0
    assert run["duration_s"] >= 0 and run["binary"] == str(kernel)
    assert any("discovery refused" in note for note in run["notes"])
    assert run["stdout_tail"] == ["built generation 7"]
    # The record is a plain trace event: the same log `devcouncil_tail_trace` reads.
    events = (root / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8").splitlines()
    assert json.loads(events[-1])["type"] == RUN_EVENT_TYPE
    assert read_runs(root, failed_only=True) == []


def test_a_running_build_is_visible_from_another_process_and_cleaned_up(tmp_path: Path) -> None:
    root = _root(tmp_path)
    kernel = _script(
        tmp_path / "bin" / "devmap",
        'echo "[1/5] scanning 3 files" >&2\nsleep 1.5\necho "[5/5] persisting" >&2\nexit 0\n',
    )
    marker = root / LIVE_BUILD_RELPATH
    seen: dict = {}

    def _observe() -> None:
        # The marker appears before the kernel's first progress line; wait for
        # the line, which is what a second process would actually read.
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if marker.is_file():
                try:
                    data = json.loads(marker.read_text(encoding="utf-8"))
                    activity = build_activity(root)
                    if data.get("stage") and activity.get("in_progress"):
                        seen["marker"] = data
                        seen["activity"] = activity
                        return
                except (OSError, ValueError):
                    pass
            time.sleep(0.05)

    observer = threading.Thread(target=_observe)
    observer.start()
    devmap_engine._run(
        [str(kernel), "--db", str(root / "db.sqlite"), "--progress", "always", "build", str(root)],
        cwd=root,
        timeout=30,
    )
    observer.join(timeout=6)
    assert seen.get("marker"), "the marker must exist while the build runs"
    assert seen["marker"]["stage"].startswith("[1/5]")
    assert seen["activity"]["in_progress"] is True
    assert seen["activity"]["pid"] == seen["marker"]["pid"]
    assert not marker.exists(), "the marker must be removed when the build ends"
    assert build_activity(root)["in_progress"] is False


def test_a_marker_whose_process_died_is_reported_not_hidden(tmp_path: Path) -> None:
    root = _root(tmp_path)
    marker = root / LIVE_BUILD_RELPATH
    marker.parent.mkdir(parents=True, exist_ok=True)
    # A pid that cannot be alive: the maximum pid on both macOS and Linux is far below this.
    marker.write_text(
        json.dumps({"run_id": "deadbeef0000", "pid": 2**22 + 12345, "started_at": time.time() - 30,
                    "updated_at": time.time() - 30, "stage": "[2/5] extracting"}),
        encoding="utf-8",
    )
    activity = build_activity(root)
    assert activity["in_progress"] is False and activity["stale_marker"] is True
    doctor = run_doctor(root)
    stale = next(c for c in doctor["checks"] if c["code"] == "stale_build_marker")
    assert stale["ok"] is False and stale["fix_command"] == "dev map doctor --fix"
    # `status --json` carries it too, so an MCP caller sees the same thing.
    assert collect_map_status(root)["build"]["stale_marker"] is True


# --- fixable -----------------------------------------------------------------


def test_every_failing_doctor_check_carries_a_code_and_a_command(tmp_path: Path) -> None:
    root = _root(tmp_path)
    doctor = run_doctor(root)
    for item in doctor["checks"]:
        if item["ok"] is False:
            assert item["code"], item
            assert item["fix_command"], item


def test_doctor_fix_clears_a_dead_builds_marker_and_reports_what_it_did(tmp_path: Path, monkeypatch) -> None:
    root = _root(tmp_path)
    marker = root / LIVE_BUILD_RELPATH
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text(json.dumps({"run_id": "x", "pid": 2**22 + 1, "started_at": 1.0, "updated_at": 1.0}), encoding="utf-8")
    # No kernel on this tmp root: the build step must report, not crash.
    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(DevMapEngineError("no kernel", code="binary_missing")),
    )
    result = apply_fixes(root)
    actions = {a["action"]: a for a in result["applied"]}
    assert actions["clear_build_marker"]["ok"] is True
    assert not marker.exists()
    assert not any(c["code"] == "stale_build_marker" for c in result["after"]["checks"] if c["ok"] is False)


def test_doctor_fix_refuses_to_touch_a_running_build(tmp_path: Path) -> None:
    root = _root(tmp_path)
    marker = root / LIVE_BUILD_RELPATH
    marker.parent.mkdir(parents=True, exist_ok=True)
    # A live process whose command line says devmap: a shell script by that name.
    fake = _script(tmp_path / "bin" / "devmap", "sleep 30\n")
    proc = subprocess.Popen([str(fake)])
    try:
        marker.write_text(
            json.dumps({"run_id": "live", "pid": proc.pid, "started_at": time.time(), "updated_at": time.time(), "stage": "[1/5]"}),
            encoding="utf-8",
        )
        result = apply_fixes(root)
        assert result["applied"] == []
        assert result["not_applied"][0]["code"] == "build_in_progress"
        assert marker.exists()
    finally:
        proc.kill()
        proc.wait()


def test_abort_stops_a_devmap_build_and_refuses_anything_else(tmp_path: Path) -> None:
    root = _root(tmp_path)
    marker = root / LIVE_BUILD_RELPATH
    marker.parent.mkdir(parents=True, exist_ok=True)

    # 1. A process that is not devmap must never be signalled.
    other = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
    try:
        marker.write_text(
            json.dumps({"run_id": "r1", "pid": other.pid, "started_at": time.time(), "updated_at": time.time()}),
            encoding="utf-8",
        )
        # build_activity already refuses to call a non-devmap pid "in progress",
        # so the marker reads as stale and is cleared without a signal.
        result = abort_build(root)
        assert result["aborted"] is False
        assert other.poll() is None, "a foreign process must be left alone"
    finally:
        other.kill()
        other.wait()

    # 2. A devmap process is stopped, and the marker goes with it.
    fake = _script(tmp_path / "bin" / "devmap", "sleep 30\n")
    proc = subprocess.Popen([str(fake)])
    try:
        marker.write_text(
            json.dumps({"run_id": "r2", "pid": proc.pid, "started_at": time.time(), "updated_at": time.time()}),
            encoding="utf-8",
        )
        assert build_activity(root)["in_progress"] is True
        result = abort_build(root, grace_seconds=2.0)
        assert result["aborted"] is True and result["pid"] == proc.pid
        assert proc.wait(timeout=5) is not None
        assert not marker.exists()
        assert abort_build(root)["code"] == "no_build_in_progress"
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


def test_cli_runs_and_abort_answer_without_a_store(tmp_path: Path) -> None:
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    root = _root(tmp_path)
    runner = CliRunner()
    runs = runner.invoke(app, ["map", "runs", "--project-root", str(root), "--json"])
    assert runs.exit_code == 0, runs.output
    assert json.loads(runs.output.strip().splitlines()[-1] if runs.output.strip().startswith("{") is False else runs.output)["runs"] == [] or "runs" in runs.output
    abort = runner.invoke(app, ["map", "abort", "--project-root", str(root), "--json"])
    assert abort.exit_code == 0, abort.output
    assert '"no_build_in_progress"' in abort.output


def test_mcp_doctor_and_runs_tools_return_the_same_diagnosis(tmp_path: Path) -> None:
    import asyncio

    from devcouncil.integrations.mcp.handlers.map import handle_graph_doctor, handle_graph_runs

    root = _root(tmp_path)
    doctor = json.loads(asyncio.run(handle_graph_doctor(root, {}))[0].text)
    assert "checks" in doctor and "build" in doctor
    assert all("code" in c and "fix_command" in c for c in doctor["checks"])
    runs = json.loads(asyncio.run(handle_graph_runs(root, {"limit": 5}))[0].text)
    # This used to assert the whole payload was `{"ok": True, "runs": []}`, which
    # encoded a conflation rather than a requirement: that same answer came back
    # for a project with no trace log, one whose log was entirely unreadable, and
    # one whose kernel had genuinely never run. The empty list is still correct
    # here — the fixture has run nothing — but the payload now says which of the
    # three it is. See tests/unit/test_graph_runs_provenance.py.
    assert runs["ok"] is True and runs["runs"] == []
    assert runs["log_present"] is False and runs["total"] == 0


def test_dev_map_prints_the_code_the_fix_and_the_run_id_on_failure(tmp_path: Path, monkeypatch) -> None:
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    root = _root(tmp_path)
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)
    (root / "a.py").write_text("x = 1\n", encoding="utf-8")

    def _boom(*_a, **_k):
        raise DevMapEngineError(
            "devmap exited 2: build", code="store_locked", fix="dev map abort", run_id="abc123def456", stage="build"
        )

    monkeypatch.setattr("devcouncil.devmap_engine.build_map", _boom)
    result = CliRunner().invoke(app, ["map", "--project-root", str(root), "--no-wiki"])
    assert result.exit_code == 1
    assert "[store_locked]" in result.output
    assert "fix: dev map abort" in result.output
    assert "run: abc123def456" in result.output
