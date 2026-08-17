from __future__ import annotations

import json
from pathlib import Path

import pytest

from devcouncil.devmap_client import DEFAULT_DB_PATH, DevMapClient, DevMapClientError


class _FakeSocket:
    def __init__(self, response: bytes) -> None:
        self._response = response
        self.sent = b""

    def settimeout(self, _timeout: float) -> None:
        pass

    def connect(self, _path: str) -> None:
        pass

    def sendall(self, payload: bytes) -> None:
        self.sent += payload

    def recv(self, _size: int) -> bytes:
        response, self._response = self._response, b""
        return response

    def close(self) -> None:
        pass


def test_socket_protocol_unwraps_versioned_result(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    endpoint = tmp_path / "devmap.sock"
    endpoint.touch()
    fake = _FakeSocket(
        json.dumps(
            {
                "ok": True,
                "protocol_version": 1,
                "result": {
                    "generation_id": 7,
                    "pending_count": 0,
                    "node_count": 3,
                    "edge_count": 2,
                    "is_fresh": True,
                },
            }
        ).encode()
        + b"\n"
    )
    monkeypatch.setattr("devcouncil.devmap_client.socket.socket", lambda *_a, **_k: fake)
    client = DevMapClient(tmp_path, socket_path=str(endpoint))

    status = client.status()

    assert status.generation_id == 7
    assert json.loads(fake.sent)["version"] == 1


@pytest.mark.parametrize(
    "response",
    [
        b"not-json\n",
        b'{"ok":false,"protocol_version":1,"error":{"code":"broken","message":"nope"}}\n',
        b'{"ok":true,"protocol_version":99,"result":{}}\n',
        b'{"ok":true,"protocol_version":1,"result":[]}\n',
    ],
)
def test_socket_protocol_never_silently_falls_back_on_bad_response(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    response: bytes,
) -> None:
    endpoint = tmp_path / "devmap.sock"
    endpoint.touch()
    fake = _FakeSocket(response)
    monkeypatch.setattr("devcouncil.devmap_client.socket.socket", lambda *_a, **_k: fake)
    client = DevMapClient(tmp_path, socket_path=str(endpoint))
    monkeypatch.setattr(
        client,
        "_run_cli_command",
        lambda *_a, **_k: pytest.fail("malformed daemon response must not fall back"),
    )

    with pytest.raises(DevMapClientError):
        client.status()


def test_search_prefers_daemon_transport(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(
        client,
        "_send_socket_request",
        lambda payload: {
            "shown": 1,
            "hidden": 0,
            "total": 1,
            "truncated": False,
            "tokens_used": 20,
            "items": [{"symbol_name": payload["query"]}],
        },
    )
    monkeypatch.setattr(
        client,
        "_run_cli_command",
        lambda *_a, **_k: pytest.fail("healthy daemon must be preferred"),
    )

    response = client.search("needle")

    assert response.shown == 1
    assert response.items[0]["symbol_name"] == "needle"


def test_budgeted_response_rejects_false_non_truncation(tmp_path: Path) -> None:
    client = DevMapClient(tmp_path)
    with pytest.raises(DevMapClientError, match="truncation invariant"):
        client._budgeted(
            {
                "shown": 0,
                "hidden": 1,
                "total": 1,
                "truncated": False,
                "tokens_used": 0,
                "items": [],
            },
            2_000,
        )


def test_scoped_trace_forwards_destination_to_ipc_and_cli(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DevMapClient(tmp_path)
    captured: dict[str, object] = {}

    def request(payload: dict[str, object], cli_args: list[str], **_kwargs: object):
        captured["payload"] = payload
        captured["cli_args"] = cli_args
        return {
            "shown": 0,
            "hidden": 0,
            "total": 0,
            "truncated": False,
            "tokens_used": 0,
            "items": [],
        }

    monkeypatch.setattr(client, "_request", request)

    client.trace("caller", depth=7, to_symbol="destination")

    assert captured["payload"] == {
        "cmd": "trace",
        "from": "caller",
        "to": "destination",
        "budget": 2000,
        "depth": 7,
    }
    assert captured["cli_args"] == [
        "trace",
        "caller",
        "destination",
        "--budget",
        "2000",
        "--depth",
        "7",
    ]


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("shown", True),
        ("hidden", 1.5),
        ("total", "1"),
        ("tokens_used", False),
        ("truncated", "false"),
    ],
)
def test_budgeted_response_rejects_coerced_scalar_types(
    tmp_path: Path, field: str, value: object
) -> None:
    client = DevMapClient(tmp_path)
    payload: dict[str, object] = {
        "shown": 0,
        "hidden": 0,
        "total": 0,
        "truncated": False,
        "tokens_used": 0,
        "items": [],
    }
    payload[field] = value
    with pytest.raises(DevMapClientError, match="invalid|must be"):
        client._budgeted(payload, 2_000)


def test_status_rejects_coerced_scalar_types(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(
        client,
        "_request",
        lambda *_args, **_kwargs: {
            "generation_id": 1,
            "pending_count": True,
            "node_count": "3",
            "edge_count": 2,
            "is_fresh": "false",
        },
    )
    with pytest.raises(DevMapClientError, match="invalid|must be"):
        client.status()


@pytest.mark.parametrize(
    "operation",
    [
        lambda client: client.search("x", limit=-1),
        lambda client: client.search("x", limit=100_001),
        lambda client: client.impact("x", depth=-1),
        lambda client: client.trace("x", depth=65),
        lambda client: client.search("x" * 4097),
    ],
)
def test_client_rejects_unbounded_work_before_transport(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation
) -> None:
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(
        client,
        "_request",
        lambda *_args, **_kwargs: pytest.fail("invalid work must not reach transport"),
    )
    with pytest.raises(DevMapClientError, match="budget|depth|query"):
        operation(client)


def test_start_daemon_reaps_child_when_readiness_times_out(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Derived from the client's own default rather than repeated here: this
    # fixture hardcoded the path and silently stopped exercising the reap when
    # the default moved off the Python index (SC23).
    db = tmp_path / DEFAULT_DB_PATH
    db.parent.mkdir(parents=True, exist_ok=True)
    db.write_bytes(b"sqlite")

    class Process:
        pid = 17

        def __init__(self) -> None:
            self.terminated = False
            self.waited = False

        def poll(self):
            return None

        def terminate(self) -> None:
            self.terminated = True

        def wait(self, timeout: float | None = None) -> int:
            self.waited = True
            return 0

        def kill(self) -> None:
            pytest.fail("cooperative child should not need SIGKILL")

    process = Process()
    times = iter([0.0, 4.0])
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(client, "_find_devmap_binary", lambda: "devmap")
    monkeypatch.setattr(
        "devcouncil.devmap_client.subprocess.Popen", lambda *_args, **_kwargs: process
    )
    monkeypatch.setattr("devcouncil.devmap_client.time.monotonic", lambda: next(times))

    assert client._start_daemon() is False
    assert process.terminated
    assert process.waited
    assert client._daemon_process is None


def test_manifest_fails_loudly_when_cli_does_not_create_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(client, "_run_cli_command", lambda *_args, **_kwargs: {})

    with pytest.raises(DevMapClientError, match="manifest"):
        client.manifest(tmp_path / "missing.json")


def test_cli_rejects_non_object_json(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DevMapClient(tmp_path)

    class Result:
        returncode = 0
        stdout = "[]"
        stderr = ""

    monkeypatch.setattr(
        "devcouncil.devmap_client.subprocess.run", lambda *_args, **_kwargs: Result()
    )
    with pytest.raises(DevMapClientError, match="JSON object"):
        client._run_cli_command(["status"])


def test_repo_map_rejects_non_object_json(tmp_path: Path) -> None:
    path = tmp_path / ".devcouncil" / "repo_map.json"
    path.parent.mkdir(parents=True)
    path.write_text("[]", encoding="utf-8")
    client = DevMapClient(tmp_path)

    with pytest.raises(DevMapClientError, match="repo map.*JSON object"):
        client.read_repo_map()


def test_try_connect_returns_none_when_status_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from devcouncil.devmap_client import try_connect

    monkeypatch.setattr(
        "devcouncil.devmap_client.DevMapClient.status",
        lambda self: (_ for _ in ()).throw(DevMapClientError("nope")),
    )
    assert try_connect(tmp_path) is None


def test_try_connect_returns_client_when_status_ok(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from devcouncil.devmap_client import DevMapStatus, try_connect

    monkeypatch.setattr(
        "devcouncil.devmap_client.DevMapClient.status",
        lambda self: DevMapStatus(
            generation_id=1,
            pending_count=0,
            node_count=2,
            edge_count=3,
            is_fresh=True,
        ),
    )
    client = try_connect(tmp_path)
    assert client is not None
    assert client.root_dir == tmp_path.resolve()


def test_resolution_unavailable_reason_tri_state() -> None:
    from devcouncil.devmap_client import resolution_unavailable_reason

    assert resolution_unavailable_reason("Available") is None
    assert resolution_unavailable_reason(None) is None
    assert resolution_unavailable_reason({"Unavailable": {"reason": "missing"}}) == "missing"
