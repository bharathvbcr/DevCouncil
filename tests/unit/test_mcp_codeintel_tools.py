from __future__ import annotations

import asyncio
import json
from pathlib import Path

from devcouncil.codeintel.service import get_codeintel_service
from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind
from devcouncil.integrations.mcp.handlers import codeintel


def _payload(result) -> dict:
    return json.loads(result[0].text)


def test_registry_tools_resolve_explicit_project_path(tmp_path: Path) -> None:
    """An explicit ``projectPath`` selects a project — inside the server root.

    This used to point at a *sibling* of the server root and assert that the
    sibling answered. That is the escape ``resolve_root`` now closes, so the
    same intent is exercised against a nested project the server legitimately
    owns; the refusal of the sibling is asserted below.
    """
    first = tmp_path / "first"
    second = first / "packages" / "second"
    first.mkdir()
    second.mkdir(parents=True)
    for root, name in ((first, "alpha"), (second, "beta")):
        (root / ".devcouncil").mkdir(exist_ok=True)
        source = root / "app.py"
        source.write_text(f"def {name}():\n    pass\n", encoding="utf-8")
        get_codeintel_service(root).persist(CodeGraph(nodes=[
            GraphNode(id="app.py", kind=NodeKind.FILE, path="app.py", name="app.py", language="python"),
            GraphNode(id=f"app.py::{name}", kind=NodeKind.FUNCTION, path="app.py", name=name, line=1, end_line=2, language="python"),
        ]))

    result = asyncio.run(
        codeintel.dispatch(
            "devcouncil_code_explore",
            first,
            {"projectPath": str(second), "query": "beta"},
        )
    )
    payload = _payload(result)
    assert payload["project_root"] == str(second.resolve())
    assert payload["definitions"][0]["name"] == "beta"


def test_registry_tools_refuse_a_project_path_outside_the_server_root(tmp_path: Path) -> None:
    first = tmp_path / "first"
    sibling = tmp_path / "sibling"
    for root in (first, sibling):
        (root / ".devcouncil").mkdir(parents=True)

    result = asyncio.run(
        codeintel.dispatch(
            "devcouncil_code_explore",
            first,
            {"projectPath": str(sibling), "query": "beta"},
        )
    )
    payload = _payload(result)
    assert payload["ok"] is False
    assert payload["code"] == "project_path_outside_root"


def test_registry_status_and_specs(tmp_path: Path) -> None:
    result = asyncio.run(codeintel.dispatch("devcouncil_code_status", tmp_path, {}))
    assert _payload(result)["state"] == "uninitialized"
    names = {tool.name for tool in codeintel.tools()}
    assert {"devcouncil_code_explore", "devcouncil_code_sync", "devcouncil_code_affected_tests"} <= names
