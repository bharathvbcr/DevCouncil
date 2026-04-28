import pytest
from pathlib import Path

from devcouncil.app.errors import ExecutionError
from devcouncil.app.errors import GatingError
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.permissions import PermissionManager, PermissionPolicy
from devcouncil.execution.task_runner import TaskRunner


def _task_for(path: str) -> Task:
    return Task(
        id="TASK-001",
        title="Patch task",
        description="Patch one file",
        planned_files=[
            PlannedFile(path=path, reason="implementation", allowed_change="modify"),
        ],
    )


def test_apply_patch_validates_real_patch_paths(tmp_path):
    runner = TaskRunner(tmp_path, PermissionManager(PermissionPolicy(), tmp_path))
    runner.patch_engine.apply_patch = lambda patch: True

    patch = """diff --git a/src/app.py b/src/app.py
--- a/src/app.py
+++ b/src/app.py
@@ -1 +1 @@
-old
+new
"""

    assert runner.apply_patch(patch, _task_for("src/app.py")) is True


def test_apply_patch_rejects_unplanned_patch_paths(tmp_path):
    runner = TaskRunner(tmp_path, PermissionManager(PermissionPolicy(), tmp_path))
    runner.patch_engine.apply_patch = lambda patch: True

    patch = """diff --git a/src/other.py b/src/other.py
--- a/src/other.py
+++ b/src/other.py
@@ -1 +1 @@
-old
+new
"""

    with pytest.raises(GatingError):
        runner.apply_patch(patch, _task_for("src/app.py"))


def test_apply_patch_rejects_read_only_planned_file(tmp_path):
    runner = TaskRunner(tmp_path, PermissionManager(PermissionPolicy(), tmp_path))
    runner.patch_engine.apply_patch = lambda patch: True
    task = Task(
        id="TASK-001",
        title="Patch task",
        description="Patch one file",
        planned_files=[
            PlannedFile(path="src/app.py", reason="context only", allowed_change="read_only"),
        ],
    )

    patch = """diff --git a/src/app.py b/src/app.py
--- a/src/app.py
+++ b/src/app.py
@@ -1 +1 @@
-old
+new
"""

    with pytest.raises(GatingError):
        runner.apply_patch(patch, task)


def test_write_file_enforces_create_vs_modify(tmp_path):
    runner = TaskRunner(tmp_path, PermissionManager(PermissionPolicy(), tmp_path))
    task = Task(
        id="TASK-001",
        title="Create task",
        description="Create one file",
        planned_files=[
            PlannedFile(path="src/app.py", reason="new file", allowed_change="create"),
        ],
    )

    runner.write_file("src/app.py", "new\n", task)
    with pytest.raises(GatingError):
        runner.write_file("src/app.py", "modify\n", task)


def test_command_log_redacts_secret_values(tmp_path):
    runner = TaskRunner(tmp_path, PermissionManager(PermissionPolicy(), tmp_path))

    log_path = runner._save_command_log(
        "TASK-001",
        "echo secret",
        "stdout",
        "api_key=abcdef1234567890\n",
    )

    content = Path(log_path).read_text(encoding="utf-8")
    assert "abcdef1234567890" not in content
    assert "[REDACTED:generic_api_key]" in content


def test_path_validation_rejects_sibling_prefix_escape(tmp_path):
    project_root = tmp_path / "DevCouncil"
    project_root.mkdir()
    sibling = tmp_path / "DevCouncil2"
    sibling.mkdir()
    runner = TaskRunner(project_root, PermissionManager(PermissionPolicy(), project_root))

    with pytest.raises(ExecutionError):
        runner._validate_path_within_root("../DevCouncil2/secret.txt")
