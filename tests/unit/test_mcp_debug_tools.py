from __future__ import annotations

import asyncio
import json
from pathlib import Path

import yaml

from devcouncil.integrations.mcp.handlers import debug


def _payload(result) -> dict:
    return json.loads(result[0].text)


def test_debug_discovery_requires_consent_the_caller_cannot_grant(tmp_path: Path) -> None:
    """Consent comes from configuration the user set, never from an argument.

    This test previously asserted the opposite — that ``{"consent": true}``
    *persists* ``auto_discover: true`` into ``.devcouncil/config.yaml`` — and so
    pinned the defect rather than the contract. One tool argument flipped the
    persistent gate that protects the other seven debug tools, process launch
    and script execution among them, and nothing outside the argument dict was
    consulted. The expectation was wrong, not the code that now refuses it.
    """
    config = tmp_path / ".devcouncil" / "config.yaml"
    config.parent.mkdir()
    config.write_text("project:\n  name: fixture\n", encoding="utf-8")

    refused = asyncio.run(debug.dispatch("devcouncil_debug_discover", tmp_path, {}))
    assert _payload(refused)["code"] == "debug_consent_required"

    self_granted = _payload(asyncio.run(debug.dispatch(
        "devcouncil_debug_discover", tmp_path, {"consent": True}
    )))
    assert self_granted["code"] == "debug_consent_required"
    assert "code_intelligence" not in (yaml.safe_load(config.read_text(encoding="utf-8")) or {})

    # Consent the user configured is honoured, unchanged.
    config.write_text(
        "project:\n  name: fixture\ncode_intelligence:\n  debug:\n    auto_discover: true\n",
        encoding="utf-8",
    )
    allowed = _payload(asyncio.run(debug.dispatch("devcouncil_debug_discover", tmp_path, {})))
    assert allowed["consent"] is True


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
    assert evaluate.input_schema["properties"]["allowSideEffects"]["const"] is True
