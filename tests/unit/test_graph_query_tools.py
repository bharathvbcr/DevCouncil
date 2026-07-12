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
