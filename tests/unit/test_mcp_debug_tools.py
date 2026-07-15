from __future__ import annotations

import asyncio
import json
from pathlib import Path

import yaml

from devcouncil.integrations.mcp.handlers import debug


def _payload(result) -> dict:
    return json.loads(result[0].text)


def test_debug_discovery_requires_and_persists_explicit_consent(tmp_path: Path) -> None:
    config = tmp_path / ".devcouncil" / "config.yaml"
    config.parent.mkdir()
    config.write_text("project:\n  name: fixture\n", encoding="utf-8")

    refused = asyncio.run(debug.dispatch("devcouncil_debug_discover", tmp_path, {}))
    assert _payload(refused)["code"] == "debug_consent_required"

    allowed = asyncio.run(debug.dispatch(
        "devcouncil_debug_discover", tmp_path, {"consent": True}
    ))
    payload = _payload(allowed)
    assert payload["consent"] is True
    raw = yaml.safe_load(config.read_text(encoding="utf-8"))
    assert raw["code_intelligence"]["debug"]["auto_discover"] is True


def test_debug_tool_registry_separates_evaluate_and_trace() -> None:
    names = {tool.name for tool in debug.tools()}
    assert {
        "devcouncil_debug_start",
        "devcouncil_debug_inspect",
        "devcouncil_debug_evaluate",
        "devcouncil_debug_trace",
        "devcouncil_debug_stop",
    } <= names
    evaluate = next(tool for tool in debug.tools() if tool.name == "devcouncil_debug_evaluate")
    assert evaluate.inputSchema["properties"]["allowSideEffects"]["const"] is True
