"""Transport-level tests for the live LSP client.

These drive the real :class:`LspClient` JSON-RPC framing / read-loop / request
plumbing against an in-process fake language server built on an ``os.pipe`` — no
real language server is ever spawned. This exercises ``start``/``shutdown``,
``_request``/``_notify``/``_write``, ``_read_loop``, ``references`` and
``_ensure_open`` without touching the network or the filesystem beyond ``tmp_path``.
"""

from __future__ import annotations

import json
import os
import subprocess
import threading
from pathlib import Path
from typing import Any, Callable

import pytest

from devcouncil.indexing import lsp_client as lc
from devcouncil.indexing.lsp_client import (
    LspClient,
    LspLocation,
    _iter_public_symbols,
    _parse_locations,
    _path_to_uri,
    _symbol_position,
    _uri_to_rel,
    first_available_command,
    language_for_path,
    lsp_refs_enabled,
)


class _FakeServer:
    """An in-process JSON-RPC server bolted onto a pipe.

    ``stdin`` (what the client writes to) is this object itself; each fully-framed
    message is decoded and handed to ``responder``. Any returned dict is framed
    back onto the ``stdout`` pipe the client's read loop consumes.
    """

    def __init__(self, responder: Callable[[dict], dict | None]) -> None:
        r_fd, w_fd = os.pipe()
        self._stdout_r = os.fdopen(r_fd, "rb", buffering=0)
        self._stdout_w = os.fdopen(w_fd, "wb", buffering=0)
        self.stdout = self._stdout_r
        self.stdin = self
        self._responder = responder
        self._buf = b""
        self._returncode: int | None = None
        self.received: list[dict] = []
        self._lock = threading.Lock()

    # --- stdin (client -> server) --------------------------------------
    def write(self, data: bytes) -> None:
        self._buf += data
        self._drain()

    def flush(self) -> None:  # pragma: no cover - trivial
        pass

    def _drain(self) -> None:
        while b"\r\n\r\n" in self._buf:
            header, _, rest = self._buf.partition(b"\r\n\r\n")
            length = None
            for line in header.split(b"\r\n"):
                if line.lower().startswith(b"content-length:"):
                    length = int(line.split(b":", 1)[1].strip())
            if length is None or len(rest) < length:
                break
            body, self._buf = rest[:length], rest[length:]
            msg = json.loads(body.decode("utf-8"))
            self.received.append(msg)
            resp = self._responder(msg)
            if resp is not None:
                self._send(resp)

    def _send(self, obj: dict) -> None:
        raw = json.dumps(obj).encode("utf-8")
        with self._lock:
            if self._returncode is not None:
                return
            self._stdout_w.write(f"Content-Length: {len(raw)}\r\n\r\n".encode("ascii") + raw)
            self._stdout_w.flush()

    # --- process protocol ----------------------------------------------
    def poll(self) -> int | None:
        return self._returncode

    def wait(self, timeout: float | None = None) -> int:
        return self._returncode or 0

    def kill(self) -> None:
        self.terminate()

    def terminate(self) -> None:  # pragma: no cover - not always hit
        with self._lock:
            self._returncode = 0
            try:
                self._stdout_w.close()
            except OSError:
                pass


def _install_server(monkeypatch, responder: Callable[[dict], dict | None]) -> dict[str, Any]:
    holder: dict[str, Any] = {}

    def fake_popen(command, **kwargs):
        server = _FakeServer(responder)
        holder["server"] = server
        return server

    monkeypatch.setattr(lc.subprocess, "Popen", fake_popen)
    return holder


def _default_responder(refs: dict[tuple[int, int], list[dict]] | None = None):
    refs = refs or {}

    def responder(msg: dict) -> dict | None:
        method = msg.get("method")
        if "id" not in msg:  # notification
            return None
        if method == "initialize":
            return {"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}}
        if method == "shutdown":
            return {"jsonrpc": "2.0", "id": msg["id"], "result": None}
        if method == "textDocument/references":
            pos = msg["params"]["position"]
            key = (pos["line"], pos["character"])
            return {"jsonrpc": "2.0", "id": msg["id"], "result": refs.get(key, [])}
        return {"jsonrpc": "2.0", "id": msg["id"], "result": None}

    return responder


# ----------------------------------------------------------------------
# start / initialize / shutdown lifecycle
# ----------------------------------------------------------------------


def test_start_initialize_and_shutdown(tmp_path, monkeypatch):
    holder = _install_server(monkeypatch, _default_responder())
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0)
    assert client.start() is True
    assert client._alive is True
    # initialize + initialized notification both sent
    server = holder["server"]
    methods = [m.get("method") for m in server.received]
    assert methods[0] == "initialize"
    assert "initialized" in methods
    # idempotent start
    assert client.start() is True
    client.shutdown()
    assert client._alive is False
    # second shutdown is a harmless no-op
    client.shutdown()


def test_start_returns_false_on_spawn_oserror(tmp_path, monkeypatch):
    def boom(command, **kwargs):
        raise OSError("no such file")

    monkeypatch.setattr(lc.subprocess, "Popen", boom)
    client = LspClient(tmp_path, "python", ["missing-ls"])
    assert client.start() is False
    assert client._proc is None
    assert client._alive is False


def test_context_manager_raises_when_start_fails(tmp_path, monkeypatch):
    def boom(command, **kwargs):
        raise OSError("nope")

    monkeypatch.setattr(lc.subprocess, "Popen", boom)
    with pytest.raises(RuntimeError):
        with LspClient(tmp_path, "python", ["missing-ls"]):
            pass


def test_context_manager_enter_exit(tmp_path, monkeypatch):
    _install_server(monkeypatch, _default_responder())
    with LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0) as client:
        assert client._alive is True
    assert client._alive is False


# ----------------------------------------------------------------------
# references round-trip through _ensure_open + _request
# ----------------------------------------------------------------------


def test_references_roundtrip_parses_locations(tmp_path, monkeypatch):
    (tmp_path / "user.py").write_text("x = 1\n", encoding="utf-8")
    other_uri = _path_to_uri(tmp_path, "user.py")
    refs = {
        (0, 4): [
            {"uri": other_uri, "range": {"start": {"line": 2, "character": 0}}},
        ]
    }
    _install_server(monkeypatch, _default_responder(refs))
    (tmp_path / "mod.py").write_text("def helper():\n    return 1\n", encoding="utf-8")
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0)
    assert client.start()
    locs = client.references("mod.py", 0, 4)
    assert locs == [LspLocation(path="user.py", line=3, character=0)]
    client.shutdown()


def test_references_returns_none_when_not_started(tmp_path):
    client = LspClient(tmp_path, "python", ["fake-ls"])
    # _ensure_open returns False when not alive
    assert client.references("mod.py", 0, 0) is None


def test_references_returns_none_when_file_missing(tmp_path, monkeypatch):
    _install_server(monkeypatch, _default_responder())
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0)
    assert client.start()
    assert client.references("does_not_exist.py", 0, 0) is None
    client.shutdown()


def test_request_times_out_when_server_silent(tmp_path, monkeypatch):
    def silent(msg: dict) -> dict | None:
        method = msg.get("method")
        if method == "initialize":
            return {"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}}
        return None  # never answer references

    (tmp_path / "mod.py").write_text("def f():\n    return 1\n", encoding="utf-8")
    _install_server(monkeypatch, silent)
    client = LspClient(tmp_path, "python", ["fake-ls"], request_timeout=0.2, init_timeout=3.0)
    assert client.start()
    # request returns None on timeout; client still alive -> empty locations
    assert client.references("mod.py", 0, 4) == []
    client.shutdown()


def test_request_returns_none_on_error_response(tmp_path, monkeypatch):
    def erroring(msg: dict) -> dict | None:
        method = msg.get("method")
        if method == "initialize":
            return {"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}}
        if "id" in msg:
            return {"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -1, "message": "boom"}}
        return None

    (tmp_path / "mod.py").write_text("def f():\n    return 1\n", encoding="utf-8")
    _install_server(monkeypatch, erroring)
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0)
    assert client.start()
    # error -> _request None, still alive -> _parse_locations(None) -> []
    assert client.references("mod.py", 0, 4) == []
    client.shutdown()


def test_ensure_open_only_opens_once(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def f():\n    return 1\n", encoding="utf-8")
    holder = _install_server(monkeypatch, _default_responder())
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0)
    assert client.start()
    client.references("mod.py", 0, 4)
    client.references("mod.py", 0, 4)
    open_notes = [m for m in holder["server"].received if m.get("method") == "textDocument/didOpen"]
    assert len(open_notes) == 1
    client.shutdown()


# ----------------------------------------------------------------------
# pure helper functions
# ----------------------------------------------------------------------


def test_uri_to_rel_non_file_scheme_returns_none(tmp_path):
    assert _uri_to_rel(tmp_path, "http://example.com/x.py") is None


def test_uri_to_rel_outside_root_returns_none(tmp_path):
    outside = Path("/definitely/not/under") / "x.py"
    assert _uri_to_rel(tmp_path, outside.as_uri()) is None


def test_parse_locations_handles_shapes(tmp_path):
    (tmp_path / "a.py").write_text("x=1\n", encoding="utf-8")
    uri = _path_to_uri(tmp_path, "a.py")
    # None -> []
    assert _parse_locations(tmp_path, None) == []
    # LocationLink shape via targetUri / targetSelectionRange
    link = {"targetUri": uri, "targetSelectionRange": {"start": {"line": 0, "character": 2}}}
    out = _parse_locations(tmp_path, [link])
    assert out == [LspLocation(path="a.py", line=1, character=2)]
    # non-dict + missing range items skipped
    assert _parse_locations(tmp_path, ["nope", {"uri": uri}]) == []


def test_symbol_position_variants():
    src = "def helper():\n    return helper()\n"
    assert _symbol_position(src, 1, "helper") == (0, 4)
    # dotted fallback: full "other.helper" absent, last segment "helper" found at col 11
    assert _symbol_position(src, 2, "other.helper") == (1, 11)
    # out of range
    assert _symbol_position(src, 99, "helper") is None
    # not found (no dot -> no fallback)
    assert _symbol_position("nothing here\n", 1, "missing") is None


def test_iter_public_symbols_python_skips_private():
    src = "def public():\n    pass\n\ndef _private():\n    pass\n\nclass Thing:\n    pass\n"
    syms = list(_iter_public_symbols("mod.py", src))
    names = {n for _, n in syms}
    assert "public" in names
    assert "Thing" in names
    assert "_private" not in names


def test_iter_public_symbols_syntax_error_yields_nothing():
    assert list(_iter_public_symbols("mod.py", "def (:\n")) == []


def test_iter_public_symbols_js_exports():
    src = "export function alpha() {}\nexport const _hidden = 1\nexport class Beta {}\n"
    syms = list(_iter_public_symbols("mod.ts", src))
    names = {n for _, n in syms}
    assert "alpha" in names
    assert "Beta" in names
    assert "_hidden" not in names


def test_language_for_path():
    assert language_for_path("a/b.py") == "python"
    assert language_for_path("x.ts") == "typescript"
    assert language_for_path("readme.md") is None


def test_first_available_command(monkeypatch):
    import shutil

    monkeypatch.setattr(shutil, "which", lambda cmd: "/usr/bin/" + cmd)
    cmd = first_available_command("python")
    assert cmd is not None and isinstance(cmd, list)
    monkeypatch.setattr(shutil, "which", lambda cmd: None)
    assert first_available_command("python") is None


def test_lsp_refs_enabled_defaults_false_on_error(tmp_path, monkeypatch):
    def boom(root):
        raise RuntimeError("no config")

    monkeypatch.setattr("devcouncil.app.config.load_config", boom)
    assert lsp_refs_enabled(tmp_path) is False


def test_parse_locations_non_dict_start_and_outside_root(tmp_path):
    (tmp_path / "a.py").write_text("x=1\n", encoding="utf-8")
    uri = _path_to_uri(tmp_path, "a.py")
    # range.start not a dict → skipped.
    bad_start = {"uri": uri, "range": {"start": "nope"}}
    assert _parse_locations(tmp_path, [bad_start]) == []
    # uri resolves outside the project root → skipped (rel is None).
    outside_uri = (Path("/definitely/elsewhere") / "z.py").as_uri()
    outside = {"uri": outside_uri, "range": {"start": {"line": 0, "character": 0}}}
    assert _parse_locations(tmp_path, [outside]) == []


# ----------------------------------------------------------------------
# start-failure and request-write-error transport branches
# ----------------------------------------------------------------------


def test_start_returns_false_when_init_unanswered_and_proc_dead(tmp_path, monkeypatch):
    holder: dict[str, Any] = {}

    def responder(msg: dict) -> dict | None:
        # Never answer initialize; mark the process dead so start() bails.
        server = holder.get("server")
        if server is not None:
            server._returncode = 1
        return None

    def fake_popen(command, **kwargs):
        server = _FakeServer(responder)
        holder["server"] = server
        return server

    monkeypatch.setattr(lc.subprocess, "Popen", fake_popen)
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=0.3)
    assert client.start() is False
    assert client._alive is False


def test_start_returns_false_when_initialized_notify_raises(tmp_path, monkeypatch):
    _install_server(monkeypatch, _default_responder())
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0)
    orig_notify = client._notify

    def flaky(method, params):
        if method == "initialized":
            raise RuntimeError("boom")
        return orig_notify(method, params)

    monkeypatch.setattr(client, "_notify", flaky)
    assert client.start() is False
    assert client._alive is False


def test_shutdown_kills_when_wait_times_out(tmp_path, monkeypatch):
    holder = _install_server(monkeypatch, _default_responder())
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0)
    assert client.start()
    server = holder["server"]
    killed = {"n": 0}

    def wait(timeout=None):
        raise subprocess.TimeoutExpired(cmd="fake-ls", timeout=timeout)

    def kill():
        killed["n"] += 1
        server._returncode = 0

    monkeypatch.setattr(server, "wait", wait)
    monkeypatch.setattr(server, "kill", kill)
    client.shutdown()
    assert killed["n"] == 1


def test_references_returns_none_when_request_kills_session(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def f():\n    return 1\n", encoding="utf-8")
    _install_server(monkeypatch, _default_responder())
    client = LspClient(tmp_path, "python", ["fake-ls"], init_timeout=3.0)
    assert client.start()

    # Simulate the request path returning None while the session is dead.
    monkeypatch.setattr(client, "_request", lambda *a, **k: None)
    client._alive = False
    assert client.references("mod.py", 0, 4) is None


# ----------------------------------------------------------------------
# LspSessionPool + confirm_unreferenced + dependents_of_file
# ----------------------------------------------------------------------


class _FakeClient:
    def __init__(self, refs_map=None, start_ok=True):
        self._alive = False
        self._refs_map = refs_map or {}
        self._start_ok = start_ok
        self.shutdown_called = False

    def start(self):
        self._alive = self._start_ok
        return self._start_ok

    def references(self, rel, line, character, *, include_declaration=False):
        return self._refs_map.get((rel, line, character))

    def shutdown(self):
        self.shutdown_called = True
        self._alive = False


def _pool(tmp_path, monkeypatch, factory, *, command=None):
    monkeypatch.setattr(
        lc, "first_available_command", lambda language: command if command is not None else ["fake"]
    )
    return lc.LspSessionPool(tmp_path, client_factory=factory)


def test_pool_client_for_unknown_language(tmp_path, monkeypatch):
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient())
    assert pool.client_for("readme.md") is None


def test_pool_client_for_no_command_marks_failed(tmp_path, monkeypatch):
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(), command=None)
    monkeypatch.setattr(lc, "first_available_command", lambda language: None)
    assert pool.client_for("mod.py") is None
    # cached in _failed → second lookup short-circuits.
    assert pool.client_for("mod.py") is None
    assert "python" in pool._failed


def test_pool_client_for_start_failure_marks_failed(tmp_path, monkeypatch):
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(start_ok=False))
    assert pool.client_for("mod.py") is None
    assert "python" in pool._failed


def test_pool_client_for_caches_and_reuses(tmp_path, monkeypatch):
    created = []

    def factory(root, language, command):
        c = _FakeClient()
        created.append(c)
        return c

    pool = _pool(tmp_path, monkeypatch, factory)
    first = pool.client_for("mod.py")
    assert first is not None
    assert pool.client_for("other.py") is first  # reused cached alive client
    assert len(created) == 1
    # A dead cached client returns None on next lookup.
    first._alive = False
    assert pool.client_for("mod.py") is None


def test_pool_close_shuts_down_clients(tmp_path, monkeypatch):
    client = _FakeClient()
    pool = _pool(tmp_path, monkeypatch, lambda *a: client)
    assert pool.client_for("mod.py") is client
    with pool:
        pass
    assert client.shutdown_called is True


def test_confirm_unreferenced_no_client(tmp_path, monkeypatch):
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(start_ok=False))
    assert pool.confirm_unreferenced("mod.py", 1, "foo") is None


def test_confirm_unreferenced_read_error(tmp_path, monkeypatch):
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient())
    # file does not exist → read raises → None
    assert pool.confirm_unreferenced("missing.py", 1, "foo") is None


def test_confirm_unreferenced_position_missing(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def foo():\n    pass\n", encoding="utf-8")
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient())
    # name not present on the given line → _symbol_position None → None
    assert pool.confirm_unreferenced("mod.py", 2, "absent_name") is None


def test_confirm_unreferenced_locs_none(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def foo():\n    pass\n", encoding="utf-8")
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(refs_map={}))
    # references returns None (unknown key) → None
    assert pool.confirm_unreferenced("mod.py", 1, "foo") is None


def test_confirm_unreferenced_external_ref_returns_false(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def foo():\n    pass\n", encoding="utf-8")
    refs = {("mod.py", 0, 4): [LspLocation(path="other.py", line=5, character=0)]}
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(refs_map=refs))
    assert pool.confirm_unreferenced("mod.py", 1, "foo") is False


def test_confirm_unreferenced_same_file_other_line_returns_false(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def foo():\n    return foo\n", encoding="utf-8")
    refs = {("mod.py", 0, 4): [LspLocation(path="mod.py", line=2, character=11)]}
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(refs_map=refs))
    assert pool.confirm_unreferenced("mod.py", 1, "foo") is False


def test_confirm_unreferenced_confirmed_true(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def foo():\n    pass\n", encoding="utf-8")
    # only self-reference on the defining line → confirmed dead.
    refs = {("mod.py", 0, 4): [LspLocation(path="mod.py", line=1, character=4)]}
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(refs_map=refs))
    assert pool.confirm_unreferenced("mod.py", 1, "foo") is True


def test_dependents_of_file_variants(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text(
        "def alpha():\n    pass\n\ndef beta():\n    pass\n", encoding="utf-8"
    )
    # alpha referenced from other.py; beta referenced nowhere.
    refs = {
        ("mod.py", 0, 4): [
            LspLocation(path="other.py", line=1, character=0),
            LspLocation(path="mod.py", line=1, character=4),
        ],
        ("mod.py", 3, 4): [],
    }
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(refs_map=refs))
    deps = pool.dependents_of_file("mod.py")
    assert deps == ["other.py"]


def test_dependents_of_file_no_client(tmp_path, monkeypatch):
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(start_ok=False))
    assert pool.dependents_of_file("mod.py") is None


def test_dependents_of_file_read_error(tmp_path, monkeypatch):
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient())
    assert pool.dependents_of_file("missing.py") is None


def test_dependents_of_file_no_symbols_returns_empty(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("x = 1\n", encoding="utf-8")
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient())
    assert pool.dependents_of_file("mod.py") == []


def test_dependents_of_file_all_queries_fail_returns_none(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def alpha():\n    pass\n", encoding="utf-8")
    # references returns None (missing key) for every symbol → any_ok False → None
    pool = _pool(tmp_path, monkeypatch, lambda *a: _FakeClient(refs_map={}))
    assert pool.dependents_of_file("mod.py") is None


# ----------------------------------------------------------------------
# filter_dead_symbols_with_lsp
# ----------------------------------------------------------------------


def test_filter_dead_symbols_empty_returns_input():
    assert lc.filter_dead_symbols_with_lsp(Path("/x"), []) == []


def test_filter_dead_symbols_keeps_malformed_entries(tmp_path):
    class _Pool:
        def confirm_unreferenced(self, path, line, name):  # pragma: no cover - unused
            raise AssertionError("should not be called for malformed entries")

        def close(self):
            pass

    bad = ["no-space-entry", "path.py:notanumber name"]
    kept = lc.filter_dead_symbols_with_lsp(tmp_path, bad, pool=_Pool())
    assert kept == bad


def test_filter_dead_symbols_drops_confirmed_false(tmp_path):
    class _Pool:
        def __init__(self):
            self.closed = False

        def confirm_unreferenced(self, path, line, name):
            return False if name == "gone" else None

        def close(self):
            self.closed = True

    pool = _Pool()
    entries = ["mod.py:10 gone", "mod.py:20 kept"]
    # own_pool False path: caller-provided pool is not closed here.
    kept = lc.filter_dead_symbols_with_lsp(tmp_path, entries, pool=pool)
    assert kept == ["mod.py:20 kept"]
    assert pool.closed is False


def test_filter_dead_symbols_owns_and_closes_pool(tmp_path, monkeypatch):
    closed = {"n": 0}

    class _Pool:
        def __init__(self, root):
            pass

        def confirm_unreferenced(self, path, line, name):
            return None

        def close(self):
            closed["n"] += 1

    monkeypatch.setattr(lc, "LspSessionPool", _Pool)
    kept = lc.filter_dead_symbols_with_lsp(tmp_path, ["mod.py:1 foo"])
    assert kept == ["mod.py:1 foo"]
    assert closed["n"] == 1
