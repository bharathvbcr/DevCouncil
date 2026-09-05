from __future__ import annotations

import asyncio
import io
import json
import queue
import socket
import sys
import threading
import types
from pathlib import Path

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.codeintel.debug import broker
from devcouncil.codeintel.debug.broker_client import DebugBrokerClient
from devcouncil.codeintel.debug.protocol import (
    DAPClient,
    DAPError,
    PendingRequest,
    encode_message,
    read_message,
)
from devcouncil.codeintel.debug.python_trace_runner import run_trace
from devcouncil.codeintel.debug.session import DebugSession, DebugSessionManager
from devcouncil.codeintel.languages import workers
from devcouncil.codeintel.service import CodeIntelService
from devcouncil.codeintel.store.sqlite import CodeIntelStore
from devcouncil.indexing.graph.schema import (
    CodeGraph,
    Confidence,
    DeadCodeEntry,
    GraphEdge,
    GraphNode,
    NodeKind,
)
from devcouncil.integrations.mcp.handlers import debug as mcp_debug
from devcouncil.integrations.mcp.handlers import codeintel as mcp_codeintel


runner = CliRunner()


class _FakeKernelClient:
    """Stands in for ``DevMapClient`` on the CLI's query commands.

    ``dev map explore`` / ``affected`` / ``search`` used to be stubbed by
    replacing ``CodeIntelQueryEngine``. That class is gone: all three now ask
    the Rust kernel through ``try_connect``, so the stub follows the seam. The
    payloads below are the kernel's own wire shapes — nested ``Response``
    objects with their counters — because that is what the commands read, and a
    stub that returned the old flat lists would pass while the real thing
    failed.
    """

    def __init__(self, *, tests: list[str] | None = None) -> None:
        self._tests = ["tests/test_app.py"] if tests is None else tests

    @staticmethod
    def _response(items: list[dict]) -> dict:
        return {
            "items": items,
            "shown": len(items),
            "hidden": 0,
            "total": len(items),
            "truncated": False,
            "tokens_used": 0,
            "resolution": "Available",
        }

    def explore(self, query, limit=20, **_kwargs):
        return {
            "query": query,
            "limit": limit,
            "definitions": self._response([
                {
                    "id": f"app.py::{query}",
                    "symbol_name": query,
                    "file_path": "app.py",
                    "kind": "Function",
                    "span": [2, 3],
                    "source": "def target():",
                    "score": 1.0,
                    "callers": self._response([{"source_symbol": "app.py::caller"}]),
                    "callees": self._response([]),
                }
            ]),
            "blast_radius": {
                "seeds": [f"app.py::{query}"],
                "unmatched_targets": [],
                "layers": self._response([]),
                "total_impacted": 0,
            },
            "budget": {"total": 8000, "definitions": 4000, "edges_per_direction": 500,
                       "blast_radius": 2000},
        }

    def affected_tests(self, targets, **_kwargs):
        rows = [] if list(targets) == ["none"] else [
            {"path": path, "depth": 1, "symbols": [], "reached_symbols": 0}
            for path in self._tests
        ]
        return {
            "targets": list(targets),
            "tests": self._response(rows),
            "blast_radius": {
                "seeds": list(targets),
                "unmatched_targets": [],
                "layers": self._response([]),
                "total_impacted": 0,
            },
        }

    def impact(self, target, depth=3):
        """Unavailable, so ``_devmap_query_payload`` degrades as it always did.

        ``dev map impact`` is not part of this migration and still falls back to
        the Python ``diff_impact``. Before ``try_connect`` was stubbed here, no
        store existed and the fallback was reached by returning ``None``; the
        stub has to reproduce that rather than accidentally answer.
        """
        from devcouncil.devmap_client import DevMapClientError

        raise DevMapClientError(f"no kernel impact in this fixture: {target}")

    def search(self, query, limit=2000, semantic=False):
        from devcouncil.devmap_client import BudgetedResponse

        items = [{
            "symbol_name": query,
            "file_path": "app.py",
            "kind": "Function",
            "span": [2, 3],
            "score": 1.0,
        }]
        return BudgetedResponse(
            shown=1, hidden=0, total=1, truncated=False, tokens_used=0,
            items=items, resolution="Available",
        )



class _FakeManager:
    def __init__(self) -> None:
        self.calls: list[tuple[str, tuple, dict]] = []

    def _record(self, name: str, *args, **kwargs):
        self.calls.append((name, args, kwargs))
        return {"operation": name}

    def list(self):
        return [{"id": "session"}]

    def start(self, *args, **kwargs):
        self._record("start", *args, **kwargs)
        return types.SimpleNamespace(as_dict=lambda: {"id": "session"})

    def set_breakpoints(self, *args, **kwargs):
        return self._record("breakpoints", *args, **kwargs)

    def control(self, *args, **kwargs):
        return self._record("control", *args, **kwargs)

    def inspect(self, *args, **kwargs):
        return self._record("inspect", *args, **kwargs)

    def evaluate(self, *args, **kwargs):
        return self._record("evaluate", *args, **kwargs)

    def capture_stack(self, *args, **kwargs):
        return self._record("capture_stack", *args, **kwargs)

    def stop(self, *args, **kwargs):
        self._record("stop", *args, **kwargs)


def test_broker_dispatch_covers_session_control_surface(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = _FakeManager()
    monkeypatch.setattr(broker, "get_debug_manager", lambda: manager)

    assert broker.dispatch(tmp_path, "ping", {})["sessions"] == [{"id": "session"}]
    assert broker.dispatch(
        tmp_path,
        "start",
        {
            "adapter_command": ["python", "-m", "debugpy"],
            "request": "attach",
            "arguments": {"port": 5678},
            "initial_breakpoints": {"app.py": ["3"]},
            "timeout": 2,
        },
    ) == {"id": "session"}
    assert broker.dispatch(
        tmp_path, "breakpoints", {"session_id": "s", "source": "app.py", "lines": [3]}
    )["operation"] == "breakpoints"
    assert broker.dispatch(
        tmp_path,
        "control",
        {"session_id": "s", "debug_action": "continue", "thread_id": 7},
    )["operation"] == "control"
    assert broker.dispatch(
        tmp_path,
        "inspect",
        {"session_id": "s", "operation": "threads", "arguments": {}},
    )["operation"] == "inspect"
    assert broker.dispatch(
        tmp_path,
        "evaluate",
        {
            "session_id": "s",
            "expression": "value",
            "frame_id": 4,
            "allow_side_effects": True,
        },
    )["operation"] == "evaluate"
    assert broker.dispatch(
        tmp_path, "capture_stack", {"session_id": "s", "thread_id": 7}
    )["operation"] == "capture_stack"
    assert broker.dispatch(
        tmp_path, "stop", {"session_id": "s", "terminate_debuggee": False}
    ) == {"stopped": "s"}
    with pytest.raises(ValueError, match="unknown broker action"):
        broker.dispatch(tmp_path, "unknown", {})


def test_loopback_broker_handler_authentication_and_client(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = _FakeManager()
    monkeypatch.setattr(broker, "get_debug_manager", lambda: manager)
    server = broker.BrokerServer(tmp_path, "secret")
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    state_path = tmp_path / ".devcouncil" / "codeintel" / "debug-broker.json"
    state_path.parent.mkdir(parents=True)
    state_path.write_text(
        json.dumps(
            {
                "host": "127.0.0.1",
                "port": server.server_address[1],
                "token": "secret",
            }
        ),
        encoding="utf-8",
    )
    try:
        client = DebugBrokerClient(tmp_path)
        assert client.call("ping")["sessions"] == [{"id": "session"}]
        client.ensure_started()

        state = json.loads(state_path.read_text(encoding="utf-8"))
        state["token"] = "wrong"
        state_path.write_text(json.dumps(state), encoding="utf-8")
        with pytest.raises(RuntimeError, match="invalid debug broker token"):
            client.call("ping")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def test_broker_client_startup_timeout_and_success(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DebugBrokerClient(tmp_path)
    attempts = iter([OSError("stale"), OSError("starting"), {"pid": 1}])

    def call(*_args, **_kwargs):
        result = next(attempts)
        if isinstance(result, Exception):
            raise result
        return result

    monkeypatch.setattr(client, "call", call)
    spawned: list[list[str]] = []
    monkeypatch.setattr(
        "devcouncil.codeintel.debug.broker_client.subprocess.Popen",
        lambda command, **_kwargs: spawned.append(command),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.debug.broker_client.time.sleep", lambda _seconds: None
    )
    client.ensure_started(timeout=1)
    assert spawned[0][-2:] == ["--root", str(tmp_path)]

    monkeypatch.setattr(client, "call", lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError()))
    ticks = iter([0.0, 0.0, 1.0])
    monkeypatch.setattr(
        "devcouncil.codeintel.debug.broker_client.time.monotonic", lambda: next(ticks)
    )
    with pytest.raises(TimeoutError, match="did not start"):
        client.ensure_started(timeout=0.5)


def test_python_trace_runner_records_calls_and_restores_argv(tmp_path: Path) -> None:
    script = tmp_path / "program.py"
    output = tmp_path / "trace" / "calls.jsonl"
    script.write_text(
        "import sys\n"
        "def child(): return sys.argv[1]\n"
        "def parent(): return child()\n"
        "assert parent() == 'arg'\n"
        "raise SystemExit(3)\n",
        encoding="utf-8",
    )
    previous = list(sys.argv)

    assert run_trace(tmp_path, output, script, ["arg"]) == 3
    assert sys.argv == previous
    rows = [json.loads(line) for line in output.read_text(encoding="utf-8").splitlines()]
    assert any(row["source"].endswith("::parent") and row["target"].endswith("::child") for row in rows)
    assert all(row["count"] >= 1 and row["first_seen"] <= row["last_seen"] for row in rows)


def test_dap_protocol_error_response_events_and_evaluate() -> None:
    assert read_message(io.BytesIO(b"")) is None
    with pytest.raises(DAPError, match="mid-message"):
        read_message(io.BytesIO(b"Content-Length: 5\r\n\r\n{}"))
    with pytest.raises(DAPError, match="must be an object"):
        read_message(io.BytesIO(b"Content-Length: 2\r\n\r\n[]"))

    client = object.__new__(DAPClient)
    client._pending = {}
    failed: queue.Queue[dict] = queue.Queue()
    failed.put({"success": False, "message": "rejected"})
    with pytest.raises(DAPError, match="rejected"):
        client.wait_response(PendingRequest(1, failed), timeout=0)
    with pytest.raises(TimeoutError, match="timed out"):
        client.wait_response(PendingRequest(2, queue.Queue()), timeout=0)

    seen: list[tuple[str, dict]] = []
    client.request = lambda command, arguments=None, **_kwargs: seen.append(
        (command, arguments or {})
    ) or {"result": "ok"}
    assert client.evaluate("value", frame_id=9, allow_side_effects=True) == {"result": "ok"}
    assert seen == [("evaluate", {"expression": "value", "context": "repl", "frameId": 9})]

    client._events = queue.Queue()
    client._events.put({"event": "output"})
    client._events.put({"event": "stopped"})
    assert client.wait_event("stopped", timeout=0.1)["event"] == "stopped"
    assert client.wait_event(timeout=0.1)["event"] == "output"


def test_dap_client_round_trip_initialize_reverse_request_and_close() -> None:
    client_sock, adapter_sock = socket.socketpair()
    reader = client_sock.makefile("rb")
    writer = client_sock.makefile("wb")
    client = DAPClient(reader, writer, sock=client_sock)
    adapter_reader = adapter_sock.makefile("rb")
    adapter_writer = adapter_sock.makefile("wb")
    received: list[dict] = []

    def adapter() -> None:
        request = read_message(adapter_reader)
        assert request is not None
        received.append(request)
        adapter_writer.write(
            encode_message(
                {
                    "seq": 1,
                    "type": "response",
                    "request_seq": request["seq"],
                    "success": True,
                    "body": {"supportsConfigurationDoneRequest": True},
                }
            )
        )
        adapter_writer.write(
            encode_message({"seq": 2, "type": "event", "event": "initialized"})
        )
        adapter_writer.write(
            encode_message(
                {
                    "seq": 3,
                    "type": "request",
                    "command": "runInTerminal",
                }
            )
        )
        adapter_writer.flush()
        reverse_response = read_message(adapter_reader)
        assert reverse_response is not None
        received.append(reverse_response)

    thread = threading.Thread(target=adapter)
    thread.start()
    assert client.initialize(adapter_id="fixture")["supportsConfigurationDoneRequest"] is True
    assert client.wait_event("initialized", timeout=1)["event"] == "initialized"
    thread.join(timeout=2)
    assert received[0]["command"] == "initialize"
    assert received[1]["success"] is False
    client.close()
    client.close()
    adapter_reader.close()
    adapter_writer.close()
    adapter_sock.close()


def test_dap_start_stdio_connect_tcp_and_process_shutdown(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    killed: list[str] = []

    class MissingPipes:
        stdin = None
        stdout = None

        def kill(self):
            killed.append("killed")

    monkeypatch.setattr(
        "devcouncil.codeintel.debug.protocol.subprocess.Popen",
        lambda *_args, **_kwargs: MissingPipes(),
    )
    with pytest.raises(DAPError, match="stdio pipes"):
        DAPClient.start_stdio(["adapter"])
    assert killed == ["killed"]

    left, right = socket.socketpair()
    monkeypatch.setattr(
        "devcouncil.codeintel.debug.protocol.socket.create_connection",
        lambda *_args, **_kwargs: left,
    )
    client = DAPClient.connect_tcp("127.0.0.1", 1234)
    right.shutdown(socket.SHUT_WR)
    client._reader_thread.join(timeout=1)
    client.close()
    right.close()


def test_worker_helpers_activation_calls_and_pool_recovery(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class Span:
        start_line = 2
        end_line = 5

    item = types.SimpleNamespace(
        name="outer",
        kind="function",
        span=Span(),
        decorators=["route"],
        children=[types.SimpleNamespace(name="inner", kind="function")],
    )
    row = workers._structure_row(item)
    assert row["start_line"] == 2
    assert row["children"][0]["name"] == "inner"

    class Node:
        type = "call_expression"
        children: list[object] = []
        named_children: list[object] = []
        start_point = (4, 0)

        def __init__(self, callee=None):
            self.callee = callee

        def child_by_field_name(self, field):
            if field == "function":
                return self.callee
            return None

    callee = types.SimpleNamespace(start_byte=0, end_byte=7)
    parser = types.SimpleNamespace(
        parse=lambda _raw: types.SimpleNamespace(root_node=Node(callee))
    )
    assert workers._call_rows(parser, "obj.run()") == [
        {"name": "run", "receiver": "obj", "line": 5}
    ]

    workers._ACTIVATION_ATTEMPTED = False
    workers._ACTIVATION_STATUS = {"installed": False, "activated": False}
    companion = types.SimpleNamespace(activate=lambda: {"activated": True, "ok": True})
    monkeypatch.setitem(sys.modules, "devcouncil_codeintel_grammars", companion)
    assert workers._activate_companion_once()["activated"] is True
    assert workers._activate_companion_once()["installed"] is True

    class Future:
        def result(self, timeout):
            assert timeout == 1.0
            raise RuntimeError("native crash")

    fake_pool = types.SimpleNamespace(
        submit=lambda *_args: Future(),
        shutdown=lambda **_kwargs: None,
    )
    pool = workers.ParserWorkerPool(max_workers=99, timeout=0)
    pool._pool = fake_pool
    assert pool.max_workers == 4
    assert pool.timeout == 1.0
    assert pool.process("python", "x = 1") is None
    assert pool._pool is None
    pool.close()
    status = workers.parser_worker_status()
    assert status["start_method"] == "spawn"


def _query_graph() -> CodeGraph:
    nodes = [
        GraphNode(id="app.py", kind=NodeKind.FILE, path="app.py", name="app.py"),
        GraphNode(
            id="app.py::target",
            kind=NodeKind.FUNCTION,
            path="app.py",
            name="target",
            line=1,
            end_line=2,
            language="python",
        ),
        GraphNode(
            id="app.py::caller",
            kind=NodeKind.FUNCTION,
            path="app.py",
            name="caller",
            line=4,
            end_line=5,
            language="python",
        ),
        GraphNode(
            id="tests/test_app.py::test_target",
            kind=NodeKind.FUNCTION,
            path="tests/test_app.py",
            name="test_target",
            line=1,
            end_line=1,
            language="python",
        ),
    ]
    return CodeGraph(
        nodes=nodes,
        edges=[
            GraphEdge(
                source="app.py::caller",
                target="app.py::target",
                kind="calls",
                confidence=Confidence.INFERRED,
            ),
            GraphEdge(
                source="tests/test_app.py::test_target",
                target="app.py::caller",
                kind="calls",
            ),
            GraphEdge(source="app.py", target="app.py::target", kind="contains"),
        ],
        dead_code=[
            DeadCodeEntry(
                id="app.py::target",
                path="app.py",
                confidence=Confidence.EXTRACTED,
                reason="no callers",
            )
        ],
    )


def test_generation_keyed_query_cache_memoises_within_a_generation(tmp_path: Path) -> None:
    """The service's per-generation cache answers a repeated key once.

    What is left of a set of assertions that also exercised
    ``CodeIntelQueryEngine``'s ``explore``/``path``/``impact``/
    ``affected_tests``/``dead``. That engine was deleted: every one of those
    surfaces is answered by the Rust kernel now, and the kernel's own tests
    (``rust-port/crates/devmap-query/tests/explore_and_affected.rs``) cover the
    behaviour with fixtures that do not need a Python graph. The cache is not
    part of that migration — it belongs to the service — so it keeps its test.
    """
    service = CodeIntelService(tmp_path)
    service.persist(_query_graph())

    calls = 0

    def load():
        nonlocal calls
        calls += 1
        return {"calls": calls}

    assert service.cached_query("fixture", "key", load) == {"calls": 1}
    assert service.cached_query("fixture", "key", load) == {"calls": 1}
    assert calls == 1


def test_runtime_merge_skips_fingerprinting_without_runtime_observations(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Fingerprinting shells out to git, so it runs only when it can match.

    Retargeted from ``CodeIntelQueryEngine._graph`` to
    ``CodeIntelService.load_with_runtime_observations``, which is where that
    loader now lives — ``run_cypher`` is its one production caller. The
    dead-candidate suppression the second half of this test used to assert went
    with the engine; see
    ``test_codeintel_debug.test_matching_runtime_observation_becomes_a_graph_edge``
    for what survives and what does not.
    """
    (tmp_path / "app.py").write_text("def target():\n    return 1\n", encoding="utf-8")
    service = CodeIntelService(tmp_path)
    service.persist(_query_graph())

    def _boom(root: Path) -> str:
        raise AssertionError("source_fingerprint must not run without runtime evidence")

    monkeypatch.setattr(
        "devcouncil.codeintel.debug.fingerprint.source_fingerprint", _boom
    )
    graph = service.load_with_runtime_observations()
    assert not [edge for edge in graph.edges if edge.extras.get("provenance") == "runtime"]

    session = service.store.start_runtime_session(
        provider="pytest", source_fingerprint="fp", build_fingerprint="bp"
    )
    inserted = service.store.add_runtime_observations(
        session,
        [
            {"source": "app.py::caller", "target": "app.py::target"},
            {"source": "", "target": "dropped"},
        ],
    )
    assert inserted == 1

    monkeypatch.setattr(
        "devcouncil.codeintel.debug.fingerprint.source_fingerprint",
        lambda root: "fp",
    )
    merged = service.load_with_runtime_observations()
    runtime = [edge for edge in merged.edges if edge.extras.get("provenance") == "runtime"]
    assert [(edge.source, edge.target) for edge in runtime] == [
        ("app.py::caller", "app.py::target")
    ]


def test_mcp_debug_dispatches_every_provider_and_manager_action(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = _FakeManager()
    monkeypatch.setattr(mcp_debug, "require_debug_consent", lambda _root: None)
    monkeypatch.setattr(mcp_debug, "get_debug_manager", lambda: manager)
    adapter = types.SimpleNamespace(command=("adapter",), as_dict=lambda: {"id": "fixture"})
    monkeypatch.setattr(mcp_debug, "adapter_by_id", lambda _adapter_id: adapter)
    monkeypatch.setattr(mcp_debug, "discover_adapters", lambda: [adapter])

    class Provider:
        def __init__(self, root):
            assert root == tmp_path

        def run(self, script, args):
            return {"script": str(script), "args": args}

    monkeypatch.setattr(mcp_debug, "PythonTraceProvider", Provider)
    monkeypatch.setattr(mcp_debug, "NodeCpuProfileProvider", Provider)
    monkeypatch.setattr(
        mcp_debug, "import_runtime_trace", lambda _root, path: {"path": str(path)}
    )

    async def invoke(name: str, arguments: dict) -> dict:
        result = await mcp_debug.dispatch(name, tmp_path, arguments)
        assert result is not None
        return json.loads(result[0].text)

    assert asyncio.run(invoke("devcouncil_debug_discover", {}))["adapters"][0]["id"] == "fixture"
    assert asyncio.run(
        invoke(
            "devcouncil_debug_start",
            {
                "adapterId": "fixture",
                "request": "attach",
                "initialBreakpoints": {"app.py": [2]},
            },
        )
    )["id"] == "session"
    for name, arguments, operation in [
        (
            "devcouncil_debug_breakpoints",
            {"sessionId": "s", "source": "app.py", "lines": [2]},
            "breakpoints",
        ),
        (
            "devcouncil_debug_control",
            {"sessionId": "s", "action": "next", "threadId": 1},
            "control",
        ),
        (
            "devcouncil_debug_inspect",
            {"sessionId": "s", "operation": "threads"},
            "inspect",
        ),
        (
            "devcouncil_debug_evaluate",
            {
                "sessionId": "s",
                "expression": "x",
                "frameId": 2,
                "allowSideEffects": True,
            },
            "evaluate",
        ),
    ]:
        assert asyncio.run(invoke(name, arguments))["operation"] == operation
    for provider, extra in [
        ("dap-stack", {"sessionId": "s", "threadId": 1}),
        ("python", {"script": "app.py", "args": ["a"]}),
        ("node", {"script": "app.js"}),
        ("import", {"path": "trace.jsonl"}),
    ]:
        payload = asyncio.run(
            invoke("devcouncil_debug_trace", {"provider": provider, **extra})
        )
        assert payload
    assert asyncio.run(
        invoke(
            "devcouncil_debug_stop",
            {"sessionId": "s", "terminateDebuggee": False},
        )
    ) == {"stopped": "s"}
    assert asyncio.run(mcp_debug.dispatch("missing", tmp_path, {})) is None
    error = asyncio.run(
        invoke("devcouncil_debug_trace", {"provider": "unsupported"})
    )
    assert error["code"] == "debug_error"


def test_debug_cli_helpers_and_broker_commands(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from devcouncil.cli.commands import debug_cmd

    config = tmp_path / ".devcouncil" / "config.yaml"
    config.parent.mkdir()
    config.write_text("project:\n  name: fixture\n", encoding="utf-8")
    debug_cmd.set_debug_consent(tmp_path, True)
    calls: list[tuple[str, dict]] = []

    class Client:
        def __init__(self, root):
            assert root == tmp_path

        def ensure_started(self):
            calls.append(("ensure", {}))

        def call(self, action, arguments):
            calls.append((action, arguments))
            return {"action": action}

    monkeypatch.setattr(debug_cmd, "DebugBrokerClient", Client)
    assert debug_cmd._config("{}") == {}
    with pytest.raises(Exception):
        debug_cmd._config("[]")
    assert debug_cmd._initial_breakpoints([f"{tmp_path / 'app.py'}:12"])
    with pytest.raises(Exception):
        debug_cmd._initial_breakpoints(["missing-line"])
    with pytest.raises(Exception):
        debug_cmd._initial_breakpoints(["app.py:nope"])

    result = runner.invoke(
        app,
        [
            "debug",
            "start",
            "--adapter-command",
            "fixture",
            "--config-json",
            '{"program":"app.py"}',
            "--breakpoint",
            f"{tmp_path / 'app.py'}:12",
            "--project-root",
            str(tmp_path),
        ],
    )
    assert result.exit_code == 0, result.output
    assert calls[-1][0] == "start"

    for arguments, action in [
        (["continue", "s"], "control"),
        (["pause", "s"], "control"),
        (["step", "s", "--thread", "2", "--kind", "stepIn"], "control"),
        (["threads", "s"], "inspect"),
        (["stack", "s", "--thread", "2"], "inspect"),
        (["scopes", "s", "--frame", "3"], "inspect"),
        (["variables", "s", "--reference", "4"], "inspect"),
        (["break", "s", "app.py", "2", "3"], "breakpoints"),
        (
            ["evaluate", "s", "x + 1", "--frame", "3", "--allow-side-effects"],
            "evaluate",
        ),
        (["stop", "s", "--keep-debuggee"], "stop"),
    ]:
        result = runner.invoke(
            app, ["debug", *arguments, "--project-root", str(tmp_path)]
        )
        assert result.exit_code == 0, result.output
        assert calls[-1][0] == action

    refused = runner.invoke(
        app,
        ["debug", "evaluate", "s", "x", "--project-root", str(tmp_path)],
    )
    assert refused.exit_code != 0


def test_debug_cli_discovery_and_trace_variants(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from devcouncil.cli.commands import debug_cmd
    from devcouncil.codeintel.debug import tracing

    config = tmp_path / ".devcouncil" / "config.yaml"
    config.parent.mkdir()
    config.write_text("project:\n  name: fixture\n", encoding="utf-8")
    debug_cmd.set_debug_consent(tmp_path, True)
    adapter = types.SimpleNamespace(
        as_dict=lambda: {
            "id": "fixture",
            "path": "/adapter",
            "version": None,
            "requests": ["launch", "attach"],
            "executable_hash": "abc",
        }
    )
    monkeypatch.setattr(debug_cmd, "discover_adapters", lambda: [adapter])
    discovered = runner.invoke(
        app, ["debug", "discover", "--project-root", str(tmp_path)]
    )
    assert discovered.exit_code == 0
    assert "unknown version" in discovered.output
    monkeypatch.setattr(debug_cmd, "discover_adapters", lambda: [])
    empty = runner.invoke(app, ["debug", "discover", "--project-root", str(tmp_path)])
    assert "No supported" in empty.output

    class Provider:
        def __init__(self, root):
            assert root == tmp_path

        def run(self, script, args):
            return {"script": str(script), "args": args}

    monkeypatch.setattr(tracing, "PythonTraceProvider", Provider)
    monkeypatch.setattr(tracing, "NodeCpuProfileProvider", Provider)
    monkeypatch.setattr(
        tracing, "import_runtime_trace", lambda _root, path: {"path": str(path)}
    )
    for option, value in [
        ("--python-script", "app.py"),
        ("--node-script", "app.js"),
        ("--import", "trace.jsonl"),
    ]:
        result = runner.invoke(
            app,
            [
                "debug",
                "trace",
                option,
                value,
                "--project-root",
                str(tmp_path),
            ],
        )
        assert result.exit_code == 0, result.output
    missing = runner.invoke(
        app, ["debug", "trace", "--project-root", str(tmp_path)]
    )
    assert missing.exit_code != 0


def test_graph_cli_status_sync_search_explore_and_affected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import devcouncil.codeintel as codeintel

    monkeypatch.setattr(
        codeintel,
        "get_codeintel_service",
        lambda _root: types.SimpleNamespace(
            status=lambda: {
                "state": "committed",
                "generation": 3,
                "node_count": 4,
                "edge_count": 2,
            }
        ),
    )
    monkeypatch.setattr(
        "devcouncil.devmap_client.try_connect", lambda _root: _FakeKernelClient()
    )
    # `status` and `sync` read and drive the kernel now, not the coordinator.
    monkeypatch.setattr(
        "devcouncil.devmap_health.kernel_status",
        lambda root: {
            "generation_id": 3,
            "pending_count": 1,
            "quarantined_count": 0,
            "node_count": 4,
            "edge_count": 2,
            "is_fresh": False,
            "degraded_reason": "fixture fallback",
        },
    )
    from devcouncil.devmap_engine import DevMapEngineError

    def _kernel_refresh(root, output, *_a, **_k):
        return types.SimpleNamespace(
            generation=3, mode="devmap-rust", degraded=False, reason="", kernel_status=None
        )

    monkeypatch.setattr("devcouncil.indexing.map_artifacts.refresh_map_artifacts", _kernel_refresh)

    for arguments, expected in [
        (["status"], "pending 1"),
        (["status", "--json"], '"generation": 3'),
        (["sync"], "Synced: generation 3"),
        (["search", "target"], "app.py::target"),
        (["search", "target", "--json"], '"matches"'),
        (["explore", "target"], "callers=1"),
        (["affected", "target"], "tests/test_app.py"),
        (["affected", "none"], "No affected tests"),
    ]:
        result = runner.invoke(
            app, ["map", *arguments, "--project-root", str(tmp_path)]
        )
        assert result.exit_code == 0, result.output
        assert expected in result.output
    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(DevMapEngineError("kernel unavailable")),
    )
    failed = runner.invoke(
        app,
        [
            "map",
            "sync",
            "fail.py",
            "--json",
            "--project-root",
            str(tmp_path),
        ],
    )
    assert failed.exit_code == 1
    assert '"ok": false' in failed.output
    assert '"engine_unavailable"' in failed.output


def test_graph_cli_init_watch_doctor_and_hooks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import devcouncil.cli.commands.map as map_command
    import devcouncil.codeintel as codeintel
    import devcouncil.indexing.map_artifacts as map_artifacts

    generated: list[tuple] = []

    def _record(*args, **kwargs):
        generated.append((args, kwargs))
        return types.SimpleNamespace(
            degraded=False,
            reason="",
            mode="full",
            generation=7,
            compatibility_export_degraded=False,
            build_incomplete=False,
        )

    monkeypatch.setattr(map_artifacts, "generate_map_artifacts", _record)
    monkeypatch.setattr(map_command, "generate_map_artifacts", _record)
    monkeypatch.setattr(map_artifacts, "refresh_map_artifacts", _record)
    monkeypatch.setattr(
        codeintel,
        "get_codeintel_service",
        lambda _root: types.SimpleNamespace(
            status=lambda: {
                "state": "committed",
                "generation": 7,
                "schema_version": 2,
                "node_count": 8,
                "edge_count": 9,
            }
        ),
    )
    initialized = runner.invoke(
        app,
        [
            "map",
            "init",
            "--json",
            "--project-root",
            str(tmp_path),
        ],
    )
    assert initialized.exit_code == 0, initialized.output
    # The kernel decides incrementality; `init` passes only `full` through.
    assert generated[0][1]["full"] is False
    assert json.loads(initialized.output)["generation"] == 7

    # `watch` is the kernel watcher: interrupt it at the first wait.
    monkeypatch.setattr(
        "devcouncil.cli.commands.map._wait_for_change",
        lambda _changed, _timeout: (_ for _ in ()).throw(KeyboardInterrupt()),
    )
    watched = runner.invoke(
        app, ["map", "watch", "--project-root", str(tmp_path)]
    )
    assert watched.exit_code == 0
    assert "Stopped watching" in watched.output

    # Doctor audits the kernel: a usable binary with no store yet is healthy.
    monkeypatch.setattr(
        "devcouncil.devmap_health.engine_info",
        lambda root: {
            "binary": "/opt/devmap",
            "built_at": "2026-09-02T16:51:00",
            "version": "devmap 0.1.0 (schema 12)",
            "schema_version": 12,
            "error": None,
        },
    )
    doctor = runner.invoke(
        app, ["map", "doctor", "--json", "--project-root", str(tmp_path)]
    )
    assert doctor.exit_code == 0
    assert json.loads(doctor.output)["ok"] is True

    no_git = runner.invoke(
        app, ["map", "hooks", "install", "--project-root", str(tmp_path)]
    )
    assert no_git.exit_code == 1
    (tmp_path / ".git" / "hooks").mkdir(parents=True)
    installed = runner.invoke(
        app, ["map", "hooks", "install", "--project-root", str(tmp_path)]
    )
    assert installed.exit_code == 0
    assert (tmp_path / ".git" / "hooks" / "post-checkout").stat().st_mode & 0o111
    (tmp_path / ".git" / "hooks" / "post-merge").write_text(
        "#!/bin/sh\nother\n", encoding="utf-8"
    )
    conflict = runner.invoke(
        app, ["map", "hooks", "install", "--project-root", str(tmp_path)]
    )
    assert conflict.exit_code == 1


def test_language_registry_detection_and_grammar_status(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The FSEvents-preflight half went with the Python watcher; the kernel
    daemon does its own watching and preflighting."""
    from devcouncil.codeintel.languages import registry

    assert registry.detect_language("Component.VUE").name == "Vue"
    assert registry.detect_language("README") is None
    assert "Python" in registry.supported_languages()

    broken_pack = types.ModuleType("tree_sitter_language_pack")

    def unavailable():
        raise RuntimeError("pack unavailable")

    broken_pack.available_languages = unavailable
    monkeypatch.setitem(sys.modules, "tree_sitter_language_pack", broken_pack)
    monkeypatch.delitem(
        sys.modules, "devcouncil_codeintel_grammars", raising=False
    )
    result = registry.grammar_status()
    assert result["ok"] is False
    assert result["error"]
    assert "platform-matched" in result["action"]


def test_debug_module_entrypoints_and_broker_cleanup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from devcouncil.codeintel.debug import python_trace_runner

    script = tmp_path / "exit.py"
    output = tmp_path / "trace.jsonl"
    script.write_text("raise SystemExit('message')\n", encoding="utf-8")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "python_trace_runner",
            "--root",
            str(tmp_path),
            "--output",
            str(output),
            str(script),
        ],
    )
    assert python_trace_runner.main() == 1

    class Server:
        server_address = ("127.0.0.1", 4321)

        def __init__(self, root, token):
            self.root = root
            self.token = token

        def serve_forever(self, poll_interval):
            assert poll_interval == 0.25

        def server_close(self):
            return None

    monkeypatch.setattr(broker, "BrokerServer", Server)
    monkeypatch.setattr(broker.secrets, "token_urlsafe", lambda _size: "token")
    monkeypatch.setattr(sys, "argv", ["broker", "--root", str(tmp_path)])
    assert broker.main() == 0
    assert not (
        tmp_path / ".devcouncil" / "codeintel" / "debug-broker.json"
    ).exists()


def test_worker_native_process_and_activation_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class Result:
        structure = [
            types.SimpleNamespace(
                name="run",
                kind="function",
                span=None,
                decorators=[],
                children=[],
            )
        ]
        imports = [
            types.SimpleNamespace(source="pkg", items=["Thing"], alias="Alias")
        ]
        exports = [types.SimpleNamespace(name="run")]

    parser = types.SimpleNamespace(
        parse=lambda _raw: types.SimpleNamespace(
            root_node=types.SimpleNamespace(children=[])
        )
    )
    pack = types.ModuleType("tree_sitter_language_pack")
    pack.available_languages = lambda: ["python"]
    pack.ProcessConfig = lambda **kwargs: kwargs
    pack.process = lambda _source, _config: Result()
    pack.get_parser = lambda _language: parser
    monkeypatch.setitem(sys.modules, "tree_sitter_language_pack", pack)
    assert workers._native_process("missing", "x") is None
    processed = workers._native_process("python", "run()")
    assert processed is not None
    assert processed["imports"][0]["alias"] == "Alias"
    assert processed["exports"] == [{"name": "run"}]

    workers._ACTIVATION_ATTEMPTED = False
    companion = types.SimpleNamespace(
        activate=lambda: (_ for _ in ()).throw(RuntimeError("bad manifest"))
    )
    monkeypatch.setitem(sys.modules, "devcouncil_codeintel_grammars", companion)
    status = workers._activate_companion_once()
    assert status["installed"] is True
    assert "bad manifest" in status["error"]


def test_advisor_preflight_probe_and_pairing_branches(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from devcouncil.executors import advisor_tool

    assert advisor_tool.normalize_model_family(None) is None
    assert advisor_tool.normalize_model_family("   ") is None
    assert advisor_tool.normalize_model_family("custom-model") is None
    assert advisor_tool.advisor_pairing_ok("custom", "custom")[0] is True
    assert advisor_tool.advisor_pairing_ok("sonnet", "")[0] is False
    assert advisor_tool.advisor_pairing_ok("fable", "fable")[0] is True
    assert advisor_tool.advisor_provider_unsupported({}) is None
    assert advisor_tool.advisor_disable_env_set(
        {advisor_tool.DISABLE_ADVISOR_ENV: "yes"}
    )
    assert advisor_tool.advisor_user_cost_trim().startswith("(Advisor:")

    advisor_tool.probe_claude_version.cache_clear()
    monkeypatch.setattr(advisor_tool.shutil, "which", lambda _name: None)
    assert advisor_tool.probe_claude_version() is None
    advisor_tool.probe_claude_version.cache_clear()
    monkeypatch.setattr(advisor_tool.shutil, "which", lambda _name: "/bin/claude")
    monkeypatch.setattr(
        advisor_tool.subprocess,
        "run",
        lambda *_args, **_kwargs: types.SimpleNamespace(
            stdout="Claude Code 2.1.80", stderr=""
        ),
    )
    assert advisor_tool.probe_claude_version() == (2, 1, 80)
    advisor_tool.claude_supports_append_system_prompt.cache_clear()
    assert advisor_tool.claude_supports_append_system_prompt() is False
    advisor_tool.claude_supports_append_system_prompt.cache_clear()
    monkeypatch.setattr(
        advisor_tool.subprocess,
        "run",
        lambda *_args, **_kwargs: types.SimpleNamespace(
            stdout="--append-system-prompt", stderr=""
        ),
    )
    assert advisor_tool.claude_supports_append_system_prompt() is True
    monkeypatch.setattr(advisor_tool, "probe_claude_version", lambda: (2, 1, 50))
    warnings = advisor_tool.warn_advisor_preflight(
        env={advisor_tool.DISABLE_ADVISOR_ENV: "true"},
        main_model="sonnet",
        advisor_model="opus",
    )
    assert len(warnings) == 2
    assert advisor_tool.decide_advisor_attach(
        main_model="sonnet", advisor_model=None
    ) == ("skip", None, None)


def test_store_uninitialized_and_payload_edge_cases(tmp_path: Path) -> None:
    from devcouncil.codeintel.store import sqlite as store_module

    store = CodeIntelStore(tmp_path)
    assert store.compatibility_export_state() == ("", None)
    assert store.load_graph() is None
    assert store.current_generation() is None
    assert store.search("target") == []
    assert store.content_for_path("app.py") is None
    assert store.file_metadata() == {}
    assert store.analysis_shards() == {}
    assert store.unresolved_references() == []
    assert store.aliases() == []
    assert store.get_extraction(
        content_hash="none",
        language="python",
        grammar_version="1",
        config_hash="cfg",
    ) is None
    assert store.runtime_observations() == []
    assert store.status().state == "uninitialized"

    extracted = GraphEdge(
        source="a",
        target="b",
        kind="calls",
        extras={"confidence_score": 2.0, "provenance": "runtime"},
    )
    assert store_module._confidence_score(extracted) == 1.0
    assert store_module._provenance(extracted) == "runtime"
    inferred = GraphEdge(
        source="a",
        target="b",
        kind="calls",
        confidence=Confidence.INFERRED,
        extras={"provenance": "invalid"},
    )
    assert store_module._confidence_score(inferred) == 0.7
    assert store_module._provenance(inferred) == "inferred"

    evidence = list(range(store_module.AMBIGUOUS_EVIDENCE_LIMIT + 3))
    ambiguous = GraphEdge(
        source="a",
        target="b",
        kind="calls",
        confidence=Confidence.AMBIGUOUS,
        extras={"evidence": evidence},
    )
    payload = store._edge_payload(ambiguous)
    assert len(payload["extras"]["evidence"]) == store_module.AMBIGUOUS_EVIDENCE_LIMIT
    assert payload["extras"]["evidence_truncated"] == 3


def test_store_missing_files_analysis_runtime_and_search_fallback(
    tmp_path: Path,
) -> None:
    store = CodeIntelStore(tmp_path)
    graph = CodeGraph(
        nodes=[
            GraphNode(
                id="missing.py",
                kind=NodeKind.FILE,
                path="missing.py",
                name="missing.py",
                language="python",
            )
        ],
        meta={
            "unresolved_references": [
                {
                    "source_id": "missing.py",
                    "name": "dynamic_name",
                    "path": "missing.py",
                },
                "invalid",
            ]
        },
    )
    generation = store.save_graph(
        graph, analysis_shards={"missing.py": {"symbols": ["dynamic_name"]}}
    )
    assert generation == 1
    assert store.content_for_path("missing.py") is None
    assert store.analysis_shards()["missing.py"]["symbols"] == ["dynamic_name"]
    assert store.unresolved_references(name="dynamic_name")[0]["line"] == 0
    assert store.search("missing.py", limit=0)[0]["id"] == "missing.py"
    assert store.search("   ") == []
    assert store.add_runtime_observations("absent", []) == 0


def test_debug_session_manager_control_inspect_evaluate_stop_and_errors(
    tmp_path: Path,
) -> None:
    class Client:
        closed = False

        def request(self, command, arguments=None, **_kwargs):
            if command == "disconnect":
                raise RuntimeError("already stopped")
            return {
                "command": command,
                "arguments": arguments,
                "secret": "token=private",
            }

        def evaluate(self, expression, **kwargs):
            return {"value": expression, "authorization": "authorization=bearer"}

        def close(self):
            self.closed = True

    client = Client()
    session = DebugSession(
        id="s",
        root=tmp_path,
        client=client,  # type: ignore[arg-type]
        adapter_command=("adapter",),
        adapter_id="fixture",
        adapter_version="1",
        adapter_requests=("launch",),
        request="launch",
        capabilities={},
        source_fingerprint="source",
        build_fingerprint="build",
        executable_hash="exe",
    )
    manager = DebugSessionManager()
    manager._sessions["s"] = session
    assert manager.list()[0]["id"] == "s"
    assert manager.set_breakpoints("s", "app.py", [2])["command"] == "setBreakpoints"
    assert manager.control("s", "pause")["arguments"] == {}
    assert manager.control("s", "next", thread_id=3)["arguments"] == {"threadId": 3}
    with pytest.raises(ValueError, match="unsupported debug action"):
        manager.control("s", "rewind")
    inspected = manager.inspect("s", "threads", {})
    assert inspected["secret"] == "token=[REDACTED]"
    with pytest.raises(ValueError, match="unsupported inspect"):
        manager.inspect("s", "memory", {})
    assert "[REDACTED]" in manager.evaluate(
        "s", "x", frame_id=None, allow_side_effects=True
    )["authorization"]
    assert manager._frame_id({"name": "frame"}) == "<unknown>:frame:0"
    manager.stop("s")
    assert client.closed and manager.list() == []
    with pytest.raises(KeyError, match="unknown debug session"):
        manager.get("s")


def test_debug_session_start_rejects_request_and_closes_failed_client(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manager = DebugSessionManager()
    with pytest.raises(ValueError, match="launch or attach"):
        manager.start(tmp_path, ["adapter"], request="bad", arguments={})

    class Client:
        closed = False

        def initialize(self, **_kwargs):
            raise RuntimeError("initialize failed")

        def close(self):
            self.closed = True

    client = Client()
    monkeypatch.setattr(
        "devcouncil.codeintel.debug.session.DAPClient.start_stdio",
        lambda *_args, **_kwargs: client,
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.debug.session.adapter_by_command", lambda _command: None
    )
    with pytest.raises(RuntimeError, match="initialize failed"):
        manager.start(tmp_path, ["adapter"], request="launch", arguments={})
    assert client.closed


def test_trace_loaders_runtime_version_and_import(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from devcouncil.codeintel.debug import tracing

    jsonl = tmp_path / "trace.jsonl"
    jsonl.write_text(
        "not json\n"
        "[]\n"
        '{"source":"a","target":"b"}\n'
        '{"source":"","target":"b"}\n',
        encoding="utf-8",
    )
    assert tracing.load_jsonl_observations(jsonl) == [{"source": "a", "target": "b"}]
    assert tracing.load_jsonl_observations(tmp_path / "missing") == []

    profile = tmp_path / "profile.cpuprofile"
    profile.write_text(
        json.dumps(
            {
                "nodes": [
                    {
                        "id": 1,
                        "callFrame": {},
                        "children": [2, 99],
                    },
                    {
                        "id": 2,
                        "callFrame": {
                            "url": "",
                            "functionName": "",
                            "lineNumber": -1,
                        },
                    },
                    "invalid",
                ],
                "samples": [2],
            }
        ),
        encoding="utf-8",
    )
    rows = tracing.load_node_cpu_profile(profile)
    assert rows[0]["source"] == "<unknown>:<anonymous>:0"
    assert rows[0]["count"] == 1

    monkeypatch.setattr(
        tracing.subprocess,
        "run",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError("missing")),
    )
    assert tracing._runtime_version("node") == ""
    monkeypatch.setattr(tracing.shutil, "which", lambda _name: None)
    with pytest.raises(FileNotFoundError, match="Node.js"):
        tracing.NodeCpuProfileProvider(tmp_path)

    imported = tracing.import_runtime_trace(tmp_path, jsonl)
    assert imported["observation_count"] == 1


def test_mcp_codeintel_dispatch_all_handlers_and_errors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def _kernel_cli_refresh(_root, _output, *_a, **_k):
        return types.SimpleNamespace(
            generation=11,
            mode="devmap-rust",
            reason="",
            degraded=False,
            kernel_status=types.SimpleNamespace(
                generation_id=11,
                pending_count=0,
                is_fresh=True,
                degraded_reason=None,
            ),
        )

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _kernel_cli_refresh
    )

    async def invoke(name, arguments):
        result = await mcp_codeintel.dispatch(name, tmp_path, arguments)
        assert result is not None
        return json.loads(result[0].text)

    # Every query tool is kernel-only now, and this fixture has no devmap
    # store — so every one of them must answer with the same tri-state refusal,
    # naming its own operation and carrying the reason. There is no stubbed
    # Python engine here any more because there is no Python engine: `explore`
    # and `affected_tests` were the last two, and a substituted answer with no
    # signal which engine produced it is the shape this whole migration removed.
    calls = [
        ("devcouncil_code_explore", {"query": "x"}, "explore"),
        ("devcouncil_code_search", {"query": "x"}, "search"),
        ("devcouncil_code_path", {"from": "a", "to": "b"}, "path"),
        ("devcouncil_code_impact", {"targets": ["a"]}, "impact"),
        ("devcouncil_code_dead", {}, "dead"),
        ("devcouncil_code_affected_tests", {"targets": ["a"]}, "affected_tests"),
    ]
    for name, arguments, operation in calls:
        payload = asyncio.run(invoke(name, arguments))
        assert payload["operation"] == operation
        assert payload["ok"] is False, f"{name} must not report an unbuilt store as a zero"
        assert "Unavailable" in payload["resolution"], f"{name} must carry the reason"
        assert payload["engine"] == "devmap-rust"
    # No daemon in this fixture, so `_sync` falls through to the kernel CLI —
    # never to a Python coordinator, which no longer exists.
    synced = asyncio.run(invoke("devcouncil_code_sync", {"paths": ["app.py"]}))
    assert synced["reconciled"] == ["app.py"]
    assert synced["source"] == "devmap-cli"
    assert synced["generation"] == 11
    # `committed` came from the stubbed Python engine while a fallback existed.
    # The Rust kernel is primary now and this fixture never builds a devmap
    # store, so `uninitialized` is the honest answer — the tri-state saying "I
    # have nothing", rather than a second engine's view of a different store
    # presented as though the primary had answered.
    status_payload = asyncio.run(invoke("devcouncil_code_status", {}))
    assert status_payload["state"] == "uninitialized"
    assert status_payload["ok"] is False, "an unbuilt store must not read as a verified zero"
    assert "Unavailable" in status_payload["resolution"], "the reason must be carried, not dropped"
    from devcouncil.devmap_engine import DevMapEngineError

    def _kernel_unavailable(*_a, **_k):
        raise DevMapEngineError("no devmap binary supports this map engine")

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts", _kernel_unavailable
    )
    failed = asyncio.run(
        invoke("devcouncil_code_sync", {"paths": ["fail.py"]})
    )
    assert failed["code"] == "engine_unavailable"
    invalid = asyncio.run(invoke("devcouncil_code_explore", {}))
    assert invalid["code"] == "invalid_arguments"
    assert asyncio.run(mcp_codeintel.dispatch("missing", tmp_path, {})) is None


def test_graph_cli_remaining_output_branches(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import devcouncil.cli.commands.map as map_command
    import devcouncil.codeintel as codeintel
    import devcouncil.indexing.graph.build as graph_build
    import devcouncil.indexing.graph.intel as intel
    import devcouncil.indexing.map_artifacts as map_artifacts

    monkeypatch.setattr(map_artifacts, "generate_map_artifacts", lambda *_a, **_k: None)
    monkeypatch.setattr(map_command, "generate_map_artifacts", lambda *_a, **_k: None)
    monkeypatch.setattr(
        map_artifacts,
        "refresh_map_artifacts",
        lambda *_a, **_k: types.SimpleNamespace(
            degraded=False,
            reason="",
            mode="full",
            generation=1,
            compatibility_export_degraded=False,
            build_incomplete=False,
        ),
    )
    monkeypatch.setattr(
        codeintel,
        "get_codeintel_service",
        lambda _root: types.SimpleNamespace(
            status=lambda: {
                "state": "committed",
                "generation": 1,
                "schema_version": 2,
                "node_count": 2,
                "edge_count": 1,
            }
        ),
    )

    assert runner.invoke(
        app, ["map", "init", "--project-root", str(tmp_path)]
    ).exit_code == 0
    status_result = runner.invoke(
        app, ["map", "status", "--project-root", str(tmp_path)]
    )
    assert status_result.exit_code == 0
    assert "not running" in status_result.output

    # Doctor fails when no kernel can run — the one thing that stops `dev map`.
    monkeypatch.setattr(
        "devcouncil.devmap_health.engine_info",
        lambda root: {
            "binary": None,
            "built_at": None,
            "version": None,
            "schema_version": None,
            "error": "no devmap binary supports this map engine",
        },
    )
    failed_doctor = runner.invoke(
        app, ["map", "doctor", "--json", "--project-root", str(tmp_path)]
    )
    assert failed_doctor.exit_code == 1

    monkeypatch.setattr(
        "devcouncil.devmap_client.try_connect", lambda _root: _FakeKernelClient()
    )
    assert '"definitions"' in runner.invoke(
        app,
        [
            "map",
            "explore",
            "target",
            "--json",
            "--project-root",
            str(tmp_path),
        ],
    ).output
    assert '"tests"' in runner.invoke(
        app,
        [
            "map",
            "affected",
            "target",
            "--json",
            "--project-root",
            str(tmp_path),
        ],
    ).output

    fake_graph = types.SimpleNamespace(dead_code=[], edges=[])
    monkeypatch.setattr(graph_build, "load_code_graph", lambda _root: fake_graph)
    monkeypatch.setattr(
        intel,
        "graph_check",
        lambda _graph, top_n: {"god_nodes": [], "circular_imports": []},
    )
    assert "(none)" in runner.invoke(
        app, ["map", "check", "--project-root", str(tmp_path)]
    ).output
    monkeypatch.setattr(
        intel,
        "extract_processes",
        lambda _graph, entry, max_depth: [{"name": "flow", "depth": 1, "steps": ["a"]}],
    )
    assert '"flow"' in runner.invoke(
        app,
        ["map", "process", "--json", "--project-root", str(tmp_path)],
    ).output
    monkeypatch.setattr(
        intel,
        "diff_impact",
        lambda *_args, **_kwargs: {
            "paths": [
                {
                    "path": "app.py",
                    "symbols": [],
                    "blast": {
                        "layers": [
                            {
                                "depth": 1,
                                "confidence": "inferred",
                                "nodes": [f"n{i}" for i in range(8)],
                            }
                        ]
                    },
                }
            ]
        },
    )
    impacted = runner.invoke(
        app,
        [
            "map",
            "impact",
            "app.py",
            "--max-depth",
            "99",
            "--project-root",
            str(tmp_path),
        ],
    )
    assert impacted.exit_code == 0
    assert "…" in impacted.output

    fake_graph.edges = [
        GraphEdge(source="a", target="b", kind="imports"),
        GraphEdge(source="b", target="c", kind="contains"),
    ]
    out = tmp_path / "links.txt"
    links = runner.invoke(
        app,
        [
            "map",
            "export",
            "--format",
            "okf-links",
            "-o",
            str(out),
            "--project-root",
            str(tmp_path),
        ],
    )
    assert links.exit_code == 0
    assert out.read_text(encoding="utf-8") == "a --imports--> b"


def test_store_export_status_search_and_runtime_filter_branches(
    tmp_path: Path,
) -> None:
    store = CodeIntelStore(tmp_path)
    store.initialize()
    assert store.status().state == "empty"
    source = tmp_path / "app.py"
    source.write_text("def target(): return 1\n", encoding="utf-8")
    graph = CodeGraph(
        nodes=[
            GraphNode(
                id="app.py",
                kind=NodeKind.FILE,
                path="app.py",
                name="app.py",
            ),
            GraphNode(
                id="app.py::target",
                kind=NodeKind.FUNCTION,
                path="app.py",
                name="target",
                line=1,
                end_line=1,
            ),
        ]
    )
    store.save_graph(graph)
    export = tmp_path / "graph.json"
    export.write_text("{}", encoding="utf-8")
    store.record_compatibility_export(export, graph)
    digest, mtime = store.compatibility_export_state()
    assert digest and mtime == export.stat().st_mtime_ns
    assert store.search('"target') == []
    assert store.search("target", limit=999)[0]["name"] == "target"
    assert store.content_for_path("unknown.py") is None
    assert store.analysis_shards(generation=999) == {}

    session = store.start_runtime_session(
        provider="fixture",
        source_fingerprint="source",
        build_fingerprint="build",
        executable_hash="exe",
        session_id="runtime",
    )
    assert session == "runtime"
    # Rows without both endpoints are dropped and must not inflate the count.
    assert (
        store.add_runtime_observations(
            session,
            [
                {"source": "a", "target": "b", "count": 0},
                {"source": "", "target": "ignored"},
            ],
        )
        == 1
    )
    store.end_runtime_session(session)
    rows = store.runtime_observations(
        source_fingerprint="other",
        include_stale=True,
        limit=200_000,
    )
    assert rows[0]["fingerprint_matches"] is False
