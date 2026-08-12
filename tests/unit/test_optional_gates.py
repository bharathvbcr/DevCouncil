"""Global gate-enforcement opt-out behavior."""

import asyncio
from pathlib import Path
import subprocess

from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.execution.stop_gate import evaluate_stop
from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.gating.policy import (
    GatePolicy,
    apply_gate_enforcement,
    effective_artifact_graph,
    effective_live_review,
)
from devcouncil.verification.verifier import Verifier, verification_task_status


def _write_config(root: Path, body: str) -> None:
    config_dir = root / ".devcouncil"
    config_dir.mkdir(exist_ok=True)
    (config_dir / "config.yaml").write_text(body, encoding="utf-8")


def _blocking_gap() -> Gap:
    return Gap(
        id="GAP-TEST",
        severity="high",
        gap_type="test_failed",
        description="A configured check failed.",
        recommended_fix="Fix the check.",
        blocking=True,
    )


def test_advisory_gate_enforcement_preserves_finding_as_nonblocking():
    original = _blocking_gap()

    result = apply_gate_enforcement([original], mode="advisory")

    assert original.blocking is True
    assert result[0].blocking is False
    assert result[0].description == original.description


def test_non_enforcing_modes_keep_security_risks_blocking():
    secret = _blocking_gap().model_copy(
        update={"id": "GAP-SECRET", "gap_type": "security_risk"}
    )

    result = apply_gate_enforcement([secret], mode="off")

    assert result[0].blocking is True


def test_off_reporting_view_demotes_historical_quality_blockers_and_task_status():
    graph = ArtifactGraph()
    graph.add_task(
        Task(id="TASK-1", title="t", description="d", status="blocked")
    )
    graph.add_gap(_blocking_gap().model_copy(update={"task_id": "TASK-1"}))

    effective = effective_artifact_graph(graph, mode="off")

    assert graph.tasks["TASK-1"].status == "blocked"
    assert graph.blocking_gaps()
    assert effective.tasks["TASK-1"].status == "done"
    assert effective.blocking_gaps() == []


def test_off_reporting_view_keeps_hard_safety_and_demotes_live_cards():
    graph = ArtifactGraph()
    graph.add_task(
        Task(id="TASK-1", title="t", description="d", status="blocked")
    )
    graph.add_gap(
        _blocking_gap().model_copy(
            update={
                "id": "GAP-SECRET",
                "gap_type": "security_risk",
                "task_id": "TASK-1",
            }
        )
    )
    live = {"blocking_cards": [{"id": "CARD-1"}]}

    effective = effective_artifact_graph(graph, mode="off")
    effective_live = effective_live_review(live, mode="off")

    assert effective.tasks["TASK-1"].status == "blocked"
    assert len(effective.blocking_gaps()) == 1
    assert effective_live["blocking_cards"] == []
    assert effective_live["stored_blocking_cards"] == [{"id": "CARD-1"}]


def test_advisory_gates_allow_plan_with_recorded_findings(tmp_path):
    _write_config(tmp_path, "project:\n  name: t\ngates:\n  mode: advisory\n")
    requirement = Requirement(
        id="REQ-1",
        title="Incomplete",
        description="No acceptance criteria",
        priority="high",
        source="user",
    )
    task = Task(id="TASK-1", title="Unmapped", description="No requirement links")

    result = GatePolicy(tmp_path).check_plan_approval([requirement], [task])

    assert result.passed is True
    assert result.gaps
    assert not any(gap.blocking for gap in result.gaps)


def test_off_gates_skip_plan_checks_entirely(tmp_path):
    _write_config(tmp_path, "project:\n  name: t\ngates:\n  mode: off\n")
    requirement = Requirement(
        id="REQ-1",
        title="Incomplete",
        description="No acceptance criteria",
        priority="high",
        source="user",
    )
    task = Task(id="TASK-1", title="Unmapped", description="No requirement links")

    result = GatePolicy(tmp_path).check_plan_approval([requirement], [task])

    assert result.passed is True
    assert result.gaps == []


def test_off_gates_override_stop_and_hook_containment(tmp_path, monkeypatch):
    _write_config(
        tmp_path,
        (
            "project:\n  name: t\n"
            "gates:\n  mode: off\n"
            "execution:\n"
            "  hook_gate:\n    mode: contain\n"
            "  stop_gate:\n    mode: block\n"
        ),
    )
    monkeypatch.delenv("DEVCOUNCIL_HOOK_GATE", raising=False)
    monkeypatch.delenv("DEVCOUNCIL_STOP_GATE", raising=False)

    hook = HookPolicy(project_root=tmp_path).evaluate(
        {"name": "Shell", "arguments": {"command": "ls"}},
        None,
    )
    stop = evaluate_stop(tmp_path, {"claim_text": "All tests pass."})

    assert hook.action == "allow"
    assert stop.decision == "pass"
    assert stop.mode == "off"


def test_off_gates_skip_quality_verification_and_mark_task_done(tmp_path, monkeypatch):
    subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
    _write_config(
        tmp_path,
        (
            "project:\n  name: t\n"
            "gates:\n  mode: off\n"
            "execution:\n"
            "  refresh_stale_map_on_verify: false\n"
            "verification:\n"
            "  rigor:\n    enabled: false\n"
        ),
    )
    task = Task(
        id="TASK-1",
        title="No work",
        description="Produces no diff",
        planned_files=[
            PlannedFile(path="src/app.py", reason="implementation", allowed_change="modify")
        ],
    )

    verifier = Verifier(tmp_path)
    monkeypatch.setattr(
        verifier,
        "_run_command",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("quality commands must not run")
        ),
    )
    gaps, _ = asyncio.run(verifier.verify_task(task, []))

    assert gaps == []
    assert verifier.last_outcome is not None
    assert verifier.last_outcome.verification_skipped is True
    assert verifier.last_outcome.mode == "off"
    assert verification_task_status(gaps, verifier.last_outcome) == "done"


def test_advisory_gates_run_quality_checks_but_demote_blockers(tmp_path):
    subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
    _write_config(
        tmp_path,
        (
            "project:\n  name: t\n"
            "gates:\n  mode: advisory\n"
            "execution:\n"
            "  refresh_stale_map_on_verify: false\n"
            "verification:\n"
            "  rigor:\n    enabled: false\n"
        ),
    )
    task = Task(
        id="TASK-1",
        title="No work",
        description="Produces no diff",
        planned_files=[
            PlannedFile(path="src/app.py", reason="implementation", allowed_change="modify")
        ],
    )

    verifier = Verifier(tmp_path)
    gaps, _ = asyncio.run(verifier.verify_task(task, []))

    assert any(gap.gap_type == "task_not_implemented" for gap in gaps)
    assert not any(gap.blocking for gap in gaps)
    assert verifier.last_outcome is not None
    assert verifier.last_outcome.verification_skipped is False
    assert verifier.last_outcome.gate_mode == "advisory"


def test_off_gates_still_block_secrets(tmp_path, monkeypatch):
    _write_config(tmp_path, "project:\n  name: t\ngates:\n  mode: off\n")
    diff = (
        "diff --git a/src/app.py b/src/app.py\n"
        "+++ b/src/app.py\n"
        "@@ -0,0 +1 @@\n"
        "+API_KEY = 'sk-abcdefghijklmnopqrstuvwxyz'\n"
    )
    verifier = Verifier(tmp_path)
    monkeypatch.setattr(verifier, "get_diff", lambda: diff)
    monkeypatch.setattr(verifier, "get_changed_files", lambda: ["src/app.py"])
    monkeypatch.setattr(
        verifier,
        "_classify_change_paths",
        lambda _paths: (["src/app.py"], []),
    )

    gaps, _ = asyncio.run(
        verifier.verify_task(Task(id="TASK-1", title="t", description="d"), [])
    )

    assert gaps
    assert all(gap.gap_type == "security_risk" for gap in gaps)
    assert all(gap.blocking for gap in gaps)
    assert verification_task_status(gaps, verifier.last_outcome) == "blocked"
