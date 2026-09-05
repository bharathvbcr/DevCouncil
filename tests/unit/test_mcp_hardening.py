"""Boundary tests for the MCP server's schema, path containment, and argv.

Every test here was written against the *unmodified* code and observed to fail
before the corresponding fix landed; each one names the specific escape it
closes rather than asserting a general "is hardened" property.
"""

from __future__ import annotations

import asyncio
import json
import subprocess
from pathlib import Path

import pytest

from devcouncil.devmap_client import DevMapClient
from devcouncil.integrations.mcp import server as mcp_server
from devcouncil.integrations.mcp import util as mcp_util
from devcouncil.integrations.mcp.handlers import codeintel, debug, read, tool_specs


def _payload(result) -> dict:
    return json.loads(result[0].text)


def _repo(root: Path) -> Path:
    """A directory that reads as a project root to ``canonical_project_root``."""
    (root / ".git").mkdir(parents=True, exist_ok=True)
    return root


# --- 1. projectPath containment -------------------------------------------------


def test_absolute_project_path_outside_server_root_is_refused(tmp_path: Path) -> None:
    server_root = _repo(tmp_path / "server")
    outside = _repo(tmp_path / "elsewhere")

    result = asyncio.run(
        codeintel.dispatch("devcouncil_code_status", server_root, {"projectPath": str(outside)})
    )
    payload = _payload(result)
    assert payload["ok"] is False
    assert payload["code"] == "project_path_outside_root"
    assert str(outside.resolve()) not in json.dumps(payload.get("project_root", ""))


def test_tilde_project_path_is_refused(tmp_path: Path) -> None:
    """``~`` expanded to the user's home and escaped the server root entirely."""
    server_root = _repo(tmp_path / "server")

    result = asyncio.run(
        codeintel.dispatch("devcouncil_code_status", server_root, {"projectPath": "~"})
    )
    assert _payload(result)["code"] == "project_path_outside_root"


def test_parent_traversal_project_path_is_refused(tmp_path: Path) -> None:
    server_root = _repo(tmp_path / "server")
    _repo(tmp_path / "elsewhere")

    result = asyncio.run(
        codeintel.dispatch(
            "devcouncil_code_status", server_root, {"projectPath": "../elsewhere"}
        )
    )
    assert _payload(result)["code"] == "project_path_outside_root"


def test_code_sync_cannot_be_pointed_at_a_directory_outside_the_root(tmp_path: Path) -> None:
    """The write case: sync creates ``<root>/.devcouncil`` wherever it is aimed."""
    server_root = _repo(tmp_path / "server")
    outside = _repo(tmp_path / "victim")

    result = asyncio.run(
        codeintel.dispatch("devcouncil_code_sync", server_root, {"projectPath": str(outside)})
    )
    assert _payload(result)["code"] == "project_path_outside_root"
    assert not (outside / ".devcouncil").exists()


def test_debug_dispatch_refuses_a_project_path_outside_the_root(tmp_path: Path) -> None:
    server_root = _repo(tmp_path / "server")
    outside = _repo(tmp_path / "elsewhere")

    result = asyncio.run(
        debug.dispatch(
            "devcouncil_debug_discover",
            server_root,
            {"projectPath": str(outside), "consent": True},
        )
    )
    payload = _payload(result)
    assert payload["ok"] is False
    assert payload["code"] == "project_path_outside_root"
    # Consent is a per-root grant: refusing the path must not have written one.
    assert not (outside / ".devcouncil").exists()


def test_project_path_inside_the_root_still_resolves(tmp_path: Path) -> None:
    """Containment must not break the legitimate nested-path case."""
    server_root = _repo(tmp_path / "server")
    nested = server_root / "packages" / "api"
    nested.mkdir(parents=True)

    result = asyncio.run(
        codeintel.dispatch("devcouncil_code_status", server_root, {"projectPath": str(nested)})
    )
    payload = _payload(result)
    assert payload.get("code") != "project_path_outside_root"
    assert payload["project_root"] == str(server_root.resolve())


def test_nested_repository_inside_the_root_still_resolves(tmp_path: Path) -> None:
    """A submodule/worktree with its own marker inside the root stays reachable."""
    server_root = _repo(tmp_path / "server")
    inner = _repo(server_root / "vendor" / "inner")

    result = asyncio.run(
        codeintel.dispatch("devcouncil_code_status", server_root, {"projectPath": str(inner)})
    )
    payload = _payload(result)
    assert payload.get("code") != "project_path_outside_root"
    assert payload["project_root"] == str(inner.resolve())


def test_symlink_into_the_root_is_allowed_and_out_of_it_is_refused(tmp_path: Path) -> None:
    """Both sides resolve before comparison, so a symlink cannot smuggle either way."""
    server_root = _repo(tmp_path / "server")
    (server_root / "real").mkdir()
    outside = _repo(tmp_path / "elsewhere")

    inward = tmp_path / "link-in"
    inward.symlink_to(server_root / "real", target_is_directory=True)
    outward = server_root / "link-out"
    outward.symlink_to(outside, target_is_directory=True)

    allowed = _payload(
        asyncio.run(
            codeintel.dispatch("devcouncil_code_status", server_root, {"projectPath": str(inward)})
        )
    )
    assert allowed.get("code") != "project_path_outside_root"

    refused = _payload(
        asyncio.run(
            codeintel.dispatch("devcouncil_code_status", server_root, {"projectPath": str(outward)})
        )
    )
    assert refused["code"] == "project_path_outside_root"


@pytest.mark.parametrize("hostile", ["\x00bad", "a" * 4000])
def test_unstattable_project_path_is_refused_not_raised(tmp_path: Path, hostile: str) -> None:
    """A path the OS will not stat leaves containment undecidable — so refuse it.

    An embedded NUL raises ValueError out of `lstat` and an over-long component
    raises OSError; neither may escape the handler as a transport error, and
    neither may be treated as contained.
    """
    server_root = _repo(tmp_path / "server")
    for dispatch, tool in (
        (codeintel.dispatch, "devcouncil_code_status"),
        (debug.dispatch, "devcouncil_debug_discover"),
    ):
        payload = _payload(
            asyncio.run(dispatch(tool, server_root, {"projectPath": hostile}))
        )
        assert payload["ok"] is False
        assert payload["code"] == "invalid_arguments"
        assert payload["argument"] == "projectPath"


def test_no_project_path_keeps_the_servers_own_root(tmp_path: Path) -> None:
    server_root = _repo(tmp_path / "server")
    payload = _payload(
        asyncio.run(codeintel.dispatch("devcouncil_code_status", server_root, {}))
    )
    assert payload["project_root"] == str(server_root.resolve())


# --- 2/3/4. schema completeness -------------------------------------------------


def test_update_task_scope_declares_create_planned_files() -> None:
    spec = {tool.name: tool for tool in tool_specs.all_tools()}["devcouncil_update_task_scope"]
    assert "create_planned_files" in (spec.input_schema.get("properties") or {})


def test_create_planned_files_survives_input_validation() -> None:
    violation = mcp_server._input_schema_violation(
        "devcouncil_update_task_scope",
        {"task_id": "TASK-1", "lease_token": "t", "create_planned_files": ["a.py"]},
    )
    assert violation is None


def test_every_tool_schema_closes_additional_properties() -> None:
    open_schemas = [
        tool.name
        for tool in tool_specs.all_tools()
        if tool.input_schema.get("additionalProperties") is not False
    ]
    assert open_schemas == []


def test_undeclared_field_is_rejected() -> None:
    violation = mcp_server._input_schema_violation(
        "devcouncil_code_search", {"query": "x", "bogus_field": 1}
    )
    assert violation is not None
    assert "bogus_field" in violation


@pytest.mark.parametrize(
    "name,arguments",
    [
        ("devcouncil_code_search", {"query": "x"}),
        ("devcouncil_code_status", {"projectPath": "/tmp"}),
        ("devcouncil_debug_start", {"adapterId": "debugpy"}),
        ("devcouncil_debug_start", {"adapterCommand": ["debugpy"]}),
        ("devcouncil_read_file", {"path": "a.py", "limit": 10}),
    ],
)
def test_declared_arguments_still_validate(name: str, arguments: dict) -> None:
    """Closing the schemas must not reject fields the handlers actually read."""
    assert mcp_server._input_schema_violation(name, arguments) is None


def test_every_numeric_range_that_was_started_is_finished() -> None:
    """A declared ``minimum`` with no ``maximum`` is a bound abandoned halfway.

    Numerics with no declared range at all (DAP thread/frame ids, process exit
    codes) are protocol-opaque and deliberately excluded: there is no defensible
    ceiling for them, and inventing one would reject valid sessions.
    """
    half_open: list[str] = []
    for tool in tool_specs.all_tools():
        for field, spec in (tool.input_schema.get("properties") or {}).items():
            if not isinstance(spec, dict) or spec.get("type") not in {"integer", "number"}:
                continue
            if "minimum" in spec and "maximum" not in spec:
                half_open.append(f"{tool.name}.{field}")
    assert half_open == []


def test_read_file_limit_over_the_maximum_is_rejected() -> None:
    violation = mcp_server._input_schema_violation(
        "devcouncil_read_file", {"path": "a.py", "limit": 10**12}
    )
    assert violation is not None


# --- 5. kernel CLI argument injection -------------------------------------------


def _captured_argv(monkeypatch: pytest.MonkeyPatch, call) -> list[str]:
    import devcouncil.devmap_client as devmap_client

    seen: list[list[str]] = []

    class _Completed:
        returncode = 0
        stdout = json.dumps(
            {"shown": 0, "hidden": 0, "total": 0, "truncated": False, "tokens_used": 0, "items": []}
        )
        stderr = ""

    def _fake_run(cmd, **_kwargs):
        seen.append(list(cmd))
        return _Completed()

    monkeypatch.setattr(devmap_client.subprocess, "run", _fake_run)
    call()
    assert seen, "no CLI invocation was captured"
    return seen[-1]


def _assert_positional_tail(argv: list[str], expected: list[str]) -> None:
    """User values must be the whole tail after ``--``, where clap reads them as values.

    ``--db`` and ``--budget`` also occur legitimately earlier in argv — the
    client's own global and subcommand options — so the invariant is about
    position, not membership: everything the caller supplied sits past the
    terminator, and nothing else does.
    """
    assert "--" in argv, f"no option terminator in {argv}"
    terminator = argv.index("--")
    assert argv[terminator + 1 :] == expected, f"positional tail is {argv[terminator + 1:]} in {argv}"


@pytest.mark.parametrize("hostile", ["--db", "--help", "-b"])
def test_search_query_cannot_be_read_as_a_kernel_flag(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, hostile: str
) -> None:
    client = DevMapClient(tmp_path, autospawn=False)
    argv = _captured_argv(monkeypatch, lambda: client.search(hostile))
    _assert_positional_tail(argv, [hostile])


def test_impact_target_cannot_be_read_as_a_kernel_flag(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DevMapClient(tmp_path, autospawn=False)
    argv = _captured_argv(monkeypatch, lambda: client.impact("--db"))
    _assert_positional_tail(argv, ["--db"])


def test_deps_target_cannot_be_read_as_a_kernel_flag(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DevMapClient(tmp_path, autospawn=False)
    argv = _captured_argv(monkeypatch, lambda: client.deps("--db"))
    _assert_positional_tail(argv, ["--db"])


def test_trace_endpoints_cannot_be_read_as_kernel_flags(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DevMapClient(tmp_path, autospawn=False)
    argv = _captured_argv(monkeypatch, lambda: client.trace("--db", to_symbol="--help"))
    # Order still carries meaning: FROM before TO.
    _assert_positional_tail(argv, ["--db", "--help"])


def test_snapshot_file_cannot_be_read_as_a_kernel_flag(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    client = DevMapClient(tmp_path, autospawn=False)
    argv = _captured_argv(monkeypatch, lambda: client.semantic_snapshots("--budget"))
    _assert_positional_tail(argv, ["--budget"])


def test_option_flags_still_precede_the_terminator(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """clap treats everything after ``--`` as positional, so options must come first."""
    client = DevMapClient(tmp_path, autospawn=False)
    argv = _captured_argv(monkeypatch, lambda: client.search("thing", limit=7))
    terminator = argv.index("--")
    assert "--budget" in argv[:terminator]
    assert argv[terminator + 1 :] == ["thing"]


# --- 6. tools/list cache hint ---------------------------------------------------


def test_tools_list_advertises_a_cache_hint() -> None:
    hint = mcp_server.app.cache_hints.get("tools/list")
    assert hint is not None, "tools/list ships with no freshness hint"
    assert hint.ttl_ms > 0
    assert hint.scope == "private"


def test_cache_hint_reaches_the_serialized_result() -> None:
    """End to end through the SDK's own serializer, not just the constructor."""
    from mcp.server.runner import ServerRunner

    runner = ServerRunner(mcp_server.app, connection=None, lifespan_state=None)
    dumped = runner._serialize(
        "tools/list", "2026-07-28", asyncio.run(mcp_server._on_list_tools(None, None))
    )
    assert dumped["ttlMs"] == mcp_server._TOOLS_LIST_CACHE_TTL_MS
    assert dumped["cacheScope"] == "private"
    assert len(dumped["tools"]) == len(tool_specs.all_tools())


def test_cache_hint_is_sieved_off_a_legacy_connection() -> None:
    """Pre-2026-07-28 peers must not receive fields their revision never defined."""
    from mcp.server.runner import ServerRunner

    runner = ServerRunner(mcp_server.app, connection=None, lifespan_state=None)
    dumped = runner._serialize(
        "tools/list", "2025-06-18", asyncio.run(mcp_server._on_list_tools(None, None))
    )
    assert "ttlMs" not in dumped
    assert "cacheScope" not in dumped
    assert len(dumped["tools"]) == len(tool_specs.all_tools())


# --- entry-root truncation reaches the tool payload -----------------------------


def test_impact_reports_an_unknowable_entry_root_as_null(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A capped ``entry_roots`` list must not answer a negative through the tool.

    The kernel caps the list for its token budget; ``is_entry_root`` now returns
    ``None`` when absence is not provable, and the handler emits the helper's
    answer directly, so the wire value is a JSON ``null`` — "unknown" — instead
    of a ``false`` the map never established.
    """
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(
        json.dumps(
            {
                "languages": ["python"],
                "files": [{"path": "src/late.py", "area": "src", "kind": "code"}],
                "subsystems": [{"area": "src", "neighbors": []}],
                "dependents": {},
                "entry_roots": ["src/early.py"],
                "liveness_meta": {"entry_roots": {"shown": 1, "total": 25, "truncated": True}},
                "generated_head": "abc123",
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )

    payload = json.loads(
        asyncio.run(mcp_server.call_tool("devcouncil_impact", {"paths": ["src/late.py"]}))[0].text
    )
    assert payload["ok"] is True
    entry = next(p for p in payload["paths"] if p["path"] == "src/late.py")
    assert entry["is_entry_root"] is None


# --- 7. the secret guard judges the file that is actually opened ----------------
#
# Every case below was executed against the unmodified handler and returned the
# secret's bytes with ``ok: true``. `.env` itself was refused the whole time --
# the guard worked, it was simply asked about the wrong string.


def _write_secrets(root: Path) -> None:
    (root / ".env").write_text("OPENAI_API_KEY=sk-REDACTED-PROOF\n", encoding="utf-8")
    (root / ".ssh").mkdir()
    (root / ".ssh" / "ID_RSA").write_text("PRIVATE KEY\n", encoding="utf-8")
    (root / "certs").mkdir()
    (root / "certs" / "Server.PEM").write_text("CERT\n", encoding="utf-8")
    (root / "app" / "Credentials").mkdir(parents=True)
    (root / "app" / "Credentials" / "token.txt").write_text("TOKEN\n", encoding="utf-8")


def _case_insensitive(root: Path) -> bool:
    probe = root / "CaseProbe.tmp"
    probe.write_text("x", encoding="utf-8")
    try:
        return (root / "caseprobe.tmp").exists()
    finally:
        probe.unlink()


def _read(root: Path, path: str) -> dict:
    return _payload(asyncio.run(read.handle_read_file(root, {"path": path})))


@pytest.mark.parametrize(
    "path,secret",
    [
        (".ENV", "sk-REDACTED-PROOF"),
        (".env/", "sk-REDACTED-PROOF"),
        (".ssh/ID_RSA", "PRIVATE KEY"),
        ("certs/Server.PEM", "CERT"),
        ("app/Credentials/token.txt", "TOKEN"),
    ],
)
def test_a_secret_spelled_in_another_case_is_still_refused(
    tmp_path: Path, path: str, secret: str
) -> None:
    """``fnmatch`` matches through ``os.path.normcase`` -- identity on POSIX --
    so the guard was case-sensitive while APFS and NTFS are not. ``.ENV``,
    ``ID_RSA`` and ``Server.PEM`` named the same bytes as the patterns and
    missed every one of them."""
    _write_secrets(tmp_path)
    if not _case_insensitive(tmp_path):
        pytest.skip("case-sensitive filesystem: these spellings name different files")
    payload = _read(tmp_path, path)
    assert payload["ok"] is False, payload
    assert payload["code"] == "secret_path"
    assert secret not in json.dumps(payload)


def test_a_symlink_to_a_secret_inside_the_root_is_refused(tmp_path: Path) -> None:
    """The guard saw ``notes.txt``; ``open()`` saw ``.env``. Judging the
    caller's spelling instead of the resolved path is the whole defect."""
    _write_secrets(tmp_path)
    (tmp_path / "notes.txt").symlink_to(tmp_path / ".env")
    payload = _read(tmp_path, "notes.txt")
    assert payload["ok"] is False, payload
    assert payload["code"] == "secret_path"
    assert "sk-REDACTED-PROOF" not in json.dumps(payload)


def test_a_symlink_pointing_outside_the_root_is_refused(tmp_path: Path) -> None:
    """Already closed by ``within_root``; pinned so the case fix cannot widen it."""
    root = tmp_path / "repo"
    root.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / ".env").write_text("OUTSIDE=sk-OUT\n", encoding="utf-8")
    (root / "escape.txt").symlink_to(outside / ".env")
    payload = _read(root, "escape.txt")
    assert payload["ok"] is False, payload
    assert payload["code"] in {"secret_path", "path_escape"}
    assert "sk-OUT" not in json.dumps(payload)


def test_parent_traversal_to_a_secret_is_refused(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / ".env").write_text("OUTSIDE=sk-OUT\n", encoding="utf-8")
    payload = _read(root, "../outside/.env")
    assert payload["ok"] is False, payload
    assert payload["code"] in {"secret_path", "path_escape"}


def test_an_ordinary_file_and_an_ordinary_symlink_still_read(tmp_path: Path) -> None:
    """The guard must narrow, not blanket-refuse: a non-secret target still reads."""
    _write_secrets(tmp_path)
    (tmp_path / "real.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "alias.py").symlink_to(tmp_path / "real.py")
    assert _read(tmp_path, "real.py")["content"] == "x = 1"
    assert _read(tmp_path, "alias.py")["content"] == "x = 1"


# --- 8. debug consent is configuration, never a tool argument -------------------


def _debug_project(root: Path, *, consent: bool = False) -> Path:
    (root / ".git").mkdir(parents=True, exist_ok=True)
    config = root / ".devcouncil" / "config.yaml"
    config.parent.mkdir(parents=True, exist_ok=True)
    body = "project:\n  name: fixture\n"
    if consent:
        body += "code_intelligence:\n  debug:\n    auto_discover: true\n"
    config.write_text(body, encoding="utf-8")
    return config


def test_debug_consent_cannot_be_granted_from_a_tool_argument(tmp_path: Path) -> None:
    """``{"consent": true}`` used to call ``set_debug_consent`` and write
    ``auto_discover: true`` into ``.devcouncil/config.yaml``, unlocking the
    other seven debug tools -- process launch and script execution among them --
    from inside the argument dict of the tool it was supposed to gate. A gate a
    caller can set by passing an argument is not a gate."""
    config = _debug_project(tmp_path)
    before = config.read_text(encoding="utf-8")

    granted = _payload(
        asyncio.run(debug.dispatch("devcouncil_debug_discover", tmp_path, {"consent": True}))
    )
    assert granted["ok"] is False, granted
    assert granted["code"] == "debug_consent_required"
    assert "dev debug discover --consent" in granted["error"]
    assert config.read_text(encoding="utf-8") == before

    # ... and the gate is still shut for every tool the grant would have opened.
    for name, arguments in [
        ("devcouncil_debug_discover", {}),
        ("devcouncil_debug_start", {"adapterId": "debugpy"}),
        ("devcouncil_debug_trace", {"provider": "python", "script": "app.py"}),
    ]:
        assert _payload(asyncio.run(debug.dispatch(name, tmp_path, arguments)))["code"] == (
            "debug_consent_required"
        )


def test_configured_debug_consent_is_still_honoured(tmp_path: Path) -> None:
    """Narrowing only: consent the *user* set in configuration still works."""
    _debug_project(tmp_path, consent=True)
    payload = _payload(
        asyncio.run(debug.dispatch("devcouncil_debug_discover", tmp_path, {"consent": True}))
    )
    assert payload["consent"] is True
    assert "adapters" in payload


# --- 9. debug path arguments are contained in the project root ------------------


@pytest.fixture
def traced(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """A consented debug project whose trace providers record instead of run."""
    _debug_project(tmp_path, consent=True)
    seen: list[str] = []

    class Provider:
        def __init__(self, root: Path) -> None:
            self.root = root

        def run(self, script, args):  # noqa: ANN001
            seen.append(str(script))
            return {"script": str(script), "args": list(args)}

    monkeypatch.setattr(debug, "PythonTraceProvider", Provider)
    monkeypatch.setattr(debug, "NodeCpuProfileProvider", Provider)
    monkeypatch.setattr(
        debug, "import_runtime_trace", lambda _root, path: seen.append(str(path)) or {}
    )
    return seen


def _escapes(root: Path) -> dict[str, str]:
    outside = root.parent / "outside-debug"
    outside.mkdir(exist_ok=True)
    (outside / "x.py").write_text("print('executed')\n", encoding="utf-8")
    link = root / "link.py"
    if not link.exists():
        link.symlink_to(outside / "x.py")
    return {
        "absolute outside the root": str(outside / "x.py"),
        "parent traversal": "../outside-debug/x.py",
        "symlink pointing outside": "link.py",
        "an absolute system path": "/etc/passwd",
    }


@pytest.mark.parametrize("provider", ["python", "node"])
def test_debug_trace_refuses_a_script_outside_the_root(
    tmp_path: Path, traced: list[str], provider: str
) -> None:
    """``_trace`` handed ``arguments["script"]`` straight to a provider whose
    first line is ``script if script.is_absolute() else self.root / script`` --
    no ``resolve()``, no containment -- and then to ``subprocess.run``. That is
    arbitrary script execution outside the project the server was started for."""
    for label, script in _escapes(tmp_path).items():
        payload = _payload(
            asyncio.run(
                debug.dispatch(
                    "devcouncil_debug_trace", tmp_path, {"provider": provider, "script": script}
                )
            )
        )
        assert payload["ok"] is False, f"{label}: {payload}"
        assert payload["code"] == "path_escape", label
    assert traced == [], f"a refused script still reached a provider: {traced}"


def test_debug_trace_refuses_an_import_path_outside_the_root(
    tmp_path: Path, traced: list[str]
) -> None:
    for label, path in _escapes(tmp_path).items():
        payload = _payload(
            asyncio.run(
                debug.dispatch(
                    "devcouncil_debug_trace", tmp_path, {"provider": "import", "path": path}
                )
            )
        )
        assert payload["ok"] is False, f"{label}: {payload}"
        assert payload["code"] == "path_escape", label
    assert traced == []


def test_debug_breakpoint_source_outside_the_root_is_refused(tmp_path: Path) -> None:
    _debug_project(tmp_path, consent=True)
    payload = _payload(
        asyncio.run(
            debug.dispatch(
                "devcouncil_debug_breakpoints",
                tmp_path,
                {"sessionId": "s", "source": "/etc/passwd", "lines": [1]},
            )
        )
    )
    assert payload["ok"] is False, payload
    assert payload["code"] == "path_escape"


def test_debug_trace_still_runs_a_script_inside_the_root(
    tmp_path: Path, traced: list[str]
) -> None:
    """Narrowing only: an in-root script still reaches its provider, resolved."""
    (tmp_path / "app.py").write_text("print(1)\n", encoding="utf-8")
    payload = _payload(
        asyncio.run(
            debug.dispatch(
                "devcouncil_debug_trace", tmp_path, {"provider": "python", "script": "app.py"}
            )
        )
    )
    assert payload["script"] == str((tmp_path / "app.py").resolve())
    assert traced == [str((tmp_path / "app.py").resolve())]


# --- 10. a read-only-annotated call writes nothing ------------------------------


def test_the_freshness_probe_on_the_read_path_writes_nothing(tmp_path: Path) -> None:
    """``with_codeintel_freshness`` runs on ~20 tools annotated
    ``readOnlyHint: true``; its fingerprint check persisted
    ``.devcouncil/cache/content_hashes.json`` on every call. The annotation is
    what a host uses to decide whether to ask the user, so the write had to go,
    not the annotation."""
    from devcouncil.devmap_engine import compute_freshness

    root = tmp_path / "repo"
    root.mkdir()
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "t@t.com"],
        ["git", "config", "user.name", "t"],
    ):
        subprocess.run(args, cwd=root, capture_output=True, text=True)
    (root / "app.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=root, capture_output=True, text=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=root, capture_output=True, text=True)
    # Stamped by the same owner the kernel uses, so the map reads fresh to
    # `map_is_stale` exactly as a kernel-written one would.
    (root / ".devcouncil").mkdir(parents=True, exist_ok=True)
    (root / ".devcouncil" / "repo_map.json").write_text(
        json.dumps({"languages": ["python"], "files": [], **compute_freshness(root)}),
        encoding="utf-8",
    )

    cache = root / ".devcouncil" / "cache" / "content_hashes.json"
    if cache.exists():
        cache.unlink()

    stale, reason = mcp_util._map_artifact_freshness(root)
    # The probe must really have run -- "wrote nothing" is worthless if the
    # check bailed out, which is the same class of defect it is guarding.
    assert stale is False, f"probe did not run: {reason}"
    assert not cache.exists(), "a readOnlyHint:true call wrote into the project"
