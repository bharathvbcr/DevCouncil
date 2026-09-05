"""Read-only MCP map query tools (repo_map / impact / liveness)."""

from __future__ import annotations

import json

import pytest

from devcouncil.integrations.mcp.handlers import map as mapmod
from devcouncil.integrations.mcp.server import call_tool, list_tools


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _write_repo_map(tmp_path, **overrides):
    payload = {
        "languages": ["python"],
        "frameworks": [],
        "package_managers": ["uv"],
        "files": [
            {"path": "src/payments/gateway.py", "area": "src/payments", "kind": "code", "summary": "charge"},
            {"path": "src/payments/models.py", "area": "src/payments", "kind": "code", "summary": "models"},
            {"path": "src/billing/invoice.py", "area": "src/billing", "kind": "code", "summary": "invoice"},
            {"path": "src/orphan.py", "area": "src", "kind": "code", "summary": "orphan"},
        ],
        "subsystems": [
            {
                "area": "src/payments",
                "summary": "Payment processing",
                "entry_points": ["src/payments/gateway.py"],
                "critical_files": ["src/payments/gateway.py", "src/payments/models.py"],
                "neighbors": ["src/billing"],
                "handoff_paths": ["payments/gateway.py -> billing/invoice.py"],
                "role_files": {"runtime": ["src/payments/gateway.py"]},
            },
            {
                "area": "src/billing",
                "summary": "Billing",
                "entry_points": ["src/billing/invoice.py"],
                "critical_files": ["src/billing/invoice.py"],
                "neighbors": ["src/payments"],
                "handoff_paths": [],
                "role_files": {},
            },
        ],
        "dependents": {
            "src/payments/models.py": ["src/payments/gateway.py", "src/billing/invoice.py"],
            "src/payments/gateway.py": ["src/billing/invoice.py"],
        },
        "entry_roots": ["src/payments/gateway.py", "src/billing/invoice.py"],
        "unwired_candidates": ["src/orphan.py", "src/payments/unused.py"],
        "unreachable_files": ["src/orphan.py"],
        "dead_symbol_candidates": [
            "src/orphan.py:3 unused_helper",
            "src/payments/models.py:10 dead_fn",
        ],
        "generated_head": "abc123",
        "indexed_hash": "hash1",
    }
    payload.update(overrides)
    dev = tmp_path / ".devcouncil"
    dev.mkdir(exist_ok=True)
    (dev / "repo_map.json").write_text(json.dumps(payload), encoding="utf-8")
    return payload


@pytest.mark.anyio
async def test_mcp_lists_map_tools():
    tools = await list_tools()
    names = {tool.name for tool in tools}
    assert "devcouncil_repo_map" in names
    assert "devcouncil_impact" in names
    assert "devcouncil_liveness" in names
    assert "devcouncil_graph_impact" in names
    assert "devcouncil_graph_query" in names
    assert "devcouncil_graph_trace" in names
    assert "devcouncil_graph_ingest" in names
    assert "devcouncil_graph_cypher" in names
    assert "devcouncil_pdg_query" in names
    assert "devcouncil_explain" in names


@pytest.mark.anyio
async def test_repo_map_summary(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )

    result = await call_tool("devcouncil_repo_map", {})
    payload = json.loads(result[0].text)

    assert payload["ok"] is True
    # Freshness comes from the kernel, or from `repo_map.json` — the artifact
    # the kernel writes — never from the Python sync coordinator, which
    # reports a different store's pending state.
    assert payload["stale"] is False
    assert payload["languages"] == ["python"]
    areas = {s["area"] for s in payload["subsystems"]}
    assert areas == {"src/payments", "src/billing"}
    assert "entry_points" not in payload["subsystems"][0]


@pytest.mark.anyio
async def test_repo_map_subsystem_detail_and_path_resolve(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: True,
    )

    by_area = json.loads((await call_tool("devcouncil_repo_map", {"subsystem": "src/payments"}))[0].text)
    assert by_area["stale"] is True
    assert by_area["subsystem"]["area"] == "src/payments"
    assert by_area["subsystem"]["entry_points"] == ["src/payments/gateway.py"]
    assert by_area["subsystem"]["neighbors"] == ["src/billing"]
    assert by_area["subsystem"]["role_files"]["runtime"] == ["src/payments/gateway.py"]

    by_path = json.loads(
        (await call_tool("devcouncil_repo_map", {"path": "src/payments/models.py"}))[0].text
    )
    assert by_path["area"] == "src/payments"
    assert by_path["subsystem"]["area"] == "src/payments"
    assert by_path["stale"] is True


@pytest.mark.anyio
async def test_repo_map_missing_returns_error(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    result = await call_tool("devcouncil_repo_map", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "map_missing"


@pytest.mark.anyio
async def test_impact_dependents_neighbors_and_crossings(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )

    # Same-area + neighbor: no cross-boundary pair.
    same = json.loads(
        (await call_tool("devcouncil_impact", {
            "paths": ["src/payments/models.py", "src/billing/invoice.py"],
        }))[0].text
    )
    assert same["ok"] is True
    # Freshness comes from the kernel, or from `repo_map.json` — the artifact
    # the kernel writes — never from the Python sync coordinator, which
    # reports a different store's pending state.
    assert same["stale"] is False
    models = next(p for p in same["paths"] if p["path"] == "src/payments/models.py")
    assert models["dependents"] == ["src/payments/gateway.py", "src/billing/invoice.py"]
    assert models["area"] == "src/payments"
    assert "src/billing" in models["neighbors"]
    assert models["is_entry_root"] is False
    assert same["cross_boundary_pairs"] == []
    assert "dependents_total" not in models  # no truncation metadata when complete


@pytest.mark.anyio
async def test_impact_includes_dependents_total_when_truncated(tmp_path, monkeypatch):
    _write_repo_map(
        tmp_path,
        dependents={"src/payments/models.py": ["src/payments/gateway.py"]},
        dependents_total={"src/payments/models.py": 40},
    )
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )
    result = await mapmod.handle_impact(tmp_path, {"paths": ["src/payments/models.py"]})
    out = _parse(result)
    item = out["paths"][0]
    assert item["dependents"] == ["src/payments/gateway.py"]
    assert item["dependents_total"] == 40


@pytest.mark.anyio
async def test_impact_cross_boundary_force(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )
    # Force a cross-boundary by removing neighbor links in a custom map. The
    # producer marker stays, so the empty lists are this map's *answer* rather
    # than a field it never computed — which is what makes the pair a crossing.
    _write_repo_map(
        tmp_path,
        meta={"devmap_rust": {"neighbors_computed": True}},
        subsystems=[
            {
                "area": "src/payments",
                "summary": "Payment processing",
                "entry_points": ["src/payments/gateway.py"],
                "critical_files": ["src/payments/gateway.py"],
                "neighbors": [],
                "handoff_paths": [],
                "role_files": {},
            },
            {
                "area": "src/billing",
                "summary": "Billing",
                "entry_points": ["src/billing/invoice.py"],
                "critical_files": ["src/billing/invoice.py"],
                "neighbors": [],
                "handoff_paths": [],
                "role_files": {},
            },
        ],
    )
    cross = json.loads(
        (await call_tool("devcouncil_impact", {
            "paths": ["src/payments/gateway.py", "src/billing/invoice.py"],
        }))[0].text
    )
    assert cross["cross_boundary_checked"] is True
    assert cross["cross_boundary_pairs"] == [{"areas": ["src/billing", "src/payments"]}]


@pytest.mark.anyio
async def test_impact_says_when_the_map_never_established_neighbors(tmp_path, monkeypatch):
    """An empty `cross_boundary_pairs` is two different answers; say which.

    Same map as above with the marker removed — which is what the kernel
    actually writes today. No pair can be called a crossing, and the response
    has to say that rather than look like a clean check.
    """
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )
    _write_repo_map(
        tmp_path,
        subsystems=[
            {
                "area": "src/payments",
                "summary": "Payment processing",
                "entry_points": ["src/payments/gateway.py"],
                "critical_files": ["src/payments/gateway.py"],
                "neighbors": [],
                "handoff_paths": [],
                "role_files": {},
            },
            {
                "area": "src/billing",
                "summary": "Billing",
                "entry_points": ["src/billing/invoice.py"],
                "critical_files": ["src/billing/invoice.py"],
                "neighbors": [],
                "handoff_paths": [],
                "role_files": {},
            },
        ],
    )
    payload = json.loads(
        (await call_tool("devcouncil_impact", {
            "paths": ["src/payments/gateway.py", "src/billing/invoice.py"],
        }))[0].text
    )
    assert payload["ok"] is True
    assert payload["cross_boundary_checked"] is False
    assert payload["cross_boundary_pairs"] == []


@pytest.mark.anyio
async def test_impact_requires_paths(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    missing = json.loads((await call_tool("devcouncil_impact", {}))[0].text)
    assert missing["code"] == "missing_argument"
    bad = json.loads((await call_tool("devcouncil_impact", {"paths": "src/a.py"}))[0].text)
    assert bad["code"] == "invalid_arguments"


@pytest.mark.anyio
async def test_liveness_lists_and_filters(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )

    full = json.loads((await call_tool("devcouncil_liveness", {}))[0].text)
    assert full["ok"] is True
    # Freshness comes from the kernel, or from `repo_map.json` — the artifact
    # the kernel writes — never from the Python sync coordinator, which
    # reports a different store's pending state.
    assert full["stale"] is False
    assert "src/orphan.py" in full["unwired_candidates"]
    assert "src/orphan.py" in full["unreachable_files"]
    assert "src/orphan.py:3 unused_helper" in full["dead_symbol_candidates"]
    assert "src/payments/gateway.py" in full["entry_roots"]

    by_prefix = json.loads(
        (await call_tool("devcouncil_liveness", {"path_prefix": "src/payments"}))[0].text
    )
    assert by_prefix["unwired_candidates"] == ["src/payments/unused.py"]
    assert by_prefix["unreachable_files"] == []
    assert by_prefix["dead_symbol_candidates"] == ["src/payments/models.py:10 dead_fn"]
    assert by_prefix["entry_roots"] == ["src/payments/gateway.py"]

    by_area = json.loads(
        (await call_tool("devcouncil_liveness", {"area": "src/payments"}))[0].text
    )
    assert by_area["unwired_candidates"] == ["src/payments/unused.py"]
    assert by_area["dead_symbol_candidates"] == ["src/payments/models.py:10 dead_fn"]


@pytest.mark.anyio
async def test_liveness_dead_code_defaults_to_inferred(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )

    from devcouncil.indexing.graph.schema import CodeGraph, Confidence, DeadCodeEntry

    graph = CodeGraph(
        schema_version=2,
        nodes=[],
        edges=[],
        dead_code=[
            DeadCodeEntry(
                id="a.py::inferred_dead",
                path="a.py",
                line=1,
                kind="function",
                confidence=Confidence.INFERRED,
                reason="no inbound call edges (method)",
            ),
            DeadCodeEntry(
                id="b.py::ambiguous_dead",
                path="b.py",
                line=2,
                kind="function",
                confidence=Confidence.AMBIGUOUS,
                reason="graph-dead but token-scan cleared (possible name collision)",
            ),
        ],
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: graph,
    )

    defaulted = json.loads((await call_tool("devcouncil_liveness", {}))[0].text)
    assert defaulted["min_confidence"] == "inferred"
    ids = {d["id"] for d in defaulted["dead_code"]}
    assert "a.py::inferred_dead" in ids
    assert "b.py::ambiguous_dead" not in ids
    assert defaulted["dead_code_hidden"] == 1

    all_tiers = json.loads(
        (await call_tool("devcouncil_liveness", {"min_confidence": "ambiguous"}))[0].text
    )
    all_ids = {d["id"] for d in all_tiers["dead_code"]}
    assert "b.py::ambiguous_dead" in all_ids
    assert all_tiers["dead_code_hidden"] == 0


# ---- pure helpers -------------------------------------------------------------


def _parse(result):
    return json.loads(result[0].text)


def test_load_repo_map_invalid_json(tmp_path):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text("{not json", encoding="utf-8")
    assert mapmod._load_repo_map(tmp_path) is None


def test_load_repo_map_non_dict(tmp_path):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text("[1,2]", encoding="utf-8")
    assert mapmod._load_repo_map(tmp_path) is None


def test_map_stale_no_data_and_exception(tmp_path, monkeypatch):
    assert mapmod._map_stale(tmp_path, None) is False
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    # Fail closed: unverifiable map must not be reported as fresh.
    assert mapmod._map_stale(tmp_path, {"files": []}) is True


def test_subsystem_detail_handles_bad_role_files():
    detail = mapmod._subsystem_detail({"area": "a", "role_files": "not-a-dict"})
    assert detail["role_files"] == {}


def test_find_subsystem_missing_returns_none():
    assert mapmod._find_subsystem({"subsystems": [{"area": "x"}]}, "y") is None


# ---- handle_repo_map branches -------------------------------------------------

@pytest.mark.anyio
async def test_repo_map_empty_string_arg_errors(tmp_path):
    result = await mapmod.handle_repo_map(tmp_path, {"subsystem": ""})
    payload = _parse(result)
    assert payload["ok"] is False
    assert payload["code"] == "invalid_arguments"


@pytest.mark.anyio
async def test_repo_map_path_resolves_area_but_unknown_subsystem(tmp_path, monkeypatch):
    # A path whose area has no matching subsystem entry -> unknown_subsystem.
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    # 'src/orphan.py' resolves to area 'src' which is not a declared subsystem.
    result = await mapmod.handle_repo_map(tmp_path, {"path": "src/orphan.py"})
    out = _parse(result)
    assert out["code"] == "unknown_subsystem"
    assert out["area"] == "src"


@pytest.mark.anyio
async def test_repo_map_symbols_for_path(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    from types import SimpleNamespace

    nodes = [
        SimpleNamespace(id="n1", kind=SimpleNamespace(value="function"), name="charge", line=3, path="src/payments/gateway.py"),
        SimpleNamespace(id="n2", kind="file", name="gateway", line=1, path="src/payments/gateway.py"),
        SimpleNamespace(id="n3", kind=SimpleNamespace(value="function"), name="other", line=9, path="src/other.py"),
    ]
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: SimpleNamespace(nodes=nodes),
    )
    # subsystem given -> detail branch; symbols only surface on the summary branch.
    await mapmod.handle_repo_map(tmp_path, {"path": "src/payments/gateway.py", "subsystem": "src/payments"})
    # path with no declared area -> summary branch with symbols computed.
    result2 = await mapmod.handle_repo_map(tmp_path, {"path": "src/nowhere.py"})
    out2 = _parse(result2)
    # path with no area -> summary branch with symbols computed for that path.
    assert out2["symbols"] == []


@pytest.mark.anyio
async def test_repo_map_summary_symbols_listed(tmp_path, monkeypatch):
    # path that has no declared subsystem area -> summary branch, symbols computed.
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    from types import SimpleNamespace

    nodes = [
        SimpleNamespace(id="s1", kind=SimpleNamespace(value="function"), name="orphan_fn", line=2, path="src/orphan.py"),
        SimpleNamespace(id="s2", kind="file", name="orphan", line=1, path="src/orphan.py"),
    ]
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: SimpleNamespace(nodes=nodes),
    )
    # 'src/orphan.py' area is 'src' -> resolved but no subsystem match, so it hits the
    # unknown_subsystem branch. Use a path outside any area to reach the summary branch.
    result = await mapmod.handle_repo_map(tmp_path, {"path": "top_level.py"})
    out = _parse(result)
    assert out["ok"] is True
    assert out["symbols"] == []  # top_level.py has no graph nodes


def test_symbols_for_path_no_graph_is_unavailable(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.indexing.graph.build.load_code_graph", lambda root: None)
    scan = mapmod._symbols_for_path(tmp_path, "a.py")
    assert scan.ok is False
    assert scan.items == []
    assert "no code graph" in scan.reason


def test_symbols_for_path_lists_symbols(tmp_path, monkeypatch):
    from types import SimpleNamespace

    nodes = [
        SimpleNamespace(id="n1", kind=SimpleNamespace(value="function"), name="f", line=3, path="a.py"),
        SimpleNamespace(id="n2", kind="file", name="a", line=1, path="a.py"),
        SimpleNamespace(id="n3", kind="class", name="C", line=5, path="a.py"),
    ]
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: SimpleNamespace(nodes=nodes),
    )
    scan = mapmod._symbols_for_path(tmp_path, "a.py")
    assert scan.ok is True
    names = {s["name"] for s in scan.items}
    assert names == {"f", "C"}  # 'file' kind excluded
    assert scan.total == 2 and scan.truncated is False


def test_symbols_for_path_reports_the_error_it_used_to_swallow(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    scan = mapmod._symbols_for_path(tmp_path, "a.py")
    assert scan.ok is False
    assert scan.items == []
    assert "boom" in scan.reason


# ---- handle_impact branches ---------------------------------------------------

@pytest.mark.anyio
async def test_impact_map_missing(tmp_path):
    result = await mapmod.handle_impact(tmp_path, {"paths": ["a.py"]})
    assert _parse(result)["code"] == "map_missing"


@pytest.mark.anyio
async def test_impact_precise_uses_lsp(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    closed = {"n": 0}

    class FakePool:
        def __init__(self, root):
            pass

        def dependents_of_file(self, path):
            return ["src/lsp_caller.py"]

        def close(self):
            closed["n"] += 1

    monkeypatch.setattr("devcouncil.indexing.lsp_client.LspSessionPool", FakePool)
    result = await mapmod.handle_impact(tmp_path, {"paths": ["src/payments/models.py"], "precise": True})
    out = _parse(result)
    assert out["precise"] is True
    item = out["paths"][0]
    assert item["resolution"] == "lsp"
    assert item["dependents"] == ["src/lsp_caller.py"]
    assert closed["n"] == 1


@pytest.mark.anyio
async def test_impact_precise_lsp_unavailable(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )

    def boom(root):
        raise RuntimeError("no lsp")

    monkeypatch.setattr("devcouncil.indexing.lsp_client.LspSessionPool", boom)
    result = await mapmod.handle_impact(tmp_path, {"paths": ["src/payments/models.py"], "precise": True})
    out = _parse(result)
    # LSP pool failed to construct -> falls back to import resolution.
    assert out["paths"][0]["resolution"] == "import"


# ---- handle_liveness error branches -------------------------------------------

@pytest.mark.anyio
async def test_liveness_bad_min_confidence(tmp_path):
    _write_repo_map(tmp_path)
    result = await mapmod.handle_liveness(tmp_path, {"min_confidence": "bogus"})
    out = _parse(result)
    assert out["code"] == "invalid_arguments"
    assert out["argument"] == "min_confidence"


@pytest.mark.anyio
async def test_liveness_empty_string_arg(tmp_path):
    _write_repo_map(tmp_path)
    result = await mapmod.handle_liveness(tmp_path, {"area": ""})
    assert _parse(result)["code"] == "invalid_arguments"


@pytest.mark.anyio
async def test_liveness_map_missing(tmp_path):
    result = await mapmod.handle_liveness(tmp_path, {})
    assert _parse(result)["code"] == "map_missing"


@pytest.mark.anyio
async def test_liveness_non_list_entry_roots(tmp_path, monkeypatch):
    _write_repo_map(tmp_path, entry_roots="not-a-list")
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    out = _parse(await mapmod.handle_liveness(tmp_path, {}))
    assert out["entry_roots"] == []


# ---- _structured_dead_code ----------------------------------------------------

def test_structured_dead_code_no_graph_is_unavailable_not_clean(tmp_path, monkeypatch):
    """No graph is "could not look", not "looked and found nothing"."""
    monkeypatch.setattr("devcouncil.indexing.graph.build.load_code_graph", lambda root: None)
    scan = mapmod._structured_dead_code(tmp_path, area=None, path_prefix=None)
    assert scan.ok is False
    assert scan.items == []
    assert "no code graph" in scan.reason


def test_structured_dead_code_reports_the_error_it_used_to_swallow(tmp_path, monkeypatch):
    """Rewritten: this test used to assert the swallow (``== ([], 0)``).

    That encoded the defect as the contract — a crashing dead-code scan was
    indistinguishable from a repository with no dead code, which is exactly the
    Class A failure this module exists to prevent. The honest contract is an
    unavailable scan carrying the reason.
    """
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    scan = mapmod._structured_dead_code(tmp_path, area=None, path_prefix=None)
    assert scan.ok is False
    assert scan.items == []
    assert "boom" in scan.reason


# ---- graph query / trace / impact ---------------------------------------------

@pytest.mark.anyio
async def test_graph_query_missing_and_ok(tmp_path, monkeypatch):
    missing = _parse(await mapmod.handle_graph_query(tmp_path, {}))
    assert missing["code"] == "missing_argument"
    monkeypatch.setattr("devcouncil.indexing.graph.query_symbol", lambda root, name: {"symbol": name})
    ok = _parse(await mapmod.handle_graph_query(tmp_path, {"name_or_path": "foo"}))
    assert ok["ok"] is True and ok["symbol"] == "foo"


@pytest.mark.anyio
async def test_graph_trace_missing_and_ok(tmp_path, monkeypatch):
    assert _parse(await mapmod.handle_graph_trace(tmp_path, {"to": "b"}))["argument"] == "from"
    assert _parse(await mapmod.handle_graph_trace(tmp_path, {"from": "a"}))["argument"] == "to"
    monkeypatch.setattr(
        "devcouncil.indexing.graph.trace_path", lambda root, a, b: {"path": [a, b]}
    )
    ok = _parse(await mapmod.handle_graph_trace(tmp_path, {"from": "a", "to": "b"}))
    assert ok["ok"] is True and ok["path"] == ["a", "b"]


@pytest.mark.anyio
async def test_graph_impact_missing_paths(tmp_path):
    out = _parse(await mapmod.handle_graph_impact(tmp_path, {}))
    assert out["code"] == "missing_argument"


@pytest.mark.anyio
async def test_graph_impact_no_graph(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.indexing.graph.build.load_code_graph", lambda root: None)
    out = _parse(await mapmod.handle_graph_impact(tmp_path, {"paths": ["a.py"]}))
    assert out["code"] == "graph_missing"


@pytest.mark.anyio
async def test_graph_impact_ok(tmp_path, monkeypatch):
    from types import SimpleNamespace

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph", lambda root: SimpleNamespace()
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.intel.diff_impact",
        lambda root, graph, paths, use_diff, max_depth: {"paths": paths or [], "diff": use_diff},
    )
    out = _parse(await mapmod.handle_graph_impact(tmp_path, {"diff": True}))
    assert out["ok"] is True and out["diff"] is True


@pytest.mark.anyio
async def test_pdg_query_missing_and_ok(tmp_path, monkeypatch):
    missing = _parse(await mapmod.handle_pdg_query(tmp_path, {"target": "fn"}))
    assert missing["code"] == "missing_argument"
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.query_pdg_controls",
        lambda root, target: {"ok": True, "target": target},
    )
    ok = _parse(await mapmod.handle_pdg_query(tmp_path, {"mode": "controls", "target": "fn"}))
    assert ok["ok"] is True and ok["target"] == "fn"
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.query_pdg_flows",
        lambda root, target, variable=None: {"ok": True, "target": target, "variable": variable},
    )
    flows = _parse(
        await mapmod.handle_pdg_query(
            tmp_path, {"mode": "flows", "target": "fn", "variable": "x"}
        )
    )
    assert flows["ok"] is True and flows["variable"] == "x"
    bad = _parse(await mapmod.handle_pdg_query(tmp_path, {"mode": "bad", "target": "fn"}))
    assert bad["code"] == "invalid_arguments"


@pytest.mark.anyio
async def test_explain_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.explain_pdg_taint",
        lambda root, path=None, category=None: {
            "ok": True,
            "count": 1,
            "findings": [{"path": path, "category": category}],
        },
    )
    out = _parse(
        await mapmod.handle_explain(tmp_path, {"path": "src/a.py", "category": "sql-injection"})
    )
    assert out["ok"] is True and out["count"] == 1

@pytest.mark.anyio
async def test_liveness_scope_without_entry_roots_stays_reliable(tmp_path, monkeypatch):
    """Filtering to an area that has no entry root must not flip the response
    to unreachable_unreliable (reliability is a global map property)."""
    _write_repo_map(tmp_path)
    monkeypatch.setattr(mapmod, "_map_stale", lambda *_: False)
    result = await mapmod.handle_liveness(tmp_path, {"area": "src"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    assert payload["unreachable_unreliable"] is False
    # Narrowed from `"warning" not in payload`: this fixture has no code graph,
    # so the response now also carries an honest "dead-code scan unavailable"
    # warning. The subject of this test is the *unreachable* warning, which must
    # still be absent.
    assert "unreachable_files omitted" not in payload.get("warning", "")
    assert payload["unreachable_files"] == ["src/orphan.py"]
    # The scoped entry_roots view is empty — that's fine and expected.
    assert payload["entry_roots"] == []


@pytest.mark.anyio
async def test_liveness_globally_empty_roots_still_unreliable(tmp_path, monkeypatch):
    _write_repo_map(tmp_path, entry_roots=[])
    monkeypatch.setattr(mapmod, "_map_stale", lambda *_: False)
    result = await mapmod.handle_liveness(tmp_path, {})
    payload = json.loads(result[0].text)
    assert payload["unreachable_unreliable"] is True
    assert payload["unreachable_files"] == []
    assert "warning" in payload


@pytest.mark.anyio
async def test_graph_ingest_paths_branch_reports_engine_unavailable(tmp_path, monkeypatch):
    """A kernel that cannot build during explicit-paths ingest must return the
    structured engine_unavailable error, not raise through MCP."""
    from devcouncil.devmap_engine import DevMapEngineError

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(
            DevMapEngineError("another devmap writer holds the store lock")
        ),
    )
    result = await mapmod.handle_graph_ingest(tmp_path, {"paths": ["a.py"]})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "engine_unavailable"


# ---- Class A: unavailable is never a clean bill of health ----------------------


def _fake_dead_graph(count, *, confidence=None):
    from devcouncil.indexing.graph.schema import CodeGraph, Confidence, DeadCodeEntry

    conf = confidence or Confidence.INFERRED
    return CodeGraph(
        schema_version=2,
        nodes=[],
        edges=[],
        dead_code=[
            DeadCodeEntry(
                id=f"a.py::dead_{i}",
                path="a.py",
                line=i + 1,
                kind="function",
                confidence=conf,
                reason="no inbound call edges",
            )
            for i in range(count)
        ],
    )


@pytest.mark.anyio
async def test_liveness_dead_code_unavailable_is_not_a_clean_bill(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: (_ for _ in ()).throw(RuntimeError("store locked")),
    )

    out = _parse(await mapmod.handle_liveness(tmp_path, {}))

    assert out["dead_code"] == []
    assert out["dead_code_available"] is False
    assert "store locked" in out["dead_code_reason"]
    assert out["dead_code_hidden"] is None
    assert out["dead_code_total"] is None
    assert "dead-code scan unavailable" in out["warning"]


@pytest.mark.anyio
async def test_liveness_dead_code_carries_total_beside_the_cap(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph", lambda root: _fake_dead_graph(250)
    )

    out = _parse(await mapmod.handle_liveness(tmp_path, {}))

    assert len(out["dead_code"]) == 200
    assert out["dead_code_available"] is True
    assert out["dead_code_shown"] == 200
    assert out["dead_code_total"] == 250
    assert out["dead_code_truncated"] is True
    # 50 candidates were dropped by the 200 cap; `dead_code_hidden` used to
    # count only the confidence filter and so reported 0 here.
    assert out["dead_code_hidden"] == 50
    assert out["dead_code_hidden_low_confidence"] == 0


@pytest.mark.anyio
async def test_liveness_dead_code_hidden_sums_confidence_and_cap(tmp_path, monkeypatch):
    from devcouncil.indexing.graph.schema import CodeGraph, Confidence, DeadCodeEntry

    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)
    graph = CodeGraph(
        schema_version=2,
        nodes=[],
        edges=[],
        dead_code=[
            DeadCodeEntry(
                id=f"a.py::dead_{i}",
                path="a.py",
                line=i + 1,
                kind="function",
                confidence=Confidence.INFERRED if i < 250 else Confidence.AMBIGUOUS,
                reason="no inbound call edges",
            )
            for i in range(255)
        ],
    )
    monkeypatch.setattr("devcouncil.indexing.graph.build.load_code_graph", lambda root: graph)

    out = _parse(await mapmod.handle_liveness(tmp_path, {}))

    assert out["dead_code_total"] == 255
    assert out["dead_code_shown"] == 200
    assert out["dead_code_hidden_low_confidence"] == 5
    assert out["dead_code_hidden"] == 55


def test_graph_degraded_fields_unknown_without_a_map(tmp_path):
    fields = mapmod._graph_degraded_fields(tmp_path)
    assert fields["graph_degraded"] is None
    assert "repo map" in fields["graph_degraded_reason"]


def test_graph_degraded_fields_known_with_a_map(tmp_path):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text(json.dumps({"languages": []}), encoding="utf-8")
    assert mapmod._graph_degraded_fields(tmp_path)["graph_degraded"] is False


# ---- Class A: `ok` is derived from the payload, never asserted -----------------


@pytest.mark.anyio
async def test_graph_query_error_payload_is_not_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query_symbol",
        lambda root, name: {"error": "no code graph; run `dev map` first", "query": name},
    )
    out = _parse(await mapmod.handle_graph_query(tmp_path, {"name_or_path": "foo"}))
    assert out["ok"] is False
    assert out["code"] == "graph_unavailable"
    assert out["error"] == "no code graph; run `dev map` first"


@pytest.mark.anyio
async def test_graph_trace_error_payload_is_not_ok(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.graph.trace_path",
        lambda root, a, b: {"error": "no code graph; run `dev map` first", "from": a, "to": b},
    )
    out = _parse(await mapmod.handle_graph_trace(tmp_path, {"from": "a", "to": "b"}))
    assert out["ok"] is False
    assert out["code"] == "graph_unavailable"


# ---- Class A: impact always says which engine answered ------------------------


@pytest.mark.anyio
async def test_impact_emits_resolution_without_precise(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)
    out = _parse(await mapmod.handle_impact(tmp_path, {"paths": ["src/payments/models.py"]}))
    item = out["paths"][0]
    assert item["resolution"] == "import"
    assert item["resolution_reason"]


@pytest.mark.anyio
async def test_impact_names_the_kernel_failure_behind_the_import_heuristic(tmp_path, monkeypatch):
    from devcouncil.devmap_client import DevMapClientError

    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )

    class Broken:
        def impact(self, path, depth=1):
            raise DevMapClientError("kernel down")

        def is_map_stale(self):
            raise DevMapClientError("kernel down")

    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: Broken())
    out = _parse(await mapmod.handle_impact(tmp_path, {"paths": ["src/payments/models.py"]}))
    item = out["paths"][0]
    assert item["resolution"] == "import"
    assert "kernel down" in item["resolution_reason"]


@pytest.mark.anyio
async def test_repo_map_symbols_unavailable_is_not_an_empty_file(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: (_ for _ in ()).throw(RuntimeError("graph unreadable")),
    )
    out = _parse(await mapmod.handle_repo_map(tmp_path, {"path": "top_level.py"}))
    assert out["symbols"] == []
    assert out["symbols_available"] is False
    assert "graph unreadable" in out["symbols_reason"]


@pytest.mark.anyio
async def test_impact_dependents_total_is_scoped_to_the_engine_that_answered(tmp_path, monkeypatch):
    """`dependents_total` describes the repo map's import universe.

    When LSP or the kernel replaced `dependents`, that total counts a different
    set, so pairing it with the replacement list is a wrong "shown of total".
    """
    _write_repo_map(
        tmp_path,
        dependents={"src/payments/models.py": ["src/payments/gateway.py"]},
        dependents_total={"src/payments/models.py": 40},
    )
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )

    class FakePool:
        def __init__(self, root):
            pass

        def dependents_of_file(self, path):
            return ["src/lsp_caller.py"]

        def close(self):
            pass

    monkeypatch.setattr("devcouncil.indexing.lsp_client.LspSessionPool", FakePool)
    out = _parse(
        await mapmod.handle_impact(
            tmp_path, {"paths": ["src/payments/models.py"], "precise": True}
        )
    )
    item = out["paths"][0]
    assert item["resolution"] == "lsp"
    assert "dependents_total" not in item


@pytest.mark.anyio
async def test_impact_reports_why_the_lsp_pool_could_not_be_built(tmp_path, monkeypatch):
    _write_repo_map(tmp_path)
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale", lambda self, d: False
    )

    def boom(root):
        raise RuntimeError("pyright missing")

    monkeypatch.setattr("devcouncil.indexing.lsp_client.LspSessionPool", boom)
    out = _parse(
        await mapmod.handle_impact(
            tmp_path, {"paths": ["src/payments/models.py"], "precise": True}
        )
    )
    item = out["paths"][0]
    assert item["resolution"] == "import"
    assert "pyright missing" in item["resolution_reason"]


def test_subsystem_detail_says_whether_neighbors_were_computed():
    """An empty `neighbors` row must not read as "this area borders nothing"."""
    uncomputed = mapmod._subsystem_detail({"area": "a", "neighbors": []})
    assert uncomputed["neighbors"] == []
    assert uncomputed["neighbors_computed"] is False

    computed = mapmod._subsystem_detail({"area": "a", "neighbors": []}, neighbors_computed=True)
    assert computed["neighbors"] == []
    assert computed["neighbors_computed"] is True
