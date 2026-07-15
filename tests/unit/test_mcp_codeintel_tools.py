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
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    for root, name in ((first, "alpha"), (second, "beta")):
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


def test_registry_status_and_specs(tmp_path: Path) -> None:
    result = asyncio.run(codeintel.dispatch("devcouncil_code_status", tmp_path, {}))
    assert _payload(result)["state"] == "uninitialized"
    names = {tool.name for tool in codeintel.tools()}
    assert {"devcouncil_code_explore", "devcouncil_code_sync", "devcouncil_code_affected_tests"} <= names
