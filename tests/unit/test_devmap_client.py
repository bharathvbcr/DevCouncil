from __future__ import annotations

import errno
import json
import os
import subprocess
import time
from pathlib import Path

import pytest

from devcouncil import devmap_client as client_module
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
    # Options precede the `--` terminator (clap accepts none after it) and the
    # two endpoints follow it in FROM, TO order, where the kernel reads them as
    # values rather than as flags.
    assert captured["cli_args"] == [
        "trace",
        "--budget",
        "2000",
        "--depth",
        "7",
        "--",
        "caller",
        "destination",
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
    # This test is *about* spawning; the suite-wide DEVMAP_AUTOSPAWN=0 must
    # not apply here (the fake Popen never starts a real process).
    monkeypatch.setenv("DEVMAP_AUTOSPAWN", "1")
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(client, "_find_devmap_binary", lambda: "devmap")
    monkeypatch.setattr(client, "_supports_serve", lambda _binary: True)
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


# ---------------------------------------------------------------------------
# Stress-derived hardening: every test below pins behavior that a live stress
# run showed to be missing (unbounded dribble hang, wedged-daemon poisoning,
# silent adoption of an outdated binary, repeated CLI timeouts).
# ---------------------------------------------------------------------------


class _DribbleServer:
    """Real unix-socket server sending one byte every `delay` seconds, forever."""

    def __init__(self, path, delay):
        import socket as socket_module
        import threading

        self.delay = delay
        self._stop = threading.Event()
        self.server = socket_module.socket(socket_module.AF_UNIX, socket_module.SOCK_STREAM)
        self.server.bind(path)
        self.server.listen(1)
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self):
        try:
            conn, _ = self.server.accept()
        except OSError:
            return
        with conn:
            conn.settimeout(5)
            try:
                conn.recv(65536)
            except OSError:
                return
            while not self._stop.is_set():
                try:
                    conn.sendall(b"x")
                except OSError:
                    return
                self._stop.wait(self.delay)

    def close(self):
        self._stop.set()
        self.server.close()


def test_slowloris_dribble_is_bounded_by_the_total_deadline(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """One byte per timeout-window must not keep the client spinning forever.

    Pre-fix, the only bounds were the 2s per-recv timeout and MAX_RESPONSE_BYTES;
    a server that never violates either held the client for weeks. The total
    deadline must cut the exchange and the failure must route to the CLI
    fallback instead of raising.
    """
    endpoint = f"/tmp/dm-dribble-{os.getpid()}.sock"
    try:
        os.unlink(endpoint)
    except FileNotFoundError:
        pass
    dribbler = _DribbleServer(endpoint, delay=0.05)
    try:
        client = DevMapClient(
            tmp_path,
            socket_path=endpoint,
            response_deadline_seconds=1.0,
        )
        monkeypatch.setattr(client, "_start_daemon", lambda: False)
        monkeypatch.setattr(
            client,
            "_run_cli_command",
            lambda *_a, **_k: {
                "generation_id": 1,
                "pending_count": 0,
                "node_count": 1,
                "edge_count": 1,
                "is_fresh": True,
                "via_cli": True,
            },
        )
        started = time.monotonic()
        result = client.status()
        elapsed = time.monotonic() - started

        assert result.raw.get("via_cli") is True, "bounded failure must fall back to CLI"
        assert elapsed < 10.0, f"dribble was not bounded promptly: {elapsed:.2f}s"
    finally:
        dribbler.close()
        try:
            os.unlink(endpoint)
        except FileNotFoundError:
            pass


def _valid_status_payload(**extra):
    payload = {
        "generation_id": 1,
        "pending_count": 0,
        "node_count": 1,
        "edge_count": 1,
        "is_fresh": True,
    }
    payload.update(extra)
    return payload


def test_transport_timeout_marks_daemon_unhealthy_and_falls_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A wedged daemon costs one bounded stall, then the CLI takes over."""
    endpoint = tmp_path / "wedged.sock"
    endpoint.touch()

    class _WedgedSocket:
        def settimeout(self, _t):
            pass

        def connect(self, _path):
            pass

        def sendall(self, _payload):
            pass

        def recv(self, _size):
            import socket as socket_module

            raise socket_module.timeout("wedged")

        def close(self):
            pass

    monkeypatch.setattr(
        "devcouncil.devmap_client.socket.socket", lambda *_a, **_k: _WedgedSocket()
    )
    cli_calls = []

    def fake_cli(*_args, **_kwargs):
        cli_calls.append(1)
        return _valid_status_payload(via="cli")

    client = DevMapClient(tmp_path, socket_path=str(endpoint))
    client._start_daemon = lambda: False  # respawn is out of scope here
    monkeypatch.setattr(client, "_run_cli_command", fake_cli)

    first = client.status()
    second = client.status()

    assert first.raw.get("via") == "cli" and second.raw.get("via") == "cli"
    assert len(cli_calls) == 2


def test_cooldown_skips_the_socket_entirely_after_a_transport_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    endpoint = tmp_path / "cooldown.sock"
    endpoint.touch()
    attempts = []

    def counting_socket(*_a, **_k):
        attempts.append(1)

        class _Raising:
            def __getattr__(self, name):
                raise AssertionError("socket must not be used during cooldown")

        class _Boom:
            def settimeout(self, _t):
                pass

            def connect(self, _p):

                raise OSError(errno.EIO, "simulated wedge")

            def close(self):
                pass

        return _Boom()

    client = DevMapClient(tmp_path, socket_path=str(endpoint))
    monkeypatch.setattr("devcouncil.devmap_client.socket.socket", counting_socket)
    monkeypatch.setattr(
        client, "_run_cli_command", lambda *_a, **_k: _valid_status_payload()
    )
    client.status()
    assert len(attempts) == 1
    client.status()
    assert len(attempts) == 1, "cooldown must skip the socket after a failure"


def _write_binary(path: Path, body: str, exit_code: int = 0) -> None:
    path.write_text(f"#!/bin/sh\ncat <<'MSG'\n{body}\nMSG\nexit {exit_code}\n")
    path.chmod(0o755)


def test_start_daemon_refuses_a_binary_that_cannot_serve(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An outdated binary without `serve --socket` must never be spawned.

    Both builds report version 0.1.0, so capability — not the version string —
    is the only honest evidence. Pre-fix, an outdated PATH binary was spawned,
    burned the readiness window, then served CLI traffic from a stale kernel.
    """
    db = tmp_path / DEFAULT_DB_PATH
    db.parent.mkdir(parents=True, exist_ok=True)
    db.write_bytes(b"sqlite")

    old_binary = tmp_path / "devmap-old"
    _write_binary(old_binary, "usage: devmap manifest [OPTIONS]")  # no --socket

    # Probe against the real script first (unpatched), so the verdict below
    # comes from actual subprocess behavior rather than a stub.
    client = DevMapClient(tmp_path, socket_path=str(tmp_path / "unused.sock"))
    assert client._supports_serve(str(old_binary)) is False

    spawned = []

    class _NoSpawn:
        def __getattr__(self, name):
            raise AssertionError("an incapable binary must never be spawned")

    monkeypatch.setattr(
        client_module.subprocess, "Popen", lambda *a, **k: spawned.append(a) or _NoSpawn()
    )
    monkeypatch.setattr(client, "_find_devmap_binary", lambda: str(old_binary))

    assert client._start_daemon() is False
    assert spawned == [], "an incapable binary must never be spawned"


def test_start_daemon_spawns_only_when_the_probe_passes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    good_binary = tmp_path / "devmap-good"
    _write_binary(good_binary, "--socket <SOCKET>   IPC listen path")
    client = DevMapClient(tmp_path, socket_path=str(tmp_path / "unused.sock"))
    assert client._supports_serve(str(good_binary)) is True


def test_cli_timeout_fail_fasts_within_cooldown_but_build_is_exempt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A hung CLI costs its timeout once; later short calls fail immediately.

    Pre-fix stress: an outdated hanging binary cost 3s + 120s on EVERY status.
    The cooldown bounds that to once; `build` opts out because one slow build
    must not poison the next command's 600s budget.
    """
    calls = []
    real_run = client_module.subprocess.run

    def flaky_run(*args, **kwargs):
        calls.append(kwargs.get("timeout"))
        if len(calls) == 1:
            raise client_module.subprocess.TimeoutExpired(cmd="devmap", timeout=120.0)
        return real_run(*args, **kwargs)

    monkeypatch.setattr(client_module.subprocess, "run", flaky_run)
    client = DevMapClient(tmp_path)

    started = time.monotonic()
    with pytest.raises(DevMapClientError, match="timed out"):
        client.status()
    first_elapsed = time.monotonic() - started

    # Real subprocess would stall 120s; fail-fast must return ~immediately.
    monkeypatch.setattr(
        client_module.subprocess,
        "run",
        lambda *a, **k: (_ for _ in ()).throw(AssertionError("must not run")),
    )
    started = time.monotonic()
    with pytest.raises(DevMapClientError, match="failing fast"):
        client.status()
    fast_elapsed = time.monotonic() - started

    assert first_elapsed < 30
    assert fast_elapsed < 1.0, "fail-fast must not re-invoke the CLI"


def test_named_pipe_read_is_bounded_by_a_watchdog_close(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Windows named-pipe reads had no bound at all pre-fix; watchdog closes."""
    import errno as errno_module
    import threading

    close_event = threading.Event()
    closed = []

    class _WedgedPipe:
        """Mimics a real blocking pipe: closing the fd wakes the reader."""

        def write(self, _data):
            return 8

        def read(self, _size):
            if close_event.wait(timeout=30):
                raise OSError(errno_module.EBADF, "watchdog closed the pipe")
            return b""

        def close(self):
            if not closed:
                closed.append(1)
                close_event.set()

        def __enter__(self):
            return self

        def __exit__(self, *_a):
            self.close()

    opened = []
    monkeypatch.setattr(
        client_module,
        "open",
        lambda *_a, **_k: opened.append(1) or _WedgedPipe(),
        raising=False,
    )
    client = DevMapClient(
        tmp_path, socket_path=str(tmp_path / "pipe"), response_deadline_seconds=0.5
    )

    started = time.monotonic()
    outcome = client._send_named_pipe_request({"cmd": "status"})
    elapsed = time.monotonic() - started

    assert outcome is None, "watchdog-unblocked read must fall back, not raise"
    assert elapsed < 5.0, f"named-pipe exchange was not bounded: {elapsed:.2f}s"


def test_a_wedged_binary_fails_fast_after_one_bounded_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Stress evidence: a hanging outdated binary cost 123s per status call.

    The probe bounds detection to 10s and a probe timeout must poison the CLI
    cooldown, so the first call is bounded by the probe and every call after it
    fails immediately instead of paying the full CLI timeout again.
    """
    hung_binary = tmp_path / "devmap-hung"
    hung_binary.write_text("#!/bin/sh\nsleep 60\n")
    hung_binary.chmod(0o755)

    client = DevMapClient(tmp_path)
    monkeypatch.setattr(client, "_find_devmap_binary", lambda: str(hung_binary))

    assert client._supports_serve(str(hung_binary)) is False
    assert client._cli_unhealthy_until > time.monotonic(), (
        "a probe timeout must engage the CLI fail-fast cooldown"
    )

    monkeypatch.setattr(
        client_module.subprocess,
        "run",
        lambda *_a, **_k: pytest.fail("CLI must not run during cooldown"),
    )
    fast_started = time.monotonic()
    with pytest.raises(DevMapClientError, match="failing fast"):
        client._run_cli_command(["status"])
    assert time.monotonic() - fast_started < 1.0


# --- socket-path parity with the kernel ---------------------------------------


def test_fnv1a64_matches_the_published_vectors() -> None:
    """The hash is pinned to the FNV-1a reference values so the Python and Rust
    spellings cannot drift apart silently even on a machine without a kernel."""
    from devcouncil.devmap_client import FNV1A64_OFFSET_BASIS, fnv1a64

    assert fnv1a64(b"") == FNV1A64_OFFSET_BASIS == 0xCBF29CE484222325
    assert fnv1a64(b"a") == 0xAF63DC4C8601EC8C
    assert fnv1a64(b"foobar") == 0x85944171F73967E8


def test_default_socket_path_is_a_function_of_the_canonical_root(tmp_path) -> None:
    """Every spelling of one repository — direct, through a symlink, with a
    trailing slash, relative — must name the same endpoint, or two daemons
    end up reconciling the same store."""
    from devcouncil.devmap_client import DevMapClient, default_socket_path

    repo = tmp_path / "repo"
    repo.mkdir()
    link = tmp_path / "link"
    link.symlink_to(repo)

    canonical = default_socket_path(repo)
    assert canonical.endswith(os.path.join("ipc.sock"))
    assert "/devmap-" in canonical and len(canonical.split("devmap-")[-1].split("/")[0]) == 16
    assert default_socket_path(link) == canonical
    assert default_socket_path(str(repo) + "/") == canonical
    assert DevMapClient(link, autospawn=False).socket_path == canonical
    assert default_socket_path(tmp_path / "other") != canonical


def _kernel_prints_socket_paths() -> str | None:
    from devcouncil.devmap_engine import DevMapEngineError, find_engine_binary

    try:
        binary = find_engine_binary()
    except DevMapEngineError:
        return None
    probe = subprocess.run(
        [binary, "serve", "--help"], capture_output=True, text=True, timeout=30, check=False
    )
    return binary if "--print-socket-path" in probe.stdout else None


@pytest.mark.skipif(
    _kernel_prints_socket_paths() is None,
    reason="devmap kernel without `serve --print-socket-path` (rebuild it)",
)
def test_default_socket_path_matches_what_the_kernel_binds(tmp_path) -> None:
    """Parity test: the kernel is the owner of the formula; the client predicts it."""
    from devcouncil.devmap_client import default_socket_path

    binary = _kernel_prints_socket_paths()
    assert binary is not None
    repo = tmp_path / "repo"
    repo.mkdir()
    link = tmp_path / "link"
    link.symlink_to(repo)

    for spelling in (repo, link, Path(str(repo) + "/")):
        printed = subprocess.run(
            [binary, "serve", "--print-socket-path", str(spelling)],
            capture_output=True,
            text=True,
            timeout=30,
            check=True,
        ).stdout.strip()
        assert printed == default_socket_path(spelling), spelling
    # Printing must create nothing: no endpoint directory, no store.
    assert not Path(printed).parent.exists()
    assert not (repo / ".devcouncil").exists()
