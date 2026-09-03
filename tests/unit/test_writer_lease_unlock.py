"""Writer-lease metadata and ``dev map unlock`` recovery tests."""

from __future__ import annotations

import json
import os
import signal
import time
from pathlib import Path

from typer.testing import CliRunner

from devcouncil.codeintel.build_control import (
    BuildStatus,
    unlock_writer_lease,
    _write_status,
)
from devcouncil.codeintel.sync.lease import WriterLease, read_holder


def test_writer_lease_writes_holder_metadata(tmp_path: Path) -> None:
    path = tmp_path / "writer.lock"
    lease = WriterLease(path)
    assert lease.acquire()
    try:
        holder = read_holder(path)
        assert holder.pid == os.getpid()
        assert holder.started_at is not None
        assert holder.started_at <= time.time()
        raw = json.loads(path.read_text(encoding="utf-8"))
        assert raw["pid"] == os.getpid()
        assert "started_at" in raw
    finally:
        lease.release()


def test_writer_lease_reentrant_acquire_releases_prior_handle(tmp_path: Path) -> None:
    path = tmp_path / "writer.lock"
    lease = WriterLease(path)
    assert lease.acquire()
    first_handle = lease._handle
    assert first_handle is not None
    assert lease.acquire()  # re-entrant: must close prior fd before re-locking
    assert lease._handle is not None
    assert lease._handle is not first_handle
    # Contender still blocked while the same object holds the lease.
    other = WriterLease(path)
    assert other.acquire() is False
    lease.release()
    assert other.acquire()
    other.release()


def test_read_holder_returns_empty_on_missing_or_invalid(tmp_path: Path) -> None:
    missing = tmp_path / "missing.lock"
    empty = read_holder(missing)
    assert empty.pid is None
    assert empty.started_at is None
    bad = tmp_path / "bad.lock"
    bad.write_text("not-json", encoding="utf-8")
    assert read_holder(bad).pid is None


def test_unlock_frees_dead_holder_without_kill(tmp_path: Path, monkeypatch) -> None:
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    lock.parent.mkdir(parents=True)
    lock.write_text(json.dumps({"pid": 999_999, "started_at": time.time()}), encoding="utf-8")
    _write_status(
        tmp_path,
        BuildStatus(
            build_id="b1",
            state="building",
            mode="incremental",
            pid=999_999,
            phase="liveness",
            started_at=time.time(),
            last_progress_at=time.time(),
        ),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._pid_alive",
        lambda _pid: False,
    )
    killed: list[int] = []
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._terminate_pid",
        lambda pid, **_k: killed.append(pid) or True,
    )
    result = unlock_writer_lease(tmp_path)
    assert result["ok"] is True
    assert result["action"] == "freed"
    assert killed == []
    assert result["target_pid"] == 999_999


def test_unlock_kills_stalled_holder(tmp_path: Path, monkeypatch) -> None:
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    lock.parent.mkdir(parents=True)
    started = time.time() - 10.0
    lock.write_text(
        json.dumps({"pid": 4242, "started_at": started}),
        encoding="utf-8",
    )
    _write_status(
        tmp_path,
        BuildStatus(
            build_id="b1",
            state="stalled",
            mode="incremental",
            pid=4242,
            phase="liveness:tokens",
            started_at=started,
            last_progress_at=started,
            stall_timeout_seconds=90.0,
        ),
    )
    alive = {4242: True}
    calls: list[tuple[int, float]] = []

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._pid_alive",
        lambda pid: bool(alive.get(pid)),
    )

    def terminate(pid: int, *, term_wait: float = 5.0) -> bool:
        calls.append((pid, term_wait))
        alive[pid] = False
        return True

    monkeypatch.setattr("devcouncil.codeintel.build_control._terminate_pid", terminate)
    result = unlock_writer_lease(tmp_path)
    assert result["ok"] is True
    assert result["action"] == "killed"
    assert calls == [(4242, 5.0)]


def test_unlock_refuses_progressing_without_force(tmp_path: Path, monkeypatch) -> None:
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    lock.parent.mkdir(parents=True)
    now = time.time()
    lock.write_text(json.dumps({"pid": 777, "started_at": now}), encoding="utf-8")
    _write_status(
        tmp_path,
        BuildStatus(
            build_id="b1",
            state="building",
            mode="full",
            pid=777,
            phase="liveness",
            started_at=now,
            last_progress_at=now,
            stall_timeout_seconds=90.0,
        ),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._pid_alive",
        lambda pid: pid == 777,
    )
    result = unlock_writer_lease(tmp_path, force=False)
    assert result["ok"] is False
    assert result["action"] == "refused"
    assert "force" in str(result.get("hint") or "").lower() or "--force" in str(
        result.get("reason") or ""
    )


def test_unlock_force_kills_progressing_holder(tmp_path: Path, monkeypatch) -> None:
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    lock.parent.mkdir(parents=True)
    now = time.time()
    lock.write_text(json.dumps({"pid": 888, "started_at": now}), encoding="utf-8")
    _write_status(
        tmp_path,
        BuildStatus(
            build_id="b1",
            state="building",
            mode="full",
            pid=888,
            phase="extract",
            started_at=now,
            last_progress_at=now,
            stall_timeout_seconds=90.0,
        ),
    )
    alive = {888: True}
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._pid_alive",
        lambda pid: bool(alive.get(pid)),
    )

    def terminate(pid: int, *, term_wait: float = 5.0) -> bool:
        alive[pid] = False
        return True

    monkeypatch.setattr("devcouncil.codeintel.build_control._terminate_pid", terminate)
    result = unlock_writer_lease(tmp_path, force=True)
    assert result["ok"] is True
    assert result["action"] == "killed"
    assert result["target_pid"] == 888


def test_unlock_age_stall_without_stalled_state(tmp_path: Path, monkeypatch) -> None:
    """Holder older than stall timeout is unlockable even if state still says building."""
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    lock.parent.mkdir(parents=True)
    started = time.time() - 120.0
    lock.write_text(json.dumps({"pid": 333, "started_at": started}), encoding="utf-8")
    _write_status(
        tmp_path,
        BuildStatus(
            build_id="b1",
            state="building",
            mode="incremental",
            pid=333,
            phase="liveness",
            started_at=started,
            # Fresh progress so read_build_status does not rewrite to stalled.
            last_progress_at=time.time(),
            stall_timeout_seconds=90.0,
        ),
    )
    alive = {333: True}
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._pid_alive",
        lambda pid: bool(alive.get(pid)),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._terminate_pid",
        lambda pid, **_k: alive.__setitem__(pid, False) or True,
    )
    result = unlock_writer_lease(tmp_path)
    assert result["ok"] is True
    assert result["action"] == "killed"


def test_busy_unlock_then_acquire(tmp_path: Path, monkeypatch) -> None:
    """Unlock of a stalled busy holder frees the flock so a contender can acquire."""
    from types import SimpleNamespace

    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    lock.parent.mkdir(parents=True)
    started = time.time() - 10.0
    _write_status(
        tmp_path,
        BuildStatus(
            build_id="b1",
            state="stalled",
            mode="full",
            pid=9001,
            phase="liveness",
            started_at=started,
            last_progress_at=started,
            stall_timeout_seconds=90.0,
        ),
    )
    # Real flock held here; unlock targets a synthetic pid and releases on terminate.
    holder = WriterLease(lock)
    assert holder.acquire()
    contender = WriterLease(lock)
    assert contender.acquire() is False

    monkeypatch.setattr(
        "devcouncil.codeintel.build_control.read_holder",
        lambda _path: SimpleNamespace(pid=9001, started_at=started),
    )
    alive = {9001: True}
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._pid_alive",
        lambda pid: bool(alive.get(pid, False)),
    )

    def terminate(pid: int, *, term_wait: float = 5.0) -> bool:
        alive[pid] = False
        holder.release()
        return True

    monkeypatch.setattr("devcouncil.codeintel.build_control._terminate_pid", terminate)
    result = unlock_writer_lease(tmp_path)
    assert result["ok"] is True
    assert result["action"] == "killed"
    assert contender.acquire()
    contender.release()


def test_map_unlock_cli_dead_pid(tmp_path: Path, monkeypatch) -> None:
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.cli.main import app

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    lock = tmp_path / ".devcouncil" / "codeintel" / "writer.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(json.dumps({"pid": 123456, "started_at": time.time()}), encoding="utf-8")
    monkeypatch.setattr(
        "devcouncil.codeintel.build_control._pid_alive",
        lambda _pid: False,
    )
    runner = CliRunner()
    result = runner.invoke(
        app,
        ["map", "unlock", "--project-root", str(tmp_path), "--json"],
    )
    assert result.exit_code == 0, result.output
    payload = json.loads(result.output)
    assert payload["ok"] is True
    assert payload["action"] == "freed"


def test_terminate_pid_term_then_kill(monkeypatch) -> None:
    from devcouncil.codeintel import build_control as bc

    signals: list[int] = []
    alive = True

    def fake_kill(pid: int, sig: int) -> None:
        signals.append(sig)
        nonlocal alive
        if sig == signal.SIGKILL or (not hasattr(signal, "SIGKILL") and len(signals) >= 2):
            alive = False

    monkeypatch.setattr(bc.os, "kill", fake_kill)
    if hasattr(bc.os, "killpg"):
        monkeypatch.setattr(
            bc.os,
            "killpg",
            lambda pid, sig: (_ for _ in ()).throw(ProcessLookupError()),
        )
    monkeypatch.setattr(bc, "_pid_alive", lambda _pid: alive)
    monkeypatch.setattr(bc.time, "sleep", lambda _s: None)
    # Make wait loops expire quickly.
    ticks = iter([0.0, 0.0, 6.0, 6.0, 6.0, 9.0, 9.0])
    monkeypatch.setattr(bc.time, "monotonic", lambda: next(ticks, 99.0))
    assert bc._terminate_pid(42, term_wait=5.0) is True
    assert signal.SIGTERM in signals
    assert any(sig in {signal.SIGKILL, signal.SIGTERM} for sig in signals[1:])
