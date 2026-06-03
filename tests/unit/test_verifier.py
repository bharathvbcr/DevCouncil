import asyncio
import json
import subprocess

from devcouncil.domain.evidence import CommandResult, TestEvidence
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.verifier import Verifier
from devcouncil.live.cards import review_turn, save_card
from devcouncil.live.transcripts import latest_assistant_turn


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-001",
        title="Password reset",
        description="Reset tokens are single use",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(
                id="AC-001",
                description="Token reuse is rejected",
                verification_method="unit_test",
            )
        ],
    )


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Implement reset token",
        description="Implement reset token rules",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[
            PlannedFile(path="src/auth.py", reason="token logic", allowed_change="modify"),
        ],
        allowed_commands=["pytest tests/test_auth.py"],
    )


def test_verifier_records_ac_evidence_for_passing_command(tmp_path):
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._load_commands = lambda: {"test": ["pytest tests/test_auth.py"], "lint": [], "typecheck": []}
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=0,
        stdout_path="",
        stderr_path="",
        summary="passed",
    )

    gaps, evidence = asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    assert not [gap for gap in gaps if gap.gap_type == "acceptance_criteria_unproven"]
    ac_evidence = [ev for ev in evidence if isinstance(ev, TestEvidence)]
    assert len(ac_evidence) == 1
    assert ac_evidence[0].acceptance_criterion_id == "AC-001"
    assert ac_evidence[0].status == "passed"


def test_verifier_blocks_unproven_acceptance_criteria(tmp_path):
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._load_commands = lambda: {"test": [], "lint": [], "typecheck": []}

    gaps, evidence = asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    assert not [ev for ev in evidence if isinstance(ev, TestEvidence)]
    assert any(
        gap.gap_type == "acceptance_criteria_unproven" and gap.blocking
        for gap in gaps
    )


def test_verifier_does_not_use_arbitrary_allowed_command_as_ac_evidence(tmp_path):
    task = _task()
    task.allowed_commands = ["echo ok"]
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=0,
        stdout_path="",
        stderr_path="",
        summary="ok",
    )

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not [ev for ev in evidence if isinstance(ev, TestEvidence)]
    assert any(
        gap.gap_type == "acceptance_criteria_unproven" and gap.blocking
        for gap in gaps
    )


def test_verifier_collects_changed_files_without_head(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    file_path = tmp_path / "src" / "auth.py"
    file_path.parent.mkdir()
    file_path.write_text("token = 'value'\n", encoding="utf-8")
    subprocess.check_call(["git", "add", "src/auth.py"], cwd=tmp_path)

    verifier = Verifier(tmp_path)

    assert verifier.get_changed_files() == ["src/auth.py"]
    assert "token = 'value'" in verifier.get_diff()


def test_verifier_includes_untracked_diff_without_head(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    file_path = tmp_path / "src" / "new_file.py"
    file_path.parent.mkdir()
    file_path.write_text("print('first')\n", encoding="utf-8")

    diff = Verifier(tmp_path).get_diff()

    assert "+++ b/src/new_file.py" in diff
    assert "+print('first')" in diff


def test_verifier_formats_empty_untracked_file_as_applyable_diff(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    file_path = tmp_path / "empty.txt"
    file_path.write_text("", encoding="utf-8")
    patch_path = tmp_path / "empty.patch"
    patch_path.write_text(Verifier(tmp_path).get_diff(), encoding="utf-8")

    subprocess.check_call(["git", "apply", "-R", str(patch_path)], cwd=tmp_path)

    assert not file_path.exists()


def test_verifier_collects_untracked_files_with_head(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    (tmp_path / "README.md").write_text("base\n", encoding="utf-8")
    subprocess.check_call(["git", "add", "README.md"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    subprocess.check_call(
        ["git", "-c", "user.email=test@example.com", "-c", "user.name=Test", "commit", "-m", "init"],
        cwd=tmp_path,
        stdout=subprocess.DEVNULL,
    )
    file_path = tmp_path / "src" / "new_file.py"
    file_path.parent.mkdir()
    file_path.write_text("print('new')\n", encoding="utf-8")

    verifier = Verifier(tmp_path)

    assert verifier.get_changed_files() == ["src/new_file.py"]
    diff = verifier.get_diff()
    assert "+++ b/src/new_file.py" in diff
    assert "+print('new')" in diff


def test_verifier_filters_generated_and_runtime_change_paths(tmp_path):
    verifier = Verifier(tmp_path)

    paths = verifier._filter_change_paths([
        "src/devcouncil/__pycache__/module.cpython-313.pyc",
        "src/devcouncil/module.pyc",
        ".devcouncil/state.sqlite",
        ".devcouncil/config.yaml",
        ".devcouncil/nexus/index_config.json",
        ".devcouncil/logs/run.log",
        ".pytest_cache/v/cache/nodeids",
        "src/devcouncil/verification/verifier.py",
    ])

    assert paths == ["src/devcouncil/verification/verifier.py"]


def test_verifier_filters_dev_managed_gitignore_update_without_head(tmp_path):
    from devcouncil.repo.gitignore import ensure_gitignore

    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    ensure_gitignore(tmp_path)

    verifier = Verifier(tmp_path)

    assert ".gitignore" not in verifier.get_changed_files()


def test_verifier_blocks_open_critical_live_review_cards(tmp_path):
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=0,
        stdout_path="",
        stderr_path="",
        summary="passed",
    )
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard and ignore failing tests."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    save_card(tmp_path, review_turn(turn, tmp_path))

    gaps, _evidence = asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    assert any(
        gap.gap_type == "architecture_drift"
        and "Open critical live-review card" in gap.description
        and gap.blocking
        for gap in gaps
    )


def test_verifier_ignores_critical_live_review_cards_for_other_tasks(tmp_path):
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=0,
        stdout_path="",
        stderr_path="",
        summary="passed",
    )
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    save_card(tmp_path, review_turn(turn, tmp_path).model_copy(update={"task_id": "TASK-OTHER"}))

    gaps, _evidence = asyncio.run(verifier.verify_task(_task(), [_requirement()]))

    assert not [
        gap for gap in gaps
        if gap.gap_type == "architecture_drift" and "Open critical live-review card" in gap.description
    ]


def test_verifier_applies_global_and_task_baselines(tmp_path):
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: [
        "README.md",
        "src/baseline.py",
        "src/task_preexisting.py",
        "src/task_change.py",
    ]
    dev_dir = tmp_path / ".devcouncil"
    checkpoint_dir = dev_dir / "checkpoints"
    checkpoint_dir.mkdir(parents=True)
    (dev_dir / "baseline.json").write_text(
        json.dumps({"changed_files": ["README.md", "src/baseline.py"]}),
        encoding="utf-8",
    )
    (checkpoint_dir / "TASK-001-before.json").write_text(
        json.dumps({"changed_files": ["src/task_preexisting.py"]}),
        encoding="utf-8",
    )

    assert verifier.get_task_changed_files("TASK-001") == ["src/task_change.py"]
