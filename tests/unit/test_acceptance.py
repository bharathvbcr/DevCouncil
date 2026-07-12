"""Coverage for verification.checks.acceptance evidence helpers."""

from __future__ import annotations

from devcouncil.verification.checks.acceptance import (
    coarse_proven_acceptance_ids,
    unproven_acceptance_ids,
)


def _prove_expected(expected: str, cmd: str) -> bool:
    return "pytest" in cmd


def test_coarse_proven_empty_when_no_acceptance_ids():
    assert coarse_proven_acceptance_ids(
        task_acceptance_ids=[],
        successful_commands=["pytest"],
        command_can_prove=_prove_expected,
    ) == set()


def test_coarse_proven_empty_when_no_commands():
    assert coarse_proven_acceptance_ids(
        task_acceptance_ids=["AC-1"],
        successful_commands=[],
        command_can_prove=_prove_expected,
    ) == set()


def test_coarse_proven_all_when_a_command_proves():
    proven = coarse_proven_acceptance_ids(
        task_acceptance_ids=["AC-1", "AC-2"],
        successful_commands=["ruff check .", "pytest -q"],
        command_can_prove=_prove_expected,
    )
    assert proven == {"AC-1", "AC-2"}


def test_coarse_proven_empty_when_no_command_proves():
    proven = coarse_proven_acceptance_ids(
        task_acceptance_ids=["AC-1"],
        successful_commands=["echo hi", "ruff check ."],
        command_can_prove=_prove_expected,
    )
    assert proven == set()


def test_unproven_filters_compiled_coarse_and_inconclusive():
    unproven = unproven_acceptance_ids(
        task_acceptance_ids=["AC-1", "AC-2", "AC-3", "AC-4"],
        compiled_pass={"AC-1": True, "AC-2": False},
        coarse_proven={"AC-3"},
        inconclusive={"AC-4"},
    )
    # AC-1 compiled-proven, AC-3 coarse-proven, AC-4 inconclusive (skipped) -> only AC-2 remains
    assert unproven == ["AC-2"]


def test_unproven_all_when_nothing_proven():
    unproven = unproven_acceptance_ids(
        task_acceptance_ids=["AC-1", "AC-2"],
        compiled_pass={},
        coarse_proven=set(),
        inconclusive=set(),
    )
    assert unproven == ["AC-1", "AC-2"]


def test_unproven_preserves_order():
    unproven = unproven_acceptance_ids(
        task_acceptance_ids=["AC-3", "AC-1", "AC-2"],
        compiled_pass={},
        coarse_proven=set(),
        inconclusive=set(),
    )
    assert unproven == ["AC-3", "AC-1", "AC-2"]
