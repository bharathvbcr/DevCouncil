from __future__ import annotations

import os
import shutil
import socket
import subprocess
import threading
import time
import urllib.request
from pathlib import Path

import pytest

from devcouncil.codeintel.debug.discovery import adapter_by_id, discover_adapters
from devcouncil.codeintel.debug.fingerprint import source_fingerprint
from devcouncil.codeintel.debug.session import DebugSessionManager
from devcouncil.codeintel.debug.tracing import NodeCpuProfileProvider
from devcouncil.codeintel.service import get_codeintel_service
from devcouncil.codeintel.sync.coordinator import SyncCoordinator
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind

FIXTURES = Path(__file__).parents[1] / "fixtures" / "codeintel_runtime"


def _wait_until(predicate, *, timeout: float = 15.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError("timed out waiting for platform integration event")


def _copy_fixture(tmp_path: Path, name: str) -> Path:
    target = tmp_path / name
    shutil.copy2(FIXTURES / name, target)
    return target


@pytest.mark.skipif(
    os.environ.get("DEVCOUNCIL_UNSANDBOXED_SOCKET_TESTS") != "1",
    reason="requires an unsandboxed loopback socket",
)
def test_dashboard_real_loopback_socket_smoke(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from devcouncil.ui import dashboard

    created: dict[str, dashboard.ThreadingHTTPServer] = {}

    class RecordingServer(dashboard.ThreadingHTTPServer):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            created["server"] = self

    monkeypatch.setattr(dashboard, "get_db", lambda root: None)
    thread = threading.Thread(
        target=dashboard.run_dashboard,
        args=(tmp_path, "127.0.0.1", 0),
        kwargs={"server_factory": RecordingServer},
        daemon=True,
    )
    thread.start()
    _wait_until(lambda: "server" in created)
    server = created["server"]
    host, port = server.server_address
    try:
        with urllib.request.urlopen(f"http://{host}:{port}/api/status", timeout=5.0) as response:
            assert response.status == 200
            assert b"UNINITIALIZED" in response.read()
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5.0)


def test_native_watcher_handles_bursts_renames_atomic_saves_pressure_and_reconciliation(
    tmp_path: Path,
) -> None:
    original = tmp_path / "rename_old.py"
    atomic = tmp_path / "atomic.py"
    original.write_text("VALUE = 1\n", encoding="utf-8")
    atomic.write_text("VALUE = 1\n", encoding="utf-8")
    service = get_codeintel_service(tmp_path)
    service.persist(CodeGraph(nodes=[
        GraphNode(id=path.name, kind=NodeKind.FILE, path=path.name, name=path.name, language="python")
        for path in (original, atomic)
    ]))
    batches: list[list[str]] = []
    lock = threading.Lock()

    def capture(paths: list[str]) -> None:
        with lock:
            batches.append(paths)

    coordinator = SyncCoordinator(
        service,
        debounce_seconds=0.1,
        reconcile_seconds=300.0,
        sync_callback=capture,
        allow_polling_fallback=False,
    )
    state = coordinator.start()
    try:
        assert state.backend_kind == "native", state.degraded_reason

        expected = {f"burst_{index}.py" for index in range(20)}
        for rel in sorted(expected):
            (tmp_path / rel).write_text("VALUE = 1\n", encoding="utf-8")

        renamed = tmp_path / "rename_new.py"
        original.rename(renamed)
        expected.update({"rename_old.py", "rename_new.py"})

        temporary = tmp_path / ".atomic.py.tmp"
        temporary.write_text("VALUE = 2\n", encoding="utf-8")
        os.replace(temporary, atomic)
        expected.add("atomic.py")

        recreated = tmp_path / "recreated.py"
        recreated.write_text("VALUE = 1\n", encoding="utf-8")
        service.persist(CodeGraph(nodes=[
            GraphNode(id="recreated.py", kind=NodeKind.FILE, path="recreated.py", name="recreated.py", language="python")
        ]))
        recreated.unlink()
        recreated.write_text("VALUE = 2\n", encoding="utf-8")
        expected.add("recreated.py")

        for index in range(48):
            directory = tmp_path / f"pressure_{index}"
            directory.mkdir()
            (directory / "module.py").write_text(f"VALUE = {index}\n", encoding="utf-8")
            expected.add(f"pressure_{index}/module.py")

        def observed() -> bool:
            with lock:
                return expected <= {path for batch in batches for path in batch}

        _wait_until(observed)

        missed = tmp_path / "missed.py"
        observer = coordinator._observer
        observer.stop()
        observer.join(timeout=5.0)
        missed.write_text("VALUE = 1\n", encoding="utf-8")
        assert "missed.py" in coordinator.reconcile()
        assert coordinator.sync_now()
        with lock:
            assert "missed.py" in {path for batch in batches for path in batch}
    finally:
        coordinator.stop()


def test_real_node_runtime_profile_records_fingerprint_scoped_observations(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("Node.js runtime is not installed")
    script = _copy_fixture(tmp_path, "node_target.js")

    result = NodeCpuProfileProvider(tmp_path).run(script, timeout=30.0)
    store = get_codeintel_service(tmp_path).store
    rows = store.runtime_observations(
        source_fingerprint=result["source_fingerprint"],
        build_fingerprint=result["build_fingerprint"],
        executable_hash=result["executable_hash"],
    )

    assert result["exit_code"] == 0
    assert result["observation_count"] > 0
    assert any("parent" in row["source"] or "parent" in row["target"] for row in rows)
    script.write_text(script.read_text(encoding="utf-8") + "\n// changed\n", encoding="utf-8")
    assert store.runtime_observations(
        source_fingerprint=source_fingerprint(tmp_path),
        build_fingerprint=result["build_fingerprint"],
        executable_hash=result["executable_hash"],
    ) == []


@pytest.mark.parametrize("request_kind", ["launch", "attach"])
def test_real_debugpy_launch_attach_breakpoint_control_and_stack(tmp_path: Path, request_kind: str) -> None:
    adapter = adapter_by_id("debugpy")
    if adapter is None:
        pytest.skip("debugpy adapter is not installed")
    script = _copy_fixture(tmp_path, "python_target.py")
    debuggee: subprocess.Popen[bytes] | None = None
    attach_debuggee: list[subprocess.Popen[bytes]] = []
    connector: threading.Timer | None = None
    configuration: dict[str, object]
    if request_kind == "launch":
        configuration = {
            "name": "DevCouncil debugpy launch smoke",
            "type": "python",
            "request": "launch",
            "program": str(script),
            "cwd": str(tmp_path),
            "console": "internalConsole",
            "justMyCode": True,
        }
    else:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = int(sock.getsockname()[1])
        def connect_debuggee() -> None:
            attach_debuggee.append(subprocess.Popen(
                [
                    adapter.command[0],
                    "-m",
                    "debugpy",
                    "--connect",
                    f"127.0.0.1:{port}",
                    "--wait-for-client",
                    str(script),
                ],
                cwd=tmp_path,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            ))

        connector = threading.Timer(0.5, connect_debuggee)
        connector.start()
        configuration = {
            "name": "DevCouncil debugpy attach smoke",
            "type": "python",
            "request": "attach",
            "listen": {"host": "127.0.0.1", "port": port},
            "justMyCode": True,
        }

    manager = DebugSessionManager()
    session = None
    try:
        session = manager.start(
            tmp_path,
            adapter.command,
            request=request_kind,
            arguments=configuration,
            initial_breakpoints={str(script): [5]},
            timeout=30.0,
        )
        stopped = session.client.wait_event("stopped", timeout=30.0)
        thread_id = int(stopped["body"]["threadId"])
        stack = manager.inspect(session.id, "stackTrace", {"threadId": thread_id})
        assert any(frame.get("source", {}).get("path") == str(script) for frame in stack["stackFrames"])
        captured = manager.capture_stack(session.id, thread_id=thread_id)
        assert captured["frames"]
        manager.control(session.id, "continue", thread_id=thread_id)
        session.client.wait_event("terminated", timeout=30.0)
    finally:
        if session is not None:
            manager.stop(session.id)
        if connector is not None:
            connector.join(timeout=5.0)
        if attach_debuggee:
            debuggee = attach_debuggee[0]
        if debuggee is not None and debuggee.poll() is None:
            debuggee.terminate()
            debuggee.wait(timeout=5.0)


@pytest.mark.parametrize("request_kind", ["launch", "attach"])
def test_real_node_adapter_launch_attach_breakpoint_control_and_stack(
    tmp_path: Path,
    request_kind: str,
) -> None:
    adapter = next(
        (value for value in discover_adapters() if value.id in {"js-debug-adapter", "node-debug2"}),
        None,
    )
    if adapter is None:
        pytest.skip("Node DAP adapter is not installed")
    script = _copy_fixture(tmp_path, "node_target.js")
    manager = DebugSessionManager()
    session = None
    debuggee: subprocess.Popen[bytes] | None = None
    configuration: dict[str, object] = {
        "name": f"DevCouncil Node {request_kind} smoke",
        "type": "pwa-node" if adapter.id == "js-debug-adapter" else "node",
        "request": request_kind,
        "cwd": str(tmp_path),
    }
    if request_kind == "launch":
        configuration.update({"program": str(script), "console": "internalConsole"})
    else:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = int(sock.getsockname()[1])
        debuggee = subprocess.Popen(
            ["node", f"--inspect-brk=127.0.0.1:{port}", str(script)],
            cwd=tmp_path,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        configuration.update({"address": "127.0.0.1", "port": port})
    try:
        session = manager.start(
            tmp_path,
            adapter.command,
            request=request_kind,
            arguments=configuration,
            initial_breakpoints={str(script): [3]},
            timeout=30.0,
        )
        stopped = session.client.wait_event("stopped", timeout=30.0)
        thread_id = int(stopped["body"]["threadId"])
        stack = manager.inspect(session.id, "stackTrace", {"threadId": thread_id})
        assert stack["stackFrames"]
        manager.control(session.id, "continue", thread_id=thread_id)
    finally:
        if session is not None:
            manager.stop(session.id)
        if debuggee is not None and debuggee.poll() is None:
            debuggee.terminate()
            debuggee.wait(timeout=5.0)
