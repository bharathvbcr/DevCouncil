"""Correction-manifest root-cause ordering (rank 10).

build_correction_manifest must steer the repair loop at the most actionable failure
(a failing test / unproven acceptance criterion) rather than an arbitrary first gap
such as an orphan_diff, and expose the full ordered list.
"""

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.planning.correction_manifest import build_correction_manifest


def _gap(gap_type, severity, description, **kw) -> Gap:
    base = dict(
        id=f"GAP-{gap_type}",
        severity=severity,
        gap_type=gap_type,
        task_id="TASK-001",
        description=description,
        recommended_fix="fix",
        blocking=True,
    )
    base.update(kw)
    return Gap(**base)


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="T",
        description="D",
        planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
        expected_tests=["pytest tests/a"],
    )


def _config(tmp_path):
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\n", encoding="utf-8"
    )


def test_root_cause_prefers_test_failed_over_orphan_diff(tmp_path):
    _config(tmp_path)
    # Orphan diff is listed first but a same-severity test_failed must win root_cause.
    gaps = [
        _gap("orphan_diff", "high", "File src/rogue.py modified but not planned.", file="src/rogue.py"),
        _gap("test_failed", "high", "Command 'pytest' failed.", suggested_command="pytest"),
    ]

    manifest = build_correction_manifest(tmp_path, _task(), gaps)

    assert "pytest" in manifest.root_cause.lower() or "command" in manifest.root_cause.lower()
    assert manifest.root_cause == "Command 'pytest' failed."
    # Full ordered list exposed, test_failed first.
    assert manifest.ordered_blocking_gaps[0] == "Command 'pytest' failed."
    assert "src/rogue.py" in manifest.ordered_blocking_gaps[1]


def test_root_cause_prefers_higher_severity_first(tmp_path):
    _config(tmp_path)
    gaps = [
        _gap("test_failed", "medium", "advisory test failed"),
        _gap("acceptance_criteria_unproven", "critical", "AC-001 critical unproven"),
    ]

    manifest = build_correction_manifest(tmp_path, _task(), gaps)

    assert manifest.root_cause == "AC-001 critical unproven"


def test_acceptance_outranks_scope_within_same_severity(tmp_path):
    _config(tmp_path)
    gaps = [
        _gap("dependency_risk", "high", "dep change"),
        _gap("acceptance_criteria_unproven", "high", "AC unproven"),
    ]

    manifest = build_correction_manifest(tmp_path, _task(), gaps)

    assert manifest.root_cause == "AC unproven"
    assert manifest.ordered_blocking_gaps == ["AC unproven", "dep change"]


def test_no_blocking_gaps_is_safe(tmp_path):
    _config(tmp_path)
    manifest = build_correction_manifest(tmp_path, _task(), [])
    assert manifest.root_cause == "Unknown failure"
    assert manifest.ordered_blocking_gaps == []
