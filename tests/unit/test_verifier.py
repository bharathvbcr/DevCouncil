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


def test_verifier_classifies_syntaxerror_command_as_malformed_not_code_failure(tmp_path):
    task = _task()
    task.expected_tests = ['python -c "import m; try: m.f()\nexcept ValueError: pass"']
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=1,
        stdout_path="",
        stderr_path="",
        summary="Exit code 1. stdout: (empty). stderr:   File \"<string>\", line 1\n    import m; try: m.f()\nSyntaxError: invalid syntax",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    # A broken command must NOT be reported as a code/test failure, and a command
    # that cannot run is not evidence of a defect, so it must not block on its own.
    assert not [g for g in gaps if g.gap_type == "test_failed"]
    bad = [g for g in gaps if g.gap_type == "invalid_verification_command"]
    assert bad and not bad[0].blocking
    assert "could not run" in bad[0].description.lower()


def test_verifier_real_test_failure_stays_test_failed(tmp_path):
    task = _task()
    task.expected_tests = ['python -c "import m; assert m.f() == 1"']
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command,
        exit_code=1,
        stdout_path="",
        stderr_path="",
        summary="Exit code 1. stdout: (empty). stderr: AssertionError",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert any(g.gap_type == "test_failed" and g.blocking for g in gaps)
    assert not [g for g in gaps if g.gap_type == "invalid_verification_command"]


def test_verifier_uses_compiled_acceptance_checks_per_criterion(tmp_path):
    # When an acceptance compiler is available, the verifier runs DevCouncil-owned
    # per-criterion checks and maps evidence 1:1 — a passing check proves exactly
    # its criterion (not "any command passed -> everything proven").
    class FakeCompiler:
        async def compile(self, task, requirements, code_context):
            return {"AC-001": ['python -c "assert True"']}

    task = _task()
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._commands_for_task = lambda task: {}  # no expected_tests; rely on compiled checks
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok",
    )

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    proven = [ev for ev in evidence if isinstance(ev, TestEvidence) and ev.acceptance_criterion_id == "AC-001" and ev.status == "passed"]
    assert proven
    assert not any(g.gap_type == "acceptance_criteria_unproven" and g.blocking for g in gaps)


def test_verifier_compiled_check_failure_blocks_its_criterion(tmp_path):
    class FakeCompiler:
        async def compile(self, task, requirements, code_context):
            return {"AC-001": ['python -c "assert calc.x"']}

    task = _task()
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._commands_for_task = lambda task: {}
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=1, stdout_path="", stderr_path="", summary="AssertionError",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert any(g.gap_type == "test_failed" and g.blocking for g in gaps)


def test_verifier_repairs_unrunnable_compiled_check(tmp_path):
    # Local-model fix: a compiled check that FAILS TO RUN (wrong import) is regenerated
    # from the launcher error and re-run. A check that never ran proves nothing, so this
    # converts a false "incomplete" into real passing evidence without weakening the gate.
    class FakeCompiler:
        async def compile_candidates(self, task, requirements, code_context, samples=1, **kwargs):
            return {"AC-001": ['python -c "import wrongname"']}

        async def repair(self, ac_id, ac_description, failing_command, error_summary, code_context):
            return 'python -c "assert True"'

    task = _task()
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._commands_for_task = lambda task: {}

    def fake_run(command, task_id="verify"):
        if "wrongname" in command:  # original, unrunnable
            return CommandResult(command=command, exit_code=1, stdout_path="", stderr_path="",
                                 summary="Exit code 1. stderr: No module named wrongname")
        return CommandResult(command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok")
    verifier._run_command = fake_run

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert [ev for ev in evidence if isinstance(ev, TestEvidence) and ev.acceptance_criterion_id == "AC-001" and ev.status == "passed"]
    assert not any(g.gap_type == "acceptance_criteria_unproven" and g.blocking for g in gaps)


def test_verifier_records_vote_mode_and_tally_in_evidence(tmp_path):
    # Auditability: a criterion proven by a multi-check majority records mode="vote" and
    # the tally in its evidence summary, so a "passed" can be inspected for rigor.
    class FakeCompiler:
        async def compile_candidates(self, task, requirements, code_context, samples=1, **kwargs):
            return {"AC-001": ['python -c "assert ok1"', 'python -c "assert ok2"', 'python -c "assert bad"']}

    task = _task()
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._commands_for_task = lambda task: {}

    def fake_run(command, task_id="verify"):
        if "bad" in command:
            return CommandResult(command=command, exit_code=1, stdout_path="", stderr_path="",
                                 summary="Exit code 1. stderr: AssertionError")
        return CommandResult(command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok")
    verifier._run_command = fake_run

    _, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    ev = [e for e in evidence if isinstance(e, TestEvidence) and e.acceptance_criterion_id == "AC-001" and e.status == "passed"]
    assert ev and ev[0].mode == "vote"
    assert "2/3 passed" in ev[0].evidence_summary


def test_verifier_majority_vote_proves_criterion(tmp_path):
    # Self-consistency: two independent checks pass, one mis-generated check fails by
    # assertion. A strict majority pass proves the criterion — the lone bad check is
    # outvoted instead of false-blocking correct code.
    class FakeCompiler:
        async def compile_candidates(self, task, requirements, code_context, samples=1, **kwargs):
            return {"AC-001": ['python -c "assert ok1"', 'python -c "assert ok2"', 'python -c "assert bad"']}

    task = _task()
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._commands_for_task = lambda task: {}

    def fake_run(command, task_id="verify"):
        if "bad" in command:
            return CommandResult(command=command, exit_code=1, stdout_path="", stderr_path="",
                                 summary="Exit code 1. stderr: AssertionError")
        return CommandResult(command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok")
    verifier._run_command = fake_run

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert [ev for ev in evidence if isinstance(ev, TestEvidence) and ev.acceptance_criterion_id == "AC-001" and ev.status == "passed"]
    assert not any(g.blocking for g in gaps)


def test_verifier_split_vote_is_inconclusive_and_non_blocking(tmp_path):
    # When independent checks split with no majority, the result is inconclusive:
    # neither proof nor a defect. The criterion stays unproven but does NOT block
    # (no false-block on a lone bad check, no false-pass of a real bug).
    class FakeCompiler:
        async def compile_candidates(self, task, requirements, code_context, samples=1, **kwargs):
            return {"AC-001": ['python -c "assert ok"', 'python -c "assert bad"']}

    task = _task()
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._commands_for_task = lambda task: {}

    def fake_run(command, task_id="verify"):
        if "bad" in command:
            return CommandResult(command=command, exit_code=1, stdout_path="", stderr_path="",
                                 summary="Exit code 1. stderr: AssertionError")
        return CommandResult(command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok")
    verifier._run_command = fake_run

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not [ev for ev in evidence if isinstance(ev, TestEvidence) and ev.status == "passed"]
    unproven = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven" and g.acceptance_criterion_id == "AC-001"]
    assert unproven and not unproven[0].blocking  # inconclusive -> surfaced, not blocked


def test_verifier_blocks_unproven_acceptance_criteria(tmp_path):
    # A task with a verification contract but no passing evidence and no way to
    # verify (no commands at all) must block — there is no proof of completion.
    task = _task()
    task.allowed_commands = []
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._load_commands = lambda: {"test": [], "lint": [], "typecheck": []}

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not [ev for ev in evidence if isinstance(ev, TestEvidence)]
    assert any(
        gap.gap_type == "acceptance_criteria_unproven" and gap.blocking
        for gap in gaps
    )


def test_verifier_does_not_block_when_only_failures_are_unrunnable(tmp_path):
    # The false-negative fix: if verification was attempted but every failing
    # command was unrunnable (missing tool / missing tests) and nothing genuinely
    # failed, do NOT block correct work — surface it as a non-blocking gap.
    task = _task()
    task.expected_tests = ["python -m flake8 src/auth.py", "python -m pytest tests/test_auth.py -q"]
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=1, stdout_path="", stderr_path="",
        summary="Exit code 1. stderr: No module named flake8",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not [g for g in gaps if g.gap_type == "test_failed"]
    assert not [g for g in gaps if g.blocking]  # nothing blocks on unrunnable verification


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


def _write_after_snapshot(tmp_path, task_id):
    snap = tmp_path / ".devcouncil" / "semantic" / task_id
    snap.mkdir(parents=True, exist_ok=True)
    (snap / "after.json").write_text("{}", encoding="utf-8")


def _patch_semantic_diff(monkeypatch, classifications):
    from devcouncil.indexing import semantic_index as si
    monkeypatch.setattr(si.SemanticIndex, "diff", lambda self, tid: {"classifications": classifications})


def test_verifier_blocks_unintended_public_symbol_removal(tmp_path, monkeypatch):
    # Drift WITHIN an allowed file: the executor removed an existing public symbol the
    # task never asked about. That is scope drift / a regression and must block.
    task = _task()
    _write_after_snapshot(tmp_path, task.id)
    _patch_semantic_diff(monkeypatch, [
        {"type": "exported_symbol_removed", "path": "src/auth.py", "name": "LegacyHelper"},
    ])
    gaps = Verifier(tmp_path)._check_semantic_diff(task, [_requirement()])
    drift = [g for g in gaps if g.gap_type == "architecture_drift"]
    assert drift and drift[0].blocking
    assert "LegacyHelper" in drift[0].description


def test_verifier_allows_intended_public_symbol_removal(tmp_path, monkeypatch):
    # If the task explicitly calls for the removal, it is an intended decision, not drift.
    task = _task()
    task.description = "Remove the deprecated LegacyHelper export and its callers."
    _write_after_snapshot(tmp_path, task.id)
    _patch_semantic_diff(monkeypatch, [
        {"type": "exported_symbol_removed", "path": "src/auth.py", "name": "LegacyHelper"},
    ])
    gaps = Verifier(tmp_path)._check_semantic_diff(task, [_requirement()])
    drift = [g for g in gaps if g.gap_type == "architecture_drift"]
    assert drift and not drift[0].blocking  # surfaced, but not blocked


def test_verifier_treats_moved_public_symbol_as_non_blocking(tmp_path, monkeypatch):
    # Removed from one file and re-exported elsewhere = a move/rename refactor, not drift.
    task = _task()
    _write_after_snapshot(tmp_path, task.id)
    _patch_semantic_diff(monkeypatch, [
        {"type": "exported_symbol_removed", "path": "src/auth.py", "name": "Helper"},
        {"type": "exported_symbol_added", "path": "src/util.py", "name": "Helper"},
    ])
    gaps = Verifier(tmp_path)._check_semantic_diff(task, [_requirement()])
    drift = [g for g in gaps if g.gap_type == "architecture_drift"]
    assert drift and not drift[0].blocking


def test_verifier_signature_change_on_planned_file_is_advisory(tmp_path, monkeypatch):
    # A public signature change on a file the task OWNS is surfaced but never blocks —
    # tasks legitimately change signatures of their planned files.
    task = _task()
    _write_after_snapshot(tmp_path, task.id)
    _patch_semantic_diff(monkeypatch, [
        {"type": "public_api_change", "path": "src/auth.py", "name": "login"},
    ])
    gaps = Verifier(tmp_path)._check_semantic_diff(task, [_requirement()])
    drift = [g for g in gaps if g.gap_type == "architecture_drift"]
    assert drift and not drift[0].blocking
    assert "signature" in drift[0].description.lower()


def test_verifier_blocks_new_third_party_import(tmp_path, monkeypatch):
    # An undeclared, not-installed third-party package = supply-chain drift -> block,
    # even in a planned file.
    task = _task()
    _write_after_snapshot(tmp_path, task.id)
    _patch_semantic_diff(monkeypatch, [
        {"type": "import_dependency_change", "path": "src/auth.py",
         "statement": "import zzz_nonexistent_pkg_qwerty"},
    ])
    gaps = Verifier(tmp_path)._check_semantic_diff(task, [_requirement()])
    dep = [g for g in gaps if g.gap_type == "dependency_risk"]
    assert dep and dep[0].blocking
    assert "zzz_nonexistent_pkg_qwerty" in dep[0].description


def test_verifier_does_not_block_stdlib_or_relative_imports(tmp_path, monkeypatch):
    # stdlib and relative/local imports are never new dependencies. On an UNPLANNED file
    # they still surface as advisory dependency_risk gaps, but must never block.
    task = _task()
    _write_after_snapshot(tmp_path, task.id)
    _patch_semantic_diff(monkeypatch, [
        {"type": "import_dependency_change", "path": "src/unplanned.py", "statement": "import os"},
        {"type": "import_dependency_change", "path": "src/unplanned.py", "statement": "from . import helpers"},
    ])
    gaps = Verifier(tmp_path)._check_semantic_diff(task, [_requirement()])
    import_gaps = [g for g in gaps if g.gap_type == "dependency_risk"]
    assert import_gaps                              # the imports WERE processed (advisory)
    assert not any(g.blocking for g in import_gaps)  # but stdlib/relative never block


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


def _rigor_config_yaml(**rigor_overrides) -> str:
    lines = [
        "project:\n  name: test\n",
        "verification:\n  rigor:\n    enabled: true\n",
    ]
    for key, value in rigor_overrides.items():
        if isinstance(value, bool):
            lines.append(f"    {key}: {'true' if value else 'false'}\n")
        elif isinstance(value, str):
            lines.append(f"    {key}: {value}\n")
        else:
            lines.append(f"    {key}: {value}\n")
    return "".join(lines)


def test_verifier_stub_gate_blocks_on_hard_task(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(_rigor_config_yaml(), encoding="utf-8")
    task = _task()
    task.difficulty = "hard"
    diff = (
        "diff --git a/src/auth.py b/src/auth.py\n"
        "--- a/src/auth.py\n"
        "+++ b/src/auth.py\n"
        "@@ -1 +1,2 @@\n"
        "+# TODO: finish token logic\n"
    )
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: diff
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    stub = [g for g in gaps if g.gap_type == "stub_detected"]
    assert stub and stub[0].blocking
    assert verifier.last_outcome is not None
    assert verifier.last_outcome.difficulty == "hard"
    assert "stub_detection_blocking" in verifier.last_outcome.rigor_applied


def test_verifier_stub_gate_advisory_on_easy_task(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(_rigor_config_yaml(), encoding="utf-8")
    task = _task()
    task.difficulty = "easy"
    diff = (
        "diff --git a/src/auth.py b/src/auth.py\n"
        "--- a/src/auth.py\n"
        "+++ b/src/auth.py\n"
        "@@ -1 +1,2 @@\n"
        "+# TODO: finish token logic\n"
    )
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: diff
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    stub = [g for g in gaps if g.gap_type == "stub_detected"]
    assert stub and not stub[0].blocking


def test_verifier_effort_gate_advisory_on_normal_task(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(_rigor_config_yaml(), encoding="utf-8")
    task = Task(
        id="TASK-001",
        title="Big scope",
        description="Touch many files",
        difficulty="normal",
        acceptance_criterion_ids=["AC-001"],
        planned_files=[
            PlannedFile(path=f"src/f{i}.py", reason="edit", allowed_change="modify")
            for i in range(4)
        ],
    )
    diff = (
        "diff --git a/src/f0.py b/src/f0.py\n"
        "--- a/src/f0.py\n"
        "+++ b/src/f0.py\n"
        "@@ -0,0 +1,1 @@\n"
        "+x = 1\n"
    )
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/f0.py"]
    verifier.get_diff = lambda: diff
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    effort = [g for g in gaps if g.gap_type == "suspicious_effort"]
    assert effort and not effort[0].blocking


def test_verifier_effort_gate_blocks_on_hard_task(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(_rigor_config_yaml(), encoding="utf-8")
    task = Task(
        id="TASK-001",
        title="Big scope",
        description="Touch many files",
        difficulty="hard",
        acceptance_criterion_ids=["AC-001"],
        planned_files=[
            PlannedFile(path=f"src/f{i}.py", reason="edit", allowed_change="modify")
            for i in range(4)
        ],
    )
    diff = (
        "diff --git a/src/f0.py b/src/f0.py\n"
        "--- a/src/f0.py\n"
        "+++ b/src/f0.py\n"
        "@@ -0,0 +1,1 @@\n"
        "+x = 1\n"
    )
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/f0.py"]
    verifier.get_diff = lambda: diff
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    effort = [g for g in gaps if g.gap_type == "suspicious_effort"]
    assert effort and effort[0].blocking
    assert "effort_heuristics_blocking" in verifier.last_outcome.rigor_applied


def test_verifier_hard_task_enforces_coverage_on_rigor(tmp_path):
    from devcouncil.verification import diff_coverage as dc

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(_rigor_config_yaml(), encoding="utf-8")
    task = _task()
    task.difficulty = "hard"
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._diff_coverage_override = (True, False, 0.0)
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )
    verifier.measure_diff_coverage = lambda task, diff_content: dc.DiffCoverageResult(
        measured=True,
        tool="pytest",
        changed_executable_lines=4,
        covered_changed_lines=0,
        uncovered_by_file={"src/auth.py": [10]},
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    cov_gaps = [g for g in gaps if g.gap_type == "diff_not_exercised"]
    assert cov_gaps and cov_gaps[0].blocking
    assert verifier.last_outcome is not None
    assert "coverage_enforced" in verifier.last_outcome.rigor_applied


def test_verifier_reviewer_required_on_hard_blocks_critical(tmp_path):
    from devcouncil.domain.gap import Gap
    from devcouncil.verification.implementation_reviewer import ReviewOutput

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        _rigor_config_yaml(reviewer_required_on_hard=True),
        encoding="utf-8",
    )
    task = _task()
    task.difficulty = "hard"
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    class _FakeReviewer:
        async def review_changes(self, task, requirements, diff):
            return ReviewOutput(
                is_satisfactory=False,
                findings=[
                    Gap(
                        id="REVIEW-1",
                        severity="critical",
                        gap_type="architecture_drift",
                        task_id=task.id,
                        description="Critical design mismatch",
                        recommended_fix="Rework the module boundary",
                        blocking=False,
                    )
                ],
            )

    verifier.reviewer = _FakeReviewer()

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    review = [g for g in gaps if g.gap_type == "architecture_drift"]
    assert review and review[0].blocking
    assert "reviewer_required" in verifier.last_outcome.rigor_applied


def test_assert_free_unplanned_test_file_stays_blocking_orphan(tmp_path):
    """Trivial assert-free unplanned test files must not be demoted to advisory orphans."""
    tests = tmp_path / "tests"
    tests.mkdir()
    (tests / "test_extra.py").write_text(
        "def test_calls_only():\n    import mod\n    mod.run()\n",
        encoding="utf-8",
    )
    task = _task()
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["src/auth.py", "tests/test_extra.py"]
    verifier.get_diff = lambda: (
        "diff --git a/tests/test_extra.py b/tests/test_extra.py\n"
        "--- /dev/null\n"
        "+++ b/tests/test_extra.py\n"
        "@@ -0,0 +1,3 @@\n"
        "+def test_calls_only():\n"
        "+    import mod\n"
        "+    mod.run()\n"
    )
    verifier._classify_change_paths = lambda changed: (["tests/test_extra.py"], [])
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    orphans = {g.file: g for g in gaps if g.gap_type == "orphan_diff"}
    assert orphans["tests/test_extra.py"].blocking


def test_agent_appended_expected_test_cannot_coarse_prove_ac(tmp_path):
    """Agent-appended expected_tests may run but must not coarse-prove acceptance criteria."""
    task = _task()
    task.expected_tests = ['python -c "assert True"']
    task.agent_appended_expected_tests = ['python -c "assert True"']
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = None
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not any(g.gap_type == "coarse_acceptance_proof" for g in gaps)
    unproven = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven"]
    assert unproven and unproven[0].blocking


def test_coarse_acceptance_proof_blocks_on_hard_task(tmp_path):
    """On hard tasks, coarse AC proof must block — not merely advise."""
    task = _task()
    task.difficulty = "hard"
    task.expected_tests = ["pytest tests/test_auth.py"]
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = None
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    coarse = [g for g in gaps if g.gap_type == "coarse_acceptance_proof"]
    assert coarse and coarse[0].blocking
    assert coarse[0].severity == "high"
    assert not any(g.gap_type == "acceptance_criteria_unproven" for g in gaps)


def test_planner_trivial_expected_test_cannot_coarse_prove(tmp_path):
    """Planner-originated expected_tests without evidence keywords must not coarse-prove."""
    task = _task()
    task.expected_tests = ['python -c "print(\'ok\')"']
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = None
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not any(g.gap_type == "coarse_acceptance_proof" for g in gaps)
    unproven = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven"]
    assert unproven and unproven[0].blocking


def test_agent_appended_allowed_command_cannot_coarse_prove_ac(tmp_path):
    """Agent-appended allowed_commands may run but must not coarse-prove acceptance
    criteria — the same self-certification guard as agent_appended_expected_tests,
    covering the side door of a task whose expected_tests is empty."""
    task = _task()
    task.expected_tests = []
    task.allowed_commands = ["./run_tests.sh"]
    task.agent_appended_allowed_commands = ["./run_tests.sh"]
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = None
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not any(g.gap_type == "coarse_acceptance_proof" for g in gaps)
    unproven = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven"]
    assert unproven and unproven[0].blocking


def test_planner_allowed_command_still_coarse_proves(tmp_path):
    """The same command WITHOUT agent-append provenance keeps its evidential value —
    the exclusion is provenance-based, not command-based."""
    task = _task()
    task.expected_tests = []
    task.allowed_commands = ["./run_tests.sh"]
    task.agent_appended_allowed_commands = []
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = None
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert any(g.gap_type == "coarse_acceptance_proof" for g in gaps)
    assert not any(g.gap_type == "acceptance_criteria_unproven" for g in gaps)
