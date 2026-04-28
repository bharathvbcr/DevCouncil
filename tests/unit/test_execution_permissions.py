import pytest

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


def test_path_validation_rejects_sibling_prefix_escape(tmp_path):
    project_root = tmp_path / "DevCouncil"
    project_root.mkdir()
    sibling = tmp_path / "DevCouncil2"
    sibling.mkdir()
    runner = TaskRunner(project_root, PermissionManager(PermissionPolicy(), project_root))

    with pytest.raises(ExecutionError):
        runner._validate_path_within_root("../DevCouncil2/secret.txt")
