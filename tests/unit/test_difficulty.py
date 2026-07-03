"""Tests for deterministic task-difficulty estimation and the rigor policy."""

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.difficulty import (
    RigorPolicy,
    difficulty_score,
    estimate_difficulty,
    resolve_rigor_policy,
)


def _task(**overrides) -> Task:
    base = dict(
        id="TASK-001",
        title="Add helper",
        description="Add a small helper function.",
        planned_files=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")],
    )
    base.update(overrides)
    return Task(**base)


def _req(priority="medium", ac_count=1) -> Requirement:
    return Requirement(
        id="REQ-001",
        title="Requirement",
        description="Do the thing.",
        priority=priority,
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id=f"AC-{i}", description="works", verification_method="unit_test")
            for i in range(ac_count)
        ],
    )


class TestEstimateDifficulty:
    def test_small_task_is_easy(self):
        assert estimate_difficulty(_task()) == "easy"

    def test_manual_override_wins(self):
        assert estimate_difficulty(_task(difficulty="hard")) == "hard"
        # Even a task that would score hard respects an easy override.
        big = _task(
            difficulty="easy",
            planned_files=[
                PlannedFile(path=f"src/f{i}.py", reason="edit", allowed_change="modify")
                for i in range(6)
            ],
            acceptance_criterion_ids=["AC-0", "AC-1", "AC-2", "AC-3", "AC-4"],
        )
        assert estimate_difficulty(big) == "easy"

    def test_many_files_and_criteria_is_hard(self):
        task = _task(
            planned_files=[
                PlannedFile(path=f"src/f{i}.py", reason="edit", allowed_change="modify")
                for i in range(5)
            ],
            acceptance_criterion_ids=[f"AC-{i}" for i in range(5)],
        )
        assert estimate_difficulty(task) == "hard"

    def test_keyword_bucket_is_capped(self):
        # A wordy description full of hard keywords alone must not reach "hard".
        task = _task(
            description="Refactor the async concurrent cache migration transaction protocol."
        )
        assert difficulty_score(task) <= 1
        assert estimate_difficulty(task) == "easy"

    def test_mixed_signals_reach_normal(self):
        task = _task(
            description="Refactor the parser",
            planned_files=[
                PlannedFile(path="src/a.py", reason="edit", allowed_change="modify"),
                PlannedFile(path="src/b.py", reason="edit", allowed_change="modify"),
                PlannedFile(path="src/c.py", reason="edit", allowed_change="modify"),
            ],
        )
        assert estimate_difficulty(task) == "normal"

    def test_read_only_files_do_not_count(self):
        task = _task(
            planned_files=[
                PlannedFile(path=f"src/f{i}.py", reason="ref", allowed_change="read_only")
                for i in range(6)
            ]
        )
        assert estimate_difficulty(task) == "easy"

    def test_linked_requirement_priority_counts(self):
        task = _task(requirement_ids=["REQ-001"], acceptance_criterion_ids=["AC-0", "AC-1", "AC-2"])
        score_low = difficulty_score(task, [_req(priority="medium")])
        score_high = difficulty_score(task, [_req(priority="critical")])
        assert score_high == score_low + 1

    def test_depends_on_and_create_modify_signals(self):
        deps = _task(depends_on=["T1", "T2"])
        assert difficulty_score(deps) == 1
        mixed = _task(
            planned_files=[
                PlannedFile(path="src/new.py", reason="add", allowed_change="create"),
                PlannedFile(path="src/old.py", reason="edit", allowed_change="modify"),
            ],
        )
        assert difficulty_score(mixed) == 1


class TestResolveRigorPolicy:
    def test_default_policy_hard_task_blocks(self):
        task = _task(difficulty="hard")
        policy = resolve_rigor_policy(task, None, config=None)
        assert policy.difficulty == "hard"
        assert policy.stub_enabled and policy.stub_blocking
        assert policy.effort_enabled and policy.effort_blocking
        assert policy.coarse_acceptance_enabled and policy.coarse_acceptance_blocking
        assert policy.enforce_coverage
        assert policy.min_acceptance_samples == 2
        assert policy.extra_repair_attempts == 1
        assert "stub_detection_blocking" in policy.applied
        assert "coarse_acceptance_proof_blocking" in policy.applied
        assert "coverage_enforced" in policy.applied

    def test_default_policy_easy_task_advisory(self):
        policy = resolve_rigor_policy(_task(), None, config=None)
        assert policy.stub_enabled and not policy.stub_blocking
        assert policy.effort_enabled and not policy.effort_blocking
        assert policy.coarse_acceptance_enabled and not policy.coarse_acceptance_blocking
        assert not policy.enforce_coverage
        assert policy.extra_repair_attempts == 0
        assert policy.applied == []

    def test_disabled_rigor_turns_gates_off(self):
        class _Rigor:
            enabled = False

        class _Verification:
            rigor = _Rigor()

        class _Config:
            verification = _Verification()

        policy = resolve_rigor_policy(_task(difficulty="hard"), None, config=_Config())
        assert not policy.stub_enabled
        assert not policy.effort_enabled
        assert not policy.enforce_coverage

    def test_mode_always_blocks_on_easy(self):
        class _Rigor:
            enabled = True
            stub_detection = "always"
            effort_heuristics = "never"
            enforce_coverage_on_hard = True
            reviewer_required_on_hard = False
            extra_repair_attempts_on_hard = 2
            min_added_lines_per_planned_file = 5

        class _Verification:
            rigor = _Rigor()

        class _Config:
            verification = _Verification()

        policy = resolve_rigor_policy(_task(), None, config=_Config())
        assert policy.stub_blocking  # "always" blocks even on easy
        assert not policy.effort_enabled  # "never" disables

    def test_broken_config_degrades_to_defaults(self):
        policy = resolve_rigor_policy(_task(), None, config=object())
        assert isinstance(policy, RigorPolicy)
        assert policy.stub_enabled

    def test_reviewer_required_recorded_in_applied(self):
        class _Rigor:
            enabled = True
            stub_detection = "hard"
            effort_heuristics = "hard"
            enforce_coverage_on_hard = False
            reviewer_required_on_hard = True
            extra_repair_attempts_on_hard = 0
            min_added_lines_per_planned_file = 5
            acceptance_samples_on_hard = 2

        class _Verification:
            rigor = _Rigor()

        class _Config:
            verification = _Verification()

        policy = resolve_rigor_policy(_task(difficulty="hard"), None, config=_Config())
        assert policy.reviewer_required
        assert "reviewer_required" in policy.applied
        assert "acceptance_samples:2" in policy.applied
