from unittest.mock import patch

from devcouncil.domain.assumption import Assumption
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.gating.policy import GatePolicy
from devcouncil.planning.question_conversion import (
    assumption_id_from_question,
    blocking_question_to_assumption,
    convert_blocking_questions_to_assumptions,
)
from devcouncil.planning.spec_service import BlockingQuestion


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-001",
        title="Auth",
        description="Auth requirement",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(
                id="AC-001",
                description="Auth behavior is tested",
                verification_method="unit_test",
            )
        ],
    )


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Implement auth",
        description="Implement auth",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[
            PlannedFile(path="src/auth.py", reason="auth logic", allowed_change="modify"),
        ],
    )


def test_assumption_id_from_question_preserves_q_prefix():
    assert assumption_id_from_question("Q-001") == "ASM-from-Q-001"


def test_assumption_id_from_question_adds_prefix_for_bare_ids():
    assert assumption_id_from_question("001") == "ASM-from-Q-001"


def test_blocking_question_to_assumption_is_non_blocking():
    question = BlockingQuestion(
        id="Q-001",
        question="Should reset invalidate sessions?",
        reason="Affects auth token lifecycle",
    )

    assumption = blocking_question_to_assumption(question)

    assert assumption.id == "ASM-from-Q-001"
    assert "Should reset invalidate sessions?" in assumption.statement
    assert "auth token lifecycle" in assumption.statement
    assert assumption.confidence == "low"
    assert assumption.impact == "medium"
    assert assumption.requires_user_confirmation is False
    assert assumption.reversible is True
    assert assumption.status == "open"


def test_convert_blocking_questions_appends_and_clears():
    existing = Assumption(
        id="ASM-001",
        statement="Existing assumption",
        confidence="high",
        impact="low",
        reversible=True,
        requires_user_confirmation=False,
        status="open",
    )
    questions = [
        BlockingQuestion(id="Q-001", question="First?", reason=""),
        BlockingQuestion(id="Q-002", question="Second?", reason=""),
    ]

    assumptions, blocking = convert_blocking_questions_to_assumptions([existing], questions)

    assert blocking == []
    assert len(assumptions) == 3
    assert assumptions[0].id == "ASM-001"
    assert assumptions[1].id == "ASM-from-Q-001"
    assert assumptions[2].id == "ASM-from-Q-002"


def test_convert_avoids_assumption_id_collision():
    existing = Assumption(
        id="ASM-from-Q-001",
        statement="Already taken",
        confidence="high",
        impact="low",
        reversible=True,
        requires_user_confirmation=False,
        status="open",
    )
    questions = [BlockingQuestion(id="Q-001", question="Duplicate?", reason="")]

    assumptions, blocking = convert_blocking_questions_to_assumptions([existing], questions)

    assert blocking == []
    assert assumptions[-1].id == "ASM-from-Q-001-1"


def test_gate_passes_after_blocking_question_conversion():
    questions = [
        BlockingQuestion(id="Q-001", question="Should reset invalidate sessions?", reason=""),
    ]
    assumptions, blocking = convert_blocking_questions_to_assumptions([], questions)

    result = GatePolicy().check_plan_approval(
        [_requirement()],
        [_task()],
        assumptions=assumptions,
        blocking_questions=blocking,
    )

    assert result.passed


def test_gate_still_blocks_unconverted_blocking_questions():
    question = BlockingQuestion(id="Q-001", question="Unanswered?", reason="")

    result = GatePolicy().check_plan_approval(
        [_requirement()],
        [_task()],
        blocking_questions=[question],
    )

    assert not result.passed
    assert any("Q-001" in gap.id for gap in result.gaps)


def test_maybe_convert_skips_in_interactive_mode():
    from devcouncil.cli.commands.plan import _maybe_convert_blocking_questions
    from devcouncil.planning.spec_service import SpecOutput
    from types import SimpleNamespace

    spec_output = SpecOutput(
        requirements=[],
        assumptions=[],
        blocking_questions=[
            BlockingQuestion(id="Q-001", question="Interactive?", reason=""),
        ],
    )
    config = SimpleNamespace(
        planning=SimpleNamespace(auto_convert_blocking_questions_in_noninteractive=True),
    )

    with patch("devcouncil.cli.commands.plan.sys.stdin.isatty", return_value=True):
        converted = _maybe_convert_blocking_questions(spec_output, config, console=SimpleNamespace(print=lambda *a, **k: None))

    assert converted.blocking_questions
    assert not converted.assumptions


def test_maybe_convert_applies_in_non_interactive_mode(tmp_path):
    from devcouncil.cli.commands.plan import _maybe_convert_blocking_questions
    from devcouncil.planning.spec_service import SpecOutput
    from types import SimpleNamespace

    spec_output = SpecOutput(
        requirements=[],
        assumptions=[],
        blocking_questions=[
            BlockingQuestion(id="Q-001", question="Batch mode?", reason=""),
        ],
    )
    config = SimpleNamespace(
        planning=SimpleNamespace(auto_convert_blocking_questions_in_noninteractive=True),
    )
    artifact_path = tmp_path / "requirements.json"

    with patch("devcouncil.cli.commands.plan.sys.stdin.isatty", return_value=False):
        converted = _maybe_convert_blocking_questions(
            spec_output,
            config,
            console=SimpleNamespace(print=lambda *a, **k: None),
            artifact_path=artifact_path,
        )

    assert converted.blocking_questions == []
    assert len(converted.assumptions) == 1
    assert artifact_path.exists()
    persisted = SpecOutput.model_validate_json(artifact_path.read_text(encoding="utf-8"))
    assert persisted.blocking_questions == []
    assert len(persisted.assumptions) == 1
