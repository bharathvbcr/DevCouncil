"""Tests for LLM-backed implementation reviewer."""

import asyncio
from unittest.mock import AsyncMock, MagicMock

from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.verification.implementation_reviewer import ImplementationReviewer, ReviewOutput


def test_review_changes_uses_linked_requirements():
    router = MagicMock()
    router.complete_structured = AsyncMock(
        return_value=ReviewOutput(is_satisfactory=True, findings=[])
    )
    task = Task(
        id="TASK-1",
        title="Add feature",
        description="Implement X",
        requirement_ids=["REQ-1"],
    )
    requirements = [
        Requirement(id="REQ-1", title="R1", description="d", priority="high", source="user"),
        Requirement(id="REQ-2", title="R2", description="d", priority="low", source="user"),
    ]
    reviewer = ImplementationReviewer(router)

    result = asyncio.run(reviewer.review_changes(task, requirements, "diff content"))

    assert result.is_satisfactory is True
    call = router.complete_structured.await_args
    prompt = call.kwargs["messages"][0]["content"]
    assert "REQ-1" in prompt
    assert "REQ-2" not in prompt


def test_review_changes_falls_back_to_all_requirements():
    router = MagicMock()
    router.complete_structured = AsyncMock(
        return_value=ReviewOutput(is_satisfactory=True, findings=[])
    )
    task = Task(id="TASK-1", title="T", description="D", requirement_ids=[])
    requirements = [
        Requirement(id="REQ-1", title="R1", description="d", priority="high", source="user"),
    ]
    reviewer = ImplementationReviewer(router)

    asyncio.run(reviewer.review_changes(task, requirements, "+++ b/file.py"))

    prompt = router.complete_structured.await_args.kwargs["messages"][0]["content"]
    assert "REQ-1" in prompt


def test_review_changes_returns_findings():
    finding = Gap(
        id="G1",
        severity="high",
        gap_type="architecture_drift",
        task_id="TASK-1",
        description="drift",
        recommended_fix="fix",
        blocking=False,
    )
    router = MagicMock()
    router.complete_structured = AsyncMock(
        return_value=ReviewOutput(is_satisfactory=False, findings=[finding])
    )
    reviewer = ImplementationReviewer(router)

    result = asyncio.run(
        reviewer.review_changes(
            Task(id="TASK-1", title="T", description="D"),
            [],
            "diff",
        )
    )

    assert result.is_satisfactory is False
    assert len(result.findings) == 1
    assert result.findings[0].id == "G1"
