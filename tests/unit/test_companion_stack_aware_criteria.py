"""Stack-aware planner/verification criteria.

Locks in the fix for the benchmark's verdict miscalibration: DevCouncil must not
block behaviorally-correct code on verification commands that target a stack the
repo doesn't have (e.g. `npm test` on a Python repo) or that are style/type quality
gates rather than correctness proofs (flake8/mypy from the config fallback).
"""

import asyncio

import yaml

from devcouncil.cli.commands.init import initialize_project
from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.verifier import Verifier


def _py_repo(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\nname='x'\n", encoding="utf-8")
    return tmp_path


def _task(**overrides) -> Task:
    base = dict(
        id="TASK-001",
        title="Implement thing",
        description="Implement thing",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[PlannedFile(path="src/thing.py", reason="logic", allowed_change="modify")],
    )
    base.update(overrides)
    return Task(**base)


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-001", title="Thing", description="Thing works", priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="works", verification_method="unit_test")],
    )


# --- init: stack-aware default commands -------------------------------------

def test_init_python_repo_gets_only_python_commands(tmp_path):
    _py_repo(tmp_path)
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    cfg = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    commands = cfg["commands"]
    flat = " ".join(commands["test"] + commands["lint"] + commands["typecheck"])
    assert "pytest" in flat
    # No cross-stack gates that would false-block a Python repo.
    assert "npm" not in flat and "eslint" not in flat and "tsc" not in flat


def test_init_no_stack_gets_empty_commands(tmp_path):
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    cfg = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert cfg["commands"] == {"test": [], "lint": [], "typecheck": []}


def test_init_scaffolds_verification_rigor_defaults(tmp_path):
    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    cfg = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    rigor = cfg["verification"]["rigor"]
    assert rigor["enabled"] is True
    assert rigor["stub_detection"] == "hard"
    assert rigor["effort_heuristics"] == "hard"
    assert rigor["enforce_coverage_on_hard"] is True
    assert rigor["extra_repair_attempts_on_hard"] == 1


# --- verifier: _command_applicable ------------------------------------------

def test_command_applicable_rejects_wrong_stack(tmp_path):
    v = Verifier(_py_repo(tmp_path))
    ok, reason = v._command_applicable("npm test")
    assert ok is False and "node" in reason
    assert v._command_applicable('python -c "import thing"')[0] is True
    assert v._command_applicable("pytest tests/test_thing.py")[0] is True


# --- verifier: wrong-stack expected_test is skipped, not a blocking failure --

def test_wrong_stack_expected_test_is_skipped_not_blocking(tmp_path):
    v = Verifier(_py_repo(tmp_path))
    v.get_changed_files = lambda: ["src/thing.py"]
    v.get_diff = lambda: "diff --git a/src/thing.py b/src/thing.py\n+x"
    # If npm test were (wrongly) executed, this stub would pass it (exit 0) and no
    # gap would appear — so a surviving skipped-gap proves it was filtered, not run.
    v._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="ran"
    )
    task = _task(expected_tests=["npm test"])
    gaps, _ = asyncio.run(v.verify_task(task, [_requirement()]))
    skipped = [g for g in gaps if g.gap_type == "skipped_verification_command"]
    assert skipped and skipped[0].blocking is False
    assert not [g for g in gaps if g.gap_type == "test_failed"]


# --- verifier: quality-only command detection -------------------------------

def test_is_quality_only_command(tmp_path):
    v = Verifier(_py_repo(tmp_path))
    for q in ["black --check stats.py", "flake8 intervals.py", "python -m mypy x.py",
              "npx eslint .", "poetry run ruff check .", "npm run lint"]:
        assert v._is_quality_only_command(q) is True, q
    for behavioral in ["pytest tests/test_x.py", 'python -c "import x; assert x.f()==1"',
                       "python -m pytest -q", "node test.js"]:
        assert v._is_quality_only_command(behavioral) is False, behavioral


def test_quality_command_in_expected_tests_is_advisory(tmp_path):
    # The planner sometimes spawns a dedicated "run black --check" task; a formatting
    # failure must not block a behaviorally-correct task.
    v = Verifier(_py_repo(tmp_path))
    v.get_changed_files = lambda: ["stats.py"]
    v.get_diff = lambda: "diff --git a/stats.py b/stats.py\n+x"
    v._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=1, stdout_path="", stderr_path="", summary="would reformat stats.py"
    )
    task = _task(expected_tests=["black --check stats.py"], acceptance_criterion_ids=[])
    gaps, _ = asyncio.run(v.verify_task(task, [_requirement()]))
    assert not [g for g in gaps if g.gap_type == "test_failed" and g.blocking]
    assert [g for g in gaps if g.gap_type == "quality_gate_failed" and not g.blocking]


# --- verifier: failing lint/typecheck fallback is advisory, not blocking -----

def test_failing_lint_fallback_is_advisory(tmp_path):
    v = Verifier(_py_repo(tmp_path))
    v.get_changed_files = lambda: ["src/thing.py"]
    v.get_diff = lambda: "diff --git a/src/thing.py b/src/thing.py\n+x"
    # Task declares no tests/commands -> verifier falls back to config commands.
    v._load_commands = lambda: {"test": [], "lint": ["ruff check ."], "typecheck": []}
    v._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=1, stdout_path="", stderr_path="", summary="E501 line too long"
    )
    task = _task(expected_tests=[], allowed_commands=[])
    gaps, _ = asyncio.run(v.verify_task(task, [_requirement()]))
    quality = [g for g in gaps if g.gap_type == "quality_gate_failed"]
    assert quality and quality[0].blocking is False
    # A style failure must not be recorded as a blocking correctness defect.
    assert not [g for g in gaps if g.gap_type == "test_failed" and g.blocking]
