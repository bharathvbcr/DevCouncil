"""Wave-4 stable coverage: hook emit, gap_ids, github retry, scaffold, trace, compiler."""

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import httpx
import pytest
import typer
from typer.testing import CliRunner

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.gap import Gap
from devcouncil.execution.stop_gate import StopGateResult


runner = CliRunner()


def test_gap_ids_normalize_prefers_blocking_and_severity():
    from devcouncil.verification.gap_ids import normalize_verify_gaps, stable_gap_id

    assert "GAP-" in stable_gap_id("T1", "missing_test", "a.py")
    soft = Gap(
        id="G1",
        severity="low",
        gap_type="missing_test",
        description="same",
        recommended_fix="x",
        blocking=False,
        file="a.py",
    )
    hard = Gap(
        id="G2",
        severity="high",
        gap_type="missing_test",
        description="same",
        recommended_fix="x",
        blocking=True,
        file="a.py",
    )
    harder = Gap(
        id="G3",
        severity="critical",
        gap_type="missing_test",
        description="same",
        recommended_fix="x",
        blocking=True,
        file="a.py",
    )
    out = normalize_verify_gaps([soft, hard, harder])
    assert len(out) == 1
    assert out[0].severity == "critical"

    mid = Gap(
        id="G4",
        severity="medium",
        gap_type="missing_test",
        description="same",
        recommended_fix="x",
        blocking=True,
        file="a.py",
    )
    out2 = normalize_verify_gaps([harder, mid])
    assert out2[0].severity == "critical"


def test_hook_emit_stop_result_branches(capsys):
    from devcouncil.cli.commands import hook as hook_cmd

    hook_cmd._emit_stop_result("claude", "not-a-result")
    assert capsys.readouterr().out == ""

    blocked = StopGateResult(decision="block", reason="nope", system_message="sys")
    hook_cmd._emit_stop_result("codex", blocked)
    out = capsys.readouterr().out
    assert "continue" in out and "stopReason" in out and "systemMessage" in out

    hook_cmd._emit_stop_result("claude", blocked)
    out = capsys.readouterr().out
    assert "decision" in out and "block" in out

    allowed = StopGateResult(decision="pass", reason="", system_message="ok")
    hook_cmd._emit_stop_result("gemini", allowed)
    out = capsys.readouterr().out
    assert "suppressOutput" in out

    hook_cmd._emit_stop_result("claude", allowed)
    out = capsys.readouterr().out
    assert "systemMessage" in out

    hook_cmd._emit_stop_result("claude", StopGateResult(decision="pass", reason=""))
    assert capsys.readouterr().out == ""


def test_hook_project_root_and_active_task(tmp_path, monkeypatch):
    from devcouncil.cli.commands import hook as hook_cmd

    monkeypatch.delenv("DEVCOUNCIL_PROJECT_ROOT", raising=False)
    root = hook_cmd._project_root(tmp_path)
    assert root == tmp_path.resolve()

    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    root2 = hook_cmd._project_root(None)
    assert root2 == tmp_path.resolve()

    monkeypatch.setattr(hook_cmd, "active_task_id", lambda _r: None)
    assert hook_cmd._active_task(tmp_path) is None

    monkeypatch.setattr(hook_cmd, "active_task_id", lambda _r: "TASK-1")
    monkeypatch.setattr(hook_cmd, "get_db", lambda _r: None)
    assert hook_cmd._active_task(tmp_path) is None


def test_scaffold_ci_command_paths(tmp_path, monkeypatch):
    from devcouncil.cli.commands import scaffold as sc

    with pytest.raises(typer.Exit) as exc:
        sc.scaffold_ci_command(project_root=tmp_path, force=False, evidence=False)
    assert exc.value.exit_code == 1

    (tmp_path / ".devcouncil").mkdir()
    monkeypatch.setattr(sc, "scaffold_ci", lambda *a, **k: None)
    sc.scaffold_ci_command(project_root=tmp_path, force=False, evidence=False)

    written = tmp_path / ".github" / "workflows" / "devcouncil.yml"
    written.parent.mkdir(parents=True, exist_ok=True)
    written.write_text("name: x\n", encoding="utf-8")
    monkeypatch.setattr(sc, "scaffold_ci", lambda *a, **k: written)
    monkeypatch.setattr(sc, "detect_stacks", lambda _r: {"python"})
    monkeypatch.setattr(sc, "scaffold_evidence_ci", lambda *a, **k: None)
    sc.scaffold_ci_command(project_root=tmp_path, force=True, evidence=True)

    evidence = tmp_path / ".github" / "workflows" / "devcouncil-evidence.yml"
    evidence.write_text("name: e\n", encoding="utf-8")
    monkeypatch.setattr(sc, "scaffold_evidence_ci", lambda *a, **k: evidence)
    sc.scaffold_ci_command(project_root=tmp_path, force=True, evidence=True)


def test_trace_incremental_modes(tmp_path, monkeypatch):
    from devcouncil.cli.commands import trace as tr

    class Ev:
        timestamp = "t"
        type = "step"
        task_id = "T1"
        summary = "hello"
        details = {"a": 1}

        def model_dump(self, by_alias=True):
            return {"type": self.type, "summary": self.summary}

        def model_dump_json(self):
            return '{"type":"step"}'

    monkeypatch.setattr(tr, "read_trace_events_since", lambda root, since: ([Ev()], 12))
    monkeypatch.setattr(tr, "read_trace_events", lambda root: [Ev(), Ev()])

    # Call the command function directly to avoid Typer option quirks.
    tr.tail(
        follow=False,
        limit=50,
        jsonl=True,
        since=None,
        json_summary=True,
        project_root=tmp_path,
    )
    tr.tail(
        follow=False,
        limit=50,
        jsonl=False,
        since=0,
        json_summary=False,
        project_root=tmp_path,
    )
    tr.tail(
        follow=False,
        limit=1,
        jsonl=True,
        since=None,
        json_summary=False,
        project_root=tmp_path,
    )


def test_github_post_with_retry(monkeypatch):
    from devcouncil.integrations.github import GitHubIntegration

    gh = GitHubIntegration("tok", "org/repo", "abc123")
    monkeypatch.setattr("devcouncil.integrations.github.asyncio.sleep", AsyncMock())

    class Resp:
        def __init__(self, code):
            self.status_code = code

        def raise_for_status(self):
            if self.status_code >= 400:
                raise httpx.HTTPStatusError("err", request=MagicMock(), response=MagicMock())

    client = AsyncMock()
    client.post = AsyncMock(side_effect=[Resp(503), Resp(200)])
    out = asyncio.run(
        gh._post_with_retry(client, "https://example/check", headers={}, json={})
    )
    assert out.status_code == 200

    client.post = AsyncMock(side_effect=httpx.TimeoutException("t"))
    with pytest.raises(httpx.TimeoutException):
        asyncio.run(
            gh._post_with_retry(
                client, "https://example/check", headers={}, json={}, max_attempts=1
            )
        )

    client.post = AsyncMock(side_effect=httpx.ConnectError("c"))
    with pytest.raises(httpx.ConnectError):
        asyncio.run(
            gh._post_with_retry(
                client, "https://example/check", headers={}, json={}, max_attempts=1
            )
        )

    class CM:
        async def __aenter__(self):
            return client

        async def __aexit__(self, *a):
            return False

    monkeypatch.setattr(httpx, "AsyncClient", lambda: CM())
    client.post = AsyncMock(return_value=Resp(200))
    monkeypatch.setattr(
        "devcouncil.reporting.github_check.GitHubCheckGenerator.generate",
        lambda graph: {"name": "DevCouncil"},
    )
    asyncio.run(gh.report_verification(ArtifactGraph()))

    client.post = AsyncMock(side_effect=RuntimeError("fail"))
    with pytest.raises(RuntimeError):
        asyncio.run(gh.report_verification(ArtifactGraph()))


def test_acceptance_compiler_repair():
    from devcouncil.verification.acceptance_compiler import (
        AcceptanceTestCompiler,
        CompiledCheck,
        CompiledChecks,
    )

    class Router:
        async def complete_structured(self, role, messages, schema, fallback=None):
            return CompiledChecks(
                checks=[
                    CompiledCheck(
                        acceptance_criterion_id="AC-1",
                        command="python -c 'print(1)'",
                    )
                ]
            )

    compiler = AcceptanceTestCompiler(Router())  # type: ignore[arg-type]
    fixed = asyncio.run(
        compiler.repair(
            "AC-1",
            "works",
            "python -c 'bad'",
            "SyntaxError",
            "code",
        )
    )
    assert fixed == "python -c 'print(1)'"

    class BadRouter:
        async def complete_structured(self, *a, **k):
            raise RuntimeError("nope")

    compiler2 = AcceptanceTestCompiler(BadRouter())  # type: ignore[arg-type]
    assert asyncio.run(compiler2.repair("AC-1", "d", "c", "e", "ctx")) is None

    class EmptyRouter:
        async def complete_structured(self, *a, **k):
            return CompiledChecks(checks=[])

    compiler3 = AcceptanceTestCompiler(EmptyRouter())  # type: ignore[arg-type]
    assert asyncio.run(compiler3.repair("AC-1", "d", "c", "e", "ctx")) is None


def test_design_resolve_and_discover(tmp_path):
    from devcouncil.cli.commands import design as design_cmd

    assert design_cmd._resolve_path(None, tmp_path) is None
    nested = tmp_path / ".devcouncil" / "knowledge" / "design"
    nested.mkdir(parents=True)
    design = nested / "design.md"
    design.write_text("# Design\n", encoding="utf-8")
    assert design_cmd._resolve_path(None, tmp_path) == design
    explicit = tmp_path / "custom-design.md"
    explicit.write_text("# C\n", encoding="utf-8")
    assert design_cmd._resolve_path(explicit, tmp_path) == explicit
    assert design_cmd._resolve_path(tmp_path / "missing.md", tmp_path) is None

    styles = tmp_path / "web"
    styles.mkdir()
    (styles / "a.css").write_text("body{}", encoding="utf-8")
    (styles / "node_modules").mkdir()
    (styles / "node_modules" / "x.css").write_text("x{}", encoding="utf-8")
    found = design_cmd._discover_style_files(tmp_path)
    assert any(p.name == "a.css" for p in found)
    assert not any("node_modules" in str(p) for p in found)

    original = design_cmd._MAX_DISCOVERED_FILES
    design_cmd._MAX_DISCOVERED_FILES = 1
    try:
        capped = design_cmd._discover_style_files(tmp_path)
        assert len(capped) <= 1
    finally:
        design_cmd._MAX_DISCOVERED_FILES = original


def test_mcp_prompt_renderers(tmp_path, monkeypatch):
    from devcouncil.integrations.mcp.handlers import prompts as pr

    monkeypatch.setattr(pr, "get_db", lambda _r: None)
    assert "not initialized" in pr.status_snapshot(tmp_path)

    class FakeDB:
        def get_session(self):
            return self

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    monkeypatch.setattr(pr, "get_db", lambda _r: FakeDB())
    monkeypatch.setattr(
        pr,
        "ArtifactGraphRepository",
        lambda session: SimpleNamespace(
            load_graph=lambda: SimpleNamespace(
                coverage_summary=lambda: {
                    "total_tasks": 1,
                    "total_gaps": 2,
                    "blocking_gaps": 1,
                }
            )
        ),
    )
    monkeypatch.setattr(
        pr,
        "StateRepository",
        lambda session: SimpleNamespace(get_state=lambda: SimpleNamespace(current_phase="EXECUTING")),
    )
    monkeypatch.setattr(pr, "compute_phase", lambda graph, phase: "EXECUTING")
    assert "Phase: EXECUTING" in pr.status_snapshot(tmp_path)

    monkeypatch.setattr(
        pr,
        "ArtifactGraphRepository",
        lambda session: (_ for _ in ()).throw(RuntimeError("x")),
    )
    assert "unavailable" in pr.status_snapshot(tmp_path)

    for name in (
        "devcouncil_implement_next_task",
        "devcouncil_repair_task",
        "devcouncil_verify_task",
        "devcouncil_review_live",
        "devcouncil_project_status",
        "devcouncil_apply_knowledge",
        "unknown",
    ):
        text = pr.render_prompt_text(name, {"client_id": "c", "task_id": "T1", "goal": "ship"}, tmp_path)
        assert isinstance(text, str) and text

    prompts = pr.list_prompts()
    assert len(prompts) >= 5
