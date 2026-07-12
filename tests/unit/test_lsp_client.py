"""Unit tests for the optional live LSP client (mocked — no real servers)."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

from devcouncil.indexing.lsp import LspInspector
from devcouncil.indexing.lsp_client import (
    LspClient,
    LspLocation,
    LspSessionPool,
    filter_dead_symbols_with_lsp,
    language_for_path,
)


@pytest.fixture
def anyio_backend():
    return "asyncio"


class _FakeClient:
    """Stand-in that never spawns a process."""

    def __init__(
        self,
        project_root: Path,
        language: str,
        command: list[str],
        *,
        refs: dict[tuple[str, int, str], list[LspLocation]] | None = None,
        fail_start: bool = False,
    ) -> None:
        self.project_root = project_root
        self.language = language
        self.command = command
        self._refs = refs or {}
        self._fail_start = fail_start
        self._alive = False
        self.shutdown_called = False

    def start(self) -> bool:
        if self._fail_start:
            return False
        self._alive = True
        return True

    def shutdown(self) -> None:
        self._alive = False
        self.shutdown_called = True

    def references(
        self,
        rel_path: str,
        line: int,
        character: int,
        *,
        include_declaration: bool = False,
    ) -> list[LspLocation] | None:
        _ = include_declaration
        abs_path = self.project_root / rel_path
        text = abs_path.read_text(encoding="utf-8")
        lines = text.splitlines()
        name = ""
        if 0 <= line < len(lines):
            # Recover name from character position for key lookup.
            row = lines[line]
            end = character
            while end < len(row) and (row[end].isalnum() or row[end] == "_"):
                end += 1
            name = row[character:end]
        key = (rel_path.replace("\\", "/"), line + 1, name)
        if key not in self._refs:
            # Also try matching any entry for this path+line.
            for (p, ln, n), locs in self._refs.items():
                if p == rel_path.replace("\\", "/") and ln == line + 1:
                    return list(locs)
            return []
        return list(self._refs[key])


def test_language_for_path():
    assert language_for_path("a/b.py") == "python"
    assert language_for_path("x.ts") == "typescript"
    assert language_for_path("readme.md") is None


def test_lsp_summary_client_vs_detection(tmp_path):
    (tmp_path / "app.py").write_text("def hello():\n    return 1\n", encoding="utf-8")
    detection = LspInspector(tmp_path).summary(["app.py"])
    client = LspInspector(tmp_path).summary(["app.py"], client_enabled=True)
    assert detection["mode"] == "detection-only"
    assert "does not run an LSP client" in detection["note"]
    assert client["mode"] == "client"
    assert "textDocument/references" in client["note"]


def test_filter_dead_symbols_clears_when_lsp_finds_refs(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text(
        "def helper():\n    return 1\n",
        encoding="utf-8",
    )
    (tmp_path / "user.py").write_text("from mod import helper\n", encoding="utf-8")

    refs = {
        ("mod.py", 1, "helper"): [
            LspLocation(path="user.py", line=1, character=0),
        ],
    }

    def factory(root: Path, language: str, command: list[str]) -> Any:
        return _FakeClient(root, language, command, refs=refs)

    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.first_available_command",
        lambda language: ["fake-ls"],
    )
    pool = LspSessionPool(tmp_path, client_factory=factory)
    try:
        kept = filter_dead_symbols_with_lsp(
            tmp_path,
            ["mod.py:1 helper"],
            pool=pool,
        )
        assert kept == []
    finally:
        pool.close()


def test_filter_dead_symbols_keeps_when_unreferenced(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text(
        "def orphan():\n    return 1\n",
        encoding="utf-8",
    )

    def factory(root: Path, language: str, command: list[str]) -> Any:
        return _FakeClient(root, language, command, refs={})

    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.first_available_command",
        lambda language: ["fake-ls"],
    )
    pool = LspSessionPool(tmp_path, client_factory=factory)
    try:
        kept = filter_dead_symbols_with_lsp(
            tmp_path,
            ["mod.py:1 orphan"],
            pool=pool,
        )
        assert kept == ["mod.py:1 orphan"]
    finally:
        pool.close()


def test_filter_dead_symbols_keeps_when_no_server(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.first_available_command",
        lambda language: None,
    )
    kept = filter_dead_symbols_with_lsp(tmp_path, ["mod.py:1 orphan"])
    assert kept == ["mod.py:1 orphan"]


def test_dependents_of_file_via_pool(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text(
        "def helper():\n    return 1\n\nclass Thing:\n    pass\n",
        encoding="utf-8",
    )
    refs = {
        ("mod.py", 1, "helper"): [
            LspLocation(path="a.py", line=2, character=0),
            LspLocation(path="b.py", line=3, character=0),
        ],
        ("mod.py", 4, "Thing"): [
            LspLocation(path="b.py", line=5, character=0),
        ],
    }

    def factory(root: Path, language: str, command: list[str]) -> Any:
        return _FakeClient(root, language, command, refs=refs)

    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.first_available_command",
        lambda language: ["fake-ls"],
    )
    with LspSessionPool(tmp_path, client_factory=factory) as pool:
        deps = pool.dependents_of_file("mod.py")
    assert deps == ["a.py", "b.py"]


def test_pool_shutdown_on_close(tmp_path, monkeypatch):
    (tmp_path / "mod.py").write_text("def x():\n    pass\n", encoding="utf-8")
    clients: list[_FakeClient] = []

    def factory(root: Path, language: str, command: list[str]) -> Any:
        client = _FakeClient(root, language, command)
        clients.append(client)
        return client

    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.first_available_command",
        lambda language: ["fake-ls"],
    )
    pool = LspSessionPool(tmp_path, client_factory=factory)
    assert pool.client_for("mod.py") is not None
    pool.close()
    assert clients[0].shutdown_called is True


def test_repo_mapper_lsp_refs_filters_dead(tmp_path, monkeypatch):
    """Light hook: map dead-symbol path confirms via LSP filter."""
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "mod.py").write_text("def unused():\n    return 1\n", encoding="utf-8")

    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.filter_dead_symbols_with_lsp",
        lambda root, cands, pool=None: [c for c in cands if "unused" not in c],
    )

    from devcouncil.indexing.repo_mapper import RepoMapper

    mapper = RepoMapper(tmp_path)
    dead = mapper._dead_symbol_candidates(["pkg/mod.py"], lsp_refs=True)
    assert not any("unused" in d for d in dead)
    # Without lsp_refs, token-scan still reports it.
    dead_raw = mapper._dead_symbol_candidates(["pkg/mod.py"], lsp_refs=False)
    assert any("unused" in d for d in dead_raw)


@pytest.mark.anyio
async def test_mcp_impact_precise_uses_lsp(tmp_path, monkeypatch):
    import json

    from devcouncil.integrations.mcp.server import call_tool

    payload = {
        "languages": ["python"],
        "files": [{"path": "mod.py", "area": "src", "kind": "code", "summary": ""}],
        "subsystems": [
            {
                "area": "src",
                "summary": "src",
                "entry_points": [],
                "critical_files": [],
                "neighbors": [],
                "handoff_paths": [],
                "role_files": {},
            }
        ],
        "dependents": {"mod.py": ["import_only.py"]},
        "generated_head": "abc",
        "indexed_hash": "h",
    }
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text(json.dumps(payload), encoding="utf-8")
    (tmp_path / "mod.py").write_text("def helper():\n    return 1\n", encoding="utf-8")

    refs = {
        ("mod.py", 1, "helper"): [
            LspLocation(path="lsp_user.py", line=1, character=0),
        ],
    }

    def factory(root: Path, language: str, command: list[str]) -> Any:
        return _FakeClient(root, language, command, refs=refs)

    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )
    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.first_available_command",
        lambda language: ["fake-ls"],
    )

    original_init = LspSessionPool.__init__

    def patched_init(self, project_root, **kw):
        original_init(self, project_root, client_factory=factory, **{
            k: v for k, v in kw.items() if k != "client_factory"
        })

    monkeypatch.setattr(LspSessionPool, "__init__", patched_init)

    result = await call_tool(
        "devcouncil_impact",
        {"paths": ["mod.py"], "precise": True},
    )
    body = json.loads(result[0].text)
    assert body["ok"] is True
    assert body["precise"] is True
    assert body["paths"][0]["dependents"] == ["lsp_user.py"]
    assert body["paths"][0]["resolution"] == "lsp"
    # Shape preserved.
    assert "neighbor_areas" in body
    assert "cross_boundary_pairs" in body
    assert "stale" in body


@pytest.mark.anyio
async def test_mcp_impact_precise_false_keeps_import_dependents(tmp_path, monkeypatch):
    import json

    from devcouncil.integrations.mcp.server import call_tool

    payload = {
        "languages": ["python"],
        "files": [{"path": "mod.py", "area": "src", "kind": "code", "summary": ""}],
        "subsystems": [],
        "dependents": {"mod.py": ["import_only.py"]},
        "generated_head": "abc",
        "indexed_hash": "h",
    }
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text(json.dumps(payload), encoding="utf-8")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.map.RepoMapper.map_is_stale",
        lambda self, data: False,
    )
    body = json.loads(
        (await call_tool("devcouncil_impact", {"paths": ["mod.py"], "precise": False}))[0].text
    )
    assert body["paths"][0]["dependents"] == ["import_only.py"]
    assert "resolution" not in body["paths"][0]


def test_dead_symbol_gate_clears_with_lsp(tmp_path, monkeypatch):
    from devcouncil.domain.task import Task
    from devcouncil.verification.checks.dead_symbols import detect_dead_symbol_gaps

    (tmp_path / "mod.py").write_text("def helper():\n    return 1\n", encoding="utf-8")
    diff = (
        "diff --git a/mod.py b/mod.py\n"
        "--- a/mod.py\n"
        "+++ b/mod.py\n"
        "@@ -0,0 +1,2 @@\n"
        "+def helper():\n"
        "+    return 1\n"
    )
    refs = {
        ("mod.py", 1, "helper"): [
            LspLocation(path="other.py", line=1, character=0),
        ],
    }

    def factory(root: Path, language: str, command: list[str]) -> Any:
        return _FakeClient(root, language, command, refs=refs)

    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.first_available_command",
        lambda language: ["fake-ls"],
    )
    original_init = LspSessionPool.__init__

    def patched_init(self, project_root, **kw):
        original_init(self, project_root, client_factory=factory, **{
            k: v for k, v in kw.items() if k != "client_factory"
        })

    monkeypatch.setattr(LspSessionPool, "__init__", patched_init)

    task = Task(id="t1", title="x", description="y", status="running")
    gaps = detect_dead_symbol_gaps(
        task=task,
        project_root=tmp_path,
        diff_content=diff,
        next_gap_id=lambda tid, p: f"{tid}-{p}-1",
        dead_symbol_blocking=True,
        git_files=["mod.py"],
        lsp_refs=True,
    )
    assert not any(g.gap_type == "dead_symbol" and g.blocking for g in gaps)


def test_uri_roundtrip_helpers(tmp_path):
    from devcouncil.indexing.lsp_client import _path_to_uri, _uri_to_rel

    (tmp_path / "a.py").write_text("x=1\n", encoding="utf-8")
    uri = _path_to_uri(tmp_path, "a.py")
    assert _uri_to_rel(tmp_path, uri) == "a.py"


def test_lsp_client_class_exists_for_real_servers():
    # Smoke: constructor does not spawn until start().
    client = LspClient(Path("."), "python", ["pyright-langserver", "--stdio"])
    assert client.language == "python"
    assert client._alive is False
