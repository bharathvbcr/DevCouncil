"""Boundary tests for the MCP server's schema, path containment, and argv.

Every test here was written against the *unmodified* code and observed to fail
before the corresponding fix landed; each one names the specific escape it
closes rather than asserting a general "is hardened" property.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path

import pytest

from devcouncil.devmap_client import DevMapClient
from devcouncil.integrations.mcp import server as mcp_server
from devcouncil.integrations.mcp.handlers import codeintel, debug, tool_specs


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
