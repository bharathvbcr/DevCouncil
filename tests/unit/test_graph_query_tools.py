"""CLI + MCP graph query/trace tools."""

from __future__ import annotations

import asyncio
import json
import subprocess

import pytest
from typer.testing import CliRunner

from devcouncil.cli.commands.graph_cmd import app as graph_app
from devcouncil.indexing.graph.build import build_code_graph, write_code_graph
from devcouncil.integrations.mcp.handlers import map as map_handlers


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


@pytest.fixture
def mapped(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": "from pkg import util\ndef main():\n    util.run()\n",
        "pkg/util.py": "def run():\n    return 1\n",
    })
    _commit(tmp_path)
    write_code_graph(tmp_path, build_code_graph(tmp_path))
    return tmp_path


def test_cli_graph_query(mapped):
    runner = CliRunner()
    result = runner.invoke(
        graph_app, ["query", "run", "--project-root", str(mapped), "--json"]
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data.get("matches", 0) >= 1


def test_cli_graph_trace(mapped):
    runner = CliRunner()
    result = runner.invoke(
        graph_app,
        ["trace", "pkg/main.py", "pkg/util.py", "--project-root", str(mapped), "--json"],
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data.get("found") is True


def test_cli_graph_dead(mapped):
    runner = CliRunner()
    result = runner.invoke(
        graph_app, ["dead", "--project-root", str(mapped), "--json"]
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert isinstance(data, list)


def test_mcp_graph_query(mapped):
    contents = asyncio.run(
        map_handlers.handle_graph_query(mapped, {"name_or_path": "run"})
    )
    payload = json.loads(contents[0].text)
    assert payload["ok"] is True
    assert payload.get("matches", 0) >= 1


def test_mcp_graph_trace(mapped):
    contents = asyncio.run(
        map_handlers.handle_graph_trace(
            mapped, {"from": "pkg/main.py", "to": "pkg/util.py"}
        )
    )
    payload = json.loads(contents[0].text)
    assert payload["ok"] is True
    assert payload.get("found") is True


@pytest.fixture
def api_repo(tmp_path):
    _write(
        tmp_path,
        {
            "pyproject.toml": '[project]\nname="t"\nversion="0"\n',
            "api/routes.py": (
                "from fastapi import APIRouter\n"
                "router = APIRouter()\n"
                "\n"
                "@router.get('/api/items')\n"
                "def list_items():\n"
                '    return {"id": 1, "name": "widget", "status": "ok"}\n'
                "\n"
                "@router.get('/api/users/{user_id}')\n"
                "def get_user(user_id: int):\n"
                '    return {"id": user_id, "name": "alice"}\n'
            ),
            "web/client.ts": (
                "export async function loadItems() {\n"
                "  const res = await fetch('/api/items');\n"
                "  const data = await res.json();\n"
                "  return data.id + data.name + data.price;\n"
                "}\n"
                "\n"
                "export async function loadUser(id: number) {\n"
                "  const resp = await fetch(`/api/users/${id}`);\n"
                "  return resp.json();\n"
                "}\n"
            ),
        },
    )
    _commit(tmp_path)
    write_code_graph(tmp_path, build_code_graph(tmp_path))
    return tmp_path


def test_api_route_map_links_handlers_and_consumers(api_repo):
    from devcouncil.indexing.graph.api_routes import route_map

    result = route_map(api_repo)
    routes = {r["path"]: r for r in result["routes"]}
    assert "/api/items" in routes
    items = routes["/api/items"]
    assert items["verb"] == "GET"
    assert any(h["name"] == "list_items" for h in items["handlers"])
    assert any(c["path"] == "web/client.ts" for c in items["consumers"])


def test_normalize_route_path_template_literal_segments():
    from devcouncil.indexing.graph.api_routes import normalize_route_path, paths_match

    assert normalize_route_path("/api/users/${id}") == "/api/users/*"
    assert paths_match("/api/users/{user_id}", "/api/users/${id}")


def test_api_route_map_matches_template_literal_fetch(api_repo):
    from devcouncil.indexing.graph.api_routes import route_map

    result = route_map(api_repo)
    routes = {r["path"]: r for r in result["routes"]}
    users = routes["/api/users/{user_id}"]
    assert any(c["url"] == "/api/users/${id}" for c in users["consumers"])


def test_api_shape_check_flags_missing_handler_keys(api_repo):
    from devcouncil.indexing.graph.api_routes import shape_check

    result = shape_check(api_repo, route_filter="/api/items")
    assert result["mismatch_count"] >= 1
    assert "price" in result["checks"][0]["missing_in_handler"]


def test_api_impact_reports_risk(api_repo):
    from devcouncil.indexing.graph.api_routes import api_impact

    result = api_impact(api_repo, "/api/items")
    assert result["found"] is True
    assert result["risk"] in {"medium", "high"}
    assert result["shape_mismatches"]


def test_cli_graph_routes_command(api_repo):
    runner = CliRunner()
    result = runner.invoke(
        graph_app, ["routes", "--project-root", str(api_repo), "--json"]
    )
    assert result.exit_code == 0
    assert json.loads(result.stdout).get("count", 0) >= 1


def test_mcp_route_map(api_repo):
    contents = asyncio.run(map_handlers.handle_route_map(api_repo, {}))
    payload = json.loads(contents[0].text)
    assert payload["ok"] is True
    assert payload.get("count", 0) >= 1
