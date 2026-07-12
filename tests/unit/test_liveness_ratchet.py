"""Liveness ratchet gate tests."""

from __future__ import annotations

from pathlib import Path

from devcouncil.domain.task import Task
from devcouncil.verification.checks.liveness_ratchet import (
    detect_liveness_regressions,
    load_liveness_baseline,
    snapshot_liveness_baseline,
)
from devcouncil.verification.difficulty import resolve_rigor_policy


def _task(*, difficulty: str = "hard") -> Task:
    return Task(
        id="TASK-1",
        title="t",
        description="d",
        planned_files=[],
        difficulty=difficulty,  # type: ignore[arg-type]
    )


def _gap_id(task_id: str, kind: str) -> str:
    return f"{task_id}-{kind}-1"


def test_missing_baseline_skips_silently():
    gaps = detect_liveness_regressions(
        None,
        {"unwired_candidates": ["pkg/a.py"]},
        set(),
        task=_task(),
        next_gap_id=_gap_id,
        blocking=True,
    )
    assert gaps == []


def test_newly_unwired_existing_file_flagged():
    baseline = {
        "complete": True,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }
    current = {
        "unwired_candidates": ["pkg/orphan.py"],
        "unreachable_files": ["pkg/orphan.py"],
        "dead_symbol_candidates": [],
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        set(),
        task=_task(),
        next_gap_id=_gap_id,
        blocking=True,
    )
    assert len(gaps) == 1
    g = gaps[0]
    assert g.gap_type == "stranded_code"
    assert g.file == "pkg/orphan.py"
    assert g.blocking is True


def test_task_added_files_excluded():
    baseline = {
        "complete": True,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }
    current = {
        "unwired_candidates": ["pkg/new.py"],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        {"pkg/new.py"},
        task=_task(),
        next_gap_id=_gap_id,
        blocking=True,
    )
    assert gaps == []


def test_newly_dead_symbol_not_flagged_without_baseline_index():
    """Brand-new dead symbols must not fire stranded_code (dead_symbol owns them)."""
    baseline = {
        "complete": True,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }
    current = {
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": ["pkg/mod.py:3 helper"],
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        set(),
        task=_task(),
        next_gap_id=_gap_id,
        blocking=False,
    )
    assert gaps == []


def test_newly_dead_preexisting_symbol_flagged():
    from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION

    baseline = {
        "complete": True,
        "scan_version": LIVENESS_SCAN_VERSION,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": ["pkg/mod.py::helper"],
    }
    current = {
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": ["pkg/mod.py:3 helper"],
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        set(),
        task=_task(),
        next_gap_id=_gap_id,
        blocking=False,
    )
    assert len(gaps) == 1
    assert gaps[0].gap_type == "stranded_code"
    assert gaps[0].file == "pkg/mod.py"
    assert gaps[0].blocking is False
    assert "helper" in gaps[0].description


def test_stale_scan_version_skips_symbol_diff_keeps_files():
    """Baselines without/mismatched scan_version skip symbol diff but still compare files."""
    baseline = {
        "complete": True,
        # no scan_version — pre-hardening baseline
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": ["pkg/mod.py::helper"],
    }
    current = {
        "unwired_candidates": ["pkg/orphan.py"],
        "unreachable_files": [],
        "dead_symbol_candidates": ["pkg/mod.py:3 helper"],
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        set(),
        task=_task(),
        next_gap_id=_gap_id,
        blocking=False,
    )
    assert len(gaps) == 1
    assert gaps[0].file == "pkg/orphan.py"
    assert not any("helper" in (g.description or "") for g in gaps)


def test_mismatched_scan_version_skips_symbol_diff():
    baseline = {
        "complete": True,
        "scan_version": 0,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": ["pkg/mod.py::helper"],
    }
    current = {
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": ["pkg/mod.py:3 helper"],
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        set(),
        task=_task(),
        next_gap_id=_gap_id,
        blocking=False,
    )
    assert gaps == []


def test_already_unwired_at_baseline_not_flagged():
    baseline = {
        "complete": True,
        "unwired_candidates": ["pkg/orphan.py"],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }
    current = {
        "unwired_candidates": ["pkg/orphan.py"],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        set(),
        task=_task(),
        next_gap_id=_gap_id,
        blocking=True,
    )
    assert gaps == []


def test_rigor_hard_blocks_ratchet():
    policy = resolve_rigor_policy(_task(difficulty="hard"), None, config=None)
    assert policy.liveness_ratchet_enabled is True
    assert policy.liveness_ratchet_blocking is True
    assert "liveness_ratchet_blocking" in policy.applied


def test_rigor_easy_advisory_ratchet():
    policy = resolve_rigor_policy(_task(difficulty="easy"), None, config=None)
    assert policy.liveness_ratchet_enabled is True
    assert policy.liveness_ratchet_blocking is False


def test_snapshot_and_load_baseline(tmp_path):
    import subprocess

    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (tmp_path / "pkg" / "orphan.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
        cwd=tmp_path, check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"],
        cwd=tmp_path, check=True, capture_output=True,
    )

    out = snapshot_liveness_baseline(tmp_path, "TASK-1")
    assert out is not None
    assert out.is_file()
    loaded = load_liveness_baseline(tmp_path, "TASK-1")
    assert loaded is not None
    assert loaded.get("complete") is True
    assert "unwired_candidates" in loaded
    assert "pkg/orphan.py" in loaded["unwired_candidates"]
    assert "symbol_index" in loaded
    from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION

    assert loaded.get("scan_version") == LIVENESS_SCAN_VERSION


def test_load_missing_baseline_returns_none(tmp_path):
    assert load_liveness_baseline(tmp_path, "NO-SUCH") is None
    assert not (Path(tmp_path) / ".devcouncil" / "liveness_baseline").exists()
