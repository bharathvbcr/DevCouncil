from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.codeintel.debug.discovery import AdapterInfo, discover_adapters
from devcouncil.codeintel.debug.fingerprint import build_fingerprint, executable_hash, source_fingerprint
from devcouncil.codeintel.debug.protocol import DAPClient, DAPError, encode_message, read_message
from devcouncil.codeintel.debug.session import DebugSessionManager, redact_value
from devcouncil.codeintel.debug.tracing import PythonTraceProvider, load_node_cpu_profile
from devcouncil.codeintel.service import get_codeintel_service
from devcouncil.codeintel.query import CodeIntelQueryEngine
from devcouncil.indexing.graph.schema import CodeGraph, Confidence, DeadCodeEntry


runner = CliRunner()


def test_dap_message_framing_round_trip_and_validation() -> None:
    message = {"seq": 1, "type": "request", "command": "threads", "arguments": {}}
    assert read_message(io.BytesIO(encode_message(message))) == message
    with pytest.raises(DAPError):
        read_message(io.BytesIO(b"Bad: header\r\n\r\n{}"))


def test_evaluate_requires_explicit_side_effect_consent() -> None:
    client = object.__new__(DAPClient)
    with pytest.raises(PermissionError, match="may execute code"):
        client.evaluate("danger()")


def test_debug_discovery_without_consent_exits_cleanly(tmp_path: Path) -> None:
    result = runner.invoke(
        app,
        ["debug", "discover", "--project-root", str(tmp_path), "--json"],
    )

    assert result.exit_code == 2
    assert "Debugger discovery/execution is disabled" in result.output
    assert "Traceback" not in result.output


def test_adapter_discovery_reports_version_and_launch_attach_support(monkeypatch) -> None:
    import devcouncil.codeintel.debug.discovery as discovery

    monkeypatch.setattr(discovery.importlib.util, "find_spec", lambda _name: None)
    monkeypatch.setattr(discovery.shutil, "which", lambda name: "/bin/js-debug-adapter" if name == "js-debug-adapter" else None)
    monkeypatch.setattr(discovery, "_command_version", lambda _command: "js-debug 1.2.3")

    adapters = discover_adapters()

    assert adapters[0].version == "js-debug 1.2.3"
    assert adapters[0].as_dict()["requests"] == ["launch", "attach"]


def test_adapter_discovery_accepts_explicit_node_debug2_install(tmp_path: Path, monkeypatch) -> None:
    import devcouncil.codeintel.debug.discovery as discovery

    adapter = tmp_path / "nodeDebug.js"
    adapter.write_text("// installed adapter\n", encoding="utf-8")
    monkeypatch.setenv("DEVCOUNCIL_NODE_DEBUG2_PATH", str(adapter))
    monkeypatch.setenv("DEVCOUNCIL_NODE_DEBUG2_VERSION", "1.42.5")
    monkeypatch.setattr(discovery.importlib.util, "find_spec", lambda _name: None)
    monkeypatch.setattr(discovery.shutil, "which", lambda name: "/usr/bin/node" if name == "node" else None)

    discovered = discover_adapters()

    assert discovered[0].id == "node-debug2"
    assert discovered[0].command == ("/usr/bin/node", str(adapter))
    assert discovered[0].version == "1.42.5"


def test_redaction_truncates_debug_values() -> None:
    assert "[REDACTED]" in redact_value("token=secret-value")
    assert redact_value("abcdef", limit=3) == "abc…[truncated]"


def test_python_trace_records_only_project_call_edges(tmp_path: Path) -> None:
    script = tmp_path / "program.py"
    script.write_text(
        "def child():\n    return 1\n\n"
        "def parent():\n    return child()\n\n"
        "parent()\n",
        encoding="utf-8",
    )

    result = PythonTraceProvider(tmp_path).run(script)
    observations = get_codeintel_service(tmp_path).store.runtime_observations(
        source_fingerprint=result["source_fingerprint"]
    )

    assert result["exit_code"] == 0
    assert any(row["source"].endswith("::parent") and row["target"].endswith("::child") for row in observations)
    assert all(row["fingerprint_matches"] for row in observations)


def test_node_cpu_profile_converts_sampled_parent_child_edges(tmp_path: Path) -> None:
    profile = tmp_path / "trace.cpuprofile"
    profile.write_text(json.dumps({
        "nodes": [
            {"id": 1, "callFrame": {"url": "a.js", "functionName": "a", "lineNumber": 0}, "children": [2]},
            {"id": 2, "callFrame": {"url": "b.js", "functionName": "b", "lineNumber": 4}},
        ],
        "samples": [2, 2],
    }), encoding="utf-8")

    rows = load_node_cpu_profile(profile)
    assert rows == [{
        "source": "a.js:a:1",
        "target": "b.js:b:5",
        "kind": "sampled_calls",
        "count": 2,
        "evidence": {"provider": "node-cpu-profile", "parent_id": 1, "child_id": 2},
    }]


def test_runtime_observations_are_fingerprint_scoped(tmp_path: Path) -> None:
    store = get_codeintel_service(tmp_path).store
    first = source_fingerprint(tmp_path)
    session = store.start_runtime_session(provider="fixture", source_fingerprint=first, build_fingerprint="b")
    store.add_runtime_observations(session, [{"source": "a", "target": "b"}])
    (tmp_path / "changed.py").write_text("x = 1\n", encoding="utf-8")
    second = source_fingerprint(tmp_path)

    assert first != second
    assert store.runtime_observations(source_fingerprint=second) == []
    stale = store.runtime_observations(source_fingerprint=second, include_stale=True)
    assert stale[0]["fingerprint_matches"] is False


def test_runtime_observations_validate_build_and_executable_fingerprints(tmp_path: Path) -> None:
    store = get_codeintel_service(tmp_path).store
    session = store.start_runtime_session(
        provider="fixture",
        source_fingerprint="source",
        build_fingerprint="build-a",
        executable_hash="exe-a",
    )
    store.add_runtime_observations(session, [{"source": "a", "target": "b"}])

    assert store.runtime_observations(
        source_fingerprint="source",
        build_fingerprint="build-b",
        executable_hash="exe-a",
    ) == []
    stale = store.runtime_observations(
        source_fingerprint="source",
        build_fingerprint="build-b",
        executable_hash="exe-a",
        include_stale=True,
    )
    assert stale[0]["fingerprint_matches"] is False


def test_source_build_and_executable_fingerprints_invalidate(tmp_path: Path) -> None:
    source_before = source_fingerprint(tmp_path)
    build_before = build_fingerprint(tmp_path, sys.executable)
    (tmp_path / "new.py").write_text("VALUE = 1\n", encoding="utf-8")
    assert source_fingerprint(tmp_path) != source_before
    assert build_fingerprint(tmp_path, sys.executable) != build_before

    executable = tmp_path / "adapter"
    executable.write_bytes(b"first")
    first = executable_hash(executable)
    executable.write_bytes(b"second")
    assert executable_hash(executable) != first


def test_debug_session_presents_adapter_version_requests_and_capabilities(
    tmp_path: Path,
    monkeypatch,
) -> None:
    class FakeClient:
        def initialize(self, **_kwargs):
            return {"supportsConfigurationDoneRequest": True}

        def begin_request(self, _request, _arguments):
            return object()

        def wait_event(self, _event, **_kwargs):
            return {}

        def request(self, _command, _arguments=None, **_kwargs):
            return {}

        def wait_response(self, _pending, **_kwargs):
            return {}

        def close(self):
            return None

    adapter = AdapterInfo(
        id="fixture",
        name="Fixture",
        command=(sys.executable, "-m", "fixture"),
        path=sys.executable,
        executable_hash=executable_hash(sys.executable),
        version="1.2.3",
    )
    monkeypatch.setattr("devcouncil.codeintel.debug.session.adapter_by_command", lambda _command: adapter)
    monkeypatch.setattr(DAPClient, "start_stdio", lambda *_args, **_kwargs: FakeClient())

    session = DebugSessionManager().start(
        tmp_path,
        adapter.command,
        request="launch",
        arguments={},
    )

    payload = session.as_dict()
    assert payload["adapter_version"] == "1.2.3"
    assert payload["adapter_requests"] == ["launch", "attach"]
    assert payload["capabilities"]["supportsConfigurationDoneRequest"] is True


def test_matching_runtime_observation_removes_dead_candidate(tmp_path: Path) -> None:
    service = get_codeintel_service(tmp_path)
    fingerprint = source_fingerprint(tmp_path)
    service.persist(CodeGraph(dead_code=[
        DeadCodeEntry(
            id="app.py::live_at_runtime",
            path="app.py",
            confidence=Confidence.EXTRACTED,
            reason="no static callers",
        )
    ]))
    session = service.store.start_runtime_session(
        provider="fixture",
        source_fingerprint=fingerprint,
        build_fingerprint="build",
    )
    service.store.add_runtime_observations(session, [{
        "source": "app.py::entry",
        "target": "app.py::live_at_runtime",
        "kind": "observed_calls",
    }])

    result = CodeIntelQueryEngine(service).dead(minimum_confidence="ambiguous")
    assert result["dead_code"] == []
    assert result["runtime_proven_live"] == ["app.py::live_at_runtime"]
