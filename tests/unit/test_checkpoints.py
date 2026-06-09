import subprocess
from pathlib import Path

from devcouncil.execution.checkpoints import CheckpointService


def _init_git_repo(path: Path) -> None:
    subprocess.check_call(["git", "init"], cwd=path)
    subprocess.check_call(["git", "config", "user.email", "test@example.com"], cwd=path)
    subprocess.check_call(["git", "config", "user.name", "Test"], cwd=path)
    (path / "README.md").write_text("hello\n", encoding="utf-8")
    subprocess.check_call(["git", "add", "README.md"], cwd=path)
    subprocess.check_call(["git", "commit", "-m", "init"], cwd=path)


def test_create_before_and_after_refs_in_temp_git_repo(tmp_path: Path):
    _init_git_repo(tmp_path)
    service = CheckpointService(tmp_path)
    before = service.create_before("TASK-001")
    assert before.git_ref_created is True
    assert service._ref_exists(CheckpointService.REF_BEFORE.format(task_id="TASK-001"))

    (tmp_path / "README.md").write_text("hello world\n", encoding="utf-8")
    after = service.create_after("TASK-001")
    assert after.git_ref_created is True
    assert (tmp_path / ".devcouncil" / "checkpoints" / "TASK-001-after.patch").exists()


def test_missing_git_repo_falls_back_without_raising(tmp_path: Path):
    service = CheckpointService(tmp_path)
    result = service.create_before("TASK-002")
    assert result.git_ref_created is False
    assert "created" in result.message.lower()


def test_rollback_prefers_git_refs_when_both_exist(tmp_path: Path):
    _init_git_repo(tmp_path)
    service = CheckpointService(tmp_path)
    service.create_before("TASK-003")
    readme = tmp_path / "README.md"
    readme.write_text("changed\n", encoding="utf-8")
    service.create_after("TASK-003")

    result = service.rollback("TASK-003")
    assert result.git_ref_created is True or "Rolled back" in result.message
    assert "changed" not in readme.read_text(encoding="utf-8")


def test_existing_patch_rollback_still_works(tmp_path: Path):
    _init_git_repo(tmp_path)
    service = CheckpointService(tmp_path)
    readme = tmp_path / "README.md"
    readme.write_text("patched\n", encoding="utf-8")
    diff = subprocess.check_output(["git", "diff"], cwd=tmp_path, text=True)
    patch_path = tmp_path / ".devcouncil" / "checkpoints" / "TASK-004-after.patch"
    patch_path.parent.mkdir(parents=True, exist_ok=True)
    patch_path.write_text(diff, encoding="utf-8")

    result = service.rollback("TASK-004")
    assert "patch" in result.message.lower() or "Rolled back" in result.message
