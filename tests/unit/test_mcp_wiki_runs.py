"""The codebase wiki and Shepherd-style run traces are surfaced over the MCP server."""

import json
import subprocess
from pathlib import Path

import pytest

from devcouncil.execution.checkpoints import CheckpointService
from devcouncil.integrations.mcp.server import call_tool


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _payload(result) -> dict:
    return json.loads(result[0].text)


def _seed_wiki(root: Path) -> None:
    from devcouncil.indexing.repo_mapper import RepoFileEntry, RepoMap, RepoSubsystem
    from devcouncil.knowledge.wiki import generate_wiki

    repo_map = RepoMap(
        languages=["python"], frameworks=[], package_managers=["uv"],
        test_commands=["pytest"], important_files=[], candidate_files=[],
        files=[RepoFileEntry(path="src/pkg/api/routes.py", area="src/pkg/api",
                             kind="code", language="python", summary="HTTP routes")],
        subsystems=[RepoSubsystem(
            area="src/pkg/api", summary="HTTP API surface.",
            entry_points=["src/pkg/api/routes.py"], critical_files=["src/pkg/api/routes.py"],
            neighbors=[], handoff_paths=[], role_files={},
        )],
    )
    wiki_dir = root / ".devcouncil" / "knowledge" / "okf" / "wiki"
    generate_wiki(root, repo_map, wiki_dir, project_name="Demo")


def _seed_run(root: Path, returncode: int = 0, status: str = "finished") -> None:
    subprocess.check_call(["git", "init"], cwd=root)
    subprocess.check_call(["git", "config", "user.email", "t@e.com"], cwd=root)
    subprocess.check_call(["git", "config", "user.name", "T"], cwd=root)
    (root / "README.md").write_text("hello\n", encoding="utf-8")
    subprocess.check_call(["git", "add", "README.md"], cwd=root)
    subprocess.check_call(["git", "commit", "-m", "init"], cwd=root)

    run_dir = root / ".devcouncil" / "runs" / "RUN-1"
    run_dir.mkdir(parents=True)
    (run_dir / "agent-run.json").write_text(json.dumps({
        "run_id": "RUN-1", "task_id": "TASK-1", "status": status, "returncode": returncode,
    }), encoding="utf-8")

    service = CheckpointService(root)
    service.create_before("TASK-1")
    (root / "README.md").write_text("changed\n", encoding="utf-8")
    service.create_after("TASK-1")


@pytest.mark.anyio
async def test_wiki_page_index_page_and_query(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _seed_wiki(tmp_path)

    listing = _payload(await call_tool("devcouncil_wiki_page", {}))
    assert listing["ok"] is True
    assert any(p["page"] == "subsystems/src-pkg-api.md" for p in listing["pages"])

    page = _payload(await call_tool("devcouncil_wiki_page", {"page": "subsystems/src-pkg-api.md"}))
    assert page["ok"] is True
    assert page["type"] == "Subsystem"
    assert "HTTP API" in page["body"] or "src/pkg/api" in page["body"]

    hit = _payload(await call_tool("devcouncil_wiki_page", {"query": "api routes"}))
    assert hit["ok"] is True
    assert hit["page"] == "subsystems/src-pkg-api.md"


@pytest.mark.anyio
async def test_wiki_page_without_wiki_reports_not_found(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    missing = _payload(await call_tool("devcouncil_wiki_page", {}))
    assert missing["ok"] is False
    assert missing["code"] == "not_found"


@pytest.mark.anyio
async def test_run_timeline_over_mcp(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _seed_run(tmp_path)

    timeline = _payload(await call_tool("devcouncil_run_timeline", {"reference": "RUN-1"}))
    assert timeline["ok"] is True
    assert timeline["task_id"] == "TASK-1"
    assert timeline["reversible"] is True
    assert {c["stage"] for c in timeline["checkpoints"]} >= {"before", "after"}

    unknown = _payload(await call_tool("devcouncil_run_timeline", {"reference": "NOPE"}))
    assert unknown["ok"] is False


@pytest.mark.anyio
async def test_run_supervise_over_mcp_degrades_to_heuristics(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    _seed_run(tmp_path, returncode=2, status="failed")

    verdict = _payload(await call_tool("devcouncil_run_supervise", {"reference": "TASK-1"}))
    assert verdict["ok"] is True
    assert verdict["verdict"] == "revert"
    assert verdict["source"] == "heuristic"
    assert verdict["reversible"] is True
    # Read-only: supervision must not have touched the workspace.
    assert (tmp_path / "README.md").read_text(encoding="utf-8") == "changed\n"
