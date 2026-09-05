from __future__ import annotations

import asyncio
import json
from pathlib import Path

from devcouncil.integrations.mcp.handlers import codeintel


def _payload(result) -> dict:
    return json.loads(result[0].text)


def test_registry_tools_resolve_explicit_project_path(
    tmp_path: Path, monkeypatch
) -> None:
    """An explicit ``projectPath`` selects a project — inside the server root.

    This used to point at a *sibling* of the server root and assert that the
    sibling answered. That is the escape ``resolve_root`` now closes, so the
    same intent is exercised against a nested project the server legitimately
    owns; the refusal of the sibling is asserted below.

    The evidence changed with the engine. It used to persist a Python
    ``CodeGraph`` per project and read the answer back, which only worked while
    ``explore`` ran on the Python engine. The kernel stub below is keyed on the
    root it is handed, so the assertion still proves *which project answered* —
    which is the whole point of the test — rather than merely that some answer
    came back.
    """
    first = tmp_path / "first"
    second = first / "packages" / "second"
    first.mkdir()
    second.mkdir(parents=True)
    for root, name in ((first, "alpha"), (second, "beta")):
        (root / ".devcouncil").mkdir(exist_ok=True)
        (root / "app.py").write_text(f"def {name}():\n    pass\n", encoding="utf-8")

    def _budgeted(items):
        return {
            "items": items,
            "shown": len(items),
            "hidden": 0,
            "total": len(items),
            "truncated": False,
            "tokens_used": 0,
            "resolution": "Available",
        }

    class _RootScopedKernel:
        """Answers with the name of the project it was actually asked about."""

        def __init__(self, root: Path) -> None:
            self._name = "beta" if root.name == "second" else "alpha"

        def explore(self, query, limit=20, **_kwargs):
            return {
                "query": query,
                "limit": limit,
                "definitions": _budgeted([{
                    "id": f"app.py::{self._name}",
                    "symbol_name": self._name,
                    "qualified_name": self._name,
                    "file_path": "app.py",
                    "kind": "Function",
                    "span": [1, 2],
                    "source": f"def {self._name}():",
                    "score": 1.0,
                    "callers": _budgeted([]),
                    "callees": _budgeted([]),
                }]),
                "blast_radius": {
                    "seeds": [], "unmatched_targets": [],
                    "layers": _budgeted([]), "total_impacted": 0,
                },
                "budget": {"total": 8000, "definitions": 4000,
                           "edges_per_direction": 500, "blast_radius": 2000},
            }

        def status(self):
            import types

            return types.SimpleNamespace(
                generation_id=1, pending_count=0, node_count=2, edge_count=0,
                is_fresh=True, degraded_reason=None, quarantined_count=0,
                raw={"schema_version": 12, "analyzer_version": "devmap"},
            )

    monkeypatch.setattr(codeintel, "try_connect", lambda root: _RootScopedKernel(root))

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
