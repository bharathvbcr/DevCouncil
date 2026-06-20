"""Repair-context + history hygiene for the `dev go` bounded self-repair loop.

Two deferred improvements are covered here:

1. The correction manifest now carries the PRIOR attempt's diff (``prior_diff``) and
   the captured failing verification output (``failing_output``) so the next executor
   repairs against what actually happened instead of re-deriving the same wrong
   approach blind. Both must be size-bounded and secret-redacted.

2. On a task that ultimately verifies, the intermediate ``[blocked]`` attempt commits
   are squashed into one verified commit so failed attempts don't pollute git history.
   The squash must preserve the checkpoint refs the verifier's empty-diff guard and
   ``dev rollback`` depend on.
"""

import subprocess
from pathlib import Path

import devcouncil.cli.commands.go as go
from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.planning import correction_manifest as cm
from devcouncil.planning.correction_manifest import build_correction_manifest
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import EvidenceRepository, GapRepository, TaskRepository


# --------------------------------------------------------------------------- #
# Part 1: correction manifest carries bounded, redacted prior-attempt context
# --------------------------------------------------------------------------- #


def _seed_project(tmp_path: Path) -> Task:
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\nexecution:\n  max_repair_attempts: 3\n", encoding="utf-8"
    )
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    task = Task(
        id="TASK-001", title="T", description="D",
        planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
        expected_tests=["pytest tests/a"],
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
        GapRepository(session).save(Gap(
            id="GAP-1", severity="high", gap_type="test_failed", task_id="TASK-001",
            description="tests failed", recommended_fix="fix", blocking=True,
        ))
    return task


def _gap() -> Gap:
    return Gap(
        id="GAP-1", severity="high", gap_type="test_failed", task_id="TASK-001",
        description="tests failed", recommended_fix="fix", blocking=True,
    )


def test_manifest_carries_prior_diff_and_failing_output(tmp_path):
    task = _seed_project(tmp_path)

    # The checkpoint service writes the prior attempt's diff here.
    checkpoints = tmp_path / ".devcouncil" / "checkpoints"
    checkpoints.mkdir(parents=True)
    (checkpoints / "TASK-001-after.patch").write_text(
        "diff --git a/src/a.py b/src/a.py\n@@ -1 +1 @@\n-old\n+broken change\n",
        encoding="utf-8",
    )

    # A failing command with captured stdout/stderr files.
    stdout = tmp_path / ".devcouncil" / "out.txt"
    stderr = tmp_path / ".devcouncil" / "err.txt"
    stdout.write_text("collected 1 item\n", encoding="utf-8")
    stderr.write_text("AssertionError: expected 4 got 5\n", encoding="utf-8")
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        EvidenceRepository(session).save_command_result("TASK-001", CommandResult(
            command="pytest tests/a", exit_code=1,
            stdout_path=str(stdout), stderr_path=str(stderr), summary="1 failed",
        ))

    manifest = build_correction_manifest(tmp_path, task, [_gap()])

    assert "broken change" in manifest.prior_diff
    assert "pytest tests/a" in manifest.failing_output
    assert "AssertionError: expected 4 got 5" in manifest.failing_output
    assert "1 failed" in manifest.failing_output


def test_prior_context_is_empty_when_absent(tmp_path):
    # First attempt: no after.patch, no failed evidence -> fields stay empty (and the
    # manifest is still produced, backward-compatible).
    task = _seed_project(tmp_path)
    manifest = build_correction_manifest(tmp_path, task, [_gap()])
    assert manifest.prior_diff == ""
    assert manifest.failing_output == ""


def test_prior_diff_is_bounded(tmp_path):
    task = _seed_project(tmp_path)
    checkpoints = tmp_path / ".devcouncil" / "checkpoints"
    checkpoints.mkdir(parents=True)
    huge = "+" + ("x" * 50_000)
    (checkpoints / "TASK-001-after.patch").write_text(huge, encoding="utf-8")

    manifest = build_correction_manifest(tmp_path, task, [_gap()])

    assert 0 < len(manifest.prior_diff) <= cm._MAX_PRIOR_DIFF_CHARS + 200
    assert "diff truncated" in manifest.prior_diff


def test_failing_output_is_bounded(tmp_path):
    task = _seed_project(tmp_path)
    stderr = tmp_path / ".devcouncil" / "err.txt"
    stderr.write_text("E" * 50_000, encoding="utf-8")
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        EvidenceRepository(session).save_command_result("TASK-001", CommandResult(
            command="pytest", exit_code=1, stdout_path="", stderr_path=str(stderr), summary="boom",
        ))

    manifest = build_correction_manifest(tmp_path, task, [_gap()])

    assert 0 < len(manifest.failing_output) <= cm._MAX_FAILING_OUTPUT_CHARS + 200
    assert "truncated" in manifest.failing_output


def test_prior_context_is_redacted(tmp_path):
    task = _seed_project(tmp_path)
    checkpoints = tmp_path / ".devcouncil" / "checkpoints"
    checkpoints.mkdir(parents=True)
    (checkpoints / "TASK-001-after.patch").write_text(
        "+api_key = sk-ant-abcdefghij0123456789klmnop\n", encoding="utf-8"
    )
    stderr = tmp_path / ".devcouncil" / "err.txt"
    stderr.write_text("token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\n", encoding="utf-8")
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        EvidenceRepository(session).save_command_result("TASK-001", CommandResult(
            command="pytest", exit_code=1, stdout_path="", stderr_path=str(stderr), summary="boom",
        ))

    manifest = build_correction_manifest(tmp_path, task, [_gap()])

    assert "sk-ant-abcdefghij0123456789klmnop" not in manifest.prior_diff
    assert "REDACTED" in manifest.prior_diff
    assert "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789" not in manifest.failing_output
    assert "REDACTED" in manifest.failing_output


def test_prior_context_reaches_serialized_manifest(tmp_path):
    # The coding-CLI executor folds the whole manifest JSON into the repair prompt, so
    # the new fields reaching model_dump_json proves they reach the next executor.
    task = _seed_project(tmp_path)
    checkpoints = tmp_path / ".devcouncil" / "checkpoints"
    checkpoints.mkdir(parents=True)
    (checkpoints / "TASK-001-after.patch").write_text("+sentinel-diff-marker\n", encoding="utf-8")

    manifest = build_correction_manifest(tmp_path, task, [_gap()])
    payload = manifest.model_dump_json()
    assert "prior_diff" in payload
    assert "failing_output" in payload
    assert "sentinel-diff-marker" in payload


# --------------------------------------------------------------------------- #
# Part 2: squash blocked attempt commits while preserving checkpoint invariants
# --------------------------------------------------------------------------- #

BEFORE_REF = "refs/devcouncil/tasks/TASK-001/before"
AFTER_REF = "refs/devcouncil/tasks/TASK-001/after"


def _git(args, cwd):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)


def _init_repo(path: Path) -> str:
    _git(["init"], path)
    _git(["config", "user.email", "t@e.com"], path)
    _git(["config", "user.name", "T"], path)
    (path / "f.txt").write_text("base\n", encoding="utf-8")
    _git(["add", "-A"], path)
    _git(["commit", "-m", "init"], path)
    return _git(["rev-parse", "HEAD"], path).stdout.strip()


def _log_subjects(path: Path) -> list[str]:
    out = _git(["log", "--format=%s"], path).stdout.strip()
    return out.splitlines() if out else []


def test_squash_collapses_blocked_commits_and_preserves_refs(tmp_path):
    base = _init_repo(tmp_path)
    # The task's `before` checkpoint ref captured at task start (the empty-diff guard
    # diffs against this).
    _git(["update-ref", BEFORE_REF, base], tmp_path)

    # Attempt 1 (blocked) commit.
    (tmp_path / "f.txt").write_text("base\nattempt1\n", encoding="utf-8")
    _git(["add", "-A"], tmp_path)
    _git(["commit", "-m", "devcouncil(e2e): TASK-001 [blocked]"], tmp_path)

    # squash_base would have been recorded just before the first [blocked] commit.
    squash_base = base

    # Final verified attempt commit.
    (tmp_path / "f.txt").write_text("base\nattempt1\nfinal\n", encoding="utf-8")
    _git(["add", "-A"], tmp_path)
    _git(["commit", "-m", "devcouncil(e2e): TASK-001 [verified]"], tmp_path)
    _git(["update-ref", AFTER_REF, _git(["rev-parse", "HEAD"], tmp_path).stdout.strip()], tmp_path)

    assert _log_subjects(tmp_path).count("devcouncil(e2e): TASK-001 [blocked]") == 1

    ok = go._squash_repair_commits(tmp_path, "TASK-001", squash_base, "verified")
    assert ok is True

    subjects = _log_subjects(tmp_path)
    # No blocked attempt remains; exactly one verified commit on top of init.
    assert "devcouncil(e2e): TASK-001 [blocked]" not in subjects
    assert subjects == ["devcouncil(e2e): TASK-001 [verified]", "init"]

    # Working tree content is fully preserved (the squash is content-neutral).
    assert (tmp_path / "f.txt").read_text(encoding="utf-8") == "base\nattempt1\nfinal\n"

    # CRITICAL INVARIANT: checkpoint refs still resolve and the empty-diff guard /
    # rollback diffs are still non-empty after the history rewrite.
    assert _git(["rev-parse", "--verify", BEFORE_REF], tmp_path).returncode == 0
    assert _git(["rev-parse", "--verify", AFTER_REF], tmp_path).returncode == 0
    assert _git(["diff", BEFORE_REF], tmp_path).stdout.strip()  # empty-diff guard sees work
    assert _git(["diff", BEFORE_REF, AFTER_REF], tmp_path).stdout.strip()  # rollback diff intact


def test_squash_noop_when_base_equals_head(tmp_path):
    base = _init_repo(tmp_path)
    # No intermediate commits -> nothing to squash.
    assert go._squash_repair_commits(tmp_path, "TASK-001", base, "verified") is False
    assert _log_subjects(tmp_path) == ["init"]


def test_squash_is_safe_outside_git(tmp_path):
    # Non-git directory: best-effort, never raises, returns False.
    assert go._squash_repair_commits(tmp_path, "TASK-001", "deadbeef", "verified") is False


def test_current_head_resolves_and_degrades(tmp_path):
    assert go._current_head(tmp_path) is None  # no git
    base = _init_repo(tmp_path)
    assert go._current_head(tmp_path) == base
