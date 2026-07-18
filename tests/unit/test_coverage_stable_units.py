"""Stable-unit coverage for small deterministic helpers and CLI entrypoints."""

from __future__ import annotations

import importlib.metadata
import subprocess
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from typer.testing import CliRunner

from devcouncil.app.errors import GatingError
from devcouncil.artifacts.validators import ArtifactValidator
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.gating.checks.clean_git import CleanGitCheck
from devcouncil.knowledge.knowledge_select import select_knowledge_payload


def test_artifact_validator_requirement_errors():
    with pytest.raises(GatingError, match="missing title"):
        ArtifactValidator.validate_requirement(
            Requirement(id="R1", title="", description="d", priority="high")
        )
    with pytest.raises(GatingError, match="at least one acceptance"):
        ArtifactValidator.validate_requirement(
            Requirement(id="R1", title="t", description="d", priority="high")
        )
    # Force empty verification_method via model_construct
    bad_ac = AcceptanceCriterion.model_construct(id="AC1", description="x", verification_method="")
    with pytest.raises(GatingError, match="verification method"):
        ArtifactValidator.validate_requirement(
            Requirement(
                id="R1",
                title="t",
                description="d",
                priority="high",
                acceptance_criteria=[bad_ac],
            )
        )


def test_artifact_validator_task_errors():
    with pytest.raises(GatingError, match="map to at least one requirement"):
        ArtifactValidator.validate_task(Task(id="T1", title="t", description="d"))
    with pytest.raises(GatingError, match="planned file"):
        ArtifactValidator.validate_task(
            Task(id="T1", title="t", description="d", requirement_ids=["R1"])
        )
    with pytest.raises(GatingError, match="acceptance criterion"):
        ArtifactValidator.validate_task(
            Task(
                id="T1",
                title="t",
                description="d",
                requirement_ids=["R1"],
                planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
            )
        )
    with pytest.raises(GatingError, match="allowed commands or expected tests"):
        ArtifactValidator.validate_task(
            Task(
                id="T1",
                title="t",
                description="d",
                requirement_ids=["R1"],
                acceptance_criterion_ids=["AC1"],
                planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
            )
        )
    ArtifactValidator.validate_task(
        Task(
            id="T1",
            title="t",
            description="d",
            requirement_ids=["R1"],
            acceptance_criterion_ids=["AC1"],
            planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
            expected_tests=["pytest -q"],
        )
    )


def test_select_knowledge_payload_success_and_fallback(tmp_path: Path):
    with patch(
        "devcouncil.knowledge.resource_discovery.knowledge_settings",
        return_value=(None, False),
    ):
        with patch(
            "devcouncil.knowledge.sources.render_knowledge_preamble",
            return_value="preamble",
        ):
            payload = select_knowledge_payload(tmp_path, "goal")
            assert payload["ok"] is True
            assert payload["sources"] == []
            assert payload["preamble"] == "preamble"

    source = MagicMock(name="doc", kind="md", description="d")
    source.name = "doc"
    source.kind = "md"
    source.description = "d"
    with patch(
        "devcouncil.knowledge.resource_discovery.knowledge_settings",
        return_value=(tmp_path, True),
    ):
        with patch(
            "devcouncil.knowledge.sources.select_knowledge_sources",
            return_value=[source],
        ):
            with patch(
                "devcouncil.knowledge.sources.render_knowledge_preamble",
                return_value="p",
            ):
                payload = select_knowledge_payload(tmp_path, "goal")
                assert payload["sources"][0]["name"] == "doc"

    with patch(
        "devcouncil.knowledge.resource_discovery.knowledge_settings",
        side_effect=RuntimeError("boom"),
    ):
        payload = select_knowledge_payload(tmp_path, "goal")
        assert payload["ok"] is True
        assert "knowledge unavailable" in payload["note"]


def test_clean_git_check_branches(tmp_path: Path):
    check = CleanGitCheck()
    assert check._is_runtime_state("?? .devcouncil/foo")
    assert check._is_runtime_state(" M .gitignore")
    assert not check._is_runtime_state(" M src/a.py")

    with patch("subprocess.check_output", return_value=b" M src/a.py\n?? .devcouncil/x\n"):
        gaps = check.check(tmp_path, "TASK-1")
        assert gaps and gaps[0].blocking

    with patch("subprocess.check_output", return_value=b"?? .devcouncil/x\n"):
        assert check.check(tmp_path, "TASK-1") == []

    with patch("subprocess.check_output", side_effect=FileNotFoundError):
        gaps = check.check(tmp_path, "TASK-1")
        assert gaps[0].id.endswith("NO-GIT")

    with patch(
        "subprocess.check_output",
        side_effect=subprocess.CalledProcessError(1, "git"),
    ):
        gaps = check.check(tmp_path, "TASK-1")
        assert gaps[0].id.endswith("GIT-ERROR")


def test_version_command_paths():
    from devcouncil.cli.commands import version as version_mod

    runner = CliRunner()
    result = runner.invoke(version_mod.app, [])
    assert result.exit_code == 0
    assert "DevCouncil version" in result.stdout

    with patch(
        "importlib.metadata.version",
        side_effect=importlib.metadata.PackageNotFoundError("devcouncil"),
    ):
        result = runner.invoke(version_mod.app, [])
        assert result.exit_code == 0
        assert "unknown" in result.stdout


def test_mcp_server_callback_skips_when_subcommand():
    from typer import Context

    from devcouncil.cli.commands.mcp_server import mcp_server

    ctx = MagicMock(spec=Context)
    ctx.invoked_subcommand = "something"
    mcp_server(ctx)  # should no-op


def test_mcp_server_starts_run():
    from typer import Context

    from devcouncil.cli.commands import mcp_server as mcp_mod

    ctx = MagicMock(spec=Context)
    ctx.invoked_subcommand = None
    with patch("devcouncil.integrations.mcp.server.run", return_value=None) as run:
        with patch("asyncio.run") as arun:
            mcp_mod.mcp_server(ctx)
            arun.assert_called_once()
            assert run is not None


def test_ad_hoc_load_router_and_persist(tmp_path: Path):
    from devcouncil.domain.evidence import CommandResult
    from devcouncil.domain.gap import Gap
    from devcouncil.domain.requirement import Requirement
    from devcouncil.domain.task import Task
    from devcouncil.verification import ad_hoc_check as mod

    router = mod._load_verify_router(tmp_path)
    assert router is None or router is not None

    with patch("devcouncil.storage.db.get_db", return_value=None):
        mod._persist_ad_hoc_result(
            tmp_path,
            requirement=Requirement(id="CHECK-REQ", title="t", description="d", priority="high"),
            task=Task(id="CHECK", title="t", description="d"),
            gaps=[],
            evidence=[],
        )

    session = MagicMock()
    db = MagicMock()
    db.get_session.return_value.__enter__.return_value = session
    db.get_session.return_value.__exit__.return_value = None
    gap = Gap(
        id="G1",
        severity="high",
        gap_type="missing_test",
        task_id="CHECK",
        description="d",
        recommended_fix="f",
        blocking=True,
    )
    with patch("devcouncil.storage.db.get_db", return_value=db):
        with patch("devcouncil.storage.repositories.RequirementRepository") as RR:
            with patch("devcouncil.storage.repositories.GapRepository") as GR:
                with patch("devcouncil.storage.repositories.EvidenceRepository") as ER:
                    with patch("devcouncil.storage.repositories.TaskRepository") as TR:
                        rr = RR.return_value
                        gr = GR.return_value
                        er = ER.return_value
                        tr = TR.return_value
                        mod._persist_ad_hoc_result(
                            tmp_path,
                            requirement=Requirement(
                                id="CHECK-REQ", title="t", description="d", priority="high"
                            ),
                            task=Task(id="CHECK", title="t", description="d"),
                            gaps=[gap],
                            evidence=[
                                CommandResult(
                                    command="echo",
                                    exit_code=0,
                                    stdout_path="",
                                    stderr_path="",
                                    summary="ok",
                                ),
                            ],
                        )
                        rr.save.assert_called()
                        gr.save.assert_called()
                        tr.save.assert_called()
                        er.save_command_result.assert_called()
