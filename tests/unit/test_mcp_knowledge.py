"""Ingested project knowledge (OKF + design) is surfaced over the MCP server.

These tests pin the resource listing/reads and the devcouncil_select_knowledge tool so a
coding agent (Claude Code, Codex, ...) can pull what project knowledge applies to a goal.
"""

import json

import pytest
from pydantic import AnyUrl

from devcouncil.integrations.mcp.server import call_tool, list_resources, read_resource


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _seed_knowledge(tmp_path):
    """Write one OKF doc (keyword-matchable via tags) and one always-on design system."""
    okf_dir = tmp_path / ".devcouncil" / "knowledge" / "okf"
    design_dir = tmp_path / ".devcouncil" / "knowledge" / "design"
    okf_dir.mkdir(parents=True)
    design_dir.mkdir(parents=True)
    (okf_dir / "payments.md").write_text(
        "---\n"
        "type: Engineering Skill\n"
        "title: Payments domain\n"
        "description: Payments domain knowledge\n"
        "tags:\n"
        "  - payments\n"
        "  - billing\n"
        "---\n\n"
        "Charge via the PaymentGateway service and never store raw card numbers.\n",
        encoding="utf-8",
    )
    (design_dir / "design.md").write_text(
        "---\n"
        "description: Acme design system\n"
        "---\n\n"
        "Primary color is #0A66C2. Use the spacing scale for all layout.\n",
        encoding="utf-8",
    )


@pytest.mark.anyio
async def test_list_resources_includes_knowledge_when_ingested(tmp_path, monkeypatch):
    _seed_knowledge(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    uris = {str(r.uri).rstrip("/") for r in await list_resources()}
    assert "devcouncil://knowledge" in uris
    # Per-source resources for both the OKF doc and the design system.
    assert any(u.startswith("devcouncil://knowledge/okf/") for u in uris)
    assert any(u.startswith("devcouncil://knowledge/design/") for u in uris)


@pytest.mark.anyio
async def test_list_resources_omits_knowledge_when_none(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    uris = {str(r.uri).rstrip("/") for r in await list_resources()}
    assert not any(u.startswith("devcouncil://knowledge") for u in uris)


@pytest.mark.anyio
async def test_read_knowledge_index_and_per_source(tmp_path, monkeypatch):
    _seed_knowledge(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    index = await read_resource(AnyUrl("devcouncil://knowledge"))
    assert isinstance(index, str)
    assert "Payments domain knowledge" in index
    assert "Acme design system" in index

    # Find the OKF per-source URI from the listing and read it back.
    okf_uri = next(
        str(r.uri)
        for r in await list_resources()
        if str(r.uri).startswith("devcouncil://knowledge/okf/")
    )
    body = await read_resource(AnyUrl(okf_uri))
    assert "PaymentGateway" in body


@pytest.mark.anyio
async def test_select_knowledge_tool_returns_matching_source(tmp_path, monkeypatch):
    _seed_knowledge(tmp_path)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_select_knowledge", {"goal": "add a payments refund flow"})
    payload = json.loads(result[0].text)
    assert payload["ok"] is True
    kinds = {s["kind"] for s in payload["sources"]}
    # OKF matched on the 'payments' tag; design is always-on.
    assert "okf" in kinds
    assert "design" in kinds
    assert "PaymentGateway" in payload["preamble"]


@pytest.mark.anyio
async def test_select_knowledge_requires_goal(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    result = await call_tool("devcouncil_select_knowledge", {})
    payload = json.loads(result[0].text)
    assert payload["ok"] is False
    assert payload["code"] == "missing_argument"
