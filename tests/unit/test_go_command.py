"""Helper-level coverage for go.py — git helpers, repair-budget helpers, signatures,
report rendering, and a few `dev go` command branches."""

from __future__ import annotations

import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest
import typer
from typer.testing import CliRunner

import devcouncil.cli.commands.go as go
from devcouncil.cli.main import app

runner = CliRunner()


def _git_repo(tmp_path: Path):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "config", "user.name", "t"], cwd=tmp_path, capture_output=True)


def _commit(tmp_path: Path, name: str, content: str):
    (tmp_path / name).write_text(content, encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "commit", "-m", f"add {name}"], cwd=tmp_path, capture_output=True)


# --- trivial helpers --------------------------------------------------------------


def test_normalize_executor():
    assert go._normalize_executor("  Claude_SDK ") == "claude-sdk"


def test_unique_task_ids():
    assert go._unique_task_ids(["A", "B", "A", "C"]) == ["A", "B", "C"]


def test_command_label():
    ctx = SimpleNamespace(info_name="go")
    assert go._command_label(ctx) == "dev go"
    ctx2 = SimpleNamespace(info_name=None)
    assert go._command_label(ctx2) == "dev e2e"


# --- _is_git_repo -----------------------------------------------------------------


def test_is_git_repo_true(tmp_path):
    _git_repo(tmp_path)
    assert go._is_git_repo(tmp_path) is True


def test_is_git_repo_false_non_git(tmp_path):
    assert go._is_git_repo(tmp_path) is False


def test_is_git_repo_false_on_error(tmp_path, monkeypatch):
    def boom(*a, **k):
        raise RuntimeError("git missing")
    monkeypatch.setattr(go, "run_git", boom)
    assert go._is_git_repo(tmp_path) is False


# --- repair-budget helpers --------------------------------------------------------


def test_max_repair_attempts_default_on_error(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "load_config", lambda root: (_ for _ in ()).throw(RuntimeError()))
    assert go._max_repair_attempts(tmp_path) == 0


def test_max_repair_attempts_reads_config(tmp_path, monkeypatch):
    cfg = SimpleNamespace(execution=SimpleNamespace(max_repair_attempts=3))
    monkeypatch.setattr(go, "load_config", lambda root: cfg)
    assert go._max_repair_attempts(tmp_path) == 3


def test_task_max_repairs_disabled_stays_disabled(tmp_path):
    assert go._task_max_repairs(tmp_path, SimpleNamespace(), 0) == 0


def test_task_max_repairs_widens(tmp_path, monkeypatch):
    cfg = SimpleNamespace()
    monkeypatch.setattr(go, "load_config", lambda root: cfg)
    import devcouncil.verification.difficulty as diff
    monkeypatch.setattr(
        diff, "resolve_rigor_policy",
        lambda task, x, config=None: SimpleNamespace(extra_repair_attempts=2),
    )
    assert go._task_max_repairs(tmp_path, SimpleNamespace(), 1) == 3


def test_task_max_repairs_swallows_errors(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "load_config", lambda root: (_ for _ in ()).throw(RuntimeError()))
    assert go._task_max_repairs(tmp_path, SimpleNamespace(), 2) == 2


# --- gap signatures ---------------------------------------------------------------


def test_blocking_gap_signature_no_db(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "get_db", lambda root: None)
    assert go._blocking_gap_signature(tmp_path, "T1") == ""


def test_remediable_incomplete_signature_no_db(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "get_db", lambda root: None)
    assert go._remediable_incomplete_signature(tmp_path, "T1") == ""


def test_blocking_gap_signature_with_gaps(tmp_path, monkeypatch):
    from devcouncil.domain.gap import Gap
    from devcouncil.domain.task import Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import GapRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="T1", title="t", description="d"))
        GapRepository(session).save(Gap(
            id="G1", severity="high", gap_type="missing_test", description="no test",
            blocking=True, recommended_fix="x", task_id="T1",
        ))
    sig = go._blocking_gap_signature(tmp_path, "T1")
    assert sig  # non-empty fingerprint


# --- executor-run introspection ---------------------------------------------------


def test_executor_run_failed_variants(tmp_path, monkeypatch):
    import devcouncil.planning.correction_manifest as cm
    monkeypatch.setattr(cm, "_latest_agent_run", lambda root, tid: None)
    assert go._executor_run_failed(tmp_path, "T1") is False
    monkeypatch.setattr(cm, "_latest_agent_run", lambda root, tid: {"returncode": 1})
    assert go._executor_run_failed(tmp_path, "T1") is True
    monkeypatch.setattr(cm, "_latest_agent_run", lambda root, tid: {"returncode": 0})
    assert go._executor_run_failed(tmp_path, "T1") is False


def test_executor_run_unavailable_detects_limits(tmp_path, monkeypatch):
    import devcouncil.planning.correction_manifest as cm
    monkeypatch.setattr(
        cm, "_latest_agent_run",
        lambda root, tid: {"returncode": 1, "status": "error", "stderr_preview": ["rate limit exceeded"]},
    )
    assert go._executor_run_unavailable(tmp_path, "T1") is True
    monkeypatch.setattr(
        cm, "_latest_agent_run",
        lambda root, tid: {"returncode": 1, "status": "error", "stderr_preview": "syntax error"},
    )
    assert go._executor_run_unavailable(tmp_path, "T1") is False
    monkeypatch.setattr(cm, "_latest_agent_run", lambda root, tid: {"returncode": 0})
    assert go._executor_run_unavailable(tmp_path, "T1") is False


# --- _reverify_task ---------------------------------------------------------------


def test_reverify_task_no_db(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "get_db", lambda root: None)
    monkeypatch.setattr(go, "_task_status", lambda root, tid: "missing")
    assert go._reverify_task(tmp_path, "T1") == "missing"


def test_reverify_task_handles_exit_and_errors(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "get_db", lambda root: SimpleNamespace())
    monkeypatch.setattr(go, "_task_status", lambda root, tid: "verified")

    def raise_exit(root, tid, sandbox, jf, db):
        raise typer.Exit(code=1)

    monkeypatch.setattr(go.verify_command, "_run_verify_body", raise_exit)
    assert go._reverify_task(tmp_path, "T1") == "verified"

    def raise_err(root, tid, sandbox, jf, db):
        raise RuntimeError("verify boom")

    monkeypatch.setattr(go.verify_command, "_run_verify_body", raise_err)
    assert go._reverify_task(tmp_path, "T1") == "verified"


# --- git commit / head / squash ---------------------------------------------------


def test_current_head_none_non_git(tmp_path):
    assert go._current_head(tmp_path) is None


def test_current_head_in_git(tmp_path):
    _git_repo(tmp_path)
    _commit(tmp_path, "a.txt", "1")
    assert go._current_head(tmp_path)


def test_commit_task_changes_clean_tree(tmp_path):
    _git_repo(tmp_path)
    _commit(tmp_path, "a.txt", "1")
    # No changes → nothing to commit.
    assert go._commit_task_changes(tmp_path, "T1", "verified") is False


def test_commit_task_changes_with_changes(tmp_path):
    _git_repo(tmp_path)
    _commit(tmp_path, "a.txt", "1")
    (tmp_path / "b.txt").write_text("2", encoding="utf-8")
    assert go._commit_task_changes(tmp_path, "T1", "verified") is True


def test_squash_repair_commits_noop_when_base_is_head(tmp_path):
    _git_repo(tmp_path)
    _commit(tmp_path, "a.txt", "1")
    head = go._current_head(tmp_path)
    assert go._squash_repair_commits(tmp_path, "T1", head, "verified") is False


def test_squash_repair_commits_collapses(tmp_path):
    _git_repo(tmp_path)
    _commit(tmp_path, "a.txt", "1")
    base = go._current_head(tmp_path)
    _commit(tmp_path, "b.txt", "2")
    (tmp_path / "c.txt").write_text("3", encoding="utf-8")
    assert go._squash_repair_commits(tmp_path, "T1", base, "verified") is True


# --- task loading / project state / report ----------------------------------------


def test_load_tasks_by_id_no_db(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "get_db", lambda root: None)
    tasks, missing = go._load_tasks_by_id(tmp_path, ["T1"])
    assert tasks == [] and missing == ["T1"]


def test_load_tasks_by_id_missing(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    tasks, missing = go._load_tasks_by_id(tmp_path, ["NOPE"])
    assert missing == ["NOPE"]


def test_record_project_done_and_blocked_no_db(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "get_db", lambda root: None)
    go._record_project_done(tmp_path)  # no raise
    go._record_project_blocked(tmp_path)  # no raise


def test_record_project_done_with_db(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    go._record_project_done(tmp_path)
    go._record_project_blocked(tmp_path)


def test_render_final_report_no_db_raises(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "get_db", lambda root: None)
    with pytest.raises(RuntimeError):
        go._render_final_report(tmp_path, json_report=True)


def test_render_final_report_markdown_and_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    md = go._render_final_report(tmp_path, json_report=False)
    js = go._render_final_report(tmp_path, json_report=True)
    assert isinstance(md, str) and isinstance(js, str)


def test_write_report_file_relative(tmp_path):
    out = go._write_report_file(tmp_path, Path("reports/final.md"), "content")
    assert out == tmp_path / "reports" / "final.md"
    assert out.read_text(encoding="utf-8") == "content"


def test_build_repair_service_none_without_provider(tmp_path):
    # No provider/API key configured in a bare tmp dir → best-effort None.
    assert go._build_repair_service(tmp_path) is None


# --- go command: executor guards --------------------------------------------------


def test_go_requires_automated_executor(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(go, "resolve_automated_executor", lambda root, ex: "manual")
    result = runner.invoke(app, ["go", "a goal", "--executor", "manual"])
    assert result.exit_code == 2
    assert "requires an automated executor" in result.output


def test_go_unsupported_executor(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(go, "resolve_automated_executor", lambda root, ex: "totally-bogus")
    result = runner.invoke(app, ["go", "a goal", "--executor", "totally-bogus"])
    assert result.exit_code == 2
    assert "Unsupported executor" in result.output


def test_go_planning_error_is_rendered(tmp_path, monkeypatch):
    from devcouncil.llm.router import StructuredOutputError

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(go, "resolve_automated_executor", lambda root, ex: "claude")

    async def boom(*a, **k):
        raise StructuredOutputError("bad", role="planner_a", model="m")

    monkeypatch.setattr(go.plan_command, "run_plan_flow", boom)
    result = runner.invoke(app, ["go", "a goal", "--executor", "claude"])
    assert result.exit_code == 1
    assert "Planning could not complete" in result.output


def test_go_no_tasks_without_force_aborts(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(go, "resolve_automated_executor", lambda root, ex: "claude")

    async def no_tasks(*a, **k):
        return []

    monkeypatch.setattr(go.plan_command, "run_plan_flow", no_tasks)
    result = runner.invoke(app, ["go", "a goal", "--executor", "claude"])
    assert result.exit_code == 1
    assert "did not produce any approved tasks" in result.output


# --- more helpers -----------------------------------------------------------------


def test_custom_cli_agents(tmp_path, monkeypatch):
    import devcouncil.executors.agent_registry as reg
    monkeypatch.setattr(
        go, "load_cli_agent_specs",
        lambda root: {
            "builtin": SimpleNamespace(built_in=True),
            "custom": SimpleNamespace(built_in=False),
        },
    )
    assert go._custom_cli_agents(tmp_path) == {"custom"}


def test_build_repair_service_success(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.app.config as config_mod
    import devcouncil.llm.provider as provider_mod
    import devcouncil.llm.router as router_mod
    import devcouncil.planning.repair_service as repair_mod

    monkeypatch.setattr(provider_mod, "validate_model_provider", lambda p: None)
    monkeypatch.setattr(config_mod, "get_api_key", lambda p, root: "key")
    monkeypatch.setattr(provider_mod, "create_provider", lambda *a, **k: object())
    monkeypatch.setattr(router_mod, "ModelRouter", lambda *a, **k: object())
    monkeypatch.setattr(repair_mod, "RepairService", lambda router: "REPAIR_SVC")
    assert go._build_repair_service(tmp_path) == "REPAIR_SVC"


def test_current_head_error(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "run_git", lambda *a, **k: (_ for _ in ()).throw(RuntimeError()))
    assert go._current_head(tmp_path) is None


def test_commit_task_changes_error(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "run_git", lambda *a, **k: (_ for _ in ()).throw(RuntimeError()))
    assert go._commit_task_changes(tmp_path, "T1", "verified") is False


def test_squash_repair_commits_error(tmp_path, monkeypatch):
    monkeypatch.setattr(go, "run_git", lambda *a, **k: (_ for _ in ()).throw(RuntimeError()))
    assert go._squash_repair_commits(tmp_path, "T1", "base", "verified") is False


# --- go command: full run in a git repo (reconciliation + report) -----------------


def _seed_task(root: Path, task_id="TASK-1", status="planned", **kw):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    db = get_db(root)
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id=task_id, title=task_id, description="d", status=status,
                planned_files=[PlannedFile(path="app.py", reason="logic", allowed_change="modify")],
                expected_tests=["pytest"], allowed_commands=["pytest"], **kw,
            )
        )


def _mark_verified(root: Path, task_id: str):
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    db = get_db(root)
    with db.get_session() as session:
        repo = TaskRepository(session)
        task = repo.get_by_id(task_id)
        task.status = "verified"
        repo.save(task)


def test_go_full_run_reconciles_and_reports(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    (tmp_path / "app.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True)
    assert runner.invoke(app, ["init"]).exit_code == 0

    monkeypatch.setattr(go, "resolve_automated_executor", lambda root, ex: "claude")

    async def plan_flow(*a, **k):
        _seed_task(tmp_path, "TASK-1")
        return ["TASK-1"]

    monkeypatch.setattr(go.plan_command, "run_plan_flow", plan_flow)
    monkeypatch.setattr(go, "_max_repair_attempts", lambda root: 0)

    def fake_run(task_id, executor=None, profile=None, stream=False, project_root=None):
        _mark_verified(project_root, task_id)

    monkeypatch.setattr(go.run_command, "run", fake_run)
    # Keep the reconciliation verify pass a cheap no-op (its own body is tested elsewhere).
    monkeypatch.setattr(go.verify_command, "verify", lambda **k: None)

    result = runner.invoke(app, ["go", "build it", "--executor", "claude"])
    assert result.exit_code == 0
    assert "Final DevCouncil report" in result.output


def test_go_force_approves_when_no_tasks(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(go, "resolve_automated_executor", lambda root, ex: "claude")

    async def no_tasks(*a, **k):
        return []

    monkeypatch.setattr(go.plan_command, "run_plan_flow", no_tasks)

    def fake_approve(run_id=None, force=False, project_root=None):
        _seed_task(project_root, "TASK-1")

    monkeypatch.setattr(go.plan_command, "approve", fake_approve)
    monkeypatch.setattr(go, "_max_repair_attempts", lambda root: 0)

    def fake_run(task_id, executor=None, profile=None, stream=False, project_root=None):
        _mark_verified(project_root, task_id)

    monkeypatch.setattr(go.run_command, "run", fake_run)
    monkeypatch.setattr(go, "_is_git_repo", lambda root: False)

    result = runner.invoke(app, ["go", "build it", "--executor", "claude", "--force"])
    assert result.exit_code == 0
    assert "Proceeding past planning gaps" in result.output
